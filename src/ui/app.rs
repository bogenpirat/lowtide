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

use windows::core::{w, Result, PCWSTR};
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
    DEV_BROADCAST_DEVICEINTERFACE_W, HDEVNOTIFY, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG,
    TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CONTEXTMENU, WM_DESTROY,
    WM_DEVICECHANGE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_HOTKEY, WM_NULL, WM_SETTINGCHANGE,
    WM_TIMER, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use super::overlay::{self, Overlay, Placement};
use super::tray::{IconState, Tray};
use crate::alert::{alert_text, info_text, AlertState};
use crate::config::Config;
use crate::hotkey::Hotkey;
use crate::model::Snapshot;
use crate::monitor::{Command, Monitor};
use crate::sources::{self, BatterySource};

pub const MAIN_CLASS: PCWSTR = w!("LowtideMain");
/// Monitor -> UI: a new snapshot is in the shared slot.
const WM_APP_SNAPSHOT: u32 = WM_APP + 1;
const WM_APP_TRAY: u32 = WM_APP + 2;
/// Sent by a second instance: show the status popup instead of starting again.
pub const WM_APP_SHOW_INFO: u32 = WM_APP + 3;

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

pub struct Options {
    pub simulate: Option<u8>,
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
        tray.set(IconState::Unknown, "Lowtide: looking for devices…", true);

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
            app.tray.notify("Lowtide: config file has errors, using defaults", &e);
        }
        if let Err(e) = app.register_hotkey() {
            app.tray.notify("Lowtide: hotkey unavailable", &e);
        }
        APP.with(|cell| *cell.borrow_mut() = Some(app));

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
                .map_err(|_| format!("{hk} is already used by another program"))?;
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
            let text = alert_text(&self.config.alert.message, &low);
            let a = &self.config.alert;
            let placement = Placement { anchor: a.anchor, margin: a.margin, offset: [0, 0] };
            let _ = self.alert.show(&text, &a.style, &placement);
            if self.info.is_visible() {
                // A changed alert must not cover the popup the user just asked for.
                self.info.raise();
            }
        }
    }

    fn update_tray(&mut self) {
        let alerting = self.alert.is_visible();
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
        let mut tip = String::from("Lowtide");
        if self.snapshot.devices.is_empty() {
            tip.push_str(": no devices found");
        }
        for d in &self.snapshot.devices {
            tip.push('\n');
            tip.push_str(&d.name);
            match &d.battery {
                Some(b) => tip.push_str(&format!(": {}% ({})", b.percent, b.charge.label())),
                None => tip.push_str(": no battery info"),
            }
            if d.stale {
                tip.push_str(", not responding");
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

fn show_tray_menu(hwnd: HWND, at: POINT) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else { return 0 };
        let _ = AppendMenuW(menu, MF_STRING, MENU_STATUS, w!("Show battery status"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_REFRESH, w!("Refresh now"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, MENU_SETTINGS, w!("Settings…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quit Lowtide"));
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
        MENU_QUIT => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        _ => {}
    }
}

extern "system" fn main_wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let handled = match msg {
        WM_APP_SNAPSHOT => with_app(|a| a.on_snapshot()).is_some(),
        WM_APP_SHOW_INFO | WM_HOTKEY => with_app(|a| a.show_info()).is_some(),
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: event in LOWORD(lparam), anchor point in wparam.
            let event = (lparam.0 & 0xFFFF) as u32;
            let at = POINT { x: (wparam.0 & 0xFFFF) as i16 as i32, y: ((wparam.0 >> 16) & 0xFFFF) as i16 as i32 };
            match event {
                WM_CONTEXTMENU => {
                    // No app borrow is held while the menu's modal loop runs.
                    let cmd = show_tray_menu(hwnd, at);
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
