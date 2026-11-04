//! Notification-area icon. The icon is drawn at runtime: a battery whose fill
//! shows the lowest charge, coloured by state, adapted to light/dark taskbars.

use windows::core::{w, Result};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS,
};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_WARNING, NIM_ADD, NIM_DELETE,
    NIM_MODIFY, NIM_SETVERSION, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO, SM_CXSMICON};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconState {
    /// No device known.
    Unknown,
    Normal(u8),
    Charging(u8),
    Alert(u8),
}

pub struct Tray {
    hwnd: HWND,
    callback_msg: u32,
    icon: HICON,
    state: Option<IconState>,
    tip: String,
}

const ICON_ID: u32 = 1;

impl Tray {
    pub fn new(hwnd: HWND, callback_msg: u32) -> Tray {
        Tray { hwnd, callback_msg, icon: HICON::default(), state: None, tip: String::new() }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut nid = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ICON_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: self.callback_msg,
            hIcon: self.icon,
            ..Default::default()
        };
        let tip: Vec<u16> = self.tip.encode_utf16().take(nid.szTip.len() - 1).collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        nid
    }

    /// (Re-)add the icon, e.g. at startup or after Explorer restarted.
    pub fn add(&mut self) {
        let state = self.state.unwrap_or(IconState::Unknown);
        self.state = None;
        self.set(state, &self.tip.clone(), true);
    }

    pub fn set(&mut self, state: IconState, tip: &str, force: bool) {
        if !force && self.state == Some(state) && self.tip == tip {
            return;
        }
        if force || self.state != Some(state) {
            if let Ok(icon) = draw_icon(state, self.hwnd) {
                let old = std::mem::replace(&mut self.icon, icon);
                if !old.is_invalid() {
                    unsafe {
                        let _ = DestroyIcon(old);
                    }
                }
            }
        }
        self.state = Some(state);
        self.tip = tip.to_string();
        let mut nid = self.data();
        unsafe {
            if force {
                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                let _ = Shell_NotifyIconW(NIM_ADD, &nid);
                nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let _ = Shell_NotifyIconW(NIM_SETVERSION, &nid);
            } else {
                let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
        }
    }

    /// Non-blocking warning balloon / toast from the tray icon.
    pub fn notify(&self, title: &str, text: &str) {
        let mut nid = self.data();
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_WARNING;
        let t: Vec<u16> = title.encode_utf16().take(nid.szInfoTitle.len() - 1).collect();
        nid.szInfoTitle[..t.len()].copy_from_slice(&t);
        let b: Vec<u16> = text.encode_utf16().take(nid.szInfo.len() - 1).collect();
        nid.szInfo[..b.len()].copy_from_slice(&b);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }

    /// Redraw for a changed theme or DPI.
    pub fn redraw(&mut self) {
        if let Some(state) = self.state {
            self.state = None;
            self.set(state, &self.tip.clone(), false);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            if !self.icon.is_invalid() {
                let _ = DestroyIcon(self.icon);
            }
        }
    }
}

fn light_taskbar() -> bool {
    let mut value = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("SystemUsesLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
        .is_ok()
            && value != 0
    }
}

/// ARGB (straight alpha) pixels of a battery glyph, `size` x `size`.
pub fn render_pixels(state: IconState, size: i32, light: bool) -> Vec<u32> {
    let s = size.max(16);
    let mut px = vec![0u32; (s * s) as usize];
    let mut rect = |x0: i32, y0: i32, x1: i32, y1: i32, argb: u32| {
        for y in y0.max(0)..y1.min(s) {
            for x in x0.max(0)..x1.min(s) {
                px[(y * s + x) as usize] = argb;
            }
        }
    };
    let outline = match (state, light) {
        (IconState::Unknown, true) => 0x8020_2020,
        (IconState::Unknown, false) => 0x80F0_F0F0,
        (_, true) => 0xFF20_2020,
        (_, false) => 0xFFF0_F0F0,
    };
    let t = (s / 16).max(1); // stroke
    let nub_w = (s / 8).max(2);
    let (x0, x1) = (0, s - nub_w);
    let (y0, y1) = (s * 5 / 16, s - s * 5 / 16 + 1);
    // Outline box and terminal nub.
    rect(x0, y0, x1, y0 + t, outline);
    rect(x0, y1 - t, x1, y1, outline);
    rect(x0, y0, x0 + t, y1, outline);
    rect(x1 - t, y0, x1, y1, outline);
    let nub_h = (y1 - y0) / 2;
    let ny = y0 + (y1 - y0 - nub_h) / 2;
    rect(x1, ny, s, ny + nub_h, outline);
    // Fill proportional to the charge.
    let (percent, fill) = match state {
        IconState::Unknown => (0, 0),
        IconState::Normal(p) => (p, 0xFF40_AAFF),
        IconState::Charging(p) => (p, 0xFF43_C05A),
        IconState::Alert(p) => (p, 0xFFF0_3A3A),
    };
    let inset = t + (s / 16).max(1);
    let (fx0, fx1) = (x0 + inset, x1 - inset);
    let width = ((fx1 - fx0) * i32::from(percent.min(100)) + 99) / 100;
    let width = if percent > 0 { width.max(1) } else { 0 };
    rect(fx0, y0 + inset, fx0 + width, y1 - inset, fill);
    if let IconState::Alert(_) = state {
        // Make an alert readable even at 0%: a full-height red bar on the left.
        rect(fx0, y0 + inset, fx0 + (s / 8).max(2), y1 - inset, fill);
    }
    px
}

fn draw_icon(state: IconState, hwnd: HWND) -> Result<HICON> {
    unsafe {
        let dpi = GetDpiForWindow(hwnd).max(96);
        let size = GetSystemMetricsForDpi(SM_CXSMICON, dpi).max(16);
        let pixels = render_pixels(state, size, light_taskbar());
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u32>(), pixels.len());
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO { fIcon: true.into(), hbmMask: mask, hbmColor: color, ..Default::default() };
        let icon = CreateIconIndirect(&info);
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mask.into());
        icon
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled(px: &[u32], color: u32) -> usize {
        px.iter().filter(|&&p| p == color).count()
    }

    #[test]
    fn fill_grows_with_charge() {
        let low = filled(&render_pixels(IconState::Normal(20), 32, false), 0xFF40_AAFF);
        let high = filled(&render_pixels(IconState::Normal(90), 32, false), 0xFF40_AAFF);
        assert!(low > 0 && high > low * 3, "{low} vs {high}");
        assert_eq!(filled(&render_pixels(IconState::Normal(0), 32, false), 0xFF40_AAFF), 0);
    }

    #[test]
    fn alert_is_visible_even_when_empty() {
        assert!(filled(&render_pixels(IconState::Alert(0), 16, true), 0xFFF0_3A3A) > 0);
    }
}
