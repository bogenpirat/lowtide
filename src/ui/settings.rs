//! Native settings dialog (template `IDD_SETTINGS` in res/app.rc).
//! It knows nothing about the app: it edits a [`Config`] copy and reports
//! through [`Hooks`] (preview, apply). Fonts and colours use the system pickers.

use std::cell::Cell;

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, DrawFocusRect, FillRect, FrameRect, GetSysColorBrush,
    InvalidateRect, COLOR_BTNSHADOW, LOGFONTW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::Dialogs::{
    ChooseColorW, ChooseFontW, CC_FULLOPEN, CC_RGBINIT, CF_INITTOLOGFONTSTRUCT, CF_NOSCRIPTSEL,
    CF_NOVERTFONTS, CF_SCREENFONTS, CHOOSECOLORW, CHOOSEFONTW,
};
use windows::Win32::UI::Controls::{
    CheckDlgButton, IsDlgButtonChecked, BST_CHECKED, BST_UNCHECKED, DRAWITEMSTRUCT, HKCOMB_NONE, HKCOMB_S, HKM_GETHOTKEY,
    HKM_SETHOTKEY, HKM_SETRULES, HOTKEYF_ALT, HOTKEYF_CONTROL, ODS_FOCUS, UDM_SETRANGE32,
};
use windows::Win32::UI::HiDpi::GetDpiForSystem;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    DialogBoxParamW, EndDialog, GetDlgItem, GetDlgItemInt, GetDlgItemTextW,
    GetWindowLongPtrW, LoadIconW, MessageBoxW, SendDlgItemMessageW,
    SendMessageW, SetDlgItemInt, SetDlgItemTextW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowTextW, CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL, ICON_BIG, ICON_SMALL, IDCANCEL, IDOK, MB_ICONWARNING,
    WINDOW_LONG_PTR_INDEX, WM_COMMAND, WM_DESTROY, WM_DRAWITEM, WM_INITDIALOG, WM_SETICON,
};

use crate::autostart;
use crate::config::{format_color, parse_color, Anchor, Config, TextStyle};
use crate::hotkey::Hotkey;
use crate::i18n::{fill, t, LanguageSetting};

/// Control ids, mirrored from res/resource.h.
mod ids {
    pub const IDI_APP: u16 = 1;
    pub const IDD_SETTINGS: u16 = 100;
    pub const THRESHOLD: i32 = 1001;
    pub const THRESHOLD_SPIN: i32 = 1002;
    pub const SUPPRESS: i32 = 1003;
    pub const POLL: i32 = 1004;
    pub const POLL_SPIN: i32 = 1005;
    pub const HOTKEY: i32 = 1006;
    pub const AUTOSTART: i32 = 1007;
    pub const LANGUAGE: i32 = 1008;
    pub const ALERT_MSG: i32 = 1010;
    pub const ALERT_ANCHOR: i32 = 1011;
    pub const ALERT_MARGIN: i32 = 1012;
    pub const ALERT_MARGIN_SPIN: i32 = 1013;
    pub const INFO_ANCHOR: i32 = 1020;
    pub const INFO_MARGIN: i32 = 1021;
    pub const INFO_MARGIN_SPIN: i32 = 1022;
    pub const INFO_OFFX: i32 = 1023;
    pub const INFO_OFFX_SPIN: i32 = 1024;
    pub const INFO_OFFY: i32 = 1025;
    pub const INFO_OFFY_SPIN: i32 = 1026;
    pub const INFO_DURATION: i32 = 1027;
    pub const INFO_DURATION_SPIN: i32 = 1028;
    pub const ALERT_STYLE: i32 = 1100;
    pub const INFO_STYLE: i32 = 1200;
    pub const PREVIEW: i32 = 1300;
    pub const APPLY: i32 = 1301;
    pub const L_GENERAL: i32 = 1401;
    pub const L_THRESHOLD: i32 = 1402;
    pub const L_POLL: i32 = 1403;
    pub const L_SECONDS: i32 = 1404;
    pub const L_HOTKEY: i32 = 1405;
    pub const L_LANGUAGE: i32 = 1406;
    pub const L_ALERT_GROUP: i32 = 1407;
    pub const L_MESSAGE: i32 = 1408;
    pub const L_PLACEHOLDERS: i32 = 1409;
    pub const L_ALERT_POSITION: i32 = 1410;
    pub const L_ALERT_MARGIN: i32 = 1411;
    pub const L_INFO_GROUP: i32 = 1420;
    pub const L_INFO_POSITION: i32 = 1421;
    pub const L_INFO_MARGIN: i32 = 1422;
    pub const L_OFFSET_X: i32 = 1423;
    pub const L_SHOW_FOR: i32 = 1424;
}

// Offsets within a text-style block.
const STYLE_FONT: i32 = 0;
const STYLE_FONT_LABEL: i32 = 1;
const STYLE_TEXT_COLOR: i32 = 2;
const STYLE_TEXT_OPACITY: i32 = 3;
const STYLE_TEXT_OPACITY_SPIN: i32 = 4;
const STYLE_BG_COLOR: i32 = 5;
const STYLE_BG_OPACITY: i32 = 6;
const STYLE_BG_OPACITY_SPIN: i32 = 7;
const STYLE_PADDING: i32 = 8;
const STYLE_PADDING_SPIN: i32 = 9;
const STYLE_RADIUS: i32 = 10;
const STYLE_RADIUS_SPIN: i32 = 11;
const STYLE_L_TEXT_COLOR: i32 = 12;
const STYLE_L_TEXT_OPACITY: i32 = 13;
const STYLE_L_PADDING: i32 = 14;
const STYLE_L_BACKGROUND: i32 = 15;
const STYLE_L_BG_OPACITY: i32 = 16;
const STYLE_L_RADIUS: i32 = 17;

pub struct Hooks<'a> {
    /// Show both overlays with these (unsaved) settings.
    pub preview: &'a dyn Fn(&Config),
    /// Dialog closed after a preview: go back to the real state.
    pub end_preview: &'a dyn Fn(),
    /// Save and activate; an error (e.g. hotkey taken) keeps the dialog open.
    pub apply: &'a dyn Fn(&Config, bool) -> Result<(), String>,
}

struct State<'a> {
    /// Working copy; fonts and colours are edited here directly.
    cfg: Config,
    /// Hotkey control value at open, to keep a Win-key hotkey the control cannot show.
    initial_hotkey_control: u16,
    initial_hotkey: String,
    hooks: Hooks<'a>,
    previewed: bool,
}

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
}

/// Show the dialog modally. If it is already open, bring it to the front.
pub fn show(owner: HWND, cfg: Config, hooks: Hooks<'_>) {
    let open = OPEN.with(Cell::get);
    if open != 0 {
        unsafe {
            let _ = SetForegroundWindow(HWND(open as *mut _));
        }
        return;
    }
    let mut state = State {
        initial_hotkey_control: 0,
        initial_hotkey: cfg.hotkey.clone(),
        cfg,
        hooks,
        previewed: false,
    };
    unsafe {
        let instance = GetModuleHandleW(None).ok().map(Into::into);
        let r = DialogBoxParamW(
            instance,
            PCWSTR(ids::IDD_SETTINGS as usize as *const u16),
            Some(owner),
            Some(dialog_proc),
            LPARAM(&mut state as *mut State as isize),
        );
        if r == -1 {
            let e = windows::core::Error::from_thread();
            warn(owner, &fill!(t().dlg_open_failed, error = e.message()));
        }
    }
    if state.previewed {
        (state.hooks.end_preview)();
    }
}

/// DWLP_USER: the slot after DWLP_MSGRESULT and DWLP_DLGPROC.
const DWLP_USER: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX((2 * size_of::<isize>()) as i32);

unsafe extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(hwnd, DWLP_USER, lparam.0);
            OPEN.with(|o| o.set(hwnd.0 as isize));
            let state = &mut *(lparam.0 as *mut State);
            init_controls(hwnd, state);
            let _ = SetForegroundWindow(hwnd);
            return 1;
        }
        let ptr = GetWindowLongPtrW(hwnd, DWLP_USER) as *mut State;
        if ptr.is_null() {
            return 0;
        }
        let state = &mut *ptr;
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                on_command(hwnd, state, id);
                1
            }
            WM_DRAWITEM => {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                draw_swatch(state, item);
                1
            }
            WM_DESTROY => {
                OPEN.with(|o| o.set(0));
                0
            }
            _ => 0,
        }
    }
}

fn on_command(hwnd: HWND, state: &mut State, id: i32) {
    match id {
        x if x == IDOK.0 => {
            if apply(hwnd, state) {
                unsafe {
                    let _ = EndDialog(hwnd, 1);
                }
            }
        }
        x if x == IDCANCEL.0 => unsafe {
            let _ = EndDialog(hwnd, 0);
        },
        ids::APPLY => {
            apply(hwnd, state);
        }
        ids::PREVIEW => {
            if let Some((cfg, _)) = read_controls(hwnd, state) {
                (state.hooks.preview)(&cfg);
                state.previewed = true;
            }
        }
        _ => {
            for (base, alert) in [(ids::ALERT_STYLE, true), (ids::INFO_STYLE, false)] {
                let style = if alert { &mut state.cfg.alert.style } else { &mut state.cfg.info.style };
                match id - base {
                    STYLE_FONT => {
                        if pick_font(hwnd, style) {
                            set_text(hwnd, base + STYLE_FONT_LABEL, &font_label(style));
                        }
                    }
                    STYLE_TEXT_COLOR => pick_color(hwnd, &mut style.text_color, base + STYLE_TEXT_COLOR),
                    STYLE_BG_COLOR => pick_color(hwnd, &mut style.background, base + STYLE_BG_COLOR),
                    _ => {}
                }
            }
        }
    }
}

/// Validate and hand the settings to the app. Returns true on success.
fn apply(hwnd: HWND, state: &mut State) -> bool {
    let Some((cfg, autostart)) = read_controls(hwnd, state) else { return false };
    match (state.hooks.apply)(&cfg, autostart) {
        Ok(()) => {
            let language_changed = cfg.language != state.cfg.language;
            state.cfg = cfg;
            if language_changed {
                // The app switched languages in `apply`; follow it right away.
                translate(hwnd, state);
            }
            true
        }
        Err(e) => {
            warn(hwnd, &e);
            false
        }
    }
}

fn warn(hwnd: HWND, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(Some(hwnd), PCWSTR(wide.as_ptr()), &HSTRING::from(t().dlg_title), MB_ICONWARNING);
    }
}

// ---- reading and writing controls -------------------------------------------------------

fn send(hwnd: HWND, id: i32, msg: u32, wparam: usize, lparam: isize) -> isize {
    unsafe { SendDlgItemMessageW(hwnd, id, msg, WPARAM(wparam), LPARAM(lparam)).0 }
}

fn set_text(hwnd: HWND, id: i32, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let _ = SetDlgItemTextW(hwnd, id, PCWSTR(wide.as_ptr()));
    }
}

fn get_text(hwnd: HWND, id: i32) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetDlgItemTextW(hwnd, id, &mut buf) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

fn set_int(hwnd: HWND, id: i32, spin: i32, value: i32, min: i32, max: i32) {
    send(hwnd, spin, UDM_SETRANGE32, min as usize, max as isize);
    unsafe {
        let _ = SetDlgItemInt(hwnd, id, value as u32, true);
    }
}

/// Integer field within `min..=max`, or a message naming the field.
fn get_int(hwnd: HWND, id: i32, name: &str, min: i32, max: i32) -> Result<i32, (i32, String)> {
    let mut ok = windows::core::BOOL(0);
    let v = unsafe { GetDlgItemInt(hwnd, id, Some(&mut ok), true) } as i32;
    if !ok.as_bool() || v < min || v > max {
        return Err((id, fill!(t().err_range, field = name, min = min, max = max)));
    }
    Ok(v)
}

fn set_check(hwnd: HWND, id: i32, on: bool) {
    unsafe {
        let _ = CheckDlgButton(hwnd, id, if on { BST_CHECKED } else { BST_UNCHECKED });
    }
}

fn get_check(hwnd: HWND, id: i32) -> bool {
    unsafe { IsDlgButtonChecked(hwnd, id) == BST_CHECKED.0 }
}

fn fill_anchor_combo(hwnd: HWND, id: i32, current: Anchor) {
    send(hwnd, id, CB_RESETCONTENT, 0, 0);
    for a in Anchor::ALL {
        let wide: Vec<u16> = a.label().encode_utf16().chain(Some(0)).collect();
        send(hwnd, id, CB_ADDSTRING, 0, wide.as_ptr() as isize);
    }
    let index = Anchor::ALL.iter().position(|&a| a == current).unwrap_or(0);
    send(hwnd, id, CB_SETCURSEL, index, 0);
}

fn get_anchor(hwnd: HWND, id: i32) -> Anchor {
    let i = send(hwnd, id, CB_GETCURSEL, 0, 0);
    Anchor::ALL.get(i.max(0) as usize).copied().unwrap_or(Anchor::Center)
}

fn fill_language_combo(hwnd: HWND, current: LanguageSetting) {
    send(hwnd, ids::LANGUAGE, CB_RESETCONTENT, 0, 0);
    for l in LanguageSetting::ALL {
        let wide: Vec<u16> = l.label().encode_utf16().chain(Some(0)).collect();
        send(hwnd, ids::LANGUAGE, CB_ADDSTRING, 0, wide.as_ptr() as isize);
    }
    let index = LanguageSetting::ALL.iter().position(|&l| l == current).unwrap_or(0);
    send(hwnd, ids::LANGUAGE, CB_SETCURSEL, index, 0);
}

fn get_language(hwnd: HWND) -> LanguageSetting {
    let i = send(hwnd, ids::LANGUAGE, CB_GETCURSEL, 0, 0);
    LanguageSetting::ALL.get(i.max(0) as usize).copied().unwrap_or_default()
}

/// Put the active language on every label, button and list, keeping selections.
fn translate(hwnd: HWND, state: &State) {
    let s = t();
    unsafe {
        let _ = SetWindowTextW(hwnd, &HSTRING::from(s.dlg_title));
    }
    for (id, text) in [
        (ids::L_GENERAL, s.dlg_general),
        (ids::L_THRESHOLD, s.dlg_threshold),
        (ids::SUPPRESS, s.dlg_suppress),
        (ids::L_POLL, s.dlg_poll),
        (ids::L_SECONDS, s.dlg_seconds),
        (ids::L_HOTKEY, s.dlg_hotkey),
        (ids::AUTOSTART, s.dlg_autostart),
        (ids::L_LANGUAGE, s.dlg_language),
        (ids::L_ALERT_GROUP, s.dlg_alert_group),
        (ids::L_MESSAGE, s.dlg_message),
        (ids::L_PLACEHOLDERS, s.dlg_placeholders),
        (ids::L_ALERT_POSITION, s.dlg_position),
        (ids::L_ALERT_MARGIN, s.dlg_margin),
        (ids::L_INFO_GROUP, s.dlg_info_group),
        (ids::L_INFO_POSITION, s.dlg_position),
        (ids::L_INFO_MARGIN, s.dlg_margin),
        (ids::L_OFFSET_X, s.dlg_offset_x),
        (ids::L_SHOW_FOR, s.dlg_show_for),
        (ids::ALERT_STYLE + STYLE_FONT, s.dlg_font_alert),
        (ids::INFO_STYLE + STYLE_FONT, s.dlg_font_info),
        (ids::PREVIEW, s.dlg_preview),
        (IDOK.0, s.dlg_ok),
        (IDCANCEL.0, s.dlg_cancel),
        (ids::APPLY, s.dlg_apply),
    ] {
        set_text(hwnd, id, text);
    }
    for (base, style) in [(ids::ALERT_STYLE, &state.cfg.alert.style), (ids::INFO_STYLE, &state.cfg.info.style)] {
        set_text(hwnd, base + STYLE_FONT_LABEL, &font_label(style));
        set_text(hwnd, base + STYLE_L_TEXT_COLOR, s.dlg_text_color);
        set_text(hwnd, base + STYLE_L_TEXT_OPACITY, s.dlg_opacity);
        set_text(hwnd, base + STYLE_L_PADDING, s.dlg_padding);
        set_text(hwnd, base + STYLE_L_BACKGROUND, s.dlg_background);
        set_text(hwnd, base + STYLE_L_BG_OPACITY, s.dlg_opacity);
        set_text(hwnd, base + STYLE_L_RADIUS, s.dlg_radius);
    }
    // Lists are rebuilt in the new language with the current choice kept.
    fill_anchor_combo(hwnd, ids::ALERT_ANCHOR, get_anchor(hwnd, ids::ALERT_ANCHOR));
    fill_anchor_combo(hwnd, ids::INFO_ANCHOR, get_anchor(hwnd, ids::INFO_ANCHOR));
    fill_language_combo(hwnd, get_language(hwnd));
    // A default message is shown in the language it will be displayed in.
    if state.cfg.alert.message.is_empty() {
        set_text(hwnd, ids::ALERT_MSG, s.alert_default_message);
    }
}

fn init_style(hwnd: HWND, base: i32, s: &TextStyle) {
    set_text(hwnd, base + STYLE_FONT_LABEL, &font_label(s));
    set_int(hwnd, base + STYLE_TEXT_OPACITY, base + STYLE_TEXT_OPACITY_SPIN, s.text_opacity.into(), 0, 100);
    set_int(hwnd, base + STYLE_BG_OPACITY, base + STYLE_BG_OPACITY_SPIN, s.background_opacity.into(), 0, 100);
    set_int(hwnd, base + STYLE_PADDING, base + STYLE_PADDING_SPIN, s.padding.into(), 0, 200);
    set_int(hwnd, base + STYLE_RADIUS, base + STYLE_RADIUS_SPIN, s.corner_radius.into(), 0, 200);
}

fn read_style(hwnd: HWND, base: i32, s: &mut TextStyle) -> Result<(), (i32, String)> {
    let n = t();
    s.text_opacity = get_int(hwnd, base + STYLE_TEXT_OPACITY, n.field_text_opacity, 0, 100)? as u8;
    s.background_opacity = get_int(hwnd, base + STYLE_BG_OPACITY, n.field_bg_opacity, 0, 100)? as u8;
    s.padding = get_int(hwnd, base + STYLE_PADDING, n.field_padding, 0, 200)? as u16;
    s.corner_radius = get_int(hwnd, base + STYLE_RADIUS, n.field_radius, 0, 200)? as u16;
    Ok(())
}

fn init_controls(hwnd: HWND, state: &mut State) {
    let c = &state.cfg;
    unsafe {
        if let Ok(icon) = LoadIconW(GetModuleHandleW(None).ok().map(Into::into), PCWSTR(ids::IDI_APP as usize as *const u16)) {
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_BIG as usize)), Some(LPARAM(icon.0 as isize)));
            SendMessageW(hwnd, WM_SETICON, Some(WPARAM(ICON_SMALL as usize)), Some(LPARAM(icon.0 as isize)));
        }
    }
    set_int(hwnd, ids::THRESHOLD, ids::THRESHOLD_SPIN, c.threshold_percent.into(), 0, 100);
    set_check(hwnd, ids::SUPPRESS, c.suppress_while_charging);
    set_int(hwnd, ids::POLL, ids::POLL_SPIN, c.poll_interval_s as i32, 5, 3600);
    set_check(hwnd, ids::AUTOSTART, autostart::is_enabled());
    fill_language_combo(hwnd, c.language);

    // Hotkey control: require Ctrl or Alt, like the config parser does.
    send(hwnd, ids::HOTKEY, HKM_SETRULES, (HKCOMB_NONE | HKCOMB_S) as usize, (HOTKEYF_CONTROL | HOTKEYF_ALT) as isize);
    let control_value = Hotkey::parse(&c.hotkey).ok().flatten().map_or(0, |h| h.to_control());
    send(hwnd, ids::HOTKEY, HKM_SETHOTKEY, control_value as usize, 0);
    state.initial_hotkey_control = control_value;

    set_text(hwnd, ids::ALERT_MSG, c.alert.effective_message());
    fill_anchor_combo(hwnd, ids::ALERT_ANCHOR, c.alert.anchor);
    set_int(hwnd, ids::ALERT_MARGIN, ids::ALERT_MARGIN_SPIN, c.alert.margin, -2000, 2000);
    init_style(hwnd, ids::ALERT_STYLE, &c.alert.style);

    fill_anchor_combo(hwnd, ids::INFO_ANCHOR, c.info.anchor);
    set_int(hwnd, ids::INFO_MARGIN, ids::INFO_MARGIN_SPIN, c.info.margin, -2000, 2000);
    set_int(hwnd, ids::INFO_OFFX, ids::INFO_OFFX_SPIN, c.info.offset[0], -4000, 4000);
    set_int(hwnd, ids::INFO_OFFY, ids::INFO_OFFY_SPIN, c.info.offset[1], -4000, 4000);
    set_int(hwnd, ids::INFO_DURATION, ids::INFO_DURATION_SPIN, c.info.duration_ms as i32, 100, 60_000);
    init_style(hwnd, ids::INFO_STYLE, &c.info.style);
    translate(hwnd, state);
}

/// All dialog values as a config plus the autostart flag. On invalid input,
/// tells the user, focuses the field and returns None.
fn read_controls(hwnd: HWND, state: &State) -> Option<(Config, bool)> {
    let result = (|| -> Result<Config, (i32, String)> {
        let n = t();
        let mut c = state.cfg.clone();
        c.language = get_language(hwnd);
        c.threshold_percent = get_int(hwnd, ids::THRESHOLD, n.field_threshold, 0, 100)? as u8;
        c.suppress_while_charging = get_check(hwnd, ids::SUPPRESS);
        c.poll_interval_s = get_int(hwnd, ids::POLL, n.field_poll, 5, 3600)? as u32;
        let hk = send(hwnd, ids::HOTKEY, HKM_GETHOTKEY, 0, 0) as u16;
        c.hotkey = if hk == state.initial_hotkey_control {
            state.initial_hotkey.clone()
        } else {
            Hotkey::from_control(hk).map(|h| h.to_string()).unwrap_or_default()
        };
        if let Err(e) = Hotkey::parse(&c.hotkey) {
            return Err((ids::HOTKEY, fill!(n.err_hotkey, error = e)));
        }

        c.alert.message = get_text(hwnd, ids::ALERT_MSG);
        if c.alert.message.trim().is_empty() {
            return Err((ids::ALERT_MSG, n.err_message_empty.into()));
        }
        c.alert.anchor = get_anchor(hwnd, ids::ALERT_ANCHOR);
        c.alert.margin = get_int(hwnd, ids::ALERT_MARGIN, n.field_alert_margin, -2000, 2000)?;
        read_style(hwnd, ids::ALERT_STYLE, &mut c.alert.style)?;

        c.info.anchor = get_anchor(hwnd, ids::INFO_ANCHOR);
        c.info.margin = get_int(hwnd, ids::INFO_MARGIN, n.field_info_margin, -2000, 2000)?;
        c.info.offset = [
            get_int(hwnd, ids::INFO_OFFX, n.field_offset_x, -4000, 4000)?,
            get_int(hwnd, ids::INFO_OFFY, n.field_offset_y, -4000, 4000)?,
        ];
        c.info.duration_ms = get_int(hwnd, ids::INFO_DURATION, n.field_duration, 100, 60_000)? as u32;
        read_style(hwnd, ids::INFO_STYLE, &mut c.info.style)?;
        Ok(c.sanitize())
    })();
    match result {
        Ok(c) => Some((c, get_check(hwnd, ids::AUTOSTART))),
        Err((id, msg)) => {
            warn(hwnd, &msg);
            unsafe {
                if let Ok(ctrl) = GetDlgItem(Some(hwnd), id) {
                    let _ = SetFocus(Some(ctrl));
                }
            }
            None
        }
    }
}

// ---- fonts and colours ------------------------------------------------------------------

fn weight_name(w: u16) -> &'static str {
    let s = t();
    match w {
        0..=149 => s.weight_thin,
        150..=249 => s.weight_extra_light,
        250..=349 => s.weight_light,
        350..=449 => s.weight_regular,
        450..=549 => s.weight_medium,
        550..=649 => s.weight_semibold,
        650..=749 => s.weight_bold,
        750..=849 => s.weight_extra_bold,
        _ => s.weight_black,
    }
}

fn font_label(style: &TextStyle) -> String {
    let s = t();
    let mut label = fill!(s.font_label, font = style.font_family, size = style.size_pt, weight = weight_name(style.weight));
    if style.italic {
        label.push_str(s.font_italic);
    }
    label
}

fn pick_font(hwnd: HWND, s: &mut TextStyle) -> bool {
    let dpi = unsafe { GetDpiForSystem() } as f32;
    let mut lf = LOGFONTW {
        lfHeight: -((s.size_pt * dpi / 72.0).round() as i32),
        lfWeight: i32::from(s.weight),
        lfItalic: u8::from(s.italic),
        ..Default::default()
    };
    let face: Vec<u16> = s.font_family.encode_utf16().take(31).collect();
    lf.lfFaceName[..face.len()].copy_from_slice(&face);
    let mut cf = CHOOSEFONTW {
        lStructSize: size_of::<CHOOSEFONTW>() as u32,
        hwndOwner: hwnd,
        lpLogFont: &mut lf,
        Flags: CF_SCREENFONTS | CF_INITTOLOGFONTSTRUCT | CF_NOVERTFONTS | CF_NOSCRIPTSEL,
        ..Default::default()
    };
    if !unsafe { ChooseFontW(&mut cf) }.as_bool() {
        return false;
    }
    let len = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(lf.lfFaceName.len());
    s.font_family = String::from_utf16_lossy(&lf.lfFaceName[..len]);
    s.size_pt = (cf.iPointSize as f32 / 10.0).clamp(6.0, 200.0);
    s.weight = (lf.lfWeight.clamp(100, 900)) as u16;
    s.italic = lf.lfItalic != 0;
    true
}

fn colorref(hex: &str) -> COLORREF {
    let (r, g, b) = parse_color(hex).unwrap_or((0, 0, 0));
    COLORREF(u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16)
}

thread_local! {
    /// The colour picker's 16 custom colours, kept for the session.
    static CUSTOM_COLORS: Cell<[COLORREF; 16]> = const { Cell::new([COLORREF(0x00FF_FFFF); 16]) };
}

fn pick_color(hwnd: HWND, hex: &mut String, button: i32) {
    let mut custom = CUSTOM_COLORS.with(Cell::get);
    let mut cc = CHOOSECOLORW {
        lStructSize: size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: hwnd,
        rgbResult: colorref(hex),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    if unsafe { ChooseColorW(&mut cc) }.as_bool() {
        let v = cc.rgbResult.0;
        *hex = format_color((v as u8, (v >> 8) as u8, (v >> 16) as u8));
        unsafe {
            if let Ok(ctrl) = GetDlgItem(Some(hwnd), button) {
                let _ = InvalidateRect(Some(ctrl), None, true);
            }
        }
    }
    CUSTOM_COLORS.with(|c| c.set(custom));
}

fn draw_swatch(state: &State, item: &DRAWITEMSTRUCT) {
    let id = item.CtlID as i32;
    let hex = match id {
        x if x == ids::ALERT_STYLE + STYLE_TEXT_COLOR => &state.cfg.alert.style.text_color,
        x if x == ids::ALERT_STYLE + STYLE_BG_COLOR => &state.cfg.alert.style.background,
        x if x == ids::INFO_STYLE + STYLE_TEXT_COLOR => &state.cfg.info.style.text_color,
        x if x == ids::INFO_STYLE + STYLE_BG_COLOR => &state.cfg.info.style.background,
        _ => return,
    };
    unsafe {
        let rc = item.rcItem;
        let brush = CreateSolidBrush(colorref(hex));
        FillRect(item.hDC, &rc, brush);
        let _ = DeleteObject(brush.into());
        FrameRect(item.hDC, &rc, GetSysColorBrush(COLOR_BTNSHADOW));
        if item.itemState.0 & ODS_FOCUS.0 != 0 {
            let inner = RECT { left: rc.left + 3, top: rc.top + 3, right: rc.right - 3, bottom: rc.bottom - 3 };
            let _ = DrawFocusRect(item.hDC, &inner);
        }
    }
}
