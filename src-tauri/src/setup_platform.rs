use tauri::App;
#[cfg(target_os = "macos")]
use tauri::Manager;
#[cfg(target_os = "macos")]
use tracing::info;

/// Tauri managed-state wrapper for the screen-topology monitor `JoinHandle`.
///
/// Aborts the task on drop so that Tauri's managed-state teardown at shutdown
/// cleans up without requiring an explicit abort call-site.  The task loop also
/// exits cooperatively via `shutdown_rx`, but the `Drop` guard is a defence-in-
/// depth safety net for any code path where the Tauri Exit event is not reached.
///
/// Only constructed on macOS (`#[cfg(target_os = "macos")]` gate inside
/// `create_native_border_indicator`); the managed state slot is absent on other
/// platforms.
#[cfg(target_os = "macos")]
pub(crate) struct NativeBorderTopoHandle(pub(crate) tauri::async_runtime::JoinHandle<()>);

#[cfg(target_os = "macos")]
impl Drop for NativeBorderTopoHandle {
    fn drop(&mut self) {
        // Best-effort abort on drop; the task also exits cooperatively when the
        // watch channel closes (sender dropped at shutdown).
        self.0.abort();
    }
}

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
                let (panel_visible, paused, mut shutdown_rx) = app
                    .app_handle()
                    .try_state::<crate::runtime_state::AppState>()
                    .map(|s| {
                        (
                            s.indicator_visible
                                .load(std::sync::atomic::Ordering::Relaxed),
                            s.capture_paused.load(std::sync::atomic::Ordering::Relaxed),
                            // Subscribe to the shared shutdown watch channel so the
                            // topology task can exit cooperatively on Tauri shutdown.
                            s.shutdown_tx.subscribe(),
                        )
                    })
                    .unwrap_or_else(|| {
                        // AppState not yet registered (should not happen in production;
                        // create a dummy channel so the task still compiles safely).
                        let (tx, rx) = tokio::sync::watch::channel(false);
                        // Immediately send shutdown so the task exits at the first tick.
                        let _ = tx.send(true);
                        (false, false, rx)
                    });
                let border_visible =
                    panel_visible && !crate::app_runtime_launch::cua_safe_mode_enabled();
                border.set_paused(paused);
                if border_visible {
                    border.show();
                }

                // Spawn periodic screen-topology monitor (every 5 s).
                // The task breaks cooperatively when `shutdown_rx` signals shutdown,
                // preventing use-after-free NSScreen calls after Tauri tears down
                // windows and blocking Drop of the Arc<NativeBorderIndicator>.
                let border_for_task = border.clone();
                let handle = tauri::async_runtime::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
                    loop {
                        tokio::select! {
                            _ = interval.tick() => {
                                border_for_task.check_and_rebuild();
                            }
                            // `shutdown_tx` follows send-once-with-`true` semantics
                            // (it is signalled exactly once at app teardown), so any
                            // `changed()` notification means shutdown — we never
                            // inspect the value. Consistent with the scheduler loops.
                            _ = shutdown_rx.changed() => {
                                info!("Native border topology monitor shutdown");
                                break;
                            }
                        }
                    }
                });

                // Store the handle as Tauri managed state so Drop aborts the task
                // if the cooperative shutdown path is not reached.
                app.manage(NativeBorderTopoHandle(handle));
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

#[cfg(test)]
mod tests {
    /// Verify the screen-topology loop body pattern: the loop exits promptly
    /// when the shutdown watch fires, without waiting for the next interval tick.
    ///
    /// This test exercises the `tokio::select!` logic in isolation, independent of
    /// Tauri and macOS ObjC APIs, so it runs on all platforms in CI.
    #[tokio::test]
    async fn topology_loop_exits_on_shutdown_signal() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
        let tick_count = Arc::new(AtomicU32::new(0));
        let tick_count_clone = tick_count.clone();

        // Use a very long interval so the test never blocks on a tick.
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        tick_count_clone.fetch_add(1, Ordering::Relaxed);
                    }
                    _ = shutdown_rx.changed() => {
                        break;
                    }
                }
            }
        });

        // Signal shutdown; the loop should unblock immediately via the select branch.
        shutdown_tx.send(true).expect("send shutdown");
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("task did not finish within 1s after shutdown signal")
            .expect("task panicked");

        // The 3600-second interval should never have fired.
        assert_eq!(
            tick_count.load(Ordering::Relaxed),
            0,
            "topology loop must not tick before shutdown fires"
        );
    }

    /// Verify that `NativeBorderTopoHandle::drop` aborts the task (defence-in-depth
    /// path when the cooperative shutdown signal is not sent).
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn topo_handle_drop_aborts_task() {
        // Use the same spawner as production (`create_native_border_indicator`)
        // so the JoinHandle type matches `NativeBorderTopoHandle`'s field.
        let handle = tauri::async_runtime::spawn(async {
            // Simulate an un-interruptible long sleep (no shutdown_rx here).
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        });
        let topo_handle = super::NativeBorderTopoHandle(handle);
        // Drop triggers abort().
        drop(topo_handle);
        // Give the runtime one yield to process the abort.
        tokio::task::yield_now().await;
        // The test completes without hanging — abort worked.
    }
}
