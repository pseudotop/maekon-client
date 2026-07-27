//! Window management helpers for the MagicOverlay: monitor resolution,
//! layout calculation, and window creation utilities.

use tauri::{
    AppHandle, Manager, Monitor, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tracing::{debug, info};

pub(super) const OVERLAY_LABEL: &str = "magic-overlay";
pub(super) const OVERLAY_URL: &str = "overlay.html";
pub(super) const TRACKING_PANEL_LABEL: &str = "tracking-panel";
pub(super) const TRACKING_BORDER_TOP_LABEL: &str = "tracking-border-top";
pub(super) const TRACKING_BORDER_BOTTOM_LABEL: &str = "tracking-border-bottom";
pub(super) const TRACKING_BORDER_LEFT_LABEL: &str = "tracking-border-left";
pub(super) const TRACKING_BORDER_RIGHT_LABEL: &str = "tracking-border-right";
pub(super) const TRACKING_BORDER_LABELS: [&str; 4] = [
    TRACKING_BORDER_TOP_LABEL,
    TRACKING_BORDER_BOTTOM_LABEL,
    TRACKING_BORDER_LEFT_LABEL,
    TRACKING_BORDER_RIGHT_LABEL,
];
const TRACKING_BORDER_SURFACE_THICKNESS: f64 = 16.0;

// ── Monitor geometry helpers ─────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct OverlayMonitorLayout {
    pub(super) origin_x: f64,
    pub(super) origin_y: f64,
    pub(super) logical_width: f64,
    pub(super) logical_height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct TrackingBorderWindowSpec {
    pub(super) label: &'static str,
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

pub(super) fn is_internal_overlay_window(label: &str) -> bool {
    matches!(label, OVERLAY_LABEL | TRACKING_PANEL_LABEL) || TRACKING_BORDER_LABELS.contains(&label)
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

pub(super) fn tracking_border_window_specs(
    layout: OverlayMonitorLayout,
    thickness: f64,
) -> [TrackingBorderWindowSpec; 4] {
    let max_thickness = layout.logical_width.min(layout.logical_height).max(1.0);
    let thickness = thickness.clamp(1.0, max_thickness);
    [
        TrackingBorderWindowSpec {
            label: TRACKING_BORDER_TOP_LABEL,
            x: layout.origin_x + thickness,
            y: layout.origin_y,
            width: (layout.logical_width - (thickness * 2.0)).max(thickness),
            height: thickness,
        },
        TrackingBorderWindowSpec {
            label: TRACKING_BORDER_BOTTOM_LABEL,
            x: layout.origin_x + thickness,
            y: layout.origin_y + layout.logical_height - thickness,
            width: (layout.logical_width - (thickness * 2.0)).max(thickness),
            height: thickness,
        },
        TrackingBorderWindowSpec {
            label: TRACKING_BORDER_LEFT_LABEL,
            x: layout.origin_x,
            y: layout.origin_y + thickness,
            width: thickness,
            height: (layout.logical_height - (thickness * 2.0)).max(thickness),
        },
        TrackingBorderWindowSpec {
            label: TRACKING_BORDER_RIGHT_LABEL,
            x: layout.origin_x + layout.logical_width - thickness,
            y: layout.origin_y + thickness,
            width: thickness,
            height: (layout.logical_height - (thickness * 2.0)).max(thickness),
        },
    ]
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

pub(super) fn focused_app_monitor(app_handle: &AppHandle) -> Option<Monitor> {
    app_handle
        .webview_windows()
        .into_values()
        .find_map(|window| {
            if is_internal_overlay_window(window.label()) {
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

// ── Window show helper ───────────────────────────────────────────────────────

pub(super) fn show_overlay_window<R: Runtime>(window: &WebviewWindow<R>) -> Result<(), String> {
    window
        .show()
        .map_err(|error| format!("window show failed: {error}"))
}

pub(super) fn sync_tracking_border_windows(
    app_handle: &AppHandle,
    visible: bool,
) -> Result<(), String> {
    if !visible {
        for label in TRACKING_BORDER_LABELS {
            if let Some(window) = app_handle.get_webview_window(label) {
                if let Err(error) = window.destroy() {
                    debug!("tracking border window destroy failed: {error}");
                }
            }
        }
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        tracing::warn!("Wayland tracking border windows disabled");
        return Err("Wayland not supported".to_string());
    }

    let layout = overlay_monitor_layout(&target_overlay_monitor(app_handle)?);
    for spec in tracking_border_window_specs(layout, TRACKING_BORDER_SURFACE_THICKNESS) {
        let window = if let Some(window) = app_handle.get_webview_window(spec.label) {
            window
        } else {
            WebviewWindowBuilder::new(app_handle, spec.label, WebviewUrl::App(OVERLAY_URL.into()))
                .title("Maekon Recording Border")
                .inner_size(spec.width, spec.height)
                .position(spec.x, spec.y)
                .transparent(true)
                .always_on_top(true)
                .decorations(false)
                .focused(false)
                .focusable(false)
                .resizable(false)
                .visible(false)
                .skip_taskbar(true)
                .shadow(false)
                .build()
                .map_err(|e| format!("border build: {e}"))?
        };

        let _ = window.set_position(tauri::LogicalPosition::new(spec.x, spec.y));
        let _ = window.set_size(tauri::LogicalSize::new(spec.width, spec.height));
        let _ = window.set_focusable(false);
        if let Err(e) = window.set_ignore_cursor_events(true) {
            debug!("border set_ignore_cursor_events failed: {e}");
        }
        show_overlay_window(&window)?;
    }

    Ok(())
}

// ── Tracking panel creation ──────────────────────────────────────────────────

/// Create the tracking panel window — a small, transparent, always-on-top
/// indicator bar centered horizontally near the top of the primary monitor.
///
/// Starts hidden; shown/hidden via the `toggle-indicator` tray menu item
/// or IPC commands. The panel renders the capture-active border indicator.
///
/// Gracefully degrades on Linux/Wayland (panel not supported).
pub fn create_tracking_panel<R: Runtime>(app_handle: &AppHandle<R>) -> Result<(), String> {
    if app_handle.get_webview_window("tracking-panel").is_some() {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    if std::env::var("WAYLAND_DISPLAY").is_ok() {
        tracing::warn!("Wayland — tracking panel disabled");
        return Err("Wayland not supported".to_string());
    }

    let monitor = app_handle
        .primary_monitor()
        .map_err(|e| format!("monitor: {e}"))?
        .ok_or("No monitor")?;

    let scale = monitor.scale_factor();
    let logical_width = monitor.size().width as f64 / scale;
    let logical_height = monitor.size().height as f64 / scale;
    let panel_width = 260.0;
    let panel_height = 36.0;
    let x = (logical_width / 2.0) - (panel_width / 2.0);

    // Dock-aware Y: use NSScreen::visibleFrame() to avoid Dock overlap.
    // macOS coords: bottom-left origin. Tauri coords: top-left origin.
    // visibleFrame().origin.y = distance from screen bottom to Dock top (in points).
    #[cfg(target_os = "macos")]
    let y = {
        use objc2::MainThreadMarker;
        MainThreadMarker::new()
            .and_then(|mtm| {
                let screen = objc2_app_kit::NSScreen::mainScreen(mtm)?;
                let vf = screen.visibleFrame();
                // Convert macOS bottom-up to Tauri top-down:
                // tauri_y = logical_height - (macos_y / scale) - panel_height - margin
                Some(logical_height - vf.origin.y / scale - panel_height - 8.0)
            })
            .unwrap_or(logical_height - panel_height - 80.0)
    };
    #[cfg(not(target_os = "macos"))]
    let y = logical_height - panel_height - 80.0;

    WebviewWindowBuilder::new(
        app_handle,
        "tracking-panel",
        WebviewUrl::App("tracking-panel.html".into()),
    )
    .title("Maekon Tracking")
    .inner_size(panel_width, panel_height)
    .position(x, y)
    .transparent(true)
    .always_on_top(true)
    .decorations(false)
    .resizable(true)
    .min_inner_size(260.0, 36.0)
    .max_inner_size(320.0, 310.0)
    .visible(false)
    .skip_taskbar(true)
    .shadow(false)
    .build()
    .map_err(|e| format!("panel build: {e}"))?;

    info!("Tracking panel window created");
    Ok(())
}

/// Reconcile the tracking panel's native WebView lifetime with its requested
/// visibility. Destroying an idle panel releases its WebView2 controller;
/// showing it again recreates the panel lazily.
pub(crate) fn set_tracking_panel_visible<R: Runtime>(
    app_handle: &AppHandle<R>,
    visible: bool,
) -> Result<(), String> {
    if visible {
        create_tracking_panel(app_handle)?;
        let panel = app_handle
            .get_webview_window(TRACKING_PANEL_LABEL)
            .ok_or_else(|| "tracking panel unavailable after creation".to_string())?;
        show_overlay_window(&panel)
    } else {
        if let Some(panel) = app_handle.get_webview_window(TRACKING_PANEL_LABEL) {
            panel
                .destroy()
                .map_err(|error| format!("tracking panel destroy failed: {error}"))?;
        }
        Ok(())
    }
}
