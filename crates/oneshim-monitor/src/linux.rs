use crate::error::MonitorError;
use oneshim_core::models::context::{MousePosition, WindowInfo};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

const SUBPROCESS_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayServer {
    X11,
    Wayland,
    Unknown,
}

pub fn detect_display_server() -> DisplayServer {
    if let Ok(session_type) = std::env::var("XDG_SESSION_TYPE") {
        match session_type.to_lowercase().as_str() {
            "x11" => return DisplayServer::X11,
            "wayland" => return DisplayServer::Wayland,
            _ => {}
        }
    }

    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        return DisplayServer::Wayland;
    }

    if std::env::var("DISPLAY").is_ok() {
        return DisplayServer::X11;
    }

    DisplayServer::Unknown
}

pub async fn get_active_window_linux() -> Result<Option<WindowInfo>, MonitorError> {
    let display_server = detect_display_server();

    match display_server {
        DisplayServer::X11 => get_active_window_x11().await,
        DisplayServer::Wayland => {
            debug!("Wayland detected — trying native window detection");

            // 1. Try GNOME Shell via gdbus
            if let Some(info) = get_active_window_gnome().await {
                return Ok(Some(info));
            }

            // 2. Try Sway/i3 via swaymsg
            if let Some(info) = get_active_window_sway().await {
                return Ok(Some(info));
            }

            // 3. Fall back to XWayland (works for X11 apps running under Wayland)
            warn!(
                "Native Wayland window detection unavailable (GNOME Shell / Sway not found). \
                 Falling back to XWayland — only X11 apps will be detected."
            );
            match get_active_window_x11().await {
                Ok(result) => Ok(result),
                Err(_) => {
                    warn!("XWayland fallback also failed — no active window detection available");
                    Ok(None)
                }
            }
        }
        DisplayServer::Unknown => {
            debug!("Display server detection failed — no active window detection");
            Ok(None)
        }
    }
}

/// Try to get the active window title on GNOME Shell via gdbus.
/// Returns None if GNOME Shell is not running or the call fails.
async fn get_active_window_gnome() -> Option<WindowInfo> {
    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/org/gnome/Shell",
                "--method",
                "org.gnome.Shell.Eval",
                r#"global.display.focus_window?.get_title() ?? """#,
            ])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        debug!("GNOME Shell gdbus call failed — not a GNOME session or Shell not reachable");
        return None;
    }

    // gdbus output format: (true, '"Window Title"')
    let stdout = String::from_utf8_lossy(&output.stdout);
    let title = parse_gnome_eval_result(&stdout)?;

    if title.is_empty() {
        debug!("GNOME Shell returned empty window title — no focused window");
        return None;
    }

    // Try to get the WM_CLASS (app name) via a second gdbus call
    let app_name = get_gnome_focus_app_name()
        .await
        .unwrap_or_else(|| "Unknown".to_string());

    debug!("GNOME Wayland active window: {} - {}", app_name, title);
    Some(WindowInfo {
        title,
        app_name,
        pid: 0, // PID not available through this GNOME Shell API
        bounds: None,
    })
}

/// Parse GNOME Shell Eval result: `(true, '"some title"')` -> `some title`
fn parse_gnome_eval_result(raw: &str) -> Option<String> {
    // Expected format: (true, '"title"') or (true, '""')
    let trimmed = raw.trim();
    // Find the second element after the comma
    let comma_pos = trimmed.find(',')?;
    let value_part = trimmed[comma_pos + 1..].trim().trim_end_matches(')');
    // Strip surrounding quotes: 'value' -> value, then strip inner double quotes
    let unquoted = value_part
        .trim()
        .trim_start_matches('\'')
        .trim_end_matches('\'')
        .trim()
        .trim_start_matches('"')
        .trim_end_matches('"');
    Some(unquoted.to_string())
}

/// Try to get the focused app name from GNOME Shell.
async fn get_gnome_focus_app_name() -> Option<String> {
    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("gdbus")
            .args([
                "call",
                "--session",
                "--dest",
                "org.gnome.Shell",
                "--object-path",
                "/org/gnome/Shell",
                "--method",
                "org.gnome.Shell.Eval",
                r#"global.display.focus_window?.get_wm_class() ?? """#,
            ])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let name = parse_gnome_eval_result(&stdout)?;
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// Try to get the active window on Sway/i3 via swaymsg.
/// Returns None if swaymsg is not available or fails.
async fn get_active_window_sway() -> Option<WindowInfo> {
    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("swaymsg").args(["-t", "get_tree"]).output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        debug!("swaymsg failed — not a Sway/i3 session");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let (title, app_id) = parse_sway_focused_window(&stdout)?;

    debug!("Sway Wayland active window: {} - {}", app_id, title);
    Some(WindowInfo {
        title,
        app_name: app_id,
        pid: 0, // Could be extracted from sway tree but adds complexity
        bounds: None,
    })
}

/// Parse swaymsg get_tree JSON output to find the focused window.
/// Looks for `"focused": true` nodes and extracts `name` and `app_id`.
fn parse_sway_focused_window(json_str: &str) -> Option<(String, String)> {
    // Minimal JSON parsing without pulling in a full JSON parser.
    // swaymsg output contains nodes with "focused":true for the active window.
    // We scan for the focused block and extract "name" and "app_id".
    let focused_marker = "\"focused\":true";
    let alt_marker = "\"focused\": true";

    let focused_pos = json_str
        .find(focused_marker)
        .or_else(|| json_str.find(alt_marker))?;

    // Search backwards from "focused":true for the enclosing object's "name" and "app_id"
    let search_start = focused_pos.saturating_sub(2000);
    let block = &json_str[search_start..focused_pos.saturating_add(500).min(json_str.len())];

    let name = extract_json_string_field(block, "name").unwrap_or_default();
    let app_id =
        extract_json_string_field(block, "app_id").unwrap_or_else(|| "Unknown".to_string());

    if name.is_empty() {
        return None;
    }

    Some((name, app_id))
}

/// Extract a JSON string field value from a text block: `"field": "value"` -> `value`
fn extract_json_string_field(block: &str, field: &str) -> Option<String> {
    let pattern = format!("\"{}\"", field);
    let field_pos = block.rfind(&pattern)?;
    let after_key = &block[field_pos + pattern.len()..];
    // Skip whitespace and colon
    let after_colon = after_key.trim_start().strip_prefix(':')?;
    let after_ws = after_colon.trim_start();
    // Extract quoted string value
    let value_start = after_ws.strip_prefix('"')?;
    let end_quote = value_start.find('"')?;
    Some(value_start[..end_quote].to_string())
}

async fn get_active_window_x11() -> Result<Option<WindowInfo>, MonitorError> {
    let window_id = match timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("xdotool").arg("getactivewindow").output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!("xdotool failure: {}", stderr);
            return Ok(None);
        }
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                debug!("xdotool - 'sudo apt install xdotool' execution required");
            } else {
                debug!("xdotool execution failure: {}", e);
            }
            return Ok(None);
        }
        Err(_) => {
            debug!("xdotool getactivewindow timed out");
            return Ok(None);
        }
    };

    if window_id.is_empty() {
        return Ok(None);
    }

    let title = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("xdotool")
            .args(["getwindowname", &window_id])
            .output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .filter(|o| o.status.success())
    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    .unwrap_or_default();

    let pid = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("xdotool")
            .args(["getwindowpid", &window_id])
            .output(),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .filter(|o| o.status.success())
    .and_then(|o| {
        String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<u32>()
            .ok()
    })
    .unwrap_or(0);

    let app_name = if pid > 0 {
        get_process_name(pid).unwrap_or_else(|| "Unknown".to_string())
    } else {
        "Unknown".to_string()
    };

    let bounds = get_window_geometry_x11(&window_id).await;

    debug!("active window: {} - {} (PID: {})", app_name, title, pid);

    Ok(Some(WindowInfo {
        title,
        app_name,
        pid,
        bounds,
    }))
}

async fn get_window_geometry_x11(
    window_id: &str,
) -> Option<oneshim_core::models::context::WindowBounds> {
    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("xdotool")
            .args(["getwindowgeometry", "--shell", window_id])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut x = 0i32;
    let mut y = 0i32;
    let mut width = 0u32;
    let mut height = 0u32;

    for line in stdout.lines() {
        if let Some(val) = line.strip_prefix("X=") {
            x = val.parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("Y=") {
            y = val.parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("WIDTH=") {
            width = val.parse().unwrap_or(0);
        } else if let Some(val) = line.strip_prefix("HEIGHT=") {
            height = val.parse().unwrap_or(0);
        }
    }

    Some(oneshim_core::models::context::WindowBounds {
        x,
        y,
        width,
        height,
    })
}

fn get_process_name(pid: u32) -> Option<String> {
    let comm_path = format!("/proc/{}/comm", pid);
    std::fs::read_to_string(&comm_path)
        .ok()
        .map(|s| s.trim().to_string())
}

pub async fn get_idle_time_linux() -> Option<u64> {
    let display_server = detect_display_server();

    match display_server {
        DisplayServer::X11 => get_idle_time_x11().await,
        DisplayServer::Wayland => {
            // 1. Try GNOME Mutter IdleMonitor D-Bus API
            if let Some(idle) = get_idle_time_gnome_mutter().await {
                return Some(idle);
            }

            // 2. Fall back to XWayland (xprintidle works for X11 apps under Wayland)
            if let Some(idle) = get_idle_time_x11().await {
                return Some(idle);
            }

            warn!(
                "Wayland idle detection unavailable — GNOME Mutter IdleMonitor not found \
                 and xprintidle failed. Returning 0 (unknown idle)."
            );
            Some(0)
        }
        DisplayServer::Unknown => None,
    }
}

/// Get idle time via GNOME Mutter IdleMonitor D-Bus interface.
/// Returns idle time in seconds, or None if not available.
async fn get_idle_time_gnome_mutter() -> Option<u64> {
    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.gnome.Mutter.IdleMonitor",
                "--print-reply",
                "/org/gnome/Mutter/IdleMonitor/Core",
                "org.gnome.Mutter.IdleMonitor.GetIdletime",
            ])
            .output(),
    )
    .await
    .ok()?
    .ok()?;

    if !output.status.success() {
        debug!("GNOME Mutter IdleMonitor not available — not a GNOME session");
        return None;
    }

    // dbus-send output format: "   uint64 12345\n" (idle time in milliseconds)
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("uint64 ") {
            if let Ok(ms) = val.trim().parse::<u64>() {
                return Some(ms / 1000);
            }
        }
    }

    debug!("Failed to parse GNOME Mutter IdleMonitor response");
    None
}

async fn get_idle_time_x11() -> Option<u64> {
    let output = match timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("xprintidle").output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(_)) => {
            return None;
        }
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                debug!("xprintidle - 'sudo apt install xprintidle' execution required");
            }
            return None;
        }
        Err(_) => {
            debug!("xprintidle timed out");
            return None;
        }
    };

    let ms_str = String::from_utf8_lossy(&output.stdout);
    let ms: u64 = ms_str.trim().parse().ok()?;

    Some(ms / 1000)
}

pub async fn get_mouse_position_linux() -> Option<MousePosition> {
    let display_server = detect_display_server();

    match display_server {
        DisplayServer::X11 => get_mouse_position_x11().await,
        DisplayServer::Wayland => {
            // No reliable Wayland-native mouse position API via CLI tools.
            // XWayland fallback works for X11 apps; for pure Wayland apps,
            // mouse position may not be available without compositor-specific
            // protocols (e.g., wlr-foreign-toplevel-management).
            match get_mouse_position_x11().await {
                Some(pos) => Some(pos),
                None => {
                    debug!(
                        "Wayland mouse position unavailable — xdotool fallback failed. \
                         Native Wayland compositors restrict cursor position access."
                    );
                    None
                }
            }
        }
        DisplayServer::Unknown => None,
    }
}

async fn get_mouse_position_x11() -> Option<MousePosition> {
    // x:1234 y:567 screen:0 window:12345678
    let output = match timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("xdotool").arg("getmouselocation").output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => output,
        Ok(Ok(_)) => {
            return None;
        }
        Ok(Err(e)) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                debug!("xdotool - mouse detection not-available");
            }
            return None;
        }
        Err(_) => {
            debug!("xdotool getmouselocation timed out");
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut x: Option<i32> = None;
    let mut y: Option<i32> = None;

    for part in stdout.split_whitespace() {
        if let Some(val) = part.strip_prefix("x:") {
            x = val.parse().ok();
        } else if let Some(val) = part.strip_prefix("y:") {
            y = val.parse().ok();
        }
    }

    match (x, y) {
        (Some(x), Some(y)) => Some(MousePosition { x, y }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_display_server_works() {
        let server = detect_display_server();
        assert!(matches!(
            server,
            DisplayServer::X11 | DisplayServer::Wayland | DisplayServer::Unknown
        ));
    }

    #[test]
    fn get_process_name_from_proc() {
        let name = get_process_name(1);
        assert!(name.is_some());
        let name = name.unwrap();
        assert!(!name.is_empty());
    }

    #[tokio::test]
    async fn active_window_returns_option() {
        let result = get_active_window_linux().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn idle_time_returns_option() {
        let result = get_idle_time_linux().await;
        if let Some(secs) = result {
            // sanity bound: less than one year in seconds
            assert!(secs < 86400 * 365);
        }
    }

    #[tokio::test]
    async fn mouse_position_returns_option() {
        let result = get_mouse_position_linux().await;
        if let Some(pos) = result {
            assert!(pos.x >= 0 && pos.x < 32000);
            assert!(pos.y >= 0 && pos.y < 32000);
        }
    }

    // ── GNOME Shell eval result parsing ──

    #[test]
    fn parse_gnome_eval_result_normal() {
        let raw = "(true, '\"Firefox\"')\n";
        let result = parse_gnome_eval_result(raw);
        assert_eq!(result.as_deref(), Some("Firefox"));
    }

    #[test]
    fn parse_gnome_eval_result_empty_title() {
        let raw = "(true, '\"\"')\n";
        let result = parse_gnome_eval_result(raw);
        assert_eq!(result.as_deref(), Some(""));
    }

    #[test]
    fn parse_gnome_eval_result_no_comma() {
        let raw = "(true)";
        let result = parse_gnome_eval_result(raw);
        assert!(result.is_none());
    }

    // ── Sway JSON field extraction ──

    #[test]
    fn extract_json_string_field_found() {
        let block = r#""name": "Terminal", "app_id": "kitty""#;
        assert_eq!(
            extract_json_string_field(block, "name").as_deref(),
            Some("Terminal")
        );
        assert_eq!(
            extract_json_string_field(block, "app_id").as_deref(),
            Some("kitty")
        );
    }

    #[test]
    fn extract_json_string_field_missing() {
        let block = r#""name": "Terminal""#;
        assert!(extract_json_string_field(block, "app_id").is_none());
    }

    #[test]
    fn parse_sway_focused_window_found() {
        let json = r#"{"name": "vim", "app_id": "Alacritty", "focused":true}"#;
        let result = parse_sway_focused_window(json);
        assert!(result.is_some());
        let (name, app_id) = result.unwrap();
        assert_eq!(name, "vim");
        assert_eq!(app_id, "Alacritty");
    }

    #[test]
    fn parse_sway_focused_window_spaced() {
        let json = r#"{"name": "editor", "app_id": "foot", "focused": true}"#;
        let result = parse_sway_focused_window(json);
        assert!(result.is_some());
        let (name, _) = result.unwrap();
        assert_eq!(name, "editor");
    }

    #[test]
    fn parse_sway_focused_window_no_focus() {
        let json = r#"{"name": "vim", "app_id": "Alacritty", "focused":false}"#;
        let result = parse_sway_focused_window(json);
        assert!(result.is_none());
    }
}
