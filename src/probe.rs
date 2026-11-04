//! `lowtide --probe`: scan once and print every device as JSON, then exit.
//! Handy for checking new hardware without the UI.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

use crate::config::Config;
use crate::i18n::{self, fill, t};
use crate::model::{DeviceState, Snapshot};
use crate::monitor::Monitor;
use crate::sources;

#[derive(Serialize)]
struct DeviceReport<'a> {
    name: &'a str,
    kind: Option<&'a str>,
    id: String,
    connection: String,
    percent: Option<u8>,
    estimated: Option<bool>,
    millivolts: Option<u16>,
    charge: Option<&'static str>,
    external_power: Option<bool>,
    stale: bool,
    updated_unix: Option<u64>,
    details: Vec<(&'a str, &'a str)>,
}

impl<'a> From<&'a DeviceState> for DeviceReport<'a> {
    fn from(d: &'a DeviceState) -> Self {
        let b = d.battery.as_ref();
        DeviceReport {
            name: &d.name,
            kind: d.kind.as_deref(),
            id: format!("{}/{}", d.id.source, d.id.key),
            connection: d.connection.label(),
            percent: b.map(|b| b.percent),
            estimated: b.map(|b| b.estimated),
            millivolts: b.and_then(|b| b.millivolts),
            charge: b.map(|b| b.charge.label()),
            external_power: b.map(|b| b.external_power),
            stale: d.stale,
            updated_unix: d
                .last_update
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|t| t.as_secs()),
            details: d.details.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect(),
        }
    }
}

pub fn scan_once() -> Snapshot {
    let mut all = sources::all();
    for s in &mut all {
        s.rescan();
    }
    for s in &all {
        eprintln!("{}", fill!(t().cli_source_count, source = s.name(), count = s.devices().len()));
    }
    let mut snap = Snapshot::from_devices(all.iter().flat_map(|s| s.devices()));
    // A sleeping wireless device may miss the first ping; the traffic wakes it.
    if snap.devices.is_empty() || snap.devices.iter().any(|d| d.stale) {
        std::thread::sleep(Duration::from_millis(300));
        for s in &mut all {
            s.poll();
        }
        snap = Snapshot::from_devices(all.iter().flat_map(|s| s.devices()));
    }
    snap
}

/// Console output for a GUI-subsystem binary, in the configured language.
fn cli_setup() {
    // Borrow the parent console so output is visible.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    let cfg = Config::path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| Config::from_toml(&text).ok())
        .unwrap_or_default();
    i18n::set_language(cfg.language.resolve());
}

/// `lowtide --watch [seconds]`: run the real monitor and print one line per
/// published change (events, polls). Useful to verify push notifications.
pub fn watch(poll_secs: u64) -> i32 {
    cli_setup();
    let started = std::time::Instant::now();
    let _monitor = Monitor::spawn(sources::all(), Duration::from_secs(poll_secs), move |snap| {
        let secs = started.elapsed().as_secs_f32();
        let s = t();
        if snap.devices.is_empty() {
            println!("[{secs:8.1}s] {}", s.cli_no_devices);
        }
        for d in &snap.devices {
            let mut battery = d.battery.as_ref().map_or(s.info_no_battery.to_string(), |b| {
                format!("{}%{} {}", b.percent, if b.estimated { s.cli_estimated } else { "" }, b.charge.label())
            });
            if d.stale {
                battery.push_str(s.cli_stale);
            }
            let line = fill!(s.cli_device_line, device = d.name, connection = d.connection.label(), battery = battery);
            println!("[{secs:8.1}s] {line}");
        }
    });
    eprintln!("{}", fill!(t().cli_watching, s = poll_secs));
    loop {
        std::thread::park();
    }
}

pub fn run() -> i32 {
    cli_setup();
    let started = SystemTime::now();
    let snap = scan_once();
    let report: Vec<DeviceReport> = snap.devices.iter().map(DeviceReport::from).collect();
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    let ms = started.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    eprintln!("{}", fill!(t().cli_scan_summary, count = report.len(), ms = ms));
    if report.is_empty() { 1 } else { 0 }
}
