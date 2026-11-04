//! Byte transport abstraction (real HID handles or a simulator in tests) and
//! [`Link`], which layers HID++ request/response matching on top of it.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::WaitForMultipleObjects;

use super::frame::{build_request, match_reply, Frame, HidppError, Reply};

pub trait Transport: Send {
    fn write(&mut self, report: &[u8]) -> Result<(), HidppError>;

    /// Wait up to `timeout` for one input report. `Ok(None)` on timeout.
    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidppError>;

    /// A handle that is signaled once input is ready (arms a read if needed).
    fn wait_handle(&mut self) -> Option<HANDLE> {
        None
    }
}

/// One HID++-capable USB interface: the long-report collection we talk on,
/// plus (for receivers) the short-report collection carrying link notifications.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceDesc {
    pub long_path: String,
    pub short_path: Option<String>,
    pub pid: u16,
    /// Collection on the vendor page used by directly attached gaming devices (0xFF43).
    pub direct: bool,
}

/// Enumerates and opens HID++ interfaces.
pub trait Bus: Send {
    fn enumerate(&mut self) -> Vec<InterfaceDesc>;
    fn open(&mut self, path: &str) -> Result<Box<dyn Transport>, HidppError>;
}

/// Frames that arrived while waiting for a reply are kept for event processing,
/// up to this many (other software may be chatty on the same interface).
const STASH_LIMIT: usize = 64;

pub struct Link {
    long: Box<dyn Transport>,
    short: Option<Box<dyn Transport>>,
    stash: Vec<Frame>,
}

impl Link {
    pub fn new(long: Box<dyn Transport>, short: Option<Box<dyn Transport>>) -> Self {
        Link { long, short, stash: Vec::new() }
    }

    /// Send a HID++ 2.0 request and wait for its reply. Unrelated input that
    /// arrives meanwhile is stashed and returned by the next [`Link::drain`].
    pub fn request(
        &mut self,
        device: u8,
        feature_index: u8,
        function: u8,
        params: &[u8],
        timeout: Duration,
    ) -> Result<Vec<u8>, HidppError> {
        self.long.write(&build_request(device, feature_index, function, params))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(HidppError::Timeout);
            }
            let Some(raw) = self.next_input(remaining)? else { continue };
            let Some(frame) = Frame::parse(&raw) else { continue };
            match match_reply(&frame, device, feature_index, function) {
                Reply::Ok(params) => return Ok(params),
                Reply::Err(e) => return Err(e),
                Reply::Unrelated => {
                    if self.stash.len() < STASH_LIMIT {
                        self.stash.push(frame);
                    }
                }
            }
        }
    }

    /// Next input report from either collection. Receivers send HID++ 1.0
    /// errors (e.g. "no device in this slot") as short reports, which Windows
    /// delivers only to the short collection, so both must be watched.
    fn next_input(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidppError> {
        let Some(short) = self.short.as_mut() else {
            return self.long.read(timeout);
        };
        if let Some(raw) = short.read(Duration::ZERO)? {
            return Ok(Some(raw));
        }
        if let Some(raw) = self.long.read(Duration::ZERO)? {
            return Ok(Some(raw));
        }
        match (self.long.wait_handle(), short.wait_handle()) {
            (Some(a), Some(b)) => {
                let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
                unsafe { WaitForMultipleObjects(&[a, b], false, ms) };
                Ok(None) // the caller loops and picks up whatever became ready
            }
            _ => self.long.read(timeout),
        }
    }

    /// Look up a feature's index via IRoot. `None` if the device lacks it.
    pub fn feature_index(
        &mut self,
        device: u8,
        feature_id: u16,
        timeout: Duration,
    ) -> Result<Option<u8>, HidppError> {
        let reply = self.request(device, 0x00, 0, &feature_id.to_be_bytes(), timeout)?;
        Ok(reply.first().copied().filter(|&i| i != 0))
    }

    /// All input that is ready now (stashed frames first), without blocking.
    pub fn drain(&mut self) -> Result<Vec<Frame>, HidppError> {
        let mut frames = std::mem::take(&mut self.stash);
        for transport in std::iter::once(&mut self.long).chain(self.short.as_mut()) {
            while let Some(raw) = transport.read(Duration::ZERO)? {
                frames.extend(Frame::parse(&raw));
            }
        }
        Ok(frames)
    }

    pub fn wait_handles(&mut self) -> Vec<HANDLE> {
        std::iter::once(&mut self.long)
            .chain(self.short.as_mut())
            .filter_map(|t| t.wait_handle())
            .collect()
    }
}
