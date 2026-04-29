use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::{command, AppHandle, Emitter, Manager, State};
use tracing::debug;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

// --- Panel position validation (physical pixels) ---

const PANEL_WIDTH: f64 = 260.0;
// PANEL_WIDTH is logical px (= physical at 1x). On HiDPI the physical panel
// is wider, but POSITION_MARGIN absorbs the difference.
const POSITION_MARGIN: f64 = 100.0;

#[derive(Debug, Clone)]
struct MonitorBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Parse "x,y" position string into (f64, f64).
/// Returns None for missing, malformed, NaN, or Infinity values.
fn parse_position(s: &str) -> Option<(f64, f64)> {
    let mut parts = s.splitn(2, ',');
    let x: f64 = parts.next()?.parse().ok()?;
    let y: f64 = parts.next()?.parse().ok()?;
    if x.is_finite() && y.is_finite() {
        Some((x, y))
    } else {
        None
    }
}

/// Check if position (x, y) falls within any monitor's physical bounds.
/// All values are in physical pixels. Returns false if monitors is empty.
fn is_position_valid(x: f64, y: f64, monitors: &[MonitorBounds]) -> bool {
    monitors.iter().any(|m| {
        x >= (m.x - PANEL_WIDTH + POSITION_MARGIN)
            && x <= (m.x + m.width - POSITION_MARGIN)
            && y >= m.y
            && y <= (m.y + m.height - POSITION_MARGIN)
    })
}

#[derive(Serialize)]
pub struct CaptureStatusResponse {
    pub paused: bool,
    pub indicator_visible: bool,
}

#[derive(Serialize)]
pub struct ConnectionStatusResponse {
    pub server: bool,
    pub llm: bool,
    pub cli: bool,
}

#[command]
pub async fn get_capture_status(
    state: State<'_, AppState>,
) -> Result<CaptureStatusResponse, IpcError> {
    Ok(CaptureStatusResponse {
        paused: state.capture_paused.load(Ordering::Relaxed),
        indicator_visible: state.indicator_visible.load(Ordering::Relaxed),
    })
}

#[command]
pub async fn toggle_capture_pause(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<CaptureStatusResponse, IpcError> {
    let was_paused = state.capture_paused.fetch_xor(true, Ordering::Relaxed);
    let new_paused = !was_paused;
    let indicator_visible = state.indicator_visible.load(Ordering::Relaxed);

    let payload =
        serde_json::json!({ "paused": new_paused, "indicator_visible": indicator_visible });
    if let Err(e) = app.emit_to("magic-overlay", "overlay:capture-state-changed", &payload) {
        debug!("emit magic-overlay failed: {e}");
    }
    if let Err(e) = app.emit_to("tracking-panel", "overlay:capture-state-changed", &payload) {
        debug!("emit tracking-panel failed: {e}");
    }
    if let Err(e) = crate::tray::sync_tray_state(&app, new_paused, indicator_visible) {
        debug!("sync_tray_state failed: {e}");
    }

    #[cfg(target_os = "macos")]
    if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>() {
        border.0.set_paused(new_paused);
    }

    Ok(CaptureStatusResponse {
        paused: new_paused,
        indicator_visible,
    })
}

#[command]
pub async fn set_indicator_visible(
    app: AppHandle,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<(), IpcError> {
    state.indicator_visible.store(visible, Ordering::Relaxed);
    let paused = state.capture_paused.load(Ordering::Relaxed);

    let payload = serde_json::json!({ "paused": paused, "indicator_visible": visible });
    if let Err(e) = app.emit_to("magic-overlay", "overlay:capture-state-changed", &payload) {
        debug!("emit magic-overlay failed: {e}");
    }
    if let Err(e) = app.emit_to("tracking-panel", "overlay:capture-state-changed", &payload) {
        debug!("emit tracking-panel failed: {e}");
    }

    if let Some(panel) = app.get_webview_window("tracking-panel") {
        if visible {
            if let Err(e) = panel.show() {
                debug!("window show failed: {e}");
            }
        } else if let Err(e) = panel.hide() {
            debug!("window hide failed: {e}");
        }
    }
    if let Err(e) = crate::tray::sync_tray_state(&app, paused, visible) {
        debug!("sync_tray_state failed: {e}");
    }

    #[cfg(target_os = "macos")]
    if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>() {
        if visible && !crate::app_runtime_launch::cua_safe_mode_enabled() {
            border.0.show();
        } else {
            border.0.hide();
        }
    }

    Ok(())
}

#[command]
pub async fn get_connection_status(
    state: State<'_, AppState>,
) -> Result<ConnectionStatusResponse, IpcError> {
    Ok(ConnectionStatusResponse {
        server: state.connection.server_connected.load(Ordering::Relaxed),
        llm: state.connection.llm_connected.load(Ordering::Relaxed),
        cli: state.connection.cli_connected.load(Ordering::Relaxed),
    })
}

#[command]
pub async fn show_main_window(app: AppHandle) -> Result<(), IpcError> {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(e) = window.show() {
            debug!("window show failed: {e}");
        }
        if let Err(e) = window.set_focus() {
            debug!("set_focus failed: {e}");
        }
        Ok(())
    } else {
        // Main window should always be present in a normal run; if it's
        // missing the app is in an unexpected state. not_found.resource_missing
        // matches the workspace wire-code convention for "named resource
        // absent".
        Err(IpcError::new(
            "not_found.resource_missing",
            "main window not found",
        ))
    }
}

#[command]
pub async fn open_devtools(app: AppHandle, label: Option<String>) -> Result<(), IpcError> {
    #[cfg(debug_assertions)]
    {
        let target = label.as_deref().unwrap_or("main");
        if let Some(window) = app.get_webview_window(target) {
            window.open_devtools();
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (app, label);
    }
    Ok(())
}

#[command]
pub async fn save_panel_position(
    state: State<'_, AppState>,
    x: f64,
    y: f64,
) -> Result<(), IpcError> {
    let pos = format!("{x},{y}");
    state.storage.set_meta("tracking_panel_position", &pos);
    Ok(())
}

#[command]
pub async fn get_panel_position(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, IpcError> {
    let raw = match state.storage.get_meta("tracking_panel_position") {
        Some(v) => v,
        None => return Ok(None),
    };

    let (x, y) = match parse_position(&raw) {
        Some(pos) => pos,
        None => {
            debug!("Saved panel position is malformed: {raw:?}, resetting to default");
            return Ok(None);
        }
    };

    let monitors: Vec<MonitorBounds> = app
        .available_monitors()
        .unwrap_or_else(|e| {
            debug!("Failed to query monitors: {e}");
            Vec::new()
        })
        .iter()
        .map(|m| MonitorBounds {
            x: m.position().x as f64,
            y: m.position().y as f64,
            width: m.size().width as f64,
            height: m.size().height as f64,
        })
        .collect();

    if is_position_valid(x, y, &monitors) {
        Ok(Some(raw))
    } else {
        debug!("Saved panel position ({x},{y}) is off-screen, resetting to default");
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_position tests ---

    #[test]
    fn test_parse_valid() {
        assert_eq!(parse_position("100.5,200.0"), Some((100.5, 200.0)));
    }

    #[test]
    fn test_parse_integers() {
        assert_eq!(parse_position("500,300"), Some((500.0, 300.0)));
    }

    #[test]
    fn test_parse_negative() {
        assert_eq!(parse_position("-100,50"), Some((-100.0, 50.0)));
    }

    #[test]
    fn test_parse_invalid_format() {
        assert_eq!(parse_position("not_a_number"), None);
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(parse_position(""), None);
    }

    #[test]
    fn test_parse_nan() {
        assert_eq!(parse_position("NaN,100"), None);
    }

    #[test]
    fn test_parse_infinity() {
        assert_eq!(parse_position("inf,100"), None);
    }

    #[test]
    fn test_parse_single_value() {
        assert_eq!(parse_position("100"), None);
    }

    // --- is_position_valid tests ---

    fn single_monitor() -> Vec<MonitorBounds> {
        vec![MonitorBounds {
            x: 0.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        }]
    }

    #[test]
    fn test_within_single_monitor() {
        assert!(is_position_valid(500.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_outside_all_monitors() {
        assert!(!is_position_valid(5000.0, 5000.0, &single_monitor()));
    }

    #[test]
    fn test_right_edge_within_margin() {
        // Right edge: x = mon_w - MARGIN = 1920 - 100 = 1820
        assert!(is_position_valid(1820.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_right_edge_beyond_margin() {
        // Right edge: x = mon_w - MARGIN + 1 = 1821
        assert!(!is_position_valid(1821.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_left_edge_within_margin() {
        // Left bound: x >= (0 - 260 + 100) = -160
        assert!(is_position_valid(-160.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_left_edge_beyond_margin() {
        // Left bound: x >= -160, so -161 is out
        assert!(!is_position_valid(-161.0, 300.0, &single_monitor()));
    }

    #[test]
    fn test_top_edge_exact() {
        // y = mon_y = 0 (exactly at top)
        assert!(is_position_valid(500.0, 0.0, &single_monitor()));
    }

    #[test]
    fn test_above_top_edge() {
        // y = -1 (above monitor top)
        assert!(!is_position_valid(500.0, -1.0, &single_monitor()));
    }

    #[test]
    fn test_multi_monitor_secondary() {
        let monitors = vec![
            MonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            MonitorBounds {
                x: 1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];
        assert!(is_position_valid(2500.0, 300.0, &monitors));
    }

    #[test]
    fn test_negative_monitor_coords() {
        let monitors = vec![
            MonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            MonitorBounds {
                x: -1920.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
        ];
        assert!(is_position_valid(-1000.0, 100.0, &monitors));
    }

    #[test]
    fn test_empty_monitors() {
        assert!(!is_position_valid(500.0, 300.0, &[]));
    }
}
