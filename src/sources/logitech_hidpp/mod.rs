//! Logitech HID++ 2.0 devices, either cabled or behind a Unifying /
//! LIGHTSPEED / Bolt receiver. Discovery is generic (no product-id lists):
//! every Logitech HID++ long-report collection is probed.

pub mod battery;
pub mod device;
pub mod frame;
#[cfg(test)]
mod sim;
pub mod transport;
pub mod win32;

use std::time::SystemTime;

use windows::Win32::Foundation::HANDLE;

use self::device::{DeviceInfo, PING_TIMEOUT};
use self::frame::{Frame, HidppError, DIRECT_INDEX, NOTIFY_DEVICE_CONNECTION, REPORT_SHORT};
use self::transport::{Bus, InterfaceDesc, Link};
use super::BatterySource;
use crate::model::{BatteryInfo, Connection, DeviceId, DeviceState};

pub const SOURCE_NAME: &str = "logitech-hidpp";
const RECEIVER_SLOTS: std::ops::RangeInclusive<u8> = 1..=6;

struct Interface {
    desc: InterfaceDesc,
    link: Link,
    /// Is this a receiver (devices on slots 1..6) rather than a device itself?
    receiver: bool,
    dead: bool,
}

struct Tracked {
    iface: usize,
    index: u8,
    info: DeviceInfo,
    battery: Option<BatteryInfo>,
    last_update: Option<SystemTime>,
    stale: bool,
    /// Feature indices may have changed (error 6, reconnect); identify again on next poll.
    reprobe: bool,
}

pub struct LogitechHidpp {
    bus: Box<dyn Bus>,
    ifaces: Vec<Interface>,
    devices: Vec<Tracked>,
    rescan_needed: bool,
}

impl LogitechHidpp {
    pub fn new(bus: Box<dyn Bus>) -> Self {
        LogitechHidpp { bus, ifaces: Vec::new(), devices: Vec::new(), rescan_needed: false }
    }

    fn open_interfaces(&mut self) {
        self.ifaces.clear();
        for desc in self.bus.enumerate() {
            let Ok(long) = self.bus.open(&desc.long_path) else { continue };
            let short = desc.short_path.as_deref().and_then(|p| self.bus.open(p).ok());
            self.ifaces.push(Interface { desc, link: Link::new(long, short), receiver: false, dead: false });
        }
    }

    /// Find what answers on an interface: either a device at 0xFF, or a receiver with slots.
    fn discover(&mut self, iface: usize) {
        let ifc = &mut self.ifaces[iface];
        if !ifc.desc.direct {
            match device::ping(&mut ifc.link, DIRECT_INDEX, PING_TIMEOUT) {
                Ok(protocol) if protocol.0 >= 2 => {
                    self.identify(iface, DIRECT_INDEX, protocol);
                    return;
                }
                Err(e) if e.is_io() => {
                    ifc.dead = true;
                    return;
                }
                _ => ifc.receiver = true,
            }
            for slot in RECEIVER_SLOTS {
                self.probe_slot(iface, slot);
            }
        } else {
            self.probe_slot(iface, DIRECT_INDEX);
        }
    }

    fn probe_slot(&mut self, iface: usize, index: u8) {
        match device::ping(&mut self.ifaces[iface].link, index, PING_TIMEOUT) {
            Ok(protocol) if protocol.0 >= 2 => self.identify(iface, index, protocol),
            Err(e) if e.is_io() => self.ifaces[iface].dead = true,
            _ => {}
        }
    }

    fn identify(&mut self, iface: usize, index: u8, protocol: (u8, u8)) {
        let link = &mut self.ifaces[iface].link;
        let info = match device::probe(link, index, protocol) {
            Ok(info) => info,
            Err(e) => {
                self.ifaces[iface].dead |= e.is_io();
                return;
            }
        };
        let mut tracked = Tracked {
            iface,
            index,
            info,
            battery: None,
            last_update: None,
            stale: false,
            reprobe: false,
        };
        refresh(&mut self.ifaces[iface], &mut tracked);
        self.devices.retain(|d| !(d.iface == iface && d.index == index));
        self.devices.push(tracked);
    }

    fn refresh_at(&mut self, pos: usize) {
        let dev = &mut self.devices[pos];
        refresh(&mut self.ifaces[dev.iface], dev);
    }

    /// Handle one unsolicited (or foreign) frame. Returns true on state change.
    fn handle_frame(&mut self, iface: usize, frame: &Frame) -> bool {
        let pos = self.devices.iter().position(|d| d.iface == iface && d.index == frame.device);

        if frame.report_id == REPORT_SHORT && frame.sub_id == NOTIFY_DEVICE_CONNECTION {
            let link_up = frame.param(0) & 0x40 == 0;
            return match (pos, link_up) {
                (Some(p), true) => {
                    self.refresh_at(p);
                    true
                }
                (Some(p), false) => {
                    let was = std::mem::replace(&mut self.devices[p].stale, true);
                    !was
                }
                (None, true) if self.ifaces[iface].receiver => {
                    self.probe_slot(iface, frame.device);
                    true
                }
                (None, _) => false,
            };
        }

        let Some(p) = pos else { return false };
        let dev = &mut self.devices[p];
        let Some((feature, fi)) = dev.info.battery else { return false };
        if frame.sub_id != fi {
            return false;
        }
        // Event 0 (swid 0) is the battery notification. A reply to the get
        // function is equally good data, even when another program asked for it.
        let is_event = frame.function() == 0 && frame.sw_id() == 0;
        let is_reply = frame.function() == feature.get_function() && frame.sw_id() != 0;
        if !(is_event || is_reply) {
            return false;
        }
        let Some(b) = feature.decode(&frame.params) else { return false };
        let changed = dev.battery.as_ref() != Some(&b) || dev.stale;
        dev.battery = Some(b);
        dev.last_update = Some(SystemTime::now());
        dev.stale = false;
        changed
    }

    fn to_state(&self, dev: &Tracked) -> DeviceState {
        let ifc = &self.ifaces[dev.iface];
        let connection = if ifc.receiver {
            Connection::Receiver { receiver_pid: ifc.desc.pid, slot: dev.index }
        } else {
            Connection::Wired
        };
        let key = match dev.info.unit_id {
            Some(unit) => format!("unit:{}", device::unit_id_hex(unit)),
            None => format!("name:{}", dev.info.name),
        };
        let mut details = vec![(
            "Protocol".to_string(),
            format!("HID++ {}.{}", dev.info.protocol.0, dev.info.protocol.1),
        )];
        if let Some(fw) = &dev.info.firmware {
            details.push(("Firmware".into(), fw.clone()));
        }
        if let Some((feature, _)) = dev.info.battery {
            details.push(("Battery feature".into(), feature.label().into()));
        }
        if !ifc.receiver {
            details.push(("USB product id".into(), format!("{:04X}", ifc.desc.pid)));
        }
        if let Some(model) = &dev.info.model_id {
            details.push(("Model id".into(), model.clone()));
        }
        DeviceState {
            id: DeviceId { source: SOURCE_NAME, key },
            name: dev.info.name.clone(),
            kind: dev.info.kind.map(str::to_string),
            connection,
            battery: dev.battery.clone(),
            last_update: dev.last_update,
            stale: dev.stale,
            details,
        }
    }

    fn reap_dead(&mut self) {
        if self.ifaces.iter().any(|i| i.dead) {
            self.rescan_needed = true;
        }
    }
}

/// Re-read a device's battery (re-identifying it first if needed). On failure
/// the last value is kept and marked stale.
fn refresh(ifc: &mut Interface, dev: &mut Tracked) {
    if dev.reprobe {
        let info = device::ping(&mut ifc.link, dev.index, PING_TIMEOUT)
            .and_then(|p| device::probe(&mut ifc.link, dev.index, p));
        match info {
            Ok(info) => {
                dev.info = info;
                dev.reprobe = false;
            }
            Err(e) => {
                ifc.dead |= e.is_io();
                dev.stale = true;
                return;
            }
        }
    }
    let Some((feature, fi)) = dev.info.battery else { return };
    match device::read_battery(&mut ifc.link, dev.index, feature, fi) {
        Ok(b) => {
            dev.battery = Some(b);
            dev.last_update = Some(SystemTime::now());
            dev.stale = false;
        }
        Err(HidppError::Timeout) => dev.stale = true,
        Err(HidppError::Io(_)) => {
            ifc.dead = true;
            dev.stale = true;
        }
        Err(HidppError::Protocol { .. }) => {
            dev.stale = true;
            dev.reprobe = true;
        }
    }
}

impl BatterySource for LogitechHidpp {
    fn name(&self) -> &'static str {
        SOURCE_NAME
    }

    fn rescan(&mut self) {
        self.devices.clear();
        self.rescan_needed = false;
        self.open_interfaces();
        for i in 0..self.ifaces.len() {
            self.discover(i);
        }
        self.reap_dead();
    }

    fn poll(&mut self) {
        for pos in 0..self.devices.len() {
            self.refresh_at(pos);
        }
        // Devices that were asleep or off during the scan show up later.
        for i in 0..self.ifaces.len() {
            if !self.ifaces[i].receiver || self.ifaces[i].dead {
                continue;
            }
            for slot in RECEIVER_SLOTS {
                if !self.devices.iter().any(|d| d.iface == i && d.index == slot) {
                    self.probe_slot(i, slot);
                }
            }
        }
        self.reap_dead();
    }

    fn wait_handles(&mut self) -> Vec<HANDLE> {
        self.ifaces.iter_mut().filter(|i| !i.dead).flat_map(|i| i.link.wait_handles()).collect()
    }

    fn process_pending(&mut self) -> bool {
        let mut changed = false;
        for i in 0..self.ifaces.len() {
            if self.ifaces[i].dead {
                continue;
            }
            match self.ifaces[i].link.drain() {
                Ok(frames) => {
                    for frame in &frames {
                        changed |= self.handle_frame(i, frame);
                    }
                }
                Err(_) => self.ifaces[i].dead = true,
            }
        }
        self.reap_dead();
        changed
    }

    fn needs_rescan(&self) -> bool {
        self.rescan_needed
    }

    fn devices(&self) -> Vec<DeviceState> {
        self.devices.iter().map(|d| self.to_state(d)).collect()
    }
}

#[cfg(test)]
mod tests;
