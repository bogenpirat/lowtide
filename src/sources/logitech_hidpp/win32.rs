//! Real HID access on Windows: SetupAPI enumeration and overlapped I/O.
//! Each handle keeps one read permanently armed, so its event can be waited on
//! together with other handles and no input report is ever dropped by cancelling.

use std::collections::HashMap;
use std::mem::size_of;
use std::time::Duration;

use windows::core::{PCWSTR, HSTRING};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
    SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT,
    SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows::Win32::Devices::HumanInterfaceDevice::{
    HidD_FreePreparsedData, HidD_GetAttributes, HidD_GetHidGuid, HidD_GetPreparsedData,
    HidP_GetCaps, HIDD_ATTRIBUTES, HIDP_CAPS, HIDP_STATUS_SUCCESS, PHIDP_PREPARSED_DATA,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING, GENERIC_READ, GENERIC_WRITE, HANDLE,
    WAIT_OBJECT_0,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, ReadFile, WriteFile, FILE_FLAG_OVERLAPPED, FILE_FLAGS_AND_ATTRIBUTES,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

use super::frame::HidppError;
use super::transport::{Bus, InterfaceDesc, Transport};

const LOGITECH_VID: u16 = 0x046D;
/// Receivers and older wired devices: vendor page 0xFF00, usage 1 = short, 2 = long reports.
const RECEIVER_PAGE: u16 = 0xFF00;
const USAGE_SHORT: u16 = 0x0001;
const USAGE_LONG: u16 = 0x0002;
/// Directly attached newer gaming devices (e.g. PRO 2 LIGHTSPEED by cable).
const DIRECT_PAGE: u16 = 0xFF43;
const DIRECT_USAGE_LONG: u16 = 0x0302;

const WRITE_TIMEOUT_MS: u32 = 1000;
const READ_BUFFER: usize = 64;

pub struct Win32Bus;

impl Bus for Win32Bus {
    fn enumerate(&mut self) -> Vec<InterfaceDesc> {
        let mut longs = Vec::new();
        let mut shorts = HashMap::new();
        for path in hid_interface_paths() {
            let Some(c) = query_collection(&path) else { continue };
            if c.vid != LOGITECH_VID {
                continue;
            }
            match (c.usage_page, c.usage) {
                (RECEIVER_PAGE, USAGE_LONG) => longs.push((path, c.pid, false)),
                (DIRECT_PAGE, DIRECT_USAGE_LONG) => longs.push((path, c.pid, true)),
                (RECEIVER_PAGE, USAGE_SHORT) => {
                    shorts.insert(sibling_key(&path), path);
                }
                _ => {}
            }
        }
        let mut out: Vec<InterfaceDesc> = longs
            .into_iter()
            .map(|(long_path, pid, direct)| InterfaceDesc {
                short_path: shorts.get(&sibling_key(&long_path)).cloned(),
                long_path,
                pid,
                direct,
            })
            .collect();
        out.sort_by_key(|d| !d.direct);
        out
    }

    fn open(&mut self, path: &str) -> Result<Box<dyn Transport>, HidppError> {
        Ok(Box::new(HidHandle::open(path)?))
    }
}

/// Paths of collections on the same USB interface differ only in `&colNN` and in
/// the last component of the instance id; strip both to pair them up.
/// `\\?\hid#vid_046d&pid_c543&mi_02&col02#8&71cd9ff&0&0001#{guid}`
fn sibling_key(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('#').collect();
    if parts.len() < 3 {
        return lower;
    }
    let hw = match parts[1].find("&col") {
        Some(i) => &parts[1][..i],
        None => parts[1],
    };
    let instance = parts[2].rsplit_once('&').map_or(parts[2], |(head, _)| head);
    format!("{hw}#{instance}")
}

fn win_err(e: windows::core::Error) -> HidppError {
    HidppError::Io(e.message())
}

fn hid_interface_paths() -> Vec<String> {
    let mut paths = Vec::new();
    unsafe {
        let guid = HidD_GetHidGuid();
        let Ok(set) =
            SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE)
        else {
            return paths;
        };
        for index in 0.. {
            let mut iface = SP_DEVICE_INTERFACE_DATA {
                cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(set, None, &guid, index, &mut iface).is_err() {
                break;
            }
            let mut needed = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &iface, None, 0, Some(&mut needed), None);
            if needed == 0 {
                continue;
            }
            // u64 storage keeps the detail struct suitably aligned.
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            let detail = buf.as_mut_ptr().cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            if SetupDiGetDeviceInterfaceDetailW(set, &iface, Some(detail), needed, None, None).is_ok() {
                let start = std::ptr::addr_of!((*detail).DevicePath).cast::<u16>();
                let max = (needed as usize - 4) / 2;
                let wide = std::slice::from_raw_parts(start, max);
                let len = wide.iter().position(|&c| c == 0).unwrap_or(max);
                paths.push(String::from_utf16_lossy(&wide[..len]));
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
    paths
}

struct CollectionInfo {
    vid: u16,
    pid: u16,
    usage_page: u16,
    usage: u16,
}

/// Read VID/PID and top-level usage without requesting read/write access.
fn query_collection(path: &str) -> Option<CollectionInfo> {
    unsafe {
        let file = CreateFileW(
            &HSTRING::from(path),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()?;
        let result = (|| {
            let mut attrs = HIDD_ATTRIBUTES { Size: size_of::<HIDD_ATTRIBUTES>() as u32, ..Default::default() };
            if !HidD_GetAttributes(file, &mut attrs) {
                return None;
            }
            let mut pp = PHIDP_PREPARSED_DATA::default();
            if !HidD_GetPreparsedData(file, &mut pp) {
                return None;
            }
            let mut caps = HIDP_CAPS::default();
            let status = HidP_GetCaps(pp, &mut caps);
            let _ = HidD_FreePreparsedData(pp);
            (status == HIDP_STATUS_SUCCESS).then_some(CollectionInfo {
                vid: attrs.VendorID,
                pid: attrs.ProductID,
                usage_page: caps.UsagePage,
                usage: caps.Usage,
            })
        })();
        let _ = CloseHandle(file);
        result
    }
}

/// An open HID collection with overlapped reads and writes.
pub struct HidHandle {
    file: HANDLE,
    // Boxed so the kernel-visible addresses stay put when the handle moves.
    read_ov: Box<OVERLAPPED>,
    write_ov: Box<OVERLAPPED>,
    buf: Box<[u8; READ_BUFFER]>,
    read_pending: bool,
}

// The raw handles are owned exclusively by this value and only used from one thread at a time.
unsafe impl Send for HidHandle {}

impl HidHandle {
    pub fn open(path: &str) -> Result<HidHandle, HidppError> {
        unsafe {
            let file = CreateFileW(
                &HSTRING::from(path),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                None,
            )
            .map_err(win_err)?;
            let read_event = CreateEventW(None, true, false, PCWSTR::null()).map_err(win_err)?;
            let write_event = CreateEventW(None, true, false, PCWSTR::null()).map_err(win_err)?;
            Ok(HidHandle {
                file,
                read_ov: Box::new(OVERLAPPED { hEvent: read_event, ..Default::default() }),
                write_ov: Box::new(OVERLAPPED { hEvent: write_event, ..Default::default() }),
                buf: Box::new([0; READ_BUFFER]),
                read_pending: false,
            })
        }
    }

    fn arm_read(&mut self) -> Result<(), HidppError> {
        if self.read_pending {
            return Ok(());
        }
        let event = self.read_ov.hEvent;
        *self.read_ov = OVERLAPPED { hEvent: event, ..Default::default() };
        // ReadFile resets the event itself; it is set again on completion,
        // including when the read completes synchronously.
        let result = unsafe {
            ReadFile(self.file, Some(&mut self.buf[..]), None, Some(&mut *self.read_ov))
        };
        match result {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
            Err(e) => return Err(win_err(e)),
        }
        self.read_pending = true;
        Ok(())
    }
}

impl Transport for HidHandle {
    fn write(&mut self, report: &[u8]) -> Result<(), HidppError> {
        unsafe {
            let event = self.write_ov.hEvent;
            *self.write_ov = OVERLAPPED { hEvent: event, ..Default::default() };
            match WriteFile(self.file, Some(report), None, Some(&mut *self.write_ov)) {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {}
                Err(e) => return Err(win_err(e)),
            }
            let mut written = 0u32;
            if WaitForSingleObject(event, WRITE_TIMEOUT_MS) != WAIT_OBJECT_0 {
                let _ = CancelIoEx(self.file, Some(&*self.write_ov));
                let _ = GetOverlappedResult(self.file, &*self.write_ov, &mut written, true);
                return Err(HidppError::Timeout);
            }
            GetOverlappedResult(self.file, &*self.write_ov, &mut written, false).map_err(win_err)
        }
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, HidppError> {
        self.arm_read()?;
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        unsafe {
            if WaitForSingleObject(self.read_ov.hEvent, ms) != WAIT_OBJECT_0 {
                return Ok(None);
            }
            let mut n = 0u32;
            match GetOverlappedResult(self.file, &*self.read_ov, &mut n, false) {
                Ok(()) => {
                    self.read_pending = false;
                    Ok(Some(self.buf[..n as usize].to_vec()))
                }
                Err(e) if e.code() == ERROR_IO_INCOMPLETE.to_hresult() => Ok(None),
                Err(e) => {
                    self.read_pending = false;
                    Err(win_err(e))
                }
            }
        }
    }

    fn wait_handle(&mut self) -> Option<HANDLE> {
        self.arm_read().ok().map(|()| self.read_ov.hEvent)
    }
}

impl Drop for HidHandle {
    fn drop(&mut self) {
        unsafe {
            if self.read_pending {
                // The kernel must be done with our buffer before it is freed.
                let _ = CancelIoEx(self.file, Some(&*self.read_ov));
                let mut n = 0u32;
                let _ = GetOverlappedResult(self.file, &*self.read_ov, &mut n, true);
            }
            let _ = CloseHandle(self.read_ov.hEvent);
            let _ = CloseHandle(self.write_ov.hEvent);
            let _ = CloseHandle(self.file);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sibling_key;

    #[test]
    fn sibling_collections_share_a_key() {
        let long = r"\\?\hid#vid_046d&pid_c543&mi_02&col02#8&71cd9ff&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let short = r"\\?\HID#VID_046D&PID_C543&MI_02&Col01#8&71cd9ff&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        let other = r"\\?\hid#vid_046d&pid_c543&mi_01&col02#8&1cba63c3&0&0001#{4d1e55b2-f16f-11cf-88cb-001111000030}";
        assert_eq!(sibling_key(long), sibling_key(short));
        assert_ne!(sibling_key(long), sibling_key(other));
    }
}
