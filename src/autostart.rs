//! "Start with Windows": a value under HKCU\...\CurrentVersion\Run.
//! The registry is the single source of truth; nothing is cached in the config.

use windows::core::{w, Result, PCWSTR};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
};

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("Lowtide");

pub fn is_enabled() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME, RRF_RT_REG_SZ, None, None, Some(&mut size))
            .is_ok()
    }
}

pub fn set(enabled: bool) -> Result<()> {
    unsafe {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| windows::core::Error::new(
                windows::core::HRESULT(0x80004005u32 as i32),
                e.to_string(),
            ))?;
            let command = format!("\"{}\"", exe.display());
            let wide: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                VALUE_NAME,
                REG_SZ.0,
                Some(wide.as_ptr().cast()),
                (wide.len() * 2) as u32,
            )
            .ok()
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, VALUE_NAME) {
                e if e == ERROR_FILE_NOT_FOUND => Ok(()),
                e => e.ok(),
            }
        }
    }
}
