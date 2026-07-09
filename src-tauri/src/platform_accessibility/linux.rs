use maekon_core::error::CoreError;
use maekon_core::models::intent::ElementBounds;
use std::process::Command;

use super::command_timeout::{command_output_with_timeout, ACCESSIBILITY_COMMAND_TIMEOUT};
use super::types::AccessibilityNode;

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn query_linux_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
    // SEC-MON-01: resolve against the shared trusted-directory allowlist
    // (maekon-monitor) once and reuse the resolved absolute path for every
    // xdotool spawn below — fail closed (no PATH fallback) when the binary
    // is not found under any trusted system directory.
    let xdotool_path = maekon_monitor::resolve_trusted_binary("xdotool").ok_or_else(|| {
        CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "xdotool not found under the trusted directory allowlist".to_string(),
        }
    })?;

    let mut active_window = Command::new(&xdotool_path);
    active_window.arg("getactivewindow");
    let window_id = successful_output(&mut active_window)
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "Linux accessibility probe requires xdotool and active X11/XWayland session"
                .to_string(),
        })?;

    let mut window_name = Command::new(&xdotool_path);
    window_name.args(["getwindowname", &window_id]);
    let title = successful_output(&mut window_name)
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "active-window".to_string());

    let mut window_pid = Command::new(&xdotool_path);
    window_pid.args(["getwindowpid", &window_id]);
    let pid = successful_output(&mut window_pid).and_then(|output| {
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse::<u32>()
            .ok()
    });

    let app_name = pid
        .and_then(read_proc_name)
        .unwrap_or_else(|| "unknown".to_string());

    let mut window_geometry = Command::new(&xdotool_path);
    window_geometry.args(["getwindowgeometry", "--shell", &window_id]);
    let geometry = successful_output(&mut window_geometry)
        .map(|output| String::from_utf8_lossy(&output.stdout).to_string())
        .ok_or_else(|| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "Failed to read active window geometry".to_string(),
        })?;

    let mut x = 0i32;
    let mut y = 0i32;
    let mut w = 0u32;
    let mut h = 0u32;
    for line in geometry.lines() {
        if let Some(value) = line.strip_prefix("X=") {
            x = value.parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("Y=") {
            y = value.parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("WIDTH=") {
            w = value.parse().unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("HEIGHT=") {
            h = value.parse().unwrap_or(0);
        }
    }

    if w == 0 || h == 0 {
        return Err(CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "Invalid active window geometry from xdotool".to_string(),
        });
    }

    Ok(vec![AccessibilityNode {
        app_name: Some(app_name),
        role: Some("window".to_string()),
        label: title,
        bounds: ElementBounds {
            x,
            y,
            width: w,
            height: h,
        },
        confidence: 0.75,
    }])
}

#[cfg(all(unix, not(target_os = "macos")))]
fn successful_output(command: &mut Command) -> Option<std::process::Output> {
    command_output_with_timeout(command, ACCESSIBILITY_COMMAND_TIMEOUT)
        .ok()
        .flatten()
        .filter(|output| output.status.success())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_proc_name(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
