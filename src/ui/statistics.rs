//! Statistics window (template `IDD_STATISTICS` in res/app.rc).
//!
//! Modeless, so it can stay open next to everything else; the app pushes new
//! numbers with [`update`] whenever a snapshot arrives, and the window asks
//! for a refresh once a minute so "days ago" and the forecast stay current.
//! It only renders: all numbers come from `stats.rs`.

use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{COLORREF, FILETIME, HWND, LPARAM, RECT, SYSTEMTIME, WPARAM};
use windows::Win32::Globalization::{GetDateFormatEx, DATE_SHORTDATE};
use windows::Win32::Graphics::Gdi::{
    CreateSolidBrush, DeleteObject, DrawTextW, FillRect, FrameRect, GetSysColor, GetSysColorBrush, InvalidateRect,
    SelectObject, SetBkMode, SetTextColor, COLOR_BTNSHADOW, COLOR_GRAYTEXT, COLOR_HIGHLIGHT, COLOR_WINDOW,
    DT_CALCRECT, DT_CENTER, DT_LEFT, DT_RIGHT, DT_SINGLELINE, DT_VCENTER, HFONT, HGDIOBJ, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::Controls::{
    DRAWITEMSTRUCT, LVCFMT_LEFT, LVCFMT_RIGHT, LVCF_FMT, LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_TEXT, LVITEMW,
    LVM_DELETEALLITEMS, LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETCOLUMNW, LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVM_SETITEMTEXTW, LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateDialogParamW, DestroyWindow, GetClientRect, GetDlgItem, GetWindowLongPtrW, IsDialogMessageW, IsIconic,
    PostMessageW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow,
    CBN_SELCHANGE, CB_ADDSTRING, CB_GETCURSEL, CB_RESETCONTENT, CB_SETCURSEL, IDCANCEL, MSG, SW_RESTORE, SW_SHOW,
    WINDOW_LONG_PTR_INDEX, WM_COMMAND, WM_DRAWITEM, WM_GETFONT, WM_INITDIALOG, WM_NCDESTROY, WM_TIMER,
};

use super::settings::{send, set_app_icon, set_text};
use crate::history::{device_key, Sample, SampleState};
use crate::i18n::{fill, t};
use crate::model::Snapshot;
use crate::stats::{self, DeviceReport, DeviceStats, FullChargeSource, Interval, CHART_DAYS, DAY};

/// Control ids, mirrored from res/resource.h.
mod ids {
    pub const IDD_STATISTICS: u16 = 101;
    pub const DEVICE: i32 = 2001;
    pub const CURRENT: i32 = 2002;
    pub const LASTS: i32 = 2003;
    pub const ALERT: i32 = 2004;
    pub const FULL: i32 = 2005;
    pub const BASIS: i32 = 2006;
    pub const TABLE: i32 = 2007;
    pub const LAST_CHARGE: i32 = 2008;
    pub const BETWEEN: i32 = 2009;
    pub const CHARGE_TIME: i32 = 2010;
    pub const LOWEST: i32 = 2011;
    pub const SINCE: i32 = 2012;
    pub const CHART: i32 = 2013;
    pub const L_DEVICE: i32 = 2101;
    pub const L_FORECAST: i32 = 2102;
    pub const L_USAGE: i32 = 2103;
    pub const L_CHARGING: i32 = 2104;
    pub const L_CHART: i32 = 2105;
}

const TIMER_REFRESH: usize = 1;
const REFRESH_MS: u32 = 60_000;
/// Width of each table column, in percent of the table.
const COLUMN_WIDTHS: [i32; 5] = [32, 17, 17, 16, 18];
const DASH: &str = "–";

/// Everything the window shows, computed by the app.
pub struct Input {
    pub reports: Vec<DeviceReport>,
    /// Devices answering right now (`source:key`).
    pub connected: Vec<String>,
    pub threshold: u8,
    pub now: i64,
}

impl Input {
    pub fn build(samples: &[Sample], snap: &Snapshot, threshold: u8) -> Input {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        Input {
            reports: stats::reports(samples, now, threshold, &local_day),
            connected: snap.devices.iter().filter(|d| !d.stale).map(|d| device_key(&d.id)).collect(),
            threshold,
            now,
        }
    }
}

struct State {
    input: Input,
    /// Device shown, by key, so it survives updates and re-sorting.
    selected: Option<String>,
    /// (key, name) of the devices in the combo box, to refill it only on change.
    listed: Vec<(String, String)>,
    owner: HWND,
    /// Posted to `owner` to ask for fresh numbers.
    refresh_msg: u32,
}

impl State {
    fn report(&self) -> Option<&DeviceReport> {
        let reports = &self.input.reports;
        self.selected
            .as_ref()
            .and_then(|key| reports.iter().find(|r| &r.device == key))
            .or_else(|| reports.first())
    }
}

thread_local! {
    static OPEN: Cell<isize> = const { Cell::new(0) };
    /// Set once the dialog took ownership of its state (in WM_INITDIALOG).
    static ADOPTED: Cell<bool> = const { Cell::new(false) };
}

fn open_hwnd() -> Option<HWND> {
    let open = OPEN.with(Cell::get);
    (open != 0).then(|| HWND(open as *mut _))
}

pub fn is_open() -> bool {
    open_hwnd().is_some()
}

/// Show the window, or bring it to the front with fresh numbers.
pub fn open(owner: HWND, refresh_msg: u32, input: Input) -> Result<(), String> {
    if let Some(hwnd) = open_hwnd() {
        update(input);
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
        return Ok(());
    }
    let state = Box::into_raw(Box::new(State { input, selected: None, listed: Vec::new(), owner, refresh_msg }));
    ADOPTED.with(|a| a.set(false));
    unsafe {
        let instance = GetModuleHandleW(None).ok().map(Into::into);
        match CreateDialogParamW(
            instance,
            PCWSTR(ids::IDD_STATISTICS as usize as *const u16),
            Some(owner),
            Some(dialog_proc),
            LPARAM(state as isize),
        ) {
            Ok(hwnd) => {
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetForegroundWindow(hwnd);
                Ok(())
            }
            Err(e) => {
                // Once adopted, the window freed the state when it was destroyed.
                if !ADOPTED.with(Cell::get) {
                    drop(Box::from_raw(state));
                }
                Err(fill!(t().stats_open_failed, error = e.message()))
            }
        }
    }
}

/// New numbers (or a new language) for the open window; nothing if it is closed.
pub fn update(input: Input) {
    let Some(hwnd) = open_hwnd() else { return };
    let Some(state) = state_of(hwnd) else { return };
    state.input = input;
    translate(hwnd);
    render(hwnd, state);
}

/// Keyboard navigation (Tab, Esc) for the modeless window; call from the message loop.
pub fn is_dialog_message(msg: &MSG) -> bool {
    open_hwnd().is_some_and(|hwnd| unsafe { IsDialogMessageW(hwnd, msg) }.as_bool())
}

/// DWLP_USER: the slot after DWLP_MSGRESULT and DWLP_DLGPROC.
const DWLP_USER: WINDOW_LONG_PTR_INDEX = WINDOW_LONG_PTR_INDEX((2 * size_of::<isize>()) as i32);

fn state_of<'a>(hwnd: HWND) -> Option<&'a mut State> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, DWLP_USER) } as *mut State;
    // The state lives until WM_NCDESTROY, and only the UI thread touches it.
    unsafe { ptr.as_mut() }
}

unsafe extern "system" fn dialog_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(hwnd, DWLP_USER, lparam.0);
            ADOPTED.with(|a| a.set(true));
            OPEN.with(|o| o.set(hwnd.0 as isize));
            set_app_icon(hwnd);
            init_table(hwnd);
            translate(hwnd);
            if let Some(state) = state_of(hwnd) {
                render(hwnd, state);
            }
            SetTimer(Some(hwnd), TIMER_REFRESH, REFRESH_MS, None);
            return 1;
        }
        if msg == WM_NCDESTROY {
            let ptr = SetWindowLongPtrW(hwnd, DWLP_USER, 0) as *mut State;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
            OPEN.with(|o| o.set(0));
            return 0;
        }
        let Some(state) = state_of(hwnd) else { return 0 };
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xFFFF) as i32;
                let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
                if id == IDCANCEL.0 {
                    let _ = DestroyWindow(hwnd);
                } else if id == ids::DEVICE && code == CBN_SELCHANGE {
                    let i = send(hwnd, ids::DEVICE, CB_GETCURSEL, 0, 0);
                    state.selected = state.input.reports.get(i.max(0) as usize).map(|r| r.device.clone());
                    render(hwnd, state);
                }
                1
            }
            WM_TIMER => {
                let _ = PostMessageW(Some(state.owner), state.refresh_msg, WPARAM(0), LPARAM(0));
                1
            }
            WM_DRAWITEM => {
                let item = &*(lparam.0 as *const DRAWITEMSTRUCT);
                if item.CtlID as i32 == ids::CHART {
                    draw_chart(item, state.report().map(|r| &r.stats));
                }
                1
            }
            _ => 0,
        }
    }
}

// ---- text -------------------------------------------------------------------------------

fn percent(n: impl std::fmt::Display) -> String {
    fill!(t().percent, n = n)
}

/// One decimal below 10, none above; ".0" is dropped. Uses the language's separator.
fn decimal(v: f64) -> String {
    let text = if v < 9.95 { format!("{v:.1}") } else { format!("{v:.0}") };
    let text = text.strip_suffix(".0").unwrap_or(&text).to_string();
    text.replace('.', t().decimal_separator)
}

fn days(d: f64) -> String {
    let s = t();
    if d < 1.0 {
        return fill!(s.duration_short_hours, n = (d * 24.0).round().max(1.0));
    }
    let n = decimal(d);
    if n == "1" { s.duration_one_day.into() } else { fill!(s.duration_days, n = n) }
}

fn hours(h: f64) -> String {
    if h < 1.0 {
        return fill!(t().duration_minutes, m = (h * 60.0).round().max(1.0));
    }
    fill!(t().duration_short_hours, n = decimal(h))
}

fn state_label(s: SampleState) -> &'static str {
    let n = t();
    match s {
        SampleState::Discharging => n.charge_discharging,
        SampleState::Charging => n.charge_charging,
        SampleState::Full => n.charge_full,
    }
}

/// Local calendar time of a unix timestamp, with the DST rules of that date.
fn local_time(unix: i64) -> Option<SYSTEMTIME> {
    let ticks = u64::try_from(unix.checked_add(11_644_473_600)?.checked_mul(10_000_000)?).ok()?;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(local)
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Local day number of a unix timestamp, for the per-day chart.
pub fn local_day(unix: i64) -> i64 {
    match local_time(unix) {
        Some(st) => days_from_civil(st.wYear.into(), st.wMonth.into(), st.wDay.into()),
        None => unix.div_euclid(DAY),
    }
}

/// Short date in the user's regional format.
fn date(unix: i64) -> String {
    let Some(st) = local_time(unix) else { return DASH.into() };
    let mut buf = [0u16; 80];
    let n = unsafe { GetDateFormatEx(PCWSTR::null(), DATE_SHORTDATE, Some(&st), PCWSTR::null(), Some(&mut buf), PCWSTR::null()) };
    if n <= 1 {
        return DASH.into();
    }
    String::from_utf16_lossy(&buf[..n as usize - 1])
}

fn translate(hwnd: HWND) {
    let s = t();
    unsafe {
        let _ = SetWindowTextW(hwnd, &HSTRING::from(s.stats_title));
    }
    for (id, text) in [
        (ids::L_DEVICE, s.stats_device),
        (ids::L_FORECAST, s.stats_forecast_group),
        (ids::L_USAGE, s.stats_usage_group),
        (ids::L_CHARGING, s.stats_charging_group),
        (ids::L_CHART, s.stats_chart_group),
        (IDCANCEL.0, s.stats_close),
    ] {
        set_text(hwnd, id, text);
    }
    for (i, text) in column_headers().iter().enumerate() {
        let mut wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let col = LVCOLUMNW { mask: LVCF_TEXT, pszText: PWSTR(wide.as_mut_ptr()), ..Default::default() };
        send(hwnd, ids::TABLE, LVM_SETCOLUMNW, i, &col as *const _ as isize);
    }
}

fn column_headers() -> [&'static str; 5] {
    let s = t();
    [s.stats_col_period, s.stats_col_used, s.stats_col_per_day, s.stats_col_charges, s.stats_col_covered]
}

/// Columns must be created with text: the header only draws text for items
/// that had one from the start (`HDF_STRING`).
fn init_table(hwnd: HWND) {
    let style = (LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER) as isize;
    send(hwnd, ids::TABLE, LVM_SETEXTENDEDLISTVIEWSTYLE, style as usize, style);
    let mut rc = RECT::default();
    unsafe {
        if let Ok(table) = GetDlgItem(Some(hwnd), ids::TABLE) {
            let _ = GetClientRect(table, &mut rc);
        }
    }
    let width = rc.right - rc.left;
    for (i, (share, text)) in COLUMN_WIDTHS.iter().zip(column_headers()).enumerate() {
        let mut wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let col = LVCOLUMNW {
            mask: LVCF_FMT | LVCF_WIDTH | LVCF_TEXT,
            fmt: if i == 0 { LVCFMT_LEFT } else { LVCFMT_RIGHT },
            cx: width * share / 100,
            pszText: PWSTR(wide.as_mut_ptr()),
            ..Default::default()
        };
        send(hwnd, ids::TABLE, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
    }
}

fn set_cell(hwnd: HWND, row: usize, col: usize, text: &str) {
    let mut wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let item = LVITEMW {
        mask: LVIF_TEXT,
        iItem: row as i32,
        iSubItem: col as i32,
        pszText: PWSTR(wide.as_mut_ptr()),
        ..Default::default()
    };
    let msg = if col == 0 { LVM_INSERTITEMW } else { LVM_SETITEMTEXTW };
    send(hwnd, ids::TABLE, msg, row, &item as *const _ as isize);
}

/// Rebuilt only when the devices change, so an open drop-down is left alone.
fn fill_devices(hwnd: HWND, state: &mut State) {
    let devices: Vec<(String, String)> =
        state.input.reports.iter().map(|r| (r.device.clone(), r.name.clone())).collect();
    if devices != state.listed {
        send(hwnd, ids::DEVICE, CB_RESETCONTENT, 0, 0);
        for (_, name) in &devices {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            send(hwnd, ids::DEVICE, CB_ADDSTRING, 0, wide.as_ptr() as isize);
        }
        state.listed = devices;
    }
    let shown = state.report().map(|r| r.device.as_str());
    if let Some(i) = state.input.reports.iter().position(|r| Some(r.device.as_str()) == shown) {
        send(hwnd, ids::DEVICE, CB_SETCURSEL, i, 0);
    }
}

fn render(hwnd: HWND, state: &mut State) {
    fill_devices(hwnd, state);
    send(hwnd, ids::TABLE, LVM_DELETEALLITEMS, 0, 0);
    let lines = match state.report() {
        Some(r) => {
            let connected = state.input.connected.contains(&r.device);
            device_lines(&r.stats, connected, state.input.threshold, state.input.now)
        }
        None => vec![(ids::CURRENT, t().stats_no_history.to_string())],
    };
    for id in [
        ids::CURRENT,
        ids::LASTS,
        ids::ALERT,
        ids::FULL,
        ids::BASIS,
        ids::LAST_CHARGE,
        ids::BETWEEN,
        ids::CHARGE_TIME,
        ids::LOWEST,
        ids::SINCE,
    ] {
        let text = lines.iter().find(|(i, _)| *i == id).map_or("", |(_, text)| text.as_str());
        set_text(hwnd, id, text);
    }
    if let Some(r) = state.report() {
        fill_table(hwnd, &r.stats);
    }
    unsafe {
        if let Ok(chart) = GetDlgItem(Some(hwnd), ids::CHART) {
            let _ = InvalidateRect(Some(chart), None, true);
        }
    }
}

/// The text of every line for one device, by control id.
fn device_lines(st: &DeviceStats, connected: bool, threshold: u8, now: i64) -> Vec<(i32, String)> {
    let s = t();
    let charge = if st.estimated { format!("~{}", percent(st.percent)) } else { percent(st.percent) };
    let status = state_label(st.state);
    let mut lines = vec![(
        ids::CURRENT,
        if connected {
            fill!(s.stats_current, percent = charge, status = status)
        } else {
            fill!(s.stats_last_reading, date = date(st.last_sample), percent = charge, status = status)
        },
    )];

    match &st.forecast {
        None => lines.push((ids::LASTS, s.stats_not_enough.into())),
        Some(f) => {
            let until = |d: f64| date(now + (d * DAY as f64) as i64);
            lines.push((
                ids::LASTS,
                match f.days_left {
                    Some(d) => fill!(s.stats_lasts, days = days(d), date = until(d)),
                    None => s.stats_lasts_charging.into(),
                },
            ));
            if let Some(d) = f.days_to_alert {
                lines.push((ids::ALERT, fill!(s.stats_alert_in, percent = percent(threshold), date = until(d), days = days(d))));
            } else if !st.state.on_power() {
                lines.push((ids::ALERT, fill!(s.stats_alert_below, percent = percent(threshold))));
            }
            let rate = percent(decimal(f.per_day));
            lines.push((
                ids::BASIS,
                match f.basis {
                    Interval::Week => fill!(s.stats_basis_week, rate = rate),
                    _ => fill!(s.stats_basis_all, rate = rate, date = date(st.tracked_since)),
                },
            ));
        }
    }
    if let Some(full) = &st.full_charge {
        lines.push((
            ids::FULL,
            match full.source {
                FullChargeSource::Cycles(n) => fill!(s.stats_full_cycles, days = days(full.days), cycles = n),
                FullChargeSource::Usage => fill!(s.stats_full_usage, days = days(full.days)),
            },
        ));
    }

    lines.push((
        ids::LAST_CHARGE,
        match &st.last_charge {
            None => s.stats_no_charge.into(),
            Some(c) if c.end.is_none() => fill!(s.stats_charging_since, date = date(c.start), from = percent(c.from)),
            Some(c) => fill!(
                s.stats_last_charge,
                date = date(c.start),
                ago = days((now - c.start).max(0) as f64 / DAY as f64),
                from = percent(c.from),
                to = percent(c.to)
            ),
        },
    ));
    if let Some(d) = st.days_between_charges {
        lines.push((ids::BETWEEN, fill!(s.stats_between, days = days(d))));
    }
    if let Some(h) = st.charge_hours {
        lines.push((ids::CHARGE_TIME, fill!(s.stats_charge_time, hours = hours(h))));
    }
    lines.push((ids::LOWEST, fill!(s.stats_lowest, percent = percent(st.lowest_30d))));
    lines.push((ids::SINCE, fill!(s.stats_since, date = date(st.tracked_since))));
    lines
}

fn fill_table(hwnd: HWND, st: &DeviceStats) {
    let s = t();
    for (row, i) in st.intervals.iter().enumerate() {
        let label = match i.interval {
            Interval::Day => s.stats_period_day,
            Interval::Week => s.stats_period_week,
            Interval::Month => s.stats_period_month,
            Interval::All => s.stats_period_all,
        };
        set_cell(hwnd, row, 0, label);
        set_cell(hwnd, row, 1, &percent(i.used));
        set_cell(hwnd, row, 2, &i.per_day.map_or(DASH.into(), |r| percent(decimal(r))));
        set_cell(hwnd, row, 3, &i.charges.to_string());
        set_cell(hwnd, row, 4, &if i.days > 0.0 { days(i.days) } else { DASH.into() });
    }
}

// ---- chart ------------------------------------------------------------------------------

fn draw_text(hdc: windows::Win32::Graphics::Gdi::HDC, text: &str, rc: &mut RECT, format: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT) {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    unsafe {
        DrawTextW(hdc, &mut wide, rc, format | DT_SINGLELINE);
    }
}

/// Charge used per day over the last 30 days: one bar per day, today on the right.
fn draw_chart(item: &DRAWITEMSTRUCT, stats: Option<&DeviceStats>) {
    let hdc = item.hDC;
    let rc = item.rcItem;
    unsafe {
        FillRect(hdc, &rc, GetSysColorBrush(COLOR_WINDOW));
        FrameRect(hdc, &rc, GetSysColorBrush(COLOR_BTNSHADOW));
        let font = HFONT(SendMessageW(item.hwndItem, WM_GETFONT, None, None).0 as *mut _);
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        SetBkMode(hdc, TRANSPARENT);
        SetTextColor(hdc, COLORREF(GetSysColor(COLOR_GRAYTEXT)));

        let mut probe = RECT::default();
        draw_text(hdc, "0", &mut probe, DT_CALCRECT);
        let text_h = probe.bottom - probe.top;
        let pad = 4;
        let plot = RECT {
            left: rc.left + pad,
            top: rc.top + pad + text_h + 2,
            right: rc.right - pad,
            bottom: rc.bottom - pad - text_h - 2,
        };
        let mut bottom_row = RECT { left: plot.left, top: plot.bottom + 2, right: plot.right, bottom: rc.bottom - pad };
        draw_text(hdc, t().stats_chart_start, &mut bottom_row, DT_LEFT);
        draw_text(hdc, t().stats_chart_today, &mut bottom_row, DT_RIGHT);

        let daily = stats.map_or([0; CHART_DAYS], |s| s.daily);
        let max = daily.iter().copied().max().unwrap_or(0);
        if max == 0 {
            let mut area = plot;
            draw_text(hdc, t().stats_chart_empty, &mut area, DT_CENTER | DT_VCENTER);
        } else {
            let mut top_row = RECT { left: plot.left, top: rc.top + pad, right: plot.right, bottom: plot.top };
            draw_text(hdc, &percent(max), &mut top_row, DT_LEFT);
            let bar = CreateSolidBrush(COLORREF(GetSysColor(COLOR_HIGHLIGHT)));
            let width = plot.right - plot.left;
            let height = plot.bottom - plot.top;
            for (i, &v) in daily.iter().enumerate() {
                if v == 0 {
                    continue;
                }
                let x0 = plot.left + width * i as i32 / CHART_DAYS as i32;
                let x1 = plot.left + width * (i as i32 + 1) / CHART_DAYS as i32;
                let gap = ((x1 - x0) / 5).max(1);
                let h = ((height as i64 * i64::from(v)) / i64::from(max)).max(1) as i32;
                let r = RECT { left: x0 + gap / 2, top: plot.bottom - h, right: x1 - (gap - gap / 2), bottom: plot.bottom };
                FillRect(hdc, &r, bar);
            }
            let _ = DeleteObject(bar.into());
        }
        let baseline = RECT { left: plot.left, top: plot.bottom, right: plot.right, bottom: plot.bottom + 1 };
        FillRect(hdc, &baseline, GetSysColorBrush(COLOR_BTNSHADOW));
        SelectObject(hdc, old_font);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_count_from_the_unix_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(days_from_civil(2024, 2, 29) + 1, days_from_civil(2024, 3, 1));
    }

    #[test]
    fn numbers_use_one_decimal_below_ten() {
        assert_eq!(decimal(4.96), "5");
        assert_eq!(decimal(3.44), "3.4");
        assert_eq!(decimal(12.6), "13");
        assert_eq!(days(0.25), "6 h");
        assert_eq!(days(1.02), "1 day");
        assert_eq!(days(2.5), "2.5 days");
        assert_eq!(hours(0.5), "30 min");
    }
}
