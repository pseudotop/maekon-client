use crate::error::MonitorError;
use crate::log_privacy::title_digest;
use maekon_core::models::context::{MousePosition, WindowInfo};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

const SUBPROCESS_TIMEOUT_SECS: u64 = 5;

/// Guards the one-time `Shell.Eval`-disabled diagnostic so the 1s scheduler loop
/// does not spam the log every tick on a modern GNOME Wayland session.
static GNOME_EVAL_DISABLED_WARNED: AtomicBool = AtomicBool::new(false);

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

            // 1. Try GNOME Shell via gdbus.
            match get_active_window_gnome().await {
                GnomeProbe::Window(info) => return Ok(Some(info)),
                GnomeProbe::EvalDisabled => {
                    // org.gnome.Shell.Eval has been disabled by default since
                    // GNOME 41, so this path can never succeed on a modern GNOME
                    // Wayland session. Surface a one-time diagnostic instead of
                    // failing silently, then fall through to the XWayland path.
                    if !GNOME_EVAL_DISABLED_WARNED.swap(true, Ordering::Relaxed) {
                        warn!(
                            "GNOME Shell.Eval is disabled (default since GNOME 41), so native \
                             GNOME Wayland active-window detection is unavailable on this session. \
                             Falling back to XWayland — only X11/XWayland apps will be detected. \
                             To capture native Wayland windows, run the desktop session on X11/Xorg."
                        );
                    }
                }
                GnomeProbe::Unavailable => {}
            }

            // 2. Try Sway/i3 via swaymsg.
            if let Some(info) = get_active_window_sway().await {
                return Ok(Some(info));
            }

            // 3. Fall back to XWayland (works for X11 apps running under Wayland).
            debug!(
                "Native Wayland window detection unavailable — falling back to XWayland \
                 (only X11/XWayland apps will be detected)."
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

/// Outcome of probing GNOME Shell for the active window via `gdbus`.
///
/// The caller distinguishes these so a modern GNOME Wayland session (where
/// `org.gnome.Shell.Eval` is disabled) can be reported explicitly instead of
/// failing silently (#6094).
enum GnomeProbe {
    /// A focused window was resolved.
    Window(WindowInfo),
    /// `org.gnome.Shell.Eval` is disabled (default since GNOME 41) — the call
    /// is rejected by GNOME Shell, so this path can never yield data.
    EvalDisabled,
    /// Not a GNOME session, GNOME Shell unreachable, or no focused window.
    Unavailable,
}

/// Returns true if a `gdbus`/`org.gnome.Shell.Eval` failure stderr indicates the
/// Eval method was rejected because it is disabled (GNOME 41+), rather than the
/// session simply not being GNOME.
fn is_gnome_eval_disabled(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    // GNOME 41+ rejects Eval; the exact wording varies across versions, but the
    // reply consistently mentions the disabled/unsafe-mode Eval method.
    lower.contains("eval") && (lower.contains("disabled") || lower.contains("unsafe mode"))
}

/// Returns true if a successful `Shell.Eval` reply still indicates refusal: GNOME
/// 41+ in safe mode returns the tuple `(false, '')`, where the leading `false`
/// means the script was not evaluated.
fn is_gnome_eval_refused(stdout: &str) -> bool {
    stdout.trim_start().starts_with("(false")
}

/// Try to get the active window title on GNOME Shell via gdbus.
async fn get_active_window_gnome() -> GnomeProbe {
    let output = match timeout(
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
            .kill_on_drop(true)
            .output(),
    )
    .await
    {
        Ok(Ok(output)) => output,
        // Timed out or `gdbus` not installed — treat as not available.
        _ => return GnomeProbe::Unavailable,
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if is_gnome_eval_disabled(&stderr) {
            debug!("GNOME Shell.Eval rejected the call — Eval is disabled (GNOME 41+)");
            return GnomeProbe::EvalDisabled;
        }
        debug!("GNOME Shell gdbus call failed — not a GNOME session or Shell not reachable");
        return GnomeProbe::Unavailable;
    }

    // gdbus output format: (true, '"Window Title"')
    let stdout = String::from_utf8_lossy(&output.stdout);

    // GNOME 41+ can return success but with the tuple's first element `false`,
    // meaning Eval ran in safe mode and refused to evaluate the script. This is
    // the silent variant of the disabled path: the call "succeeds" yet yields no
    // data. Treat it as EvalDisabled so the caller emits the diagnostic (#6094).
    if is_gnome_eval_refused(&stdout) {
        debug!("GNOME Shell.Eval returned (false, …) — Eval refused in safe mode (GNOME 41+)");
        return GnomeProbe::EvalDisabled;
    }

    let title = match parse_gnome_eval_result(&stdout) {
        Some(title) => title,
        None => return GnomeProbe::Unavailable,
    };

    if title.is_empty() {
        debug!("GNOME Shell returned empty window title — no focused window");
        return GnomeProbe::Unavailable;
    }

    // Try to get the WM_CLASS (app name) via a second gdbus call
    let app_name = get_gnome_focus_app_name()
        .await
        .unwrap_or_else(|| "Unknown".to_string());

    // Title is PII — log a content-free digest only (#5591).
    debug!(
        "GNOME Wayland active window: {} ({})",
        app_name,
        title_digest(&title)
    );
    GnomeProbe::Window(WindowInfo {
        title,
        app_name,
        app_bundle_id: None,
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
            .kill_on_drop(true)
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
        Command::new("swaymsg")
            .args(["-t", "get_tree"])
            .kill_on_drop(true)
            .output(),
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

    // Title is PII — log a content-free digest only (#5591).
    debug!(
        "Sway Wayland active window: {} ({})",
        app_id,
        title_digest(&title)
    );
    Some(WindowInfo {
        title,
        app_name: app_id,
        app_bundle_id: None,
        pid: 0, // Could be extracted from sway tree but adds complexity
        bounds: None,
    })
}

/// Snap `index` down to the nearest UTF-8 char boundary at or below it.
///
/// Mirrors the unstable `str::floor_char_boundary`; reimplemented here so the
/// crate stays on the workspace MSRV. `index` may be `>= s.len()`.
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Snap `index` up to the nearest UTF-8 char boundary at or above it.
///
/// Mirrors the unstable `str::ceil_char_boundary`; reimplemented here so the
/// crate stays on the workspace MSRV. `index` may be `>= s.len()`.
fn ceil_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
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

    // Search backwards from "focused":true for the enclosing object's "name" and "app_id".
    // The title is attacker-controllable and may contain multi-byte UTF-8 (CJK, emoji);
    // snap the raw byte offsets to char boundaries so slicing never panics mid-codepoint.
    let search_start = floor_char_boundary(json_str, focused_pos.saturating_sub(2000));
    let search_end = ceil_char_boundary(
        json_str,
        focused_pos.saturating_add(500).min(json_str.len()),
    );
    let block = &json_str[search_start..search_end];

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
        Command::new("xdotool")
            .arg("getactivewindow")
            .kill_on_drop(true)
            .output(),
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
            .kill_on_drop(true)
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
            .kill_on_drop(true)
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
        get_process_name(pid)
            .await
            .unwrap_or_else(|| "Unknown".to_string())
    } else {
        "Unknown".to_string()
    };

    let bounds = get_window_geometry_x11(&window_id).await;

    // Title is PII — log a content-free digest only (#5591).
    debug!(
        "active window: {} ({}) (PID: {})",
        app_name,
        title_digest(&title),
        pid
    );

    Ok(Some(WindowInfo {
        title,
        app_name,
        app_bundle_id: None,
        pid,
        bounds,
    }))
}

async fn get_window_geometry_x11(
    window_id: &str,
) -> Option<maekon_core::models::context::WindowBounds> {
    let output = timeout(
        Duration::from_secs(SUBPROCESS_TIMEOUT_SECS),
        Command::new("xdotool")
            .args(["getwindowgeometry", "--shell", window_id])
            .kill_on_drop(true)
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

    Some(maekon_core::models::context::WindowBounds {
        x,
        y,
        width,
        height,
    })
}

/// Reads the process name from `/proc/<pid>/comm`.
///
/// Uses `tokio::fs::read_to_string` so the async runtime is not blocked.
async fn get_process_name(pid: u32) -> Option<String> {
    let comm_path = format!("/proc/{}/comm", pid);
    tokio::fs::read_to_string(&comm_path)
        .await
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
            .kill_on_drop(true)
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
        Command::new("xprintidle").kill_on_drop(true).output(),
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
        Command::new("xdotool")
            .arg("getmouselocation")
            .kill_on_drop(true)
            .output(),
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

    #[tokio::test]
    async fn get_process_name_from_proc() {
        let name = get_process_name(1).await;
        assert!(name.is_some());
        let name = name.unwrap();
        assert!(!name.is_empty());
    }

    #[tokio::test]
    async fn active_window_returns_option() {
        // Environment-dependent probe: headless CI yields None, a desktop
        // session Some — the pinned contract is exactly "never Err" (#5594).
        get_active_window_linux()
            .await
            .expect("active-window probe must not error without a display server");
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

    // ── GNOME Shell.Eval disabled detection (#6094) ──

    #[test]
    fn is_gnome_eval_disabled_detects_disabled_reply() {
        // Representative GNOME 41+ rejection wordings.
        assert!(is_gnome_eval_disabled(
            "Error: GDBus.Error:org.freedesktop.DBus.Error.Failed: Eval is disabled"
        ));
        assert!(is_gnome_eval_disabled(
            "Eval method is only available in unsafe mode"
        ));
    }

    #[test]
    fn is_gnome_eval_disabled_ignores_unrelated_failures() {
        // Service-not-found (not a GNOME session) must NOT be treated as disabled.
        assert!(!is_gnome_eval_disabled(
            "Error: GDBus.Error:org.freedesktop.DBus.Error.ServiceUnknown: \
             The name org.gnome.Shell was not provided by any .service files"
        ));
        assert!(!is_gnome_eval_disabled(""));
    }

    #[test]
    fn is_gnome_eval_refused_detects_false_tuple() {
        // GNOME 41+ safe-mode refusal: the call succeeds but yields (false, '').
        assert!(is_gnome_eval_refused("(false, '')\n"));
        assert!(is_gnome_eval_refused("  (false, '')"));
        // A real result starts with (true, …) and must not be treated as refused.
        assert!(!is_gnome_eval_refused("(true, '\"Firefox\"')\n"));
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

    #[test]
    fn char_boundary_helpers_snap_off_boundary_indices() {
        // "é" is two bytes (0xC3 0xA9); byte index 2 lands mid-codepoint.
        let s = "aéb";
        assert_eq!(floor_char_boundary(s, 2), 1);
        assert_eq!(ceil_char_boundary(s, 2), 3);
        // Out-of-range indices clamp to len without panicking.
        assert_eq!(floor_char_boundary(s, 999), s.len());
        assert_eq!(ceil_char_boundary(s, 999), s.len());
    }

    #[test]
    fn parse_sway_focused_window_non_ascii_title_no_panic() {
        // Attacker-controllable title with multi-byte CJK + emoji characters.
        // The 2000/500-byte search window must snap to char boundaries rather
        // than slice mid-codepoint (#6092).
        let json = r#"{"name": "프로젝트 보고서 🚀 résumé", "app_id": "kitty", "focused":true}"#;
        let result = parse_sway_focused_window(json);
        assert!(result.is_some());
        let (name, app_id) = result.unwrap();
        assert_eq!(name, "프로젝트 보고서 🚀 résumé");
        assert_eq!(app_id, "kitty");
    }

    #[test]
    fn parse_sway_focused_window_multibyte_straddling_window_edge() {
        // Force a multi-byte char to straddle the ±2000-byte backward window
        // edge: a long CJK prefix pushes "focused":true past 2000 bytes so the
        // raw `focused_pos - 2000` offset lands inside a 3-byte codepoint.
        // Pre-fix this slice panicked; post-fix it must return cleanly.
        let prefix = "한".repeat(800); // 800 * 3 = 2400 bytes of multi-byte text
        let json =
            format!(r#"{{"pad": "{prefix}", "name": "에디터", "app_id": "foot", "focused":true}}"#);
        let result = parse_sway_focused_window(&json);
        assert!(result.is_some());
        let (name, app_id) = result.unwrap();
        assert_eq!(name, "에디터");
        assert_eq!(app_id, "foot");
    }
}
