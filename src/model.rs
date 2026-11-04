//! Vendor-neutral description of battery-powered devices. Sources produce
//! these; the monitor, alert logic and UI consume only these types.

use std::time::SystemTime;

use crate::i18n::{fill, t};

/// Stable identity of a physical device, unique across all sources.
/// The same device reached through different paths (cable, receiver) must
/// produce the same id so it can be de-duplicated.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DeviceId {
    pub source: &'static str,
    pub key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeState {
    Discharging,
    Charging,
    Full,
    NotCharging,
    Error,
    Unknown,
}

impl ChargeState {
    pub fn label(self) -> &'static str {
        let s = t();
        match self {
            ChargeState::Discharging => s.charge_discharging,
            ChargeState::Charging => s.charge_charging,
            ChargeState::Full => s.charge_full,
            ChargeState::NotCharging => s.charge_not_charging,
            ChargeState::Error => s.charge_error,
            ChargeState::Unknown => s.charge_unknown,
        }
    }

    /// True while the battery is being (or has been) replenished from external power.
    pub fn is_charging(self) -> bool {
        matches!(self, ChargeState::Charging | ChargeState::Full)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Connection {
    /// Directly attached by cable.
    Wired,
    /// Attached through a wireless receiver; `slot` is the receiver-side index.
    Receiver { receiver_pid: u16, slot: u8 },
}

impl Connection {
    pub fn label(&self) -> String {
        match self {
            Connection::Wired => t().connection_wired.into(),
            Connection::Receiver { receiver_pid, slot } => fill!(t().connection_receiver, pid = format!("{receiver_pid:04X}"), slot = slot),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BatteryInfo {
    /// Charge in percent, always within 0..=100.
    pub percent: u8,
    /// True if `percent` was derived (from voltage or a coarse level) rather than reported.
    pub estimated: bool,
    pub millivolts: Option<u16>,
    pub charge: ChargeState,
    pub external_power: bool,
}

impl BatteryInfo {
    pub fn new(percent: u8, charge: ChargeState) -> Self {
        BatteryInfo {
            percent: percent.min(100),
            estimated: false,
            millivolts: None,
            charge,
            external_power: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceState {
    pub id: DeviceId,
    pub name: String,
    /// Device category, e.g. "mouse", if the source knows it.
    pub kind: Option<String>,
    pub connection: Connection,
    pub battery: Option<BatteryInfo>,
    /// When the battery was last read successfully.
    pub last_update: Option<SystemTime>,
    /// The device stopped answering; `battery` is the last known value.
    pub stale: bool,
    /// Additional facts worth showing on demand (firmware, protocol, ...).
    pub details: Vec<(String, String)>,
}

impl DeviceState {
    /// Higher is better when two entries describe the same physical device.
    fn preference(&self) -> (bool, bool, Option<SystemTime>) {
        (self.connection == Connection::Wired, !self.stale, self.last_update)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub devices: Vec<DeviceState>,
}

impl Snapshot {
    /// Merge entries that describe the same physical device, keeping the
    /// preferred one (wired over wireless, fresh over stale). Output is sorted by name.
    pub fn from_devices(devices: impl IntoIterator<Item = DeviceState>) -> Snapshot {
        let mut merged: Vec<DeviceState> = Vec::new();
        for dev in devices {
            match merged.iter_mut().find(|d| d.id == dev.id) {
                Some(existing) if dev.preference() > existing.preference() => *existing = dev,
                Some(_) => {}
                None => merged.push(dev),
            }
        }
        merged.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Snapshot { devices: merged }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(key: &str, connection: Connection, stale: bool) -> DeviceState {
        DeviceState {
            id: DeviceId { source: "test", key: key.into() },
            name: key.into(),
            kind: None,
            connection,
            battery: None,
            last_update: None,
            stale,
            details: vec![],
        }
    }

    #[test]
    fn merge_prefers_wired_entry_of_same_device() {
        let wireless = dev("a", Connection::Receiver { receiver_pid: 0xC539, slot: 1 }, false);
        let wired = dev("a", Connection::Wired, false);
        let snap = Snapshot::from_devices([wireless, wired.clone()]);
        assert_eq!(snap.devices, vec![wired]);
    }

    #[test]
    fn merge_prefers_fresh_over_stale() {
        let stale = dev("a", Connection::Receiver { receiver_pid: 1, slot: 1 }, true);
        let fresh = dev("a", Connection::Receiver { receiver_pid: 2, slot: 1 }, false);
        let snap = Snapshot::from_devices([stale, fresh.clone()]);
        assert_eq!(snap.devices, vec![fresh]);
    }

    #[test]
    fn merge_keeps_distinct_devices() {
        let snap = Snapshot::from_devices([
            dev("b", Connection::Wired, false),
            dev("a", Connection::Wired, false),
        ]);
        let names: Vec<_> = snap.devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn battery_percent_is_clamped() {
        assert_eq!(BatteryInfo::new(250, ChargeState::Unknown).percent, 100);
    }
}
