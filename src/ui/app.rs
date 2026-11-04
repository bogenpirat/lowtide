//! The UI thread: a hidden top-level window that receives snapshots from the
//! monitor, tray and hotkey input, device and display changes, and drives the
//! two overlays.
//!
//! The two overlays are independent windows. The alert's visibility depends
//! only on battery state; the info popup only on the hotkey and its timer.
//! Neither one ever hides or replaces the other.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use windows::core::{w, Result, HSTRING, PCWSTR};
use windows::Win32::Devices::HumanInterfaceDevice::HidD_GetHidGuid;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Direct2D::{D2D1CreateFactory, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED};
use windows::Win32::Graphics::DirectWrite::{DWriteCreateFactory, IDWriteFactory, DWRITE_FACTORY_TYPE_SHARED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows::Win32::UI::Shell::{NINF_KEY, NIN_SELECT};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow,
    DispatchMessageW, GetMessageW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassW,
    RegisterDeviceNotificationW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
    TrackPopupMenuEx, TranslateMessage, UnregisterDeviceNotification, DBT_DEVICEARRIVAL,
    DBT_DEVICEREMOVECOMPLETE, DBT_DEVTYP_DEVICEINTERFACE, DEVICE_NOTIFY_WINDOW_HANDLE,
    DEV_BROADCAST_DEVICEINTERFACE_W, HDEVNOTIFY, MF_CHECKED, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_DESTROY,
    WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_HOTKEY, WM_NULL, WM_SETTINGCHANGE,
    WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use windows::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_HOTKEY_CLASS, ICC_STANDARD_CLASSES, ICC_UPDOWN_CLASS, INITCOMMONCONTROLSEX,
};

use super::overlay::{self, Overlay, Placement};
use super::settings;
use super::tray::{IconState, Tray};
use crate::alert::{alert_text, info_text, AlertState};
use crate::autostart;
use crate::config::Config;
use crate::hotkey::Hotkey;
use crate::i18n::{self, fill, t};
use crate::model::{BatteryInfo, ChargeState, Connection, DeviceId, DeviceState, Snapshot};
use crate::monitor::{Command, Monitor};
use crate::sources::{self, BatterySource};

pub const MAIN_CLASS: PCWSTR = w!("LowtideMain");
/// Monitor -> UI: a new snapshot is in the shared slot.
const WM_APP_SNAPSHOT: u32 = WM_APP + 1;
const WM_APP_TRAY: u32 = WM_APP + 2;
/// Sent by a second instance: show the status popup instead of starting again.
pub const WM_APP_SHOW_INFO: u32 = WM_APP + 3;
/// Sent by `lowtide --settings` (or posted to ourselves at startup): open the settings dialog.
pub const WM_APP_OPEN_SETTINGS: u32 = WM_APP + 4;

/// Keyboard activation of the tray icon (not exported by the windows crate).
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
const HOTKEY_ID: i32 = 1;
const TIMER_INFO: usize = 1;
const TIMER_RESCAN: usize = 2;
/// Plug events come in bursts (one per HID collection); rescan once after they settle.
const RESCAN_DEBOUNCE_MS: u32 = 1500;

const MENU_STATUS: usize = 1;
const MENU_REFRESH: usize = 2;
const MENU_SETTINGS: usize = 3;
const MENU_QUIT: usize = 4;
const MENU_MUTE: usize = 5;

pub struct Options {
    pub simulate: Option<u8>,
    pub open_settings: bool,
}

struct App {
    hwnd: HWND,
    config: Config,
    alert: Overlay,
    info: Overlay,
    alert_state: AlertState,
    shared: Arc<Mutex<Snapshot>>,
    snapshot: Snapshot,
    monitor: Monitor,
    tray: Tray,
    hotkey_registered: bool,
    taskbar_created: u32,
    devnotify: Option<HDEVNOTIFY>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` on the app unless it is already borrowed further up the stack
/// (modal loops such as menus and dialogs dispatch messages re-entrantly).
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut app| app.as_mut().map(f)))
}

/// HWND is a raw pointer and not `Send`; the monitor only needs it to post messages.
#[derive(Clone, Copy)]
struct PostTarget(usize);

impl PostTarget {
    fn post(self, msg: u32) {
        unsafe {
            let _ = PostMessageW(Some(HWND(self.0 as *mut _)), msg, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn run(opts: Options) -> Result<()> {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_STANDARD_CLASSES | ICC_HOTKEY_CLASS | ICC_UPDOWN_CLASS,
        };
        let _ = InitCommonControlsEx(&icc);
        let instance = GetModuleHandleW(None)?;
        let wc = WNDCLASSW {
            lpfnWndProc: Some(main_wndproc),
            hInstance: instance.into(),
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        RegisterClassW(&wc);
        overlay::register_class()?;

        // A real (never shown) top-level window: message-only windows miss the
        // broadcasts we need (display changes, theme changes, TaskbarCreated).
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            MAIN_CLASS,
            w!("Lowtide"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            None,
        )?;

        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let (config, config_error) = Config::load();
        i18n::set_language(config.language.resolve());

        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let sources: Vec<Box<dyn BatterySource>> = match opts.simulate {
            Some(p) => vec![Box::new(sources::simulated::Simulated::new(p))],
            None => sources::all(),
        };
        let target = PostTarget(hwnd.0 as usize);
        let slot = shared.clone();
        let monitor = Monitor::spawn(sources, Duration::from_secs(config.poll_interval_s.into()), move |snap| {
            *slot.lock().unwrap() = snap;
            target.post(WM_APP_SNAPSHOT);
        });

        let mut tray = Tray::new(hwnd, WM_APP_TRAY);
        tray.set(IconState::Unknown, t().tray_scanning, true);

        let mut app = App {
            hwnd,
            alert: Overlay::new(d2d.clone(), dwrite.clone())?,
            info: Overlay::new(d2d, dwrite)?,
            config,
            alert_state: AlertState::default(),
            shared,
            snapshot: Snapshot::default(),
            monitor,
            tray,
            hotkey_registered: false,
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
            devnotify: register_hid_arrival(hwnd),
        };
        if let Some(e) = config_error {
            app.tray.notify(t().notify_config_error, &e);
        }
        if let Err(e) = app.register_hotkey() {
            app.tray.notify(t().notify_hotkey_unavailable, &e);
        }
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        if opts.open_settings {
            let _ = PostMessageW(Some(hwnd), WM_APP_OPEN_SETTINGS, WPARAM(0), LPARAM(0));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Dropping the app stops the monitor thread and removes the tray icon.
        APP.with(|cell| cell.borrow_mut().take());
    }
    Ok(())
}

fn register_hid_arrival(hwnd: HWND) -> Option<HDEVNOTIFY> {
    unsafe {
        let filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
            dbcc_classguid: HidD_GetHidGuid(),
            ..Default::default()
        };
        RegisterDeviceNotificationW(
            HANDLE(hwnd.0),
            (&filter as *const DEV_BROADCAST_DEVICEINTERFACE_W).cast(),
            DEVICE_NOTIFY_WINDOW_HANDLE,
        )
        .ok()
    }
}

impl App {
    fn register_hotkey(&mut self) -> std::result::Result<(), String> {
        unsafe {
            if self.hotkey_registered {
                let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_ID);
                self.hotkey_registered = false;
            }
            let Some(hk) = Hotkey::parse(&self.config.hotkey)? else { return Ok(()) };
            RegisterHotKey(Some(self.hwnd), HOTKEY_ID, hk.modifiers(), u32::from(hk.vk))
                .map_err(|_| fill!(t().hotkey_in_use, hotkey = hk))?;
            self.hotkey_registered = true;
        }
        Ok(())
    }

    fn on_snapshot(&mut self) {
        self.snapshot = self.shared.lock().unwrap().clone();
        self.update_alert();
        self.update_tray();
        if self.info.is_visible() {
            self.render_info();
        }
    }

    fn update_alert(&mut self) {
        let low = self.alert_state.update(&self.snapshot, &self.config);
        if low.is_empty() {
            self.alert.hide();
        } else {
            let text = alert_text(self.config.alert.effective_message(), &low);
            let a = &self.config.alert;
            let placement = Placement { anchor: a.anchor, margin: a.margin, offset: [0, 0] };
            let _ = self.alert.show(&text, &a.style, &placement);
            if self.info.is_visible() {
                // A changed alert must not cover the popup the user just asked for.
                self.info.raise();
            }
        }
    }

    /// Tray "mute": silence the current alert until the device is charged back up,
    /// or bring a muted alert back. Not saved anywhere, so a restart starts unmuted.
    fn toggle_mute(&mut self) {
        if self.alert_state.is_muted() {
            self.alert_state.unmute();
        } else {
            self.alert_state.mute();
        }
        self.update_alert();
        self.update_tray();
    }

    /// Menu state of the mute entry: None when nothing is low (greyed out),
    /// otherwise whether it is checked.
    fn mute_menu_state(&self) -> Option<bool> {
        self.alert_state.any_alerting().then(|| self.alert_state.is_muted())
    }

    fn update_tray(&mut self) {
        // Muting hides the overlay, not the fact that a battery is low.
        let alerting = self.alert_state.any_alerting();
        let lowest = self
            .snapshot
            .devices
            .iter()
            .filter_map(|d| d.battery.as_ref().map(|b| (d, b)))
            .min_by_key(|(_, b)| b.percent);
        let state = match lowest {
            None => IconState::Unknown,
            Some((_, b)) if alerting => IconState::Alert(b.percent),
            Some((_, b)) if b.charge.is_charging() => IconState::Charging(b.percent),
            Some((_, b)) => IconState::Normal(b.percent),
        };
        let s = t();
        let header = if self.snapshot.devices.is_empty() {
            s.tray_no_devices
        } else if self.alert_state.is_muted() {
            s.tray_muted
        } else {
            "Lowtide"
        };
        let mut tip = String::from(header);
        for d in &self.snapshot.devices {
            tip.push('\n');
            match &d.battery {
                Some(b) => tip.push_str(&fill!(s.tray_device, device = d.name, percent = b.percent, status = b.charge.label())),
                None => tip.push_str(&format!("{}: {}", d.name, s.info_no_battery)),
            }
            if d.stale {
                tip.push_str(s.tray_not_responding);
            }
        }
        self.tray.set(state, &tip, false);
    }

    fn render_info(&mut self) {
        let text = info_text(&self.snapshot, SystemTime::now());
        let i = &self.config.info;
        let placement = Placement { anchor: i.anchor, margin: i.margin, offset: i.offset };
        let _ = self.info.show(&text, &i.style, &placement);
    }

    /// Hotkey or tray click: show the popup now, restart its timer, and ask for
    /// fresh readings (the popup updates in place when they arrive).
    fn show_info(&mut self) {
        self.render_info();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_INFO, self.config.info.duration_ms, None);
        }
        self.monitor.send(Command::RefreshNow);
    }

    /// Settings dialog "Preview": both overlays with unsaved settings and sample data.
    fn preview(&mut self, cfg: &Config) {
        let sample = DeviceState {
            id: DeviceId { source: "preview", key: "1".into() },
            name: t().sample_device.into(),
            kind: Some("mouse".into()),
            connection: Connection::Receiver { receiver_pid: 0xC539, slot: 1 },
            battery: Some(BatteryInfo::new(cfg.threshold_percent.saturating_sub(1), ChargeState::Discharging)),
            last_update: Some(SystemTime::now()),
            stale: false,
            details: vec![],
        };
        let a = &cfg.alert;
        let placement = Placement { anchor: a.anchor, margin: a.margin, offset: [0, 0] };
        let _ = self.alert.show(&alert_text(a.effective_message(), &[&sample]), &a.style, &placement);

        let snap = if self.snapshot.devices.is_empty() {
            Snapshot { devices: vec![sample] }
        } else {
            self.snapshot.clone()
        };
        let i = &cfg.info;
        let placement = Placement { anchor: i.anchor, margin: i.margin, offset: i.offset };
        let _ = self.info.show(&info_text(&snap, SystemTime::now()), &i.style, &placement);
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_INFO, i.duration_ms, None);
        }
    }

    /// Back from a preview to what the real battery state says.
    fn end_preview(&mut self) {
        self.alert.hide();
        self.update_alert();
    }

    /// Activate and save new settings. On a hotkey conflict nothing changes.
    fn apply_config(&mut self, new: Config, start_with_windows: bool) -> std::result::Result<(), String> {
        let old = std::mem::replace(&mut self.config, new);
        i18n::set_language(self.config.language.resolve());
        if let Err(e) = self.register_hotkey() {
            self.config = old;
            i18n::set_language(self.config.language.resolve());
            let _ = self.register_hotkey();
            return Err(e);
        }
        if start_with_windows != autostart::is_enabled() {
            autostart::set(start_with_windows)
                .map_err(|e| fill!(t().autostart_failed, error = e.message()))?;
        }
        self.config.save().map_err(|e| fill!(t().save_failed, error = e))?;
        if old.poll_interval_s != self.config.poll_interval_s {
            self.monitor
                .send(Command::SetPollInterval(Duration::from_secs(self.config.poll_interval_s.into())));
        }
        self.update_alert();
        self.update_tray();
        if self.info.is_visible() {
            self.render_info();
        }
        Ok(())
    }

    fn relayout(&mut self) {
        self.alert.relayout();
        self.info.relayout();
        if self.info.is_visible() {
            self.info.raise();
        }
        self.tray.redraw();
    }

    fn on_timer(&mut self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
        match id {
            TIMER_INFO => self.info.hide(),
            TIMER_RESCAN => self.monitor.send(Command::Rescan),
            _ => {}
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            if self.hotkey_registered {
                let _ = UnregisterHotKey(Some(self.hwnd), HOTKEY_ID);
            }
            if let Some(n) = self.devnotify.take() {
                let _ = UnregisterDeviceNotification(n);
            }
        }
    }
}

fn show_tray_menu(hwnd: HWND, at: POINT, mute: Option<bool>) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let s = t();
        let _ = AppendMenuW(menu, MF_STRING, MENU_STATUS, &HSTRING::from(s.menu_status));
        let _ = AppendMenuW(menu, MF_STRING, MENU_REFRESH, &HSTRING::from(s.menu_refresh));
        let mute_flags = match mute {
            None => MF_STRING | MF_GRAYED,
            Some(true) => MF_STRING | MF_CHECKED,
            Some(false) => MF_STRING,
        };
        let _ = AppendMenuW(menu, mute_flags, MENU_MUTE, &HSTRING::from(s.menu_mute));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, &HSTRING::from(s.menu_settings));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, &HSTRING::from(s.menu_quit));
        // Required so the menu closes when the user clicks elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN).0,
            at.x,
            at.y,
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        cmd.0 as usize
    }
}

fn run_menu_command(hwnd: HWND, cmd: usize) {
    match cmd {
        MENU_STATUS => {
            with_app(|a| a.show_info());
        }
        MENU_REFRESH => {
            with_app(|a| a.monitor.send(Command::RefreshNow));
        }
        MENU_MUTE => {
            with_app(|a| a.toggle_mute());
        }
        MENU_SETTINGS => open_settings(hwnd),
        MENU_QUIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {}
    }
}

/// The dialog runs its own modal loop, so no app borrow may be held across it;
/// each hook borrows the app only for its own duration.
fn open_settings(hwnd: HWND) {
    let Some(cfg) = with_app(|a| a.config.clone()) else { return };
    let preview = |c: &Config| {
        with_app(|a| a.preview(c));
    };
    let end_preview = || {
        with_app(|a| a.end_preview());
    };
    let apply = |c: &Config, autostart: bool| {
        with_app(|a| a.apply_config(c.clone(), autostart)).unwrap_or_else(|| Err(t().app_busy.into()))
    };
    settings::show(hwnd, cfg, settings::Hooks { preview: &preview, end_preview: &end_preview, apply: &apply });
}

extern "system" fn main_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = match msg {
        WM_APP_SNAPSHOT => with_app(|a| a.on_snapshot()).is_some(),
        WM_APP_SHOW_INFO | WM_HOTKEY => with_app(|a| a.show_info()).is_some(),
        WM_APP_OPEN_SETTINGS => {
            open_settings(hwnd);
            true
        }
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: event in LOWORD(lparam), anchor point in wparam.
            let event = (lparam.0 & 0xFFFF) as u32;
            let at = POINT { x: (wparam.0 & 0xFFFF) as i16 as i32, y: ((wparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            match event {
                WM_CONTEXTMENU => {
                    // No app borrow is held while the menu's modal loop runs.
                    let mute = with_app(|a| a.mute_menu_state()).flatten();
                    let cmd = show_tray_menu(hwnd, at, mute);
                    run_menu_command(hwnd, cmd);
                }
                NIN_SELECT | NIN_KEYSELECT => {
                    with_app(|a| a.show_info());
                }
                _ => {}
            }
            true
        }
        WM_TIMER => with_app(|a| a.on_timer(wparam.0)).is_some(),
        WM_DEVICECHANGE => {
            if matches!(wparam.0 as u32, DBT_DEVICEARRIVAL | DBT_DEVICEREMOVECOMPLETE) {
                unsafe {
                    SetTimer(Some(hwnd), TIMER_RESCAN, RESCAN_DEBOUNCE_MS, None);
                }
            }
            true
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE | WM_DPICHANGED => {
            with_app(|a| a.relayout());
            false // let DefWindowProc see these too
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            true
        }
        _ => {
            let taskbar = with_app(|a| a.taskbar_created).unwrap_or(0);
            if taskbar != 0 && msg == taskbar {
                with_app(|a| a.tray.add());
                true
            } else {
                false
            }
        }
    };
    if handled { LRESULT(0) } else { unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) } }
}
