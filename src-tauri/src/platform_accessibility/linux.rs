use maekon_core::error::CoreError;
use maekon_core::models::intent::ElementBounds;
use std::process::Command;

use super::types::AccessibilityNode;

#[cfg(all(unix, not(target_os = "macos")))]
pub(super) fn query_linux_accessibility_nodes() -> Result<Vec<AccessibilityNode>, CoreError> {
    let window_id = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "Linux accessibility probe requires xdotool and active X11/XWayland session"
                .to_string(),
        })?;

    let title = Command::new("xdotool")
        .args(["getwindowname", &window_id])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "active-window".to_string());

    let pid = Command::new("xdotool")
        .args(["getwindowpid", &window_id])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u32>()
                .ok()
        });

    let app_name = pid
        .and_then(read_proc_name)
        .unwrap_or_else(|| "unknown".to_string());

    let geometry = Command::new("xdotool")
        .args(["getwindowgeometry", "--shell", &window_id])
        .output()
        .ok()
        .filter(|output| output.status.success())
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
fn read_proc_name(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
