use super::{OVERLAY_LABEL, TRACKING_PANEL_LABEL};
use tauri::{AppHandle, Manager, Monitor, WebviewWindow};
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct OverlayMonitorLayout {
    pub(super) origin_x: f64,
    pub(super) origin_y: f64,
    pub(super) logical_width: f64,
    pub(super) logical_height: f64,
}

pub(super) fn show_overlay_window(window: &WebviewWindow) -> Result<(), String> {
    window
        .show()
        .map_err(|error| format!("window show failed: {error}"))
}

pub(super) fn overlay_monitor_layout_from_parts(
    position_x: i32,
    position_y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
) -> OverlayMonitorLayout {
    let scale = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };

    OverlayMonitorLayout {
        origin_x: position_x as f64 / scale,
        origin_y: position_y as f64 / scale,
        logical_width: width as f64 / scale,
        logical_height: height as f64 / scale,
    }
}

pub(super) fn overlay_monitor_layout(monitor: &Monitor) -> OverlayMonitorLayout {
    let position = monitor.position();
    let size = monitor.size();
    overlay_monitor_layout_from_parts(
        position.x,
        position.y,
        size.width,
        size.height,
        monitor.scale_factor(),
    )
}

fn focused_app_monitor(app_handle: &AppHandle) -> Option<Monitor> {
    app_handle
        .webview_windows()
        .into_values()
        .find_map(|window| {
            if matches!(window.label(), OVERLAY_LABEL | TRACKING_PANEL_LABEL) {
                return None;
            }
            match window.is_focused() {
                Ok(true) => window.current_monitor().ok().flatten(),
                Ok(false) => None,
                Err(error) => {
                    debug!("focused window monitor check failed: {error}");
                    None
                }
            }
        })
}

pub(super) fn target_overlay_monitor(app_handle: &AppHandle) -> Result<Monitor, String> {
    if let Some(monitor) = focused_app_monitor(app_handle) {
        return Ok(monitor);
    }

    if let Ok(cursor) = app_handle.cursor_position() {
        match app_handle.monitor_from_point(cursor.x, cursor.y) {
            Ok(Some(monitor)) => return Ok(monitor),
            Ok(None) => debug!("cursor position did not resolve to a monitor; falling back"),
            Err(error) => debug!("monitor_from_point failed: {error}; falling back"),
        }
    }

    app_handle
        .primary_monitor()
        .map_err(|error| format!("Failed to get primary monitor: {error}"))?
        .ok_or_else(|| "No primary monitor found".to_string())
}
