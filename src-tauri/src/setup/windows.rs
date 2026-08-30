use tauri::{App, Manager};
use tracing::debug;

pub(crate) fn prepare(app: &App) {
    let app_handle = app.handle().clone();
    let precreate_auxiliary_webviews = crate::app_runtime_launch::precreate_auxiliary_webviews(
        crate::app_runtime_launch::cua_safe_mode_enabled(),
    );

    // CUA-safe sessions keep only the main WebView alive at startup. Normal
    // sessions may show the bounded tracking panel, but the display-sized
    // MagicOverlay must remain absent until actual overlay content requests it.
    // A hidden pre-created WebView is still enumerated by ScreenCaptureKit on
    // macOS and selecting it produces a full-screen black frame (#11647).
    if precreate_auxiliary_webviews {
        if let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() {
            let indicator_visible = state
                .indicator_visible
                .load(std::sync::atomic::Ordering::Relaxed);
            if let Err(e) =
                crate::magic_overlay::set_tracking_panel_visible(&app_handle, indicator_visible)
            {
                debug!("tracking panel startup reconcile failed: {e}");
            }
        }
    }

    if let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() {
        crate::magic_overlay::sync_passive_tracking_surface(
            &app_handle,
            state
                .capture_paused
                .load(std::sync::atomic::Ordering::Relaxed),
            state
                .indicator_visible
                .load(std::sync::atomic::Ordering::Relaxed),
        );
    }
}
