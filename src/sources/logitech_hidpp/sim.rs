//! In-memory HID++ simulator for tests: scripted receivers and devices that
//! answer IRoot, DeviceName, FW info and battery requests like real hardware.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::frame::{HidppError, LONG_LEN, REPORT_LONG, REPORT_SHORT};
use super::transport::{Bus, InterfaceDesc, Transport};

#[derive(Clone)]
pub struct SimDevice {
    pub name: String,
    pub kind: u8,
    pub unit_id: [u8; 4],
    /// Feature ids by index; index 0 must be IRoot (0x0000).
    pub features: Vec<u16>,
    pub battery: Arc<Mutex<Vec<u8>>>,
    /// A sleeping device does not answer at all.
    pub asleep: Arc<Mutex<bool>>,
}

impl SimDevice {
    pub fn new(name: &str, unit_id: [u8; 4], battery_feature: u16, payload: &[u8]) -> SimDevice {
        SimDevice {
            name: name.into(),
            kind: 3,
            unit_id,
            features: vec![0x0000, 0x0001, 0x0003, 0x0005, 0x1B04, battery_feature],
            battery: Arc::new(Mutex::new(payload.to_vec())),
            asleep: Arc::new(Mutex::new(false)),
        }
    }

    pub fn battery_index(&self) -> u8 {
        self.features.iter().position(|&f| (0x1000..=0x1004).contains(&f)).unwrap() as u8
    }

    pub fn set_battery(&self, payload: &[u8]) {
        *self.battery.lock().unwrap() = payload.to_vec();
    }

    pub fn set_asleep(&self, asleep: bool) {
        *self.asleep.lock().unwrap() = asleep;
    }

    fn respond(&self, feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>, u8> {
        let Some(&feature) = self.features.get(usize::from(feature_index)) else {
            return Err(0x06);
        };
        match (feature, function) {
            (0x0000, 0) => {
                let wanted = u16::from_be_bytes([params[0], params[1]]);
                let idx = self.features.iter().position(|&f| f == wanted).unwrap_or(0);
                Ok(vec![idx as u8, 0, 0])
            }
            (0x0000, 1) => Ok(vec![4, 2, params[2]]),
            (0x0003, 0) => {
                let u = self.unit_id;
                Ok(vec![1, u[0], u[1], u[2], u[3], 0, 0, 0x40, 0x87, 0, 0, 0, 0, 0, 0])
            }
            (0x0003, 1) => Ok(vec![0, b'M', b'P', b'M', 0x06, 0x01, 0x00, 0x12]),
            (0x0005, 0) => Ok(vec![self.name.len() as u8]),
            (0x0005, 1) => {
                let off = usize::from(params[0]).min(self.name.len());
                let end = (off + 16).min(self.name.len());
                Ok(self.name.as_bytes()[off..end].to_vec())
            }
            (0x0005, 2) => Ok(vec![self.kind]),
            (0x1004, 1) | (0x1001, 0) | (0x1000, 0) => Ok(self.battery.lock().unwrap().clone()),
            _ => Err(0x07),
        }
    }
}

/// Shared input queue of one simulated collection; tests push events into it.
pub type Queue = Arc<Mutex<VecDeque<Vec<u8>>>>;

pub struct SimInterface {
    pub desc: InterfaceDesc,
    pub receiver: bool,
    pub devices: HashMap<u8, SimDevice>,
    pub long_queue: Queue,
    pub short_queue: Queue,
}

impl SimInterface {
    pub fn receiver(pid: u16, slots: &[(u8, SimDevice)]) -> SimInterface {
        SimInterface {
            desc: InterfaceDesc {
                long_path: format!("sim-long-{pid:04x}"),
                short_path: Some(format!("sim-short-{pid:04x}")),
                pid,
                direct: false,
            },
            receiver: true,
            devices: slots.iter().cloned().collect(),
            long_queue: Queue::default(),
            short_queue: Queue::default(),
        }
    }

    /// A cabled device; `direct` selects the 0xFF43 page (newer) over 0xFF00 (older).
    pub fn wired(pid: u16, direct: bool, device: SimDevice) -> SimInterface {
        SimInterface {
            desc: InterfaceDesc {
                long_path: format!("sim-long-{pid:04x}"),
                short_path: None,
                pid,
                direct,
            },
            receiver: false,
            devices: HashMap::from([(0xFF, device)]),
            long_queue: Queue::default(),
            short_queue: Queue::default(),
        }
    }

    pub fn push_long(&self, bytes: &[u8]) {
        let mut r = bytes.to_vec();
        r.resize(LONG_LEN, 0);
        self.long_queue.lock().unwrap().push_back(r);
    }

    pub fn push_short(&self, bytes: &[u8]) {
        let mut r = bytes.to_vec();
        r.resize(7, 0);
        self.short_queue.lock().unwrap().push_back(r);
    }
}

#[derive(Default)]
pub struct SimBus {
    pub interfaces: Vec<Arc<SimInterface>>,
}

impl Bus for SimBus {
    fn enumerate(&mut self) -> Vec<InterfaceDesc> {
        self.interfaces.iter().map(|i| i.desc.clone()).collect()
    }

    fn open(&mut self, path: &str) -> Result<Box<dyn Transport>, HidppError> {
        for i in &self.interfaces {
            if i.desc.long_path == path {
                return Ok(Box::new(SimTransport { iface: i.clone(), short: false }));
            }
            if i.desc.short_path.as_deref() == Some(path) {
                return Ok(Box::new(SimTransport { iface: i.clone(), short: true }));
            }
        }
        Err(HidppError::Io("no such device".into()))
    }
}

struct SimTransport {
    iface: Arc<SimInterface>,
    short: bool,
}

impl Transport for SimTransport {
    fn write(&mut self, r: &[u8]) -> Result<(), HidppError> {
        assert!(!self.short, "requests go to the long collection");
        assert_eq!((r[0], r.len()), (REPORT_LONG, LONG_LEN));
        let (dev, fi, addr) = (r[1], r[2], r[3]);
        let iface = &self.iface;
        match iface.devices.get(&dev) {
            Some(d) if *d.asleep.lock().unwrap() => {}
            Some(d) => match d.respond(fi, addr >> 4, &r[4..]) {
                Ok(payload) => {
                    let mut out = vec![REPORT_LONG, dev, fi, addr];
                    out.extend(payload);
                    iface.push_long(&out);
                }
                Err(code) => iface.push_long(&[REPORT_LONG, dev, 0xFF, fi, addr, code]),
            },
            // Receivers answer for themselves and for empty slots with a HID++ 1.0 error.
            None if iface.receiver => {
                let code = if dev == 0xFF { 0x01 } else { 0x09 };
                let mut err = vec![REPORT_SHORT, dev, 0x8F, fi, addr, code, 0];
                err.resize(7, 0);
                iface.long_queue.lock().unwrap().push_back(err);
            }
            None => {}
        }
        Ok(())
    }

    fn read(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, HidppError> {
        let q = if self.short { &self.iface.short_queue } else { &self.iface.long_queue };
        Ok(q.lock().unwrap().pop_front())
    }
}
