//! Discovering what a HID++ 2.0 device is and which battery feature it has.

use std::time::Duration;

use super::battery::BatteryFeature;
use super::frame::HidppError;
use super::transport::Link;
use crate::model::BatteryInfo;

pub const FEATURE_DEVICE_FW_INFO: u16 = 0x0003;
pub const FEATURE_DEVICE_NAME: u16 = 0x0005;
const FW_TYPE_APPLICATION: u8 = 0;

/// Presence pings are short: an empty or sleeping receiver slot should not stall a scan.
pub const PING_TIMEOUT: Duration = Duration::from_millis(500);
pub const REQUEST_TIMEOUT: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub protocol: (u8, u8),
    pub name: String,
    pub kind: Option<&'static str>,
    pub unit_id: Option<[u8; 4]>,
    pub model_id: Option<String>,
    pub firmware: Option<String>,
    /// Battery feature and its index on this device.
    pub battery: Option<(BatteryFeature, u8)>,
}

/// IRoot ping. Returns the HID++ protocol version; HID++ 1.0 endpoints
/// (such as a receiver itself) answer with a 1.0 error.
pub fn ping(link: &mut Link, index: u8, timeout: Duration) -> Result<(u8, u8), HidppError> {
    const MARKER: u8 = 0x5A;
    let r = link.request(index, 0x00, 1, &[0, 0, MARKER], timeout)?;
    match r.as_slice() {
        [major, minor, MARKER, ..] => Ok((*major, *minor)),
        _ => Err(HidppError::Protocol { code: 0, v1: false }),
    }
}

/// Full identification of a device that answered a ping.
pub fn probe(link: &mut Link, index: u8, protocol: (u8, u8)) -> Result<DeviceInfo, HidppError> {
    let t = REQUEST_TIMEOUT;
    let mut info = DeviceInfo {
        protocol,
        name: String::new(),
        kind: None,
        unit_id: None,
        model_id: None,
        firmware: None,
        battery: None,
    };

    if let Some(fi) = link.feature_index(index, FEATURE_DEVICE_NAME, t)? {
        let len = usize::from(*link.request(index, fi, 0, &[], t)?.first().unwrap_or(&0));
        let mut name = Vec::with_capacity(len);
        while name.len() < len {
            let chunk = link.request(index, fi, 1, &[name.len() as u8], t)?;
            if chunk.is_empty() {
                break;
            }
            name.extend_from_slice(&chunk);
        }
        name.truncate(len);
        info.name = String::from_utf8_lossy(&name).trim_end_matches('\0').trim().to_string();
        // Device type is optional (feature version >= 1); ignore failures.
        if let Ok(kind) = link.request(index, fi, 2, &[], t) {
            info.kind = kind.first().copied().and_then(kind_name);
        }
    }

    if let Some(fi) = link.feature_index(index, FEATURE_DEVICE_FW_INFO, t)? {
        let r = link.request(index, fi, 0, &[], t)?;
        if r.len() >= 13 {
            let unit = [r[1], r[2], r[3], r[4]];
            info.unit_id = (unit != [0; 4] && unit != [0xFF; 4]).then_some(unit);
            info.model_id = Some(hex(&r[7..13]));
        }
        // Entities include bootloader, hardware, sensors...; report the main application.
        let entities = r.first().copied().unwrap_or(0).min(8);
        for entity in 0..entities {
            match link.request(index, fi, 1, &[entity], t) {
                Ok(fw) if fw.first() == Some(&FW_TYPE_APPLICATION) => {
                    info.firmware = format_firmware(&fw);
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }

    for feature in BatteryFeature::ALL {
        if let Some(fi) = link.feature_index(index, feature.id(), t)? {
            info.battery = Some((feature, fi));
            break;
        }
    }

    if info.name.is_empty() {
        info.name = crate::i18n::t().generic_logitech_device.into();
    }
    Ok(info)
}

pub fn read_battery(
    link: &mut Link,
    index: u8,
    feature: BatteryFeature,
    feature_index: u8,
) -> Result<BatteryInfo, HidppError> {
    let r = link.request(index, feature_index, feature.get_function(), &[], REQUEST_TIMEOUT)?;
    feature.decode(&r).ok_or(HidppError::Protocol { code: 0, v1: false })
}

/// 0x0003 getFwInfo: [type, prefix (3 ASCII), number (BCD), revision (BCD), build (u16 BE BCD), ...]
fn format_firmware(r: &[u8]) -> Option<String> {
    if r.len() < 8 {
        return None;
    }
    let prefix = String::from_utf8_lossy(&r[1..4]).trim().to_string();
    let build = u16::from_be_bytes([r[6], r[7]]);
    Some(format!("{prefix} {:02X}.{:02X}.B{build:04X}", r[4], r[5]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// 0x0005 getDeviceType values.
fn kind_name(code: u8) -> Option<&'static str> {
    Some(match code {
        0 => "keyboard",
        1 => "remote control",
        2 => "numpad",
        3 => "mouse",
        4 => "trackpad",
        5 => "trackball",
        6 => "presenter",
        7 => "receiver",
        8 => "headset",
        9 => "webcam",
        10 => "steering wheel",
        11 => "joystick",
        12 => "gamepad",
        13 => "dock",
        14 => "speaker",
        15 => "microphone",
        _ => return None,
    })
}

pub fn unit_id_hex(unit: [u8; 4]) -> String {
    hex(&unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_formatting() {
        let fw = [0x00, b'R', b'Q', b'M', 0x06, 0x01, 0x00, 0x12, 0, 0];
        assert_eq!(format_firmware(&fw).as_deref(), Some("RQM 06.01.B0012"));
        assert_eq!(format_firmware(&[0, 1, 2]), None);
    }
}
