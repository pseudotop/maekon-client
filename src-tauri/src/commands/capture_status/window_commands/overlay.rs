//! Overlay window placement debug command.
//!
//! ADR-013 split from `capture_status/window_commands.rs`.

#[cfg(debug_assertions)]
use std::time::Duration;
use tauri::AppHandle;
#[cfg(debug_assertions)]
use tauri::{LogicalPosition, LogicalSize, Manager};

use crate::ipc_error::IpcError;

use super::super::types::DebugWindowStateResponse;
#[cfg(debug_assertions)]
use super::debug_window_state_response;

#[cfg(not(debug_assertions))]
pub async fn debug_place_overlay_for_window(
    app: AppHandle,
    target_label: String,
    interactive: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    let _ = (app, target_label, interactive);
    Ok(super::debug_window_state_disabled())
}

#[cfg(debug_assertions)]
pub async fn debug_place_overlay_for_window(
    app: AppHandle,
    target_label: String,
    interactive: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    let target_label = target_label.trim().to_string();
    let Some(target_window) = app.get_webview_window(&target_label) else {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label: "magic-overlay".to_string(),
            exists: false,
            visible: None,
            focused: None,
            fullscreen: None,
            outer_position: None,
            inner_size: None,
            cursor_position: None,
            cursor_monitor_index: None,
            resolved_monitor_index: None,
            current_monitor: None,
            available_monitors: Vec::new(),
            error_code: Some("not_found.target_window_missing".to_string()),
            error_message: Some("target window not found".to_string()),
        });
    };
    let Some(overlay_window) = app.get_webview_window("magic-overlay") else {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label: "magic-overlay".to_string(),
            exists: false,
            visible: None,
            focused: None,
            fullscreen: None,
            outer_position: None,
            inner_size: None,
            cursor_position: None,
            cursor_monitor_index: None,
            resolved_monitor_index: None,
            current_monitor: None,
            available_monitors: Vec::new(),
            error_code: Some("not_found.overlay_window_missing".to_string()),
            error_message: Some("overlay window not found".to_string()),
        });
    };
    let Some(monitor) = target_window.current_monitor().ok().flatten() else {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label: "magic-overlay".to_string(),
            exists: true,
            visible: overlay_window.is_visible().ok(),
            focused: overlay_window.is_focused().ok(),
            fullscreen: overlay_window.is_fullscreen().ok(),
            outer_position: overlay_window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            inner_size: overlay_window
                .inner_size()
                .ok()
                .map(|size| (size.width, size.height)),
            cursor_position: app
                .cursor_position()
                .ok()
                .map(|position| (position.x, position.y)),
            cursor_monitor_index: None,
            resolved_monitor_index: None,
            current_monitor: None,
            available_monitors: Vec::new(),
            error_code: Some("not_found.target_monitor_missing".to_string()),
            error_message: Some("target window monitor not found".to_string()),
        });
    };

    let scale = if monitor.scale_factor().is_finite() && monitor.scale_factor() > 0.0 {
        monitor.scale_factor()
    } else {
        1.0
    };
    let position = monitor.position();
    let size = monitor.size();
    let x = position.x as f64 / scale;
    let y = position.y as f64 / scale;
    let width = size.width as f64 / scale;
    let height = size.height as f64 / scale;

    if let Err(error) = overlay_window.set_size(LogicalSize::new(width, height)) {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label: "magic-overlay".to_string(),
            exists: true,
            visible: overlay_window.is_visible().ok(),
            focused: overlay_window.is_focused().ok(),
            fullscreen: overlay_window.is_fullscreen().ok(),
            outer_position: overlay_window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            inner_size: overlay_window
                .inner_size()
                .ok()
                .map(|size| (size.width, size.height)),
            cursor_position: app
                .cursor_position()
                .ok()
                .map(|position| (position.x, position.y)),
            cursor_monitor_index: None,
            resolved_monitor_index: None,
            current_monitor: None,
            available_monitors: Vec::new(),
            error_code: Some("window.set_size_failed".to_string()),
            error_message: Some(error.to_string()),
        });
    }
    if let Err(error) = overlay_window.set_position(LogicalPosition::new(x, y)) {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label: "magic-overlay".to_string(),
            exists: true,
            visible: overlay_window.is_visible().ok(),
            focused: overlay_window.is_focused().ok(),
            fullscreen: overlay_window.is_fullscreen().ok(),
            outer_position: overlay_window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            inner_size: overlay_window
                .inner_size()
                .ok()
                .map(|size| (size.width, size.height)),
            cursor_position: app
                .cursor_position()
                .ok()
                .map(|position| (position.x, position.y)),
            cursor_monitor_index: None,
            resolved_monitor_index: None,
            current_monitor: None,
            available_monitors: Vec::new(),
            error_code: Some("window.set_position_failed".to_string()),
            error_message: Some(error.to_string()),
        });
    }
    let _ = overlay_window.set_focusable(false);
    let _ = overlay_window.show();
    let _ = overlay_window.set_ignore_cursor_events(!interactive);

    tokio::time::sleep(Duration::from_millis(250)).await;

    Ok(debug_window_state_response(
        &app,
        "magic-overlay".to_string(),
    ))
}
