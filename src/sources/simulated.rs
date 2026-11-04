//! `--simulate N`: a fake device at N % so overlays, tray and hotkey can be
//! tried without (or with a full) battery.

use std::time::SystemTime;

use windows::Win32::Foundation::HANDLE;

use super::BatterySource;
use crate::model::{BatteryInfo, ChargeState, Connection, DeviceId, DeviceState};

pub struct Simulated {
    percent: u8,
}

impl Simulated {
    pub fn new(percent: u8) -> Simulated {
        Simulated { percent: percent.min(100) }
    }
}

impl BatterySource for Simulated {
    fn name(&self) -> &'static str {
        "simulated"
    }
    fn rescan(&mut self) {}
    fn poll(&mut self) {}
    fn wait_handles(&mut self) -> Vec<HANDLE> {
        Vec::new()
    }
    fn process_pending(&mut self) -> bool {
        false
    }
    fn needs_rescan(&self) -> bool {
        false
    }
    fn devices(&self) -> Vec<DeviceState> {
        vec![DeviceState {
            id: DeviceId { source: "simulated", key: "1".into() },
            name: "Simulated mouse".into(),
            kind: Some("mouse".into()),
            connection: Connection::Receiver { receiver_pid: 0, slot: 1 },
            battery: Some(BatteryInfo::new(self.percent, ChargeState::Discharging)),
            last_update: Some(SystemTime::now()),
            stale: false,
            details: vec![("Firmware".into(), "SIM 00.00.B0000".into())],
        }]
    }
}
