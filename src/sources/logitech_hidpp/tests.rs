use std::sync::Arc;

use super::sim::{SimBus, SimDevice, SimInterface};
use super::LogitechHidpp;
use crate::model::{ChargeState, Connection, Snapshot};
use crate::sources::BatterySource;

const G903_UNIT: [u8; 4] = [0xDE, 0xAD, 0x90, 0x03];
const PRO2_UNIT: [u8; 4] = [0x12, 0x34, 0x56, 0x78];

/// G903 reports 0x1001 Battery Voltage; 3811 mV = 50%.
fn g903(mv: u16, flags: u8) -> SimDevice {
    let [hi, lo] = mv.to_be_bytes();
    SimDevice::new("G903 LIGHTSPEED Wireless Gaming Mouse", G903_UNIT, 0x1001, &[hi, lo, flags])
}

fn pro2(percent: u8) -> SimDevice {
    SimDevice::new("PRO 2 LIGHTSPEED", PRO2_UNIT, 0x1004, &[percent, 8, 0, 0])
}

fn source(ifaces: Vec<Arc<SimInterface>>) -> LogitechHidpp {
    LogitechHidpp::new(Box::new(SimBus { interfaces: ifaces }))
}

fn snapshot(src: &LogitechHidpp) -> Snapshot {
    Snapshot::from_devices(src.devices())
}

#[test]
fn g903_behind_lightspeed_receiver() {
    let rx = Arc::new(SimInterface::receiver(0xC539, &[(2, g903(3811, 0x00))]));
    let mut src = source(vec![rx]);
    src.rescan();

    let snap = snapshot(&src);
    assert_eq!(snap.devices.len(), 1);
    let d = &snap.devices[0];
    assert_eq!(d.name, "G903 LIGHTSPEED Wireless Gaming Mouse");
    assert_eq!(d.kind.as_deref(), Some("mouse"));
    assert_eq!(d.connection, Connection::Receiver { receiver_pid: 0xC539, slot: 2 });
    let b = d.battery.as_ref().unwrap();
    assert_eq!((b.percent, b.estimated, b.millivolts), (50, true, Some(3811)));
    assert_eq!(b.charge, ChargeState::Discharging);
    assert!(!d.stale);
    assert!(d.details.iter().any(|(k, v)| k == "Firmware" && v == "MPM 06.01.B0012"));
}

#[test]
fn pushed_voltage_event_updates_without_polling() {
    let dev = g903(3811, 0x00);
    let rx = Arc::new(SimInterface::receiver(0xC539, &[(1, dev.clone())]));
    let mut src = source(vec![rx.clone()]);
    src.rescan();

    // 3671 mV = 10%, delivered as unsolicited event 0 (swid 0).
    let [hi, lo] = 3671u16.to_be_bytes();
    rx.push_long(&[0x11, 0x01, dev.battery_index(), 0x00, hi, lo, 0x00]);
    assert!(src.process_pending());
    assert_eq!(snapshot(&src).devices[0].battery.as_ref().unwrap().percent, 10);

    // Nothing new: no change reported.
    assert!(!src.process_pending());
}

#[test]
fn sleeping_device_keeps_last_value_and_goes_stale() {
    let dev = g903(3811, 0x00);
    let rx = Arc::new(SimInterface::receiver(0xC539, &[(1, dev.clone())]));
    let mut src = source(vec![rx]);
    src.rescan();

    dev.set_asleep(true);
    src.poll();
    let d = &snapshot(&src).devices[0];
    assert!(d.stale);
    assert_eq!(d.battery.as_ref().unwrap().percent, 50);

    dev.set_asleep(false);
    dev.set_battery(&[0x0E, 0x56, 0x00]); // 3670 mV
    src.poll();
    let d = &snapshot(&src).devices[0];
    assert!(!d.stale);
    assert_eq!(d.battery.as_ref().unwrap().percent, 10);
}

#[test]
fn same_mouse_on_cable_and_receiver_is_listed_once_as_wired() {
    // G903 plugged in by cable (USB PID C086, older 0xFF00 page, index 0xFF)
    // while its receiver link still answers.
    let rx = Arc::new(SimInterface::receiver(0xC539, &[(1, g903(3811, 0x00))]));
    let cable = Arc::new(SimInterface::wired(0xC086, false, g903(3900, 0x80)));
    let mut src = source(vec![rx, cable]);
    src.rescan();

    assert_eq!(src.devices().len(), 2, "both paths answer");
    let snap = snapshot(&src);
    assert_eq!(snap.devices.len(), 1);
    let d = &snap.devices[0];
    assert_eq!(d.connection, Connection::Wired);
    assert_eq!(d.battery.as_ref().unwrap().charge, ChargeState::Charging);
    assert!(d.details.iter().any(|(k, v)| k == "USB product id" && v == "C086"));
}

#[test]
fn pro2_direct_by_cable_uses_unified_battery() {
    let cable = Arc::new(SimInterface::wired(0xC09B, true, pro2(84)));
    let mut src = source(vec![cable]);
    src.rescan();
    let d = &snapshot(&src).devices[0];
    assert_eq!(d.connection, Connection::Wired);
    let b = d.battery.as_ref().unwrap();
    assert_eq!((b.percent, b.estimated), (84, false));
}

#[test]
fn both_supported_mice_on_one_receiver() {
    let rx = Arc::new(SimInterface::receiver(0xC543, &[(1, pro2(84)), (3, g903(3811, 0))]));
    let mut src = source(vec![rx]);
    src.rescan();
    let names: Vec<_> = snapshot(&src).devices.iter().map(|d| d.name.clone()).collect();
    assert_eq!(names, ["G903 LIGHTSPEED Wireless Gaming Mouse", "PRO 2 LIGHTSPEED"]);
}

#[test]
fn link_notifications_mark_stale_and_trigger_reread() {
    let dev = pro2(84);
    let rx = Arc::new(SimInterface::receiver(0xC543, &[(1, dev.clone())]));
    let mut src = source(vec![rx.clone()]);
    src.rescan();

    // 0x41 with flags bit 0x40 set: link lost.
    rx.push_short(&[0x10, 0x01, 0x41, 0x04, 0x40, 0x9B, 0x40]);
    assert!(src.process_pending());
    assert!(snapshot(&src).devices[0].stale);

    // Link back; the source re-reads immediately.
    dev.set_battery(&[83, 8, 0, 0]);
    rx.push_short(&[0x10, 0x01, 0x41, 0x04, 0x00, 0x9B, 0x40]);
    assert!(src.process_pending());
    let d = &snapshot(&src).devices[0];
    assert!(!d.stale);
    assert_eq!(d.battery.as_ref().unwrap().percent, 83);
}

#[test]
fn device_switched_on_after_scan_is_found_by_link_notification() {
    let dev = pro2(60);
    dev.set_asleep(true);
    let rx = Arc::new(SimInterface::receiver(0xC543, &[(2, dev.clone())]));
    let mut src = source(vec![rx.clone()]);
    src.rescan();
    assert!(src.devices().is_empty());

    dev.set_asleep(false);
    rx.push_short(&[0x10, 0x02, 0x41, 0x04, 0x00, 0x9B, 0x40]);
    assert!(src.process_pending());
    assert_eq!(snapshot(&src).devices[0].battery.as_ref().unwrap().percent, 60);
}

#[test]
fn device_switched_on_after_scan_is_found_by_poll() {
    let dev = pro2(60);
    dev.set_asleep(true);
    let rx = Arc::new(SimInterface::receiver(0xC543, &[(2, dev.clone())]));
    let mut src = source(vec![rx]);
    src.rescan();
    assert!(src.devices().is_empty());

    dev.set_asleep(false);
    src.poll();
    assert_eq!(src.devices().len(), 1);
}

#[test]
fn battery_replies_requested_by_other_software_are_used() {
    let dev = pro2(84);
    let rx = Arc::new(SimInterface::receiver(0xC543, &[(1, dev.clone())]));
    let mut src = source(vec![rx.clone()]);
    src.rescan();

    let fi = dev.battery_index();
    // G HUB (another swid) asked get_status (fn 1) and got 70%.
    rx.push_long(&[0x11, 0x01, fi, 0x13, 70, 4, 0, 0]);
    assert!(src.process_pending());
    assert_eq!(snapshot(&src).devices[0].battery.as_ref().unwrap().percent, 70);

    // Its get_capabilities reply (fn 0, swid != 0) must not be mistaken for status.
    rx.push_long(&[0x11, 0x01, fi, 0x03, 0x0F, 0x03, 0, 0]);
    assert!(!src.process_pending());
    assert_eq!(snapshot(&src).devices[0].battery.as_ref().unwrap().percent, 70);
}

#[test]
fn vanished_interface_requests_rescan() {
    let dev = pro2(84);
    let rx = Arc::new(SimInterface::receiver(0xC543, &[(1, dev)]));
    let mut src = source(vec![rx]);
    src.rescan();
    assert!(!src.needs_rescan());
    src.ifaces[0].dead = true;
    src.poll();
    assert!(src.needs_rescan());
}
