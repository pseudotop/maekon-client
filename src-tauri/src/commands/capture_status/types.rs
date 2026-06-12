//! Response DTOs and the `debug_window_state_response` helper shared between
//! `mod.rs` and `window_commands`.
//!
//! ADR-013 split from `capture_status/mod.rs`.

use serde::Serialize;
use tauri::{AppHandle, Manager};
use tracing::debug;

use super::position::{resolve_point_monitor_index, resolve_window_monitor_index, MonitorBounds};

#[derive(Serialize)]
pub struct DebugMonitorInfo {
    pub index: usize,
    pub name: Option<String>,
    pub position: (i32, i32),
    pub size: (u32, u32),
    pub scale_factor: f64,
    pub contains_window_center: bool,
    pub current: bool,
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

#[derive(Serialize)]
pub struct DebugWindowFocusResponse {
    pub ok: bool,
    pub label: String,
    pub visible: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Serialize)]
pub struct DebugWindowStateResponse {
    pub ok: bool,
    pub label: String,
    pub exists: bool,
    pub visible: Option<bool>,
    pub focused: Option<bool>,
    pub fullscreen: Option<bool>,
    pub outer_position: Option<(i32, i32)>,
    pub inner_size: Option<(u32, u32)>,
    pub cursor_position: Option<(f64, f64)>,
    pub cursor_monitor_index: Option<usize>,
    pub resolved_monitor_index: Option<usize>,
    pub current_monitor: Option<DebugMonitorInfo>,
    pub available_monitors: Vec<DebugMonitorInfo>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Serialize)]
pub struct DebugWindowNormalizationResponse {
    pub ok: bool,
    pub requested: crate::window_state::MainWindowState,
    pub normalized: crate::window_state::MainWindowState,
    pub available_monitors: Vec<crate::window_state::MonitorBounds>,
    pub requested_fits_available_monitor: bool,
    pub normalized_fits_available_monitor: bool,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

pub(super) fn debug_window_state_response(
    app: &AppHandle,
    label: String,
) -> DebugWindowStateResponse {
    let Some(window) = app.get_webview_window(&label) else {
        return DebugWindowStateResponse {
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
        };
    };

    let outer_position = window
        .outer_position()
        .ok()
        .map(|position| (position.x, position.y));
    let inner_size = window
        .inner_size()
        .ok()
        .map(|size| (size.width, size.height));
    let monitor_handles = app.available_monitors().unwrap_or_else(|error| {
        debug!("debug window state monitor query failed: {error}");
        Vec::new()
    });
    let monitor_bounds: Vec<MonitorBounds> = monitor_handles
        .iter()
        .map(|monitor| MonitorBounds {
            x: monitor.position().x as f64,
            y: monitor.position().y as f64,
            width: monitor.size().width as f64,
            height: monitor.size().height as f64,
        })
        .collect();
    let resolved_monitor_index = match (outer_position, inner_size) {
        (Some((x, y)), Some((width, height))) => resolve_window_monitor_index(
            x as f64,
            y as f64,
            width as f64,
            height as f64,
            &monitor_bounds,
        ),
        _ => None,
    };
    let cursor_position = app
        .cursor_position()
        .ok()
        .map(|position| (position.x, position.y));
    let cursor_monitor =
        cursor_position.and_then(|(x, y)| app.monitor_from_point(x, y).ok().flatten());
    let current_monitor = window.current_monitor().ok().flatten();
    let available_monitors: Vec<DebugMonitorInfo> = monitor_handles
        .iter()
        .enumerate()
        .map(|(index, monitor)| {
            let contains_window_center = resolved_monitor_index == Some(index);
            let current = current_monitor.as_ref().is_some_and(|current| {
                current.position() == monitor.position()
                    && current.size() == monitor.size()
                    && (current.scale_factor() - monitor.scale_factor()).abs() < f64::EPSILON
            });
            DebugMonitorInfo {
                index,
                name: monitor.name().map(ToString::to_string),
                position: (monitor.position().x, monitor.position().y),
                size: (monitor.size().width, monitor.size().height),
                scale_factor: monitor.scale_factor(),
                contains_window_center,
                current,
            }
        })
        .collect();
    let cursor_monitor_index = cursor_monitor
        .as_ref()
        .and_then(|cursor| {
            monitor_handles.iter().position(|monitor| {
                cursor.position() == monitor.position()
                    && cursor.size() == monitor.size()
                    && (cursor.scale_factor() - monitor.scale_factor()).abs() < f64::EPSILON
            })
        })
        .or_else(|| {
            cursor_position.and_then(|(x, y)| resolve_point_monitor_index(x, y, &monitor_bounds))
        });
    let current_monitor = available_monitors
        .iter()
        .find(|monitor| monitor.current)
        .or_else(|| resolved_monitor_index.and_then(|index| available_monitors.get(index)))
        .map(|monitor| DebugMonitorInfo {
            index: monitor.index,
            name: monitor.name.clone(),
            position: monitor.position,
            size: monitor.size,
            scale_factor: monitor.scale_factor,
            contains_window_center: monitor.contains_window_center,
            current: monitor.current,
        });

    DebugWindowStateResponse {
        ok: true,
        label,
        exists: true,
        visible: window.is_visible().ok(),
        focused: window.is_focused().ok(),
        fullscreen: window.is_fullscreen().ok(),
        outer_position,
        inner_size,
        cursor_position,
        cursor_monitor_index,
        resolved_monitor_index,
        current_monitor,
        available_monitors,
        error_code: None,
        error_message: None,
    }
}
