//! Decoding of the three HID++ 2.0 battery features into [`BatteryInfo`].
//! Each feature's "get" reply and its unsolicited event share one payload layout.

use crate::model::{BatteryInfo, ChargeState};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryFeature {
    /// 0x1004: percentage + coarse level + charging state (PRO 2 LIGHTSPEED and newer).
    Unified,
    /// 0x1001: voltage + flags; percentage must be estimated (G903, G703, GPro era).
    Voltage,
    /// 0x1000: legacy percentage feature.
    Status,
}

impl BatteryFeature {
    /// In order of preference.
    pub const ALL: [BatteryFeature; 3] =
        [BatteryFeature::Unified, BatteryFeature::Voltage, BatteryFeature::Status];

    pub fn id(self) -> u16 {
        match self {
            BatteryFeature::Status => 0x1000,
            BatteryFeature::Voltage => 0x1001,
            BatteryFeature::Unified => 0x1004,
        }
    }

    /// Function number of the "read current state" call.
    pub fn get_function(self) -> u8 {
        match self {
            BatteryFeature::Unified => 1, // get_status (0 is get_capabilities)
            BatteryFeature::Voltage | BatteryFeature::Status => 0,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BatteryFeature::Unified => "0x1004 Unified Battery",
            BatteryFeature::Voltage => "0x1001 Battery Voltage",
            BatteryFeature::Status => "0x1000 Battery Status",
        }
    }

    pub fn decode(self, payload: &[u8]) -> Option<BatteryInfo> {
        match self {
            BatteryFeature::Unified => decode_unified(payload),
            BatteryFeature::Voltage => decode_voltage(payload),
            BatteryFeature::Status => decode_status(payload),
        }
    }
}

/// 0x1004 get_status: [state of charge %, level bitmask, charging status, external power].
pub fn decode_unified(p: &[u8]) -> Option<BatteryInfo> {
    if p.len() < 4 {
        return None;
    }
    let (soc, level, status, external) = (p[0], p[1], p[2], p[3]);
    let charge = match status {
        0 => ChargeState::Discharging,
        1 | 2 => ChargeState::Charging,
        3 => ChargeState::Full,
        4 => ChargeState::Error,
        _ => ChargeState::Unknown,
    };
    // Devices without state-of-charge support report 0% and only a coarse level.
    let (percent, estimated) = if soc == 0 && level != 0 {
        let approx = if level & 8 != 0 {
            90
        } else if level & 4 != 0 {
            50
        } else if level & 2 != 0 {
            20
        } else {
            5
        };
        (approx, true)
    } else {
        (soc.min(100), false)
    };
    Some(BatteryInfo {
        percent,
        estimated,
        millivolts: None,
        charge,
        external_power: external != 0,
    })
}

/// 0x1001 get_battery_info: [voltage mV (u16 BE), flags].
/// flags bit 7: external power; bits 0-1 (only valid with external power):
/// 0 charging, 1 full, 2 not charging, 3 error.
pub fn decode_voltage(p: &[u8]) -> Option<BatteryInfo> {
    if p.len() < 3 {
        return None;
    }
    let millivolts = u16::from_be_bytes([p[0], p[1]]);
    let flags = p[2];
    let external_power = flags & 0x80 != 0;
    let charge = if external_power {
        match flags & 0x03 {
            0 => ChargeState::Charging,
            1 => ChargeState::Full,
            2 => ChargeState::NotCharging,
            _ => ChargeState::Error,
        }
    } else {
        ChargeState::Discharging
    };
    Some(BatteryInfo {
        percent: percent_from_millivolts(millivolts),
        estimated: true,
        millivolts: Some(millivolts),
        charge,
        external_power,
    })
}

/// 0x1000 get_battery_level_status: [level %, next level %, status].
pub fn decode_status(p: &[u8]) -> Option<BatteryInfo> {
    if p.len() < 3 || p[0] == 0 {
        return None; // 0 means "unknown"
    }
    let charge = match p[2] {
        0 => ChargeState::Discharging,
        1 | 2 | 4 => ChargeState::Charging,
        3 => ChargeState::Full,
        5 | 6 => ChargeState::Error,
        _ => ChargeState::Unknown,
    };
    Some(BatteryInfo::new(p[0], charge))
}

/// Discharge curve of a single-cell Li-ion/LiPo battery, as used by Solaar
/// for 0x1001 devices: (millivolts, percent), descending.
const VOLTAGE_CURVE: [(u16, u8); 13] = [
    (4186, 100),
    (4067, 90),
    (3989, 80),
    (3922, 70),
    (3859, 60),
    (3811, 50),
    (3778, 40),
    (3751, 30),
    (3717, 20),
    (3671, 10),
    (3646, 5),
    (3579, 2),
    (3500, 0),
];

/// Linear interpolation on [`VOLTAGE_CURVE`], clamped to 0..=100.
pub fn percent_from_millivolts(mv: u16) -> u8 {
    let (top_mv, top_pct) = VOLTAGE_CURVE[0];
    let (bottom_mv, bottom_pct) = VOLTAGE_CURVE[VOLTAGE_CURVE.len() - 1];
    if mv >= top_mv {
        return top_pct;
    }
    if mv <= bottom_mv {
        return bottom_pct;
    }
    for pair in VOLTAGE_CURVE.windows(2) {
        let ((hi_mv, hi_pct), (lo_mv, lo_pct)) = (pair[0], pair[1]);
        if (lo_mv..=hi_mv).contains(&mv) {
            let span = f32::from(hi_mv - lo_mv);
            let frac = f32::from(mv - lo_mv) / span;
            let pct = f32::from(lo_pct) + f32::from(hi_pct - lo_pct) * frac;
            return pct.round() as u8;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_reports_percentage() {
        // Matches the PRO 2 LIGHTSPEED reading from logi_battery.py: 84%, full, discharging.
        let b = decode_unified(&[84, 8, 0, 0]).unwrap();
        assert_eq!((b.percent, b.estimated, b.charge), (84, false, ChargeState::Discharging));
        assert!(!b.external_power);
    }

    #[test]
    fn unified_level_only_is_estimated() {
        let b = decode_unified(&[0, 2, 0, 0]).unwrap();
        assert_eq!((b.percent, b.estimated), (20, true));
        let b = decode_unified(&[0, 1, 0, 0]).unwrap();
        assert_eq!((b.percent, b.estimated), (5, true));
    }

    #[test]
    fn unified_charging_with_external_power() {
        let b = decode_unified(&[40, 4, 1, 1]).unwrap();
        assert_eq!(b.charge, ChargeState::Charging);
        assert!(b.external_power);
        assert_eq!(decode_unified(&[100, 8, 3, 1]).unwrap().charge, ChargeState::Full);
    }

    #[test]
    fn unified_clamps_bogus_percentage() {
        assert_eq!(decode_unified(&[180, 8, 0, 0]).unwrap().percent, 100);
    }

    #[test]
    fn voltage_curve_points_and_interpolation() {
        assert_eq!(percent_from_millivolts(3811), 50);
        assert_eq!(percent_from_millivolts(4186), 100);
        assert_eq!(percent_from_millivolts(3500), 0);
        // Between (3717, 20) and (3671, 10): 3700 -> 16.3 -> 16
        assert_eq!(percent_from_millivolts(3700), 16);
    }

    #[test]
    fn voltage_curve_clamps() {
        assert_eq!(percent_from_millivolts(4400), 100);
        assert_eq!(percent_from_millivolts(3100), 0);
        assert_eq!(percent_from_millivolts(0), 0);
        assert_eq!(percent_from_millivolts(u16::MAX), 100);
    }

    #[test]
    fn voltage_curve_is_monotonic() {
        let mut last = 0;
        for mv in 3400..4300 {
            let p = percent_from_millivolts(mv);
            assert!(p >= last && p <= 100, "{mv} mV -> {p}");
            last = p;
        }
    }

    #[test]
    fn voltage_discharging() {
        // 3811 mV = 0x0EE3, no flags
        let b = decode_voltage(&[0x0E, 0xE3, 0x00]).unwrap();
        assert_eq!(b.percent, 50);
        assert!(b.estimated);
        assert_eq!(b.millivolts, Some(3811));
        assert_eq!(b.charge, ChargeState::Discharging);
        assert!(!b.external_power);
    }

    #[test]
    fn voltage_charge_bits_only_count_with_external_power() {
        // Status bits set without external power must be ignored.
        assert_eq!(decode_voltage(&[0x0E, 0xE3, 0x01]).unwrap().charge, ChargeState::Discharging);
        assert_eq!(decode_voltage(&[0x0E, 0xE3, 0x80]).unwrap().charge, ChargeState::Charging);
        assert_eq!(decode_voltage(&[0x10, 0x5A, 0x81]).unwrap().charge, ChargeState::Full);
        assert_eq!(decode_voltage(&[0x0E, 0xE3, 0x82]).unwrap().charge, ChargeState::NotCharging);
    }

    #[test]
    fn legacy_status() {
        let b = decode_status(&[30, 20, 0]).unwrap();
        assert_eq!((b.percent, b.charge), (30, ChargeState::Discharging));
        assert_eq!(decode_status(&[0, 0, 0]), None);
        assert_eq!(decode_status(&[90, 100, 1]).unwrap().charge, ChargeState::Charging);
    }

    #[test]
    fn truncated_payloads_are_rejected() {
        assert_eq!(decode_unified(&[1, 2, 3]), None);
        assert_eq!(decode_voltage(&[1, 2]), None);
        assert_eq!(decode_status(&[1]), None);
    }
}
