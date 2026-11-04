#![windows_subsystem = "windows"]

mod alert;
mod autostart;
mod config;
mod hotkey;
mod model;
mod monitor;
mod probe;
mod sources;
mod ui;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, LPARAM, WPARAM};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, MessageBoxW, PostMessageW, MB_ICONERROR};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--probe") {
        std::process::exit(probe::run());
    }
    if let Some(i) = args.iter().position(|a| a == "--watch") {
        let secs = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(60);
        std::process::exit(probe::watch(secs));
    }
    let simulate = args
        .iter()
        .position(|a| a == "--simulate")
        .map(|i| args.get(i + 1).and_then(|s| s.parse::<u8>().ok()).unwrap_or(5));

    let open_settings = args.iter().any(|a| a == "--settings");

    // One instance per user session; starting again shows the status popup
    // (or, with --settings, the settings dialog) of the running instance.
    // The handle is intentionally leaked: it must live as long as the process.
    let _mutex = unsafe { CreateMutexW(None, false, w!("Local\\Lowtide.SingleInstance")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS && simulate.is_none() {
        unsafe {
            if let Ok(hwnd) = FindWindowW(ui::app::MAIN_CLASS, None) {
                let msg = if open_settings { ui::app::WM_APP_OPEN_SETTINGS } else { ui::app::WM_APP_SHOW_INFO };
                let _ = PostMessageW(Some(hwnd), msg, WPARAM(0), LPARAM(0));
            }
        }
        return;
    }

    if let Err(e) = ui::app::run(ui::app::Options { simulate, open_settings }) {
        let text: Vec<u16> = format!("Lowtide could not start:\n{e}").encode_utf16().chain(Some(0)).collect();
        unsafe {
            MessageBoxW(None, windows::core::PCWSTR(text.as_ptr()), w!("Lowtide"), MB_ICONERROR);
        }
        std::process::exit(1);
    }
}
