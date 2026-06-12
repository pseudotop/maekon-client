use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tracing::info;

/// Create the tracking panel window — a small, transparent, always-on-top
/// indicator bar centered horizontally near the top of the primary monitor.
///
/// Starts hidden; shown/hidden via the `toggle-indicator` tray menu item
/// or IPC commands. The panel renders the capture-active border indicator.
///
/// Gracefully degrades on Linux/Wayland (panel not supported).
pub fn create_tracking_panel(app_handle: &AppHandle) -> Result<(), String> {
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

    #[cfg(target_os = "macos")]
    let y = {
        use objc2::MainThreadMarker;
        MainThreadMarker::new()
            .and_then(|mtm| {
                let screen = objc2_app_kit::NSScreen::mainScreen(mtm)?;
                let vf = screen.visibleFrame();
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
    .max_inner_size(320.0, 430.0)
    .visible(false)
    .skip_taskbar(true)
    .shadow(false)
    .build()
    .map_err(|e| format!("panel build: {e}"))?;

    info!("Tracking panel window created");
    Ok(())
}
