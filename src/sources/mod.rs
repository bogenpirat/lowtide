//! Battery sources: one module per hardware family. The monitor drives every
//! registered source through [`BatterySource`] and never sees vendor details.

pub mod logitech_hidpp;

use windows::Win32::Foundation::HANDLE;

use crate::model::DeviceState;

pub trait BatterySource: Send {
    /// Short identifier, used as `DeviceId::source`.
    fn name(&self) -> &'static str;

    /// Re-discover hardware from scratch and read every device once.
    fn rescan(&mut self);

    /// Scheduled refresh of all known devices.
    fn poll(&mut self);

    /// Waitable handles that become signaled when the source has unsolicited
    /// input (battery events, link changes). Called before every wait; may be empty.
    fn wait_handles(&mut self) -> Vec<HANDLE>;

    /// Consume whatever input is ready without blocking.
    /// Returns true if any device state changed.
    fn process_pending(&mut self) -> bool;

    /// True if hardware vanished or appeared and a rescan is needed.
    fn needs_rescan(&self) -> bool;

    /// Current view of all devices this source knows about.
    fn devices(&self) -> Vec<DeviceState>;
}

/// All sources compiled into this build.
pub fn all() -> Vec<Box<dyn BatterySource>> {
    vec![Box::new(logitech_hidpp::LogitechHidpp::new(Box::new(
        logitech_hidpp::win32::Win32Bus,
    )))]
}
