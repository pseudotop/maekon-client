//! Main window normalization debug commands.
//!
//! ADR-013 split from `capture_status/window_commands.rs`.

#[cfg(debug_assertions)]
use std::time::Duration;
use tauri::AppHandle;
#[cfg(debug_assertions)]
use tauri::Manager;

use crate::ipc_error::IpcError;

use super::super::types::{DebugWindowNormalizationResponse, DebugWindowStateResponse};
#[cfg(debug_assertions)]
use super::debug_window_state_response;

#[cfg(debug_assertions)]
fn state_fits_available_monitor(
    state: crate::window_state::MainWindowState,
    monitors: &[crate::window_state::MonitorBounds],
) -> bool {
    let right = i64::from(state.x) + i64::from(state.width);
    let bottom = i64::from(state.y) + i64::from(state.height);
    monitors.iter().any(|monitor| {
        let monitor_right = i64::from(monitor.x) + i64::from(monitor.width);
        let monitor_bottom = i64::from(monitor.y) + i64::from(monitor.height);
        i64::from(state.x) >= i64::from(monitor.x)
            && i64::from(state.y) >= i64::from(monitor.y)
            && right <= monitor_right
            && bottom <= monitor_bottom
    })
}

#[cfg(not(debug_assertions))]
pub async fn debug_normalize_main_window_state(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<DebugWindowNormalizationResponse, IpcError> {
    let _ = (app, x, y, width, height);
    Ok(DebugWindowNormalizationResponse {
        ok: false,
        requested: crate::window_state::MainWindowState {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        normalized: crate::window_state::MainWindowState {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        },
        available_monitors: Vec::new(),
        requested_fits_available_monitor: false,
        normalized_fits_available_monitor: false,
        error_code: Some("debug_only".to_string()),
        error_message: Some("debug commands are not available in release builds".to_string()),
    })
}

#[cfg(debug_assertions)]
pub async fn debug_normalize_main_window_state(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<DebugWindowNormalizationResponse, IpcError> {
    let requested = crate::window_state::MainWindowState {
        x,
        y,
        width,
        height,
    };
    let available_monitors: Vec<crate::window_state::MonitorBounds> = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|monitor| crate::window_state::MonitorBounds {
            x: monitor.position().x,
            y: monitor.position().y,
            width: monitor.size().width,
            height: monitor.size().height,
        })
        .collect();
    let normalized =
        crate::window_state::normalize_state_for_available_monitors(requested, &available_monitors);
    Ok(DebugWindowNormalizationResponse {
        ok: true,
        requested,
        normalized,
        requested_fits_available_monitor: state_fits_available_monitor(
            requested,
            &available_monitors,
        ),
        normalized_fits_available_monitor: state_fits_available_monitor(
            normalized,
            &available_monitors,
        ),
        available_monitors,
        error_code: None,
        error_message: None,
    })
}

#[cfg(not(debug_assertions))]
pub async fn debug_normalize_main_window_bounds(
    app: AppHandle,
) -> Result<DebugWindowStateResponse, IpcError> {
    let _ = app;
    Ok(super::debug_window_state_disabled())
}

#[cfg(debug_assertions)]
pub async fn debug_normalize_main_window_bounds(
    app: AppHandle,
) -> Result<DebugWindowStateResponse, IpcError> {
    if let Some(window) = app.get_webview_window("main") {
        crate::window_state::ensure_main_webview_window_on_available_monitor(&window);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Ok(debug_window_state_response(&app, "main".to_string()))
}
