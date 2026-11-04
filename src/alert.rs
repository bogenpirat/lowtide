//! What the overlays say: which devices are in alert (with hysteresis) and
//! the text of the alert and of the on-demand info popup. No UI code here.

use std::collections::BTreeSet;
use std::time::SystemTime;

use crate::config::Config;
use crate::i18n::{fill, t};
use crate::model::{DeviceId, DeviceState, Snapshot};

/// A device enters alert below the threshold and leaves only once it is this
/// many points above it, so voltage noise cannot make the alert flicker.
pub const HYSTERESIS_PERCENT: u8 = 2;

/// Which devices are in alert, and which of those the user muted.
///
/// Muting lasts until the device has been charged back up (to the same exit
/// level that ends an alert); the next drop below the threshold alerts again.
/// It is kept in memory only, so every start of the program begins unmuted.
#[derive(Default)]
pub struct AlertState {
    alerting: BTreeSet<DeviceId>,
    muted: BTreeSet<DeviceId>,
}

impl AlertState {
    /// Update from a new snapshot; returns the devices whose alert should be shown
    /// (in alert and not muted).
    pub fn update<'a>(&mut self, snap: &'a Snapshot, cfg: &Config) -> Vec<&'a DeviceState> {
        let exit_level = cfg.threshold_percent.saturating_add(HYSTERESIS_PERCENT);
        let mut now = BTreeSet::new();
        for d in &snap.devices {
            let Some(b) = &d.battery else { continue };
            // Charged back up: the mute has done its job. Charging alone is not
            // enough; a short top-up that stays low keeps the alert muted.
            if b.percent >= exit_level {
                self.muted.remove(&d.id);
            }
            let charging = b.charge.is_charging() || b.external_power;
            if cfg.suppress_while_charging && charging {
                continue;
            }
            let was = self.alerting.contains(&d.id);
            let alert = b.percent < cfg.threshold_percent || (was && b.percent < exit_level);
            if alert {
                now.insert(d.id.clone());
            }
        }
        self.alerting = now;
        snap.devices
            .iter()
            .filter(|d| self.alerting.contains(&d.id) && !self.muted.contains(&d.id))
            .collect()
    }

    /// Any device is low, whether its alert is muted or not.
    pub fn any_alerting(&self) -> bool {
        !self.alerting.is_empty()
    }

    /// A muted device is still low.
    pub fn is_muted(&self) -> bool {
        self.alerting.iter().any(|id| self.muted.contains(id))
    }

    /// Silence every device currently in alert.
    pub fn mute(&mut self) {
        self.muted.extend(self.alerting.iter().cloned());
    }

    pub fn unmute(&mut self) {
        self.muted.clear();
    }
}

/// Alert text: the configured template, one line per device in alert.
pub fn alert_text(template: &str, devices: &[&DeviceState]) -> String {
    devices
        .iter()
        .map(|d| {
            let b = d.battery.as_ref();
            template
                .replace("{device}", &d.name)
                .replace("{percent}", &b.map_or("?".into(), |b| b.percent.to_string()))
                .replace("{status}", b.map_or("unknown", |b| b.charge.label()))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Info popup text: everything worth knowing about every device.
pub fn info_text(snap: &Snapshot, now: SystemTime) -> String {
    if snap.devices.is_empty() {
        return t().info_no_devices.into();
    }
    let blocks: Vec<String> = snap
        .devices
        .iter()
        .map(|d| {
            let mut lines = Vec::new();
            match &d.battery {
                Some(b) => {
                    let approx = if b.estimated { "~" } else { "" };
                    lines.push(format!("{}  {approx}{}%", d.name, b.percent));
                    let mut status = b.charge.label().to_string();
                    if b.external_power && !b.charge.is_charging() {
                        status.push_str(t().info_external_power);
                    }
                    lines.push(format!("{status} · {}", d.connection.label()));
                    if let Some(mv) = b.millivolts {
                        lines.push(format!("{:.2} V", f32::from(mv) / 1000.0));
                    }
                }
                None => {
                    lines.push(d.name.clone());
                    lines.push(format!("{} · {}", t().info_no_battery, d.connection.label()));
                }
            }
            if d.stale {
                let ago = d.last_update.and_then(|t| now.duration_since(t).ok());
                lines.push(match ago {
                    Some(a) => fill!(t().info_not_responding_since, ago = human_duration(a.as_secs())),
                    None => t().info_not_responding.into(),
                });
            }
            if let Some((_, fw)) = d.details.iter().find(|(k, _)| k == "Firmware") {
                lines.push(fill!(t().info_firmware, version = fw));
            }
            lines.join("\n")
        })
        .collect();
    blocks.join("\n\n")
}

fn human_duration(secs: u64) -> String {
    let s = t();
    match secs {
        0..=59 => fill!(s.duration_seconds, s = secs),
        60..=3599 => fill!(s.duration_minutes, m = (secs / 60)),
        _ => fill!(s.duration_hours, h = (secs / 3600), m = (secs % 3600 / 60)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::model::{BatteryInfo, ChargeState, Connection};

    fn device(name: &str, percent: u8, charge: ChargeState) -> DeviceState {
        DeviceState {
            id: DeviceId { source: "t", key: name.into() },
            name: name.into(),
            kind: Some("mouse".into()),
            connection: Connection::Receiver { receiver_pid: 0xC543, slot: 1 },
            battery: Some(BatteryInfo::new(percent, charge)),
            last_update: None,
            stale: false,
            details: vec![("Firmware".into(), "MPM 31.04.B0012".into())],
        }
    }

    fn snap(devices: Vec<DeviceState>) -> Snapshot {
        Snapshot { devices }
    }

    fn alerting(state: &mut AlertState, devices: Vec<DeviceState>, cfg: &Config) -> Vec<String> {
        let s = snap(devices);
        state.update(&s, cfg).iter().map(|d| d.name.clone()).collect()
    }

    #[test]
    fn alert_below_threshold_with_hysteresis() {
        let cfg = Config { threshold_percent: 15, ..Config::default() };
        let mut st = AlertState::default();
        let d = |p| vec![device("m", p, ChargeState::Discharging)];
        assert!(alerting(&mut st, d(15), &cfg).is_empty());
        assert_eq!(alerting(&mut st, d(14), &cfg), ["m"]);
        // Noise back to 15/16 keeps the alert; 17 (= threshold + 2) clears it.
        assert_eq!(alerting(&mut st, d(15), &cfg), ["m"]);
        assert_eq!(alerting(&mut st, d(16), &cfg), ["m"]);
        assert!(alerting(&mut st, d(17), &cfg).is_empty());
        // And it does not re-enter at 16.
        assert!(alerting(&mut st, d(16), &cfg).is_empty());
    }

    #[test]
    fn mute_lasts_until_charged_back_above_threshold() {
        let cfg = Config { threshold_percent: 15, ..Config::default() };
        let mut st = AlertState::default();
        let d = |p, c| vec![device("m", p, c)];
        let dis = ChargeState::Discharging;
        assert_eq!(alerting(&mut st, d(10, dis), &cfg), ["m"]);
        st.mute();
        assert!(alerting(&mut st, d(9, dis), &cfg).is_empty(), "muted while still low");
        assert!(st.is_muted() && st.any_alerting());
        // Noise around the threshold and a short top-up do not end the mute.
        assert!(alerting(&mut st, d(16, dis), &cfg).is_empty());
        assert!(alerting(&mut st, d(12, ChargeState::Charging), &cfg).is_empty());
        assert!(alerting(&mut st, d(12, dis), &cfg).is_empty());
        // Charged back up to the exit level: mute is over, next drop alerts again.
        assert!(alerting(&mut st, d(17, ChargeState::Charging), &cfg).is_empty());
        assert!(alerting(&mut st, d(40, dis), &cfg).is_empty());
        assert!(!st.is_muted());
        assert_eq!(alerting(&mut st, d(14, dis), &cfg), ["m"]);
    }

    #[test]
    fn mute_only_silences_devices_already_in_alert() {
        let cfg = Config { threshold_percent: 15, ..Config::default() };
        let mut st = AlertState::default();
        let dis = ChargeState::Discharging;
        alerting(&mut st, vec![device("a", 5, dis), device("b", 80, dis)], &cfg);
        st.mute();
        // "b" running low later is a new alert.
        assert_eq!(alerting(&mut st, vec![device("a", 5, dis), device("b", 10, dis)], &cfg), ["b"]);
    }

    #[test]
    fn unmute_shows_the_alert_again_and_fresh_state_is_unmuted() {
        let cfg = Config::default();
        let mut st = AlertState::default();
        let low = || vec![device("m", 5, ChargeState::Discharging)];
        alerting(&mut st, low(), &cfg);
        st.mute();
        assert!(alerting(&mut st, low(), &cfg).is_empty());
        st.unmute();
        assert_eq!(alerting(&mut st, low(), &cfg), ["m"]);
        // A restart creates a new state: nothing is muted.
        assert_eq!(alerting(&mut AlertState::default(), low(), &cfg), ["m"]);
    }

    #[test]
    fn charging_suppresses_alert_when_configured() {
        let mut cfg = Config { threshold_percent: 15, ..Config::default() };
        let mut st = AlertState::default();
        let charging = vec![device("m", 5, ChargeState::Charging)];
        assert!(alerting(&mut st, charging.clone(), &cfg).is_empty());
        cfg.suppress_while_charging = false;
        assert_eq!(alerting(&mut st, charging, &cfg), ["m"]);
    }

    #[test]
    fn threshold_zero_never_alerts_and_100_always_does() {
        let mut st = AlertState::default();
        let low = vec![device("m", 0, ChargeState::Discharging)];
        let cfg0 = Config { threshold_percent: 0, ..Config::default() };
        assert!(alerting(&mut st, low.clone(), &cfg0).is_empty());
        let cfg100 = Config { threshold_percent: 100, ..Config::default() };
        let full = vec![device("m", 99, ChargeState::Discharging)];
        assert_eq!(alerting(&mut st, full, &cfg100), ["m"]);
    }

    #[test]
    fn stale_devices_keep_alerting_on_last_value() {
        let cfg = Config::default();
        let mut st = AlertState::default();
        let mut d = device("m", 5, ChargeState::Discharging);
        d.stale = true;
        assert_eq!(alerting(&mut st, vec![d], &cfg), ["m"]);
    }

    #[test]
    fn alert_text_fills_placeholders_per_device() {
        let a = device("G903", 9, ChargeState::Discharging);
        let b = device("PRO 2", 4, ChargeState::Discharging);
        let text = alert_text("{device}: {percent}% ({status})", &[&a, &b]);
        assert_eq!(text, "G903: 9% (discharging)\nPRO 2: 4% (discharging)");
    }

    #[test]
    fn info_text_lists_details() {
        let mut d = device("PRO 2 LIGHTSPEED", 84, ChargeState::Discharging);
        d.battery.as_mut().unwrap().estimated = true;
        d.battery.as_mut().unwrap().millivolts = Some(3811);
        let text = info_text(&snap(vec![d]), SystemTime::now());
        assert_eq!(
            text,
            "PRO 2 LIGHTSPEED  ~84%\n\
             discharging · wireless (receiver C543, slot 1)\n\
             3.81 V\n\
             Firmware MPM 31.04.B0012"
        );
    }

    #[test]
    fn info_text_reports_stale_age() {
        let now = SystemTime::now();
        let mut d = device("m", 50, ChargeState::Discharging);
        d.stale = true;
        d.last_update = Some(now - Duration::from_secs(185));
        assert!(info_text(&snap(vec![d]), now).contains("not responding, last read 3 min ago"));
        assert_eq!(info_text(&Snapshot::default(), now), "No battery-powered devices found");
    }
}
