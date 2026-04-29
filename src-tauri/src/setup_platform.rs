use tauri::App;
#[cfg(target_os = "macos")]
use tauri::Manager;
#[cfg(target_os = "macos")]
use tracing::info;

pub(crate) fn apply(app: &mut App) {
    create_native_border_indicator(app);
    configure_tracking_panel_drag(app);
}

#[cfg(target_os = "macos")]
fn create_native_border_indicator(app: &mut App) {
    use objc2::MainThreadMarker;

    if let Some(mtm) = MainThreadMarker::new() {
        info!("Native border: MainThreadMarker acquired");
        match crate::native_border::NativeBorderIndicator::new(mtm) {
            Some(border) => {
                let border = std::sync::Arc::new(border);
                let (panel_visible, paused) = app
                    .app_handle()
                    .try_state::<crate::runtime_state::AppState>()
                    .map(|s| {
                        (
                            s.indicator_visible
                                .load(std::sync::atomic::Ordering::Relaxed),
                            s.capture_paused.load(std::sync::atomic::Ordering::Relaxed),
                        )
                    })
                    .unwrap_or((false, false));
                let border_visible =
                    panel_visible && !crate::app_runtime_launch::cua_safe_mode_enabled();
                border.set_paused(paused);
                if border_visible {
                    border.show();
                }

                // Spawn periodic screen topology monitor (every 5s)
                let border_for_task = border.clone();
                tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                    loop {
                        interval.tick().await;
                        border_for_task.check_and_rebuild();
                    }
                });

                app.manage(crate::native_border::NativeBorderState(border));
                info!(
                    "Native border indicator created (panel_visible={panel_visible}, border_visible={border_visible}, paused={paused})"
                );
            }
            None => {
                tracing::warn!("Native border: window creation failed");
            }
        }
    } else {
        tracing::warn!("Native border: not on main thread");
    }
}

#[cfg(not(target_os = "macos"))]
fn create_native_border_indicator(_app: &mut App) {}

#[cfg(target_os = "macos")]
fn configure_tracking_panel_drag(app: &mut App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // Workaround for -webkit-app-region not working on transparent borderless windows with WKWebView.
    if let Some(panel) = app.app_handle().get_webview_window("tracking-panel") {
        if let Ok(handle) = panel.window_handle() {
            if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
                let ns_view =
                    unsafe { &*(appkit.ns_view.as_ptr() as *const objc2_app_kit::NSView) };
                if let Some(ns_window) = ns_view.window() {
                    ns_window.setMovableByWindowBackground(true);
                    info!("Tracking panel: movableByWindowBackground enabled");
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_tracking_panel_drag(_app: &mut App) {}
