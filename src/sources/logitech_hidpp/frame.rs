//! HID++ report framing: building requests, parsing input reports and
//! recognising replies and errors (HID++ 1.0 and 2.0).

use std::fmt;

pub const REPORT_SHORT: u8 = 0x10;
pub const REPORT_LONG: u8 = 0x11;
pub const LONG_LEN: usize = 20;

/// Our software id, echoed back in replies. Non-zero so replies can be told
/// apart from unsolicited events (swid 0) and from other software (G HUB etc.).
pub const SW_ID: u8 = 0x0A;

/// Device index addressing a directly connected device (or the receiver itself).
pub const DIRECT_INDEX: u8 = 0xFF;

const ERROR_HIDPP20: u8 = 0xFF;
const ERROR_HIDPP10: u8 = 0x8F;

/// HID++ 1.0 sub-id: device connection / link status notification from a receiver.
pub const NOTIFY_DEVICE_CONNECTION: u8 = 0x41;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HidppError {
    /// The device answered with an error code. `v1` marks a HID++ 1.0 error
    /// (typically from a receiver about an empty or unreachable slot).
    Protocol { code: u8, v1: bool },
    Timeout,
    Io(String),
}

impl HidppError {
    pub fn is_io(&self) -> bool {
        matches!(self, HidppError::Io(_))
    }
}

impl fmt::Display for HidppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HidppError::Protocol { code, v1: false } => {
                let name = match code {
                    1 => "unknown",
                    2 => "invalid argument",
                    3 => "out of range",
                    4 => "hardware error",
                    5 => "logitech internal",
                    6 => "invalid feature index",
                    7 => "invalid function",
                    8 => "busy",
                    9 => "unsupported",
                    _ => "?",
                };
                write!(f, "HID++ 2.0 error {code}: {name}")
            }
            HidppError::Protocol { code, v1: true } => write!(f, "HID++ 1.0 error 0x{code:02X}"),
            HidppError::Timeout => f.write_str("no response"),
            HidppError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

/// A parsed HID++ input report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub report_id: u8,
    pub device: u8,
    /// Feature index (HID++ 2.0) or sub-id (HID++ 1.0).
    pub sub_id: u8,
    /// `function << 4 | software id` (HID++ 2.0) or register address (HID++ 1.0).
    pub address: u8,
    pub params: Vec<u8>,
}

impl Frame {
    pub fn parse(raw: &[u8]) -> Option<Frame> {
        if raw.len() < 4 || !matches!(raw[0], REPORT_SHORT | REPORT_LONG | 0x12) {
            return None;
        }
        Some(Frame {
            report_id: raw[0],
            device: raw[1],
            sub_id: raw[2],
            address: raw[3],
            params: raw[4..].to_vec(),
        })
    }

    pub fn function(&self) -> u8 {
        self.address >> 4
    }

    pub fn sw_id(&self) -> u8 {
        self.address & 0x0F
    }

    /// Byte `i` of the parameters, or 0 if the report was shorter.
    pub fn param(&self, i: usize) -> u8 {
        self.params.get(i).copied().unwrap_or(0)
    }
}

/// Build a long (20 byte) HID++ 2.0 request.
pub fn build_request(device: u8, feature_index: u8, function: u8, params: &[u8]) -> [u8; LONG_LEN] {
    let mut out = [0u8; LONG_LEN];
    out[0] = REPORT_LONG;
    out[1] = device;
    out[2] = feature_index;
    out[3] = (function << 4) | SW_ID;
    let n = params.len().min(LONG_LEN - 4);
    out[4..4 + n].copy_from_slice(&params[..n]);
    out
}

/// What an incoming frame means for an outstanding request.
#[derive(Debug, PartialEq, Eq)]
pub enum Reply {
    Ok(Vec<u8>),
    Err(HidppError),
    /// Not addressed to this request; may be an event worth processing.
    Unrelated,
}

pub fn match_reply(frame: &Frame, device: u8, feature_index: u8, function: u8) -> Reply {
    if frame.device != device {
        return Reply::Unrelated;
    }
    let func_sw = (function << 4) | SW_ID;
    let is_error = matches!(frame.sub_id, ERROR_HIDPP20 | ERROR_HIDPP10);
    if is_error && frame.address == feature_index && frame.param(0) == func_sw {
        return Reply::Err(HidppError::Protocol {
            code: frame.param(1),
            v1: frame.sub_id == ERROR_HIDPP10,
        });
    }
    if frame.sub_id == feature_index && frame.address == func_sw {
        return Reply::Ok(frame.params.clone());
    }
    Reply::Unrelated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_layout() {
        let r = build_request(1, 0x00, 0, &[0x10, 0x04]);
        assert_eq!(&r[..6], &[0x11, 0x01, 0x00, 0x0A, 0x10, 0x04]);
        assert_eq!(r.len(), 20);
        assert!(r[6..].iter().all(|&b| b == 0));
    }

    #[test]
    fn parse_rejects_short_and_foreign_reports() {
        assert_eq!(Frame::parse(&[0x11, 0x01, 0x02]), None);
        assert_eq!(Frame::parse(&[0x02, 0x01, 0x02, 0x03]), None);
    }

    #[test]
    fn matches_own_reply() {
        let f = Frame::parse(&[0x11, 0x01, 0x05, 0x1A, 0x54, 0x08]).unwrap();
        assert_eq!(match_reply(&f, 1, 5, 1), Reply::Ok(vec![0x54, 0x08]));
    }

    #[test]
    fn ignores_other_software_and_other_devices() {
        let other_sw = Frame::parse(&[0x11, 0x01, 0x05, 0x1C, 0x54]).unwrap();
        assert_eq!(match_reply(&other_sw, 1, 5, 1), Reply::Unrelated);
        let other_dev = Frame::parse(&[0x11, 0x02, 0x05, 0x1A, 0x54]).unwrap();
        assert_eq!(match_reply(&other_dev, 1, 5, 1), Reply::Unrelated);
        let event = Frame::parse(&[0x11, 0x01, 0x05, 0x00, 0x54]).unwrap();
        assert_eq!(match_reply(&event, 1, 5, 1), Reply::Unrelated);
    }

    #[test]
    fn decodes_v2_and_v1_errors() {
        let v2 = Frame::parse(&[0x11, 0x01, 0xFF, 0x05, 0x1A, 0x06]).unwrap();
        assert_eq!(
            match_reply(&v2, 1, 5, 1),
            Reply::Err(HidppError::Protocol { code: 6, v1: false })
        );
        let v1 = Frame::parse(&[0x10, 0x03, 0x8F, 0x00, 0x1A, 0x09, 0x00]).unwrap();
        assert_eq!(
            match_reply(&v1, 3, 0, 1),
            Reply::Err(HidppError::Protocol { code: 9, v1: true })
        );
    }
}
