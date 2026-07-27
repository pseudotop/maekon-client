//! Window management Tauri commands.
//!
//! ADR-013 split from `capture_status/window_commands.rs`.
//!
//! | Submodule | Commands handled |
//! |---------|---------|
//! | `mod.rs` | `show_main_window`, `debug_focus_window`, `debug_window_state`, `debug_set_window_fullscreen`, `debug_set_window_bounds`, `open_devtools` |
//! | `normalization` | `debug_normalize_main_window_state`, `debug_normalize_main_window_bounds` |
//! | `overlay` | `debug_place_overlay_for_window` |

mod normalization;
mod overlay;

#[cfg(debug_assertions)]
use std::time::Duration;
use tauri::{AppHandle, Manager};
#[cfg(debug_assertions)]
use tauri::{LogicalPosition, LogicalSize};
use tracing::debug;

use crate::ipc_error::IpcError;

use super::types::{DebugWindowFocusResponse, DebugWindowStateResponse};

#[cfg(debug_assertions)]
pub(super) use super::types::debug_window_state_response;

#[cfg(not(debug_assertions))]
pub(super) fn debug_window_state_disabled() -> DebugWindowStateResponse {
    DebugWindowStateResponse {
        ok: false,
        label: String::new(),
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
        error_code: Some("debug_only".to_string()),
        error_message: Some("debug commands are not available in release builds".to_string()),
    }
}

pub async fn debug_normalize_main_window_state(
    app: AppHandle,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<super::types::DebugWindowNormalizationResponse, IpcError> {
    normalization::debug_normalize_main_window_state(app, x, y, width, height).await
}

pub async fn debug_normalize_main_window_bounds(
    app: AppHandle,
) -> Result<DebugWindowStateResponse, IpcError> {
    normalization::debug_normalize_main_window_bounds(app).await
}

pub async fn debug_place_overlay_for_window(
    app: AppHandle,
    target_label: String,
    interactive: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    overlay::debug_place_overlay_for_window(app, target_label, interactive).await
}

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

#[cfg(not(debug_assertions))]
pub async fn debug_focus_window(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowFocusResponse, IpcError> {
    let _ = (app, label);
    Ok(DebugWindowFocusResponse {
        ok: false,
        label: String::new(),
        visible: false,
        error_code: Some("debug_only".to_string()),
        error_message: Some("debug commands are not available in release builds".to_string()),
    })
}

#[cfg(debug_assertions)]
pub async fn debug_focus_window(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowFocusResponse, IpcError> {
    let label = label.trim().to_string();
    if label.is_empty() {
        return Ok(DebugWindowFocusResponse {
            ok: false,
            label,
            visible: false,
            error_code: Some("validation.invalid_argument".to_string()),
            error_message: Some("window label is empty".to_string()),
        });
    }

    let Some(window) = app.get_webview_window(&label) else {
        return Ok(DebugWindowFocusResponse {
            ok: false,
            label,
            visible: false,
            error_code: Some("not_found.resource_missing".to_string()),
            error_message: Some("window not found".to_string()),
        });
    };

    if let Err(error) = window.show() {
        return Ok(DebugWindowFocusResponse {
            ok: false,
            label,
            visible: false,
            error_code: Some("window.show_failed".to_string()),
            error_message: Some(error.to_string()),
        });
    }
    if let Err(error) = window.set_focus() {
        return Ok(DebugWindowFocusResponse {
            ok: false,
            label,
            visible: false,
            error_code: Some("window.focus_failed".to_string()),
            error_message: Some(error.to_string()),
        });
    }

    let visible = window.is_visible().unwrap_or(true);
    Ok(DebugWindowFocusResponse {
        ok: true,
        label,
        visible,
        error_code: None,
        error_message: None,
    })
}

#[cfg(not(debug_assertions))]
pub async fn debug_window_state(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowStateResponse, IpcError> {
    let _ = (app, label);
    Ok(debug_window_state_disabled())
}

#[cfg(debug_assertions)]
pub async fn debug_window_state(
    app: AppHandle,
    label: String,
) -> Result<DebugWindowStateResponse, IpcError> {
    let label = label.trim().to_string();
    Ok(debug_window_state_response(&app, label))
}

#[cfg(not(debug_assertions))]
pub async fn debug_set_window_fullscreen(
    app: AppHandle,
    label: String,
    fullscreen: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    let _ = (app, label, fullscreen);
    Ok(debug_window_state_disabled())
}

#[cfg(debug_assertions)]
pub async fn debug_set_window_fullscreen(
    app: AppHandle,
    label: String,
    fullscreen: bool,
) -> Result<DebugWindowStateResponse, IpcError> {
    let label = label.trim().to_string();
    if label.is_empty() {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label,
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
            error_code: Some("validation.invalid_argument".to_string()),
            error_message: Some("window label is empty".to_string()),
        });
    }

    let Some(window) = app.get_webview_window(&label) else {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label,
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
            error_code: Some("not_found.resource_missing".to_string()),
            error_message: Some("window not found".to_string()),
        });
    };

    if let Err(error) = window.set_fullscreen(fullscreen) {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label,
            exists: true,
            visible: window.is_visible().ok(),
            focused: window.is_focused().ok(),
            fullscreen: window.is_fullscreen().ok(),
            outer_position: window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            inner_size: window
                .inner_size()
                .ok()
                .map(|size| (size.width, size.height)),
            cursor_position: None,
            cursor_monitor_index: None,
            resolved_monitor_index: None,
            current_monitor: None,
            available_monitors: Vec::new(),
            error_code: Some("window.fullscreen_failed".to_string()),
            error_message: Some(error.to_string()),
        });
    }

    tokio::time::sleep(Duration::from_millis(300)).await;

    Ok(debug_window_state_response(&app, label))
}

#[cfg(not(debug_assertions))]
pub async fn debug_set_window_bounds(
    app: AppHandle,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<DebugWindowStateResponse, IpcError> {
    let _ = (app, label, x, y, width, height);
    Ok(debug_window_state_disabled())
}

#[cfg(debug_assertions)]
pub async fn debug_set_window_bounds(
    app: AppHandle,
    label: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<DebugWindowStateResponse, IpcError> {
    let label = label.trim().to_string();
    if label.is_empty()
        || !x.is_finite()
        || !y.is_finite()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label,
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
            error_code: Some("validation.invalid_argument".to_string()),
            error_message: Some("window label is empty or bounds are invalid".to_string()),
        });
    }

    let Some(window) = app.get_webview_window(&label) else {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label,
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
            error_code: Some("not_found.resource_missing".to_string()),
            error_message: Some("window not found".to_string()),
        });
    };

    if let Err(error) = window.set_size(LogicalSize::new(width, height)) {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label,
            exists: true,
            visible: window.is_visible().ok(),
            focused: window.is_focused().ok(),
            fullscreen: window.is_fullscreen().ok(),
            outer_position: window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            inner_size: window
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
    if let Err(error) = window.set_position(LogicalPosition::new(x, y)) {
        return Ok(DebugWindowStateResponse {
            ok: false,
            label,
            exists: true,
            visible: window.is_visible().ok(),
            focused: window.is_focused().ok(),
            fullscreen: window.is_fullscreen().ok(),
            outer_position: window
                .outer_position()
                .ok()
                .map(|position| (position.x, position.y)),
            inner_size: window
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

    tokio::time::sleep(Duration::from_millis(250)).await;

    Ok(debug_window_state_response(&app, label))
}

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
