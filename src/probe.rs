//! `lowtide --probe`: scan once and print every device as JSON, then exit.
//! Handy for checking new hardware without the UI.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};

use crate::model::{DeviceState, Snapshot};
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

pub fn run() -> i32 {
    // GUI-subsystem binary: borrow the parent console so output is visible.
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
    let started = SystemTime::now();
    let snap = scan_once();
    let report: Vec<DeviceReport> = snap.devices.iter().map(DeviceReport::from).collect();
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    let ms = started.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    eprintln!("{} device(s), scan took {ms} ms", report.len());
    if report.is_empty() { 1 } else { 0 }
}
