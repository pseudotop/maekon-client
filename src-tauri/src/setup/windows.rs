use tauri::{App, Manager};
use tracing::debug;

pub(crate) fn prepare(app: &App) {
    let app_handle = app.handle().clone();
    let precreate_auxiliary_webviews = crate::app_runtime_launch::precreate_auxiliary_webviews(
        crate::app_runtime_launch::cua_safe_mode_enabled(),
    );

    // CUA-safe sessions keep only the main WebView alive at startup. Auxiliary
    // surfaces are created lazily when a test explicitly requests them.
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

        // Pre-create MagicOverlay for normal sessions so asynchronous overlay
        // events have a ready listener. CUA-safe sessions create it only for
        // explicit overlay checks and tear it down again when idle.
        if let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() {
            if let Some(ref overlay) = state.magic_overlay {
                if let Err(e) = overlay.ensure_window() {
                    debug!("ensure_window failed: {e}");
                }
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
