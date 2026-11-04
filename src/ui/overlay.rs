//! A click-through, never-activated, always-on-top text box on the primary
//! monitor. Rendered with Direct2D/DirectWrite into a premultiplied 32-bit
//! DIB and handed to the compositor with `UpdateLayeredWindow`; nothing is
//! redrawn unless the content or the display layout changes.

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    ID2D1Factory, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FEATURE_LEVEL_DEFAULT,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    IDWriteFactory, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_ITALIC,
    DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_TEXT_ALIGNMENT,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING,
    DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
    MonitorFromPoint, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, MONITORINFO,
    MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow,
    UpdateLayeredWindow, HTTRANSPARENT, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::config::{parse_color, Anchor, TextStyle};

const CLASS_NAME: PCWSTR = w!("LowtideOverlay");

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub anchor: Anchor,
    /// Distance from the work-area edges, DIPs.
    pub margin: i32,
    /// Extra shift after anchoring, DIPs.
    pub offset: [i32; 2],
}

#[derive(Clone, Debug, PartialEq)]
struct Content {
    text: String,
    style: TextStyle,
    placement: Placement,
}

pub struct Overlay {
    hwnd: HWND,
    d2d: ID2D1Factory,
    dwrite: IDWriteFactory,
    content: Option<Content>,
    visible: bool,
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

pub fn register_class() -> Result<()> {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None)?.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
    Ok(())
}

impl Overlay {
    pub fn new(d2d: ID2D1Factory, dwrite: IDWriteFactory) -> Result<Overlay> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                CLASS_NAME,
                w!("Lowtide"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(GetModuleHandleW(None)?.into()),
                None,
            )?
        };
        Ok(Overlay { hwnd, d2d, dwrite, content: None, visible: false })
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Show (or update) the overlay. Re-renders only if something changed.
    pub fn show(&mut self, text: &str, style: &TextStyle, placement: &Placement) -> Result<()> {
        let content = Content { text: text.into(), style: style.clone(), placement: placement.clone() };
        if self.content.as_ref() != Some(&content) || !self.visible {
            self.render(&content)?;
            self.content = Some(content);
        }
        if !self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            self.visible = true;
        }
        self.raise();
        Ok(())
    }

    pub fn hide(&mut self) {
        if self.visible {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
        }
    }

    /// Put this overlay above every other topmost window, including other overlays.
    pub fn raise(&self) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    /// Display layout, DPI or work area changed: lay out and draw again.
    pub fn relayout(&mut self) {
        if let (true, Some(c)) = (self.visible, self.content.clone()) {
            let _ = self.render(&c);
        }
    }

    fn render(&self, c: &Content) -> Result<()> {
        let screen = primary_screen();
        let scale = screen.dpi as f32 / 96.0;
        let style = &c.style;
        let pad = f32::from(style.padding);

        // Text layout, in DIPs.
        let format = unsafe {
            self.dwrite.CreateTextFormat(
                &HSTRING::from(style.font_family.as_str()),
                None,
                DWRITE_FONT_WEIGHT(i32::from(style.weight)),
                if style.italic { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL },
                DWRITE_FONT_STRETCH_NORMAL,
                style.size_pt * 96.0 / 72.0,
                w!(""),
            )?
        };
        let work_w = (screen.work.right - screen.work.left) as f32 / scale;
        let max_w = (work_w * 0.9 - 2.0 * pad).max(50.0);
        let wide: Vec<u16> = c.text.encode_utf16().collect();
        let layout = unsafe { self.dwrite.CreateTextLayout(&wide, &format, max_w, 10_000.0)? };
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            layout.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            layout.GetMetrics(&mut m)?;
            // Shrink the layout box to the text so alignment works within it.
            layout.SetMaxWidth(m.width.ceil())?;
            layout.SetTextAlignment(alignment_for(c.placement.anchor))?;
        }
        let box_w = m.width.ceil() + 2.0 * pad;
        let box_h = m.height.ceil() + 2.0 * pad;
        let px_w = ((box_w * scale).ceil() as i32).max(1);
        let px_h = ((box_h * scale).ceil() as i32).max(1);

        unsafe {
            let screen_dc = GetDC(None);
            let mem_dc = CreateCompatibleDC(Some(screen_dc));
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: px_w,
                    biHeight: -px_h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let result = (|| -> Result<()> {
                let bitmap = CreateDIBSection(Some(mem_dc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
                let old = SelectObject(mem_dc, bitmap.into());
                let drawn = self.draw(mem_dc, px_w, px_h, screen.dpi, c, &layout, box_w, box_h, pad);
                let shown = drawn.and_then(|()| {
                    let (x, y) = place(
                        c.placement.anchor,
                        screen.work,
                        (px_w, px_h),
                        (c.placement.margin as f32 * scale).round() as i32,
                        [
                            (c.placement.offset[0] as f32 * scale).round() as i32,
                            (c.placement.offset[1] as f32 * scale).round() as i32,
                        ],
                    );
                    let blend = BLENDFUNCTION {
                        BlendOp: AC_SRC_OVER as u8,
                        BlendFlags: 0,
                        SourceConstantAlpha: 255,
                        AlphaFormat: AC_SRC_ALPHA as u8,
                    };
                    UpdateLayeredWindow(
                        self.hwnd,
                        Some(screen_dc),
                        Some(&POINT { x, y }),
                        Some(&SIZE { cx: px_w, cy: px_h }),
                        Some(mem_dc),
                        Some(&POINT { x: 0, y: 0 }),
                        COLORREF(0),
                        Some(&blend),
                        ULW_ALPHA,
                    )
                });
                SelectObject(mem_dc, old);
                let _ = DeleteObject(bitmap.into());
                shown
            })();
            let _ = DeleteDC(mem_dc);
            ReleaseDC(None, screen_dc);
            result
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        dc: windows::Win32::Graphics::Gdi::HDC,
        px_w: i32,
        px_h: i32,
        dpi: u32,
        c: &Content,
        layout: &windows::Win32::Graphics::DirectWrite::IDWriteTextLayout,
        box_w: f32,
        box_h: f32,
        pad: f32,
    ) -> Result<()> {
        let style = &c.style;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            // Software rendering: no GPU device for a few hundred pixels of text.
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: dpi as f32,
            dpiY: dpi as f32,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        unsafe {
            let rt = self.d2d.CreateDCRenderTarget(&props)?;
            rt.BindDC(dc, &RECT { left: 0, top: 0, right: px_w, bottom: px_h })?;
            rt.BeginDraw();
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            if style.background_opacity > 0 {
                let bg = rt.CreateSolidColorBrush(&color(&style.background, style.background_opacity), None)?;
                let r = f32::from(style.corner_radius).min(box_h / 2.0);
                let rect = D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F { left: 0.0, top: 0.0, right: box_w, bottom: box_h },
                    radiusX: r,
                    radiusY: r,
                };
                rt.FillRoundedRectangle(&rect, &bg);
            }
            // Without a solid box, a soft shadow keeps the text readable on any wallpaper.
            if style.background_opacity < 40 && style.text_opacity > 0 {
                let a = f32::from(style.text_opacity) / 100.0 * 0.6;
                let shadow = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a }, None)?;
                let o = windows_numerics::Vector2 { X: pad + 1.0, Y: pad + 1.0 };
                rt.DrawTextLayout(o, layout, &shadow, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            let fg = rt.CreateSolidColorBrush(&color(&style.text_color, style.text_opacity), None)?;
            rt.DrawTextLayout(windows_numerics::Vector2 { X: pad, Y: pad }, layout, &fg, D2D1_DRAW_TEXT_OPTIONS_NONE);
            rt.EndDraw(None, None)
        }
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn color(hex: &str, opacity_percent: u8) -> D2D1_COLOR_F {
    let (r, g, b) = parse_color(hex).unwrap_or((255, 255, 255));
    D2D1_COLOR_F {
        r: f32::from(r) / 255.0,
        g: f32::from(g) / 255.0,
        b: f32::from(b) / 255.0,
        a: f32::from(opacity_percent.min(100)) / 100.0,
    }
}

fn alignment_for(anchor: Anchor) -> DWRITE_TEXT_ALIGNMENT {
    match anchor {
        Anchor::TopLeft | Anchor::Left | Anchor::BottomLeft => DWRITE_TEXT_ALIGNMENT_LEADING,
        Anchor::TopRight | Anchor::Right | Anchor::BottomRight => DWRITE_TEXT_ALIGNMENT_TRAILING,
        Anchor::Top | Anchor::Center | Anchor::Bottom => DWRITE_TEXT_ALIGNMENT_CENTER,
    }
}

struct Screen {
    work: RECT,
    dpi: u32,
}

/// Work area (screen minus taskbar) and effective DPI of the primary monitor.
fn primary_screen() -> Screen {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Screen { work: info.rcWork, dpi: dx.max(48) }
    }
}

/// Top-left position of a `size` box anchored inside `work` (physical pixels).
pub fn place(anchor: Anchor, work: RECT, size: (i32, i32), margin: i32, offset: [i32; 2]) -> (i32, i32) {
    let (w, h) = size;
    let left = work.left + margin;
    let right = work.right - margin - w;
    let hcenter = work.left + (work.right - work.left - w) / 2;
    let top = work.top + margin;
    let bottom = work.bottom - margin - h;
    let vcenter = work.top + (work.bottom - work.top - h) / 2;
    let (x, y) = match anchor {
        Anchor::TopLeft => (left, top),
        Anchor::Top => (hcenter, top),
        Anchor::TopRight => (right, top),
        Anchor::Left => (left, vcenter),
        Anchor::Center => (hcenter, vcenter),
        Anchor::Right => (right, vcenter),
        Anchor::BottomLeft => (left, bottom),
        Anchor::Bottom => (hcenter, bottom),
        Anchor::BottomRight => (right, bottom),
    };
    (x + offset[0], y + offset[1])
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1920x1080 monitor with a 48 px taskbar at the bottom.
    const WORK: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1032 };

    #[test]
    fn corners_respect_margin_and_work_area() {
        assert_eq!(place(Anchor::TopLeft, WORK, (200, 50), 24, [0, 0]), (24, 24));
        assert_eq!(place(Anchor::BottomRight, WORK, (200, 50), 24, [0, 0]), (1920 - 24 - 200, 1032 - 24 - 50));
        assert_eq!(place(Anchor::TopRight, WORK, (200, 50), 0, [0, 0]), (1720, 0));
    }

    #[test]
    fn center_ignores_margin_and_applies_offset() {
        assert_eq!(place(Anchor::Center, WORK, (200, 50), 24, [0, 0]), (860, 491));
        assert_eq!(place(Anchor::Center, WORK, (200, 50), 24, [10, -20]), (870, 471));
    }

    #[test]
    fn secondary_monitor_left_of_primary_has_negative_origin() {
        let work = RECT { left: -2560, top: 0, right: 0, bottom: 1400 };
        assert_eq!(place(Anchor::Bottom, work, (100, 40), 10, [0, 0]), (-1330, 1350));
    }
}
