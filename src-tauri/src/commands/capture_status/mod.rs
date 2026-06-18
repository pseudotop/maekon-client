//! Capture status, connection status, panel position, and window management commands.
//!
//! ADR-013 split: 1,129-line `capture_status.rs` → focused submodules.
//!
//! | Submodule | Commands |
//! |---------|---------|
//! | `mod.rs` | `get_capture_status`, `toggle_capture_pause`, `set_indicator_visible`, `get_connection_status`, `save_panel_position`, `get_panel_position` |
//! | `window_commands` | `show_main_window`, `debug_focus_window`, `debug_window_state`, `debug_set_window_fullscreen`, `debug_set_window_bounds`, `debug_normalize_main_window_state`, `debug_normalize_main_window_bounds`, `debug_place_overlay_for_window`, `open_devtools` |
//! | `position` | Panel position validation, monitor bounds, coordinate helpers |
//! | `types` | Response DTOs and `debug_window_state_response` |

pub mod window_commands;

mod position;
mod types;

use types::{
    CaptureStatusResponse, ConnectionStatusResponse, DebugWindowFocusResponse,
    DebugWindowNormalizationResponse, DebugWindowStateResponse,
};

use std::sync::atomic::Ordering;
use tauri::{command, AppHandle, Emitter, Manager, State};
use tracing::debug;

use crate::ipc_error::IpcError;
use crate::runtime_state::AppState;

use position::{is_position_valid, parse_position, MonitorBounds};
// ---------------------------------------------------------------------------
// Capture / connection status commands
// ---------------------------------------------------------------------------

#[command]
pub async fn show_main_window(app: AppHandle) -> Result<(), IpcError> {
    window_commands::show_main_window(app).await
}

#[command]
pub async fn debug_focus_window(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowFocusResponse, IpcError> {
    window_commands::debug_focus_window(app, label).await
}

#[command]
pub async fn debug_window_state(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_window_state(app, label).await
}

#[command]
pub async fn debug_set_window_bounds(
    app: AppHandle,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_set_window_bounds(app, label, x, y, width, height).await
}

#[command]
pub async fn debug_place_overlay_for_window(
    app: AppHandle,
    target_label: String,
    interactive: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_place_overlay_for_window(app, target_label, interactive).await
}

#[command]
pub async fn debug_normalize_main_window_state(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<DebugWindowNormalizationResponse, IpcError> {
    window_commands::debug_normalize_main_window_state(app, x, y, width, height).await
}

#[command]
pub async fn debug_normalize_main_window_bounds(
    app: AppHandle,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_normalize_main_window_bounds(app).await
}

#[command]
pub async fn debug_set_window_fullscreen(
    app: AppHandle,
    label: String,
    fullscreen: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    window_commands::debug_set_window_fullscreen(app, label, fullscreen).await
}

#[command]
pub async fn open_devtools(app: AppHandle, label: Option<String>) -> Result<(), IpcError> {
    window_commands::open_devtools(app, label).await
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
    crate::magic_overlay::sync_passive_tracking_surface(&app, new_paused, indicator_visible);

    #[cfg(target_os = "macos")]
    if let Some(border) = app.try_state::<crate::native_border::NativeBorderState>() {
        border.0.set_paused(new_paused);
    }

    // Privacy: re-gate any running VAD listener at once so the Privacy page /
    // tracking-panel pause button tears down the mic immediately (not after the
    // ≤2 s backstop tick), AND remember/auto-rearm VAD across the pause toggle.
    // Shared helper — every pause site must route through this.
    crate::commands::audio::on_capture_pause_toggled(&app, new_paused);

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
    crate::magic_overlay::sync_passive_tracking_surface(&app, paused, visible);

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

// ---------------------------------------------------------------------------
// Panel position commands
// ---------------------------------------------------------------------------

/// F-RR-06: set_meta acquires a parking_lot write lock (blocking).
/// Move it off the tokio reactor thread via spawn_blocking.
#[command]
pub async fn save_panel_position(
    state: State<'_, AppState>,
    x: f64,
    y: f64,
) -> Result<(), IpcError> {
    let storage = state.storage.clone();
    let pos = format!("{x},{y}");
    tokio::task::spawn_blocking(move || {
        storage.set_meta("tracking_panel_position", &pos);
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("save_panel_position task join failed: {join_err}"),
        )
    })
}

/// F-RR-06: get_meta acquires a parking_lot read lock (blocking).
/// Move it off the tokio reactor thread via spawn_blocking.
#[command]
pub async fn get_panel_position(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Option<String>, IpcError> {
    let storage = state.storage.clone();
    let raw_opt: Option<String> =
        tokio::task::spawn_blocking(move || storage.get_meta("tracking_panel_position"))
            .await
            .map_err(|join_err| {
                IpcError::new(
                    "internal.generic",
                    format!("get_panel_position task join failed: {join_err}"),
                )
            })?;
    let raw: String = match raw_opt {
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
    use super::position::{
        is_position_valid, parse_position, resolve_point_monitor_index,
        resolve_window_monitor_index, MonitorBounds,
    };

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

    #[test]
    fn debug_window_state_monitor_resolution_selects_secondary_by_center() {
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

        let selected = resolve_window_monitor_index(2100.0, 120.0, 800.0, 600.0, &monitors);

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn debug_window_state_cursor_resolution_selects_secondary_point() {
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

        let selected = resolve_point_monitor_index(2400.0, 500.0, &monitors);

        assert_eq!(selected, Some(1));
    }
}
