#![cfg(target_os = "windows")]

use crate::error::MonitorError;
use oneshim_core::models::context::{MousePosition, WindowBounds, WindowInfo};
use tracing::debug;
use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId,
};

pub fn get_active_window_windows() -> Result<Option<WindowInfo>, MonitorError> {
    // SAFETY: GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId are
    // read-only Win32 queries with no preconditions. Null HWND is checked before
    // use. title_buf is a stack-allocated array with length passed to the API.
    // GetWindowThreadProcessId writes to a valid &mut u32. No resources to free.
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.is_null() {
            debug!("no active window (GetForegroundWindow returned null)");
            return Ok(None);
        }

        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32);
        let title = if len > 0 {
            String::from_utf16_lossy(&title_buf[..len as usize])
        } else {
            String::new()
        };

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);

        let app_name = get_process_name(pid).unwrap_or_else(|| "Unknown".to_string());

        let bounds = get_window_bounds(hwnd);

        debug!(
            "active window: {app_name} - {title} (PID: {pid}, {:?})",
            bounds.map(|b| format!("{}x{} at ({},{})", b.width, b.height, b.x, b.y))
        );

        Ok(Some(WindowInfo {
            title,
            app_name,
            pid,
            bounds,
        }))
    }
}

fn get_window_bounds(hwnd: HWND) -> Option<WindowBounds> {
    // SAFETY: GetWindowRect writes into a stack-allocated RECT via valid &mut.
    // hwnd is a non-null handle obtained from GetForegroundWindow.
    // zeroed() produces a valid RECT (all-zero POD struct).
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) != 0 {
            let width = (rect.right - rect.left) as u32;
            let height = (rect.bottom - rect.top) as u32;

            if width > 0 && height > 0 {
                Some(WindowBounds {
                    x: rect.left,
                    y: rect.top,
                    width,
                    height,
                })
            } else {
                None
            }
        } else {
            None
        }
    }
}

fn get_process_name(pid: u32) -> Option<String> {
    use sysinfo::{Pid, System};

    let mut sys = System::new();
    sys.refresh_processes(
        sysinfo::ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
    );

    sys.process(Pid::from_u32(pid))
        .map(|p| p.name().to_string_lossy().to_string())
}

pub fn get_idle_time_windows() -> Option<u64> {
    // SAFETY: GetLastInputInfo requires cbSize to be set correctly, which we do.
    // LASTINPUTINFO is a POD struct; zeroed() + cbSize assignment is valid.
    // GetTickCount has no preconditions. No resources to free.
    unsafe {
        let mut last_input: LASTINPUTINFO = std::mem::zeroed();
        last_input.cbSize = std::mem::size_of::<LASTINPUTINFO>() as u32;

        if GetLastInputInfo(&mut last_input) != 0 {
            let current_tick = windows_sys::Win32::System::SystemInformation::GetTickCount();
            let idle_ms = current_tick.wrapping_sub(last_input.dwTime);
            Some((idle_ms / 1000) as u64)
        } else {
            None
        }
    }
}

pub fn get_mouse_position_windows() -> Option<MousePosition> {
    // SAFETY: GetCursorPos writes into a stack-allocated POINT via valid &mut.
    // zeroed() produces a valid POINT (all-zero POD struct). No resources to free.
    unsafe {
        let mut point: POINT = std::mem::zeroed();
        if GetCursorPos(&mut point) != 0 {
            Some(MousePosition {
                x: point.x,
                y: point.y,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {}
