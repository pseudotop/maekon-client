#![cfg(target_os = "windows")]

use crate::active_window_parse::{
    decode_window_title, idle_secs_from_ticks, window_bounds_from_edges,
};
use crate::error::MonitorError;
use maekon_core::models::context::{MousePosition, WindowBounds, WindowInfo};
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
        let title = decode_window_title(&title_buf, len);

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);

        let app_name = get_process_name(pid).unwrap_or_else(|| "Unknown".to_string());

        let bounds = get_window_bounds(hwnd);

        // Title is PII — log a content-free digest only (#5591).
        debug!(
            "active window: {app_name} ({}) (PID: {pid}, {:?})",
            crate::log_privacy::title_digest(&title),
            bounds.map(|b| format!("{}x{} at ({},{})", b.width, b.height, b.x, b.y))
        );

        Ok(Some(WindowInfo {
            title,
            app_name,
            app_bundle_id: None,
            pid,
            bounds,
        }))
    }
}

/// Detects whether the foreground **external** (non-Maekon-owned) window is
/// fullscreen / monitor-covering (#8849). Compares `GetForegroundWindow` →
/// `GetWindowRect` against the rect of the monitor the window sits on
/// (`MonitorFromWindow` + `GetMonitorInfoW`), so it catches both exclusive
/// fullscreen and borderless "fake" fullscreen (#8858). Windows owned by this
/// process are excluded (the overlay policy's owned-window path is the SSOT).
/// `None` when there is no foreground window or the coordinates cannot be
/// obtained.
pub fn foreground_window_is_fullscreen_windows() -> Option<bool> {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcessId;

    // SAFETY: all are read-only Win32 queries. GetForegroundWindow's HWND is
    // null-checked, RECT/MONITORINFO are zeroed() POD structs, and the required
    // MONITORINFO.cbSize is set before the GetMonitorInfoW call. No resources to free.
    unsafe {
        let hwnd: HWND = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }

        // External windows only: exclude windows owned by our process (or of unknown owner).
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || pid == GetCurrentProcessId() {
            return Some(false);
        }

        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }

        let hmonitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if hmonitor.is_null() {
            return None;
        }
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmonitor, &mut info) == 0 {
            return None;
        }

        let window = WindowBounds {
            x: rect.left,
            y: rect.top,
            width: (rect.right - rect.left).max(0) as u32,
            height: (rect.bottom - rect.top).max(0) as u32,
        };
        let m = info.rcMonitor;
        let monitor = WindowBounds {
            x: m.left,
            y: m.top,
            width: (m.right - m.left).max(0) as u32,
            height: (m.bottom - m.top).max(0) as u32,
        };
        Some(crate::foreground_fullscreen::window_covers_monitor(
            window,
            monitor,
            crate::foreground_fullscreen::COVER_TOLERANCE_PX,
        ))
    }
}

fn get_window_bounds(hwnd: HWND) -> Option<WindowBounds> {
    // SAFETY: GetWindowRect writes into a stack-allocated RECT via valid &mut.
    // hwnd is a non-null handle obtained from GetForegroundWindow.
    // zeroed() produces a valid RECT (all-zero POD struct).
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) != 0 {
            window_bounds_from_edges(rect.left, rect.top, rect.right, rect.bottom)
        } else {
            None
        }
    }
}

fn get_process_name(pid: u32) -> Option<String> {
    use sysinfo::{Pid, System};

    if pid == 0 {
        return None;
    }

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
            Some(idle_secs_from_ticks(current_tick, last_input.dwTime))
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
mod tests {
    use super::*;

    /// F-RR-C23-02: get_process_name creates a fresh sysinfo::System and calls
    /// refresh_processes — verify that it returns None gracefully for a PID that
    /// sysinfo cannot resolve (PID 0 = System Idle Process, not accessible via
    /// OpenProcess). This also exercises the None-path without a live window.
    #[test]
    fn get_process_name_returns_none_for_pid_zero() {
        // PID 0 is the Windows System Idle Process; sysinfo cannot open it via
        // NtQuerySystemInformation in the regular process-refresh path, so the
        // function must return None rather than panicking.
        assert!(
            get_process_name(0).is_none(),
            "get_process_name(0) should return None for the System Idle Process"
        );
    }

    /// F-RR-C23-02: get_process_name returns Some for the current process PID,
    /// which is always refresh-accessible on Windows without elevated privileges.
    #[test]
    fn get_process_name_returns_some_for_current_pid() {
        // SAFETY: GetCurrentProcessId has no preconditions and cannot fail.
        let pid = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessId() };
        let name = get_process_name(pid);
        assert!(
            name.is_some(),
            "get_process_name should return Some for the current process PID {pid}"
        );
        assert!(
            !name.unwrap().is_empty(),
            "process name for current PID must not be empty"
        );
    }
}
