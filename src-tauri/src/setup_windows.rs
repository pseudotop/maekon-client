use tauri::{App, Manager};
use tracing::debug;

pub(crate) fn prepare(app: &App) {
    let app_handle = app.handle().clone();

    // Create tracking panel window (starts hidden, auto-shown if indicator is configured visible)
    if let Err(e) = crate::magic_overlay::create_tracking_panel(&app_handle) {
        debug!("create_tracking_panel failed: {e}");
    }
    if let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() {
        if state
            .indicator_visible
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            if let Some(panel) = app_handle.get_webview_window("tracking-panel") {
                if let Err(e) = panel.show() {
                    debug!("window show failed: {e}");
                }
            }
        }
    }

    // Pre-create MagicOverlay window (hidden). Shown on-demand when coaching
    // popups, suggestions, or focus highlights are triggered.
    // NOTE: Do NOT show() here — a visible full-screen overlay at the same
    // window level blocks tracking panel drag on macOS.
    if let Some(state) = app_handle.try_state::<crate::runtime_state::AppState>() {
        if let Some(ref overlay) = state.magic_overlay {
            if let Err(e) = overlay.ensure_window() {
                debug!("ensure_window failed: {e}");
            }
        }
    }
}
