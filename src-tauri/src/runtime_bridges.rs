use oneshim_web::update_control::UpdateControl;
use oneshim_web::RealtimeEvent;
use tauri::{AppHandle, Emitter};
use tokio::runtime::Handle;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info};

pub(crate) struct RuntimeBridgeSpawner;

impl RuntimeBridgeSpawner {
    pub(crate) fn spawn_os_signal_bridge(handle: &Handle, shutdown_tx: &watch::Sender<bool>) {
        let signal_shutdown_tx = shutdown_tx.clone();
        handle.spawn(async move {
            let lifecycle = crate::lifecycle::LifecycleManager::default();
            lifecycle.wait_for_signal().await;
            info!("OS signal received — triggering shutdown");
            if let Err(e) = signal_shutdown_tx.send(true) {
                debug!("channel send failed: {e}");
            }
        });
    }

    pub(crate) fn spawn_realtime_event_bridge(
        handle: &Handle,
        app_handle: &AppHandle,
        event_tx: &broadcast::Sender<RealtimeEvent>,
    ) {
        let app_handle_for_events = app_handle.clone();
        let mut event_rx = event_tx.subscribe();
        handle.spawn(async move {
            while let Ok(event) = event_rx.recv().await {
                if let Err(error) = app_handle_for_events.emit_to("main", "realtime-event", &event)
                {
                    tracing::debug!("emit error (window may be hidden): {error}");
                }
            }
        });
    }

    /// Forward update status changes from broadcast channel to Tauri frontend.
    ///
    /// Uses `emit_to("main", ...)` to target only the main window, matching
    /// the pattern used by `spawn_realtime_event_bridge` and tray update events.
    pub(crate) fn spawn_update_event_bridge(
        handle: &Handle,
        app_handle: &AppHandle,
        update_control: &UpdateControl,
    ) {
        let app = app_handle.clone();
        let mut rx = update_control.subscribe();
        handle.spawn(async move {
            let mut last_notified_version: Option<String> = None;
            while let Ok(status) = rx.recv().await {
                if let Err(e) = app.emit_to("main", "update:status-changed", &status) {
                    tracing::debug!("update event emit error (window may be hidden): {e}");
                }
                if status.phase == oneshim_api_contracts::update::UpdatePhase::PendingApproval {
                    if let Some(ref pending) = status.pending {
                        let version = &pending.latest_version;
                        if last_notified_version.as_deref() != Some(version) {
                            last_notified_version = Some(version.clone());
                            let _ = tauri_plugin_notification::NotificationExt::notification(&app)
                                .builder()
                                .title("Update Available")
                                .body(format!("Version {} is ready to install", version))
                                .show();
                        }
                    }
                }
            }
        });
    }
}
