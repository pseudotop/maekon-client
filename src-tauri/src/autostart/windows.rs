//! Windows autostart implementation.
//!
//! Registers/removes the binary path in the current-user Run registry key:
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` → value `MAEKON`.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use tracing::debug;

use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};

const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "MAEKON";

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Failed to verify binary path: {e}"))?;
    let exe_str = exe.to_string_lossy();
    let exe_wide = to_wide(&exe_str);

    let subkey_wide = to_wide(SUBKEY);
    let value_wide = to_wide(VALUE_NAME);

    // SAFETY: RegOpenKeyExW opens HKCU\...\Run with KEY_WRITE.
    // subkey_wide and value_wide are null-terminated UTF-16 vecs alive for the block.
    // hkey is written by RegOpenKeyExW and closed via RegCloseKey before return.
    // exe_wide cast to *const u8 is valid for REG_SZ byte data. No aliasing issues.
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey_wide.as_ptr(),
            0,
            KEY_WRITE,
            &mut hkey,
        );
        if result != 0 {
            return Err(format!("Failed to open registry: code {result}"));
        }

        let byte_len = (exe_wide.len() * 2) as u32;
        let result = RegSetValueExW(
            hkey,
            value_wide.as_ptr(),
            0,
            REG_SZ,
            exe_wide.as_ptr() as *const u8,
            byte_len,
        );
        RegCloseKey(hkey);

        if result != 0 {
            return Err(format!("Failed to set registry value: code {result}"));
        }
    }

    Ok(())
}

pub fn disable() -> Result<(), String> {
    let subkey_wide = to_wide(SUBKEY);
    let value_wide = to_wide(VALUE_NAME);

    // SAFETY: RegOpenKeyExW opens HKCU\...\Run with KEY_WRITE.
    // subkey_wide and value_wide are null-terminated UTF-16 vecs alive for the block.
    // hkey is closed via RegCloseKey before return. RegDeleteValueW failure is ignored.
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey_wide.as_ptr(),
            0,
            KEY_WRITE,
            &mut hkey,
        );
        if result != 0 {
            return Ok(());
        }

        let delete_result = RegDeleteValueW(hkey, value_wide.as_ptr());
        if delete_result != 0 {
            debug!("RegDeleteValueW failed with code: {delete_result}");
        }
        RegCloseKey(hkey);
    }

    Ok(())
}

pub fn is_enabled() -> Result<bool, String> {
    let subkey_wide = to_wide(SUBKEY);
    let value_wide = to_wide(VALUE_NAME);

    // SAFETY: RegOpenKeyExW opens HKCU\...\Run with KEY_READ.
    // RegQueryValueExW is called with null output pointers (existence check only).
    // hkey is closed via RegCloseKey before return. No data written.
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey_wide.as_ptr(),
            0,
            KEY_READ,
            &mut hkey,
        );
        if result != 0 {
            return Ok(false);
        }

        let result = RegQueryValueExW(
            hkey,
            value_wide.as_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        RegCloseKey(hkey);

        Ok(result == 0)
    }
}
