use tauri::App;
#[cfg(target_os = "macos")]
use tauri::Manager;

pub(crate) fn apply(app: &mut App) {
    // #11647: do not create the former display-sized transparent NSWindow on
    // macOS. ScreenCaptureKit enumerates that auxiliary window as ordinary
    // shareable content, and selecting it yields a black full-screen frame.
    // Passive capture state remains visible through the tray, the bounded
    // tracking panel, and the main-window status surface.
    configure_tracking_panel_drag(app);
}

#[cfg(target_os = "macos")]
// This raw AppKit handle bridge is verified by the physical macOS drag check.
// Its observable contract is the movable tracking panel, not a synthetic
// replacement of an opaque Tauri/AppKit call with `()`.
#[mutants::skip]
fn configure_tracking_panel_drag(app: &mut App) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    // Workaround for -webkit-app-region not working on transparent borderless windows with WKWebView.
    if let Some(panel) = app.app_handle().get_webview_window("tracking-panel") {
        if let Ok(handle) = panel.window_handle() {
            if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
                // SAFETY: `appkit.ns_view` is a non-null `NSView*` published by
                // raw-window-handle for the live "tracking-panel" webview
                // window; it stays valid while that window exists, which it does
                // for this synchronous main-thread call. We reborrow it as
                // `&NSView` only to read `window()` / set a property and never
                // store the reference past this block.
                let ns_view =
                    unsafe { &*(appkit.ns_view.as_ptr() as *const objc2_app_kit::NSView) };
                if let Some(ns_window) = ns_view.window() {
                    ns_window.setMovableByWindowBackground(true);
                    tracing::info!("Tracking panel: movableByWindowBackground enabled");
                }
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn configure_tracking_panel_drag(_app: &mut App) {}
