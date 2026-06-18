use maekon_web::update_control::UpdateControl;
use maekon_web::RealtimeEvent;
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
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        if let Err(error) =
                            app_handle_for_events.emit_to("main", "realtime-event", &event)
                        {
                            tracing::debug!("emit error (window may be hidden): {error}");
                        }
                    }
                    // `Lagged` is recoverable: the receiver fell behind and dropped `n`
                    // events. Keep the bridge alive so the frontend continues to receive
                    // subsequent realtime events for the session.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("realtime event bridge lagged by {n} events, continuing");
                        continue;
                    }
                    // `Closed` is terminal: the sender was dropped, so the bridge exits.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
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
            loop {
                match rx.recv().await {
                    Ok(status) => {
                        if let Err(e) = app.emit_to("main", "update:status-changed", &status) {
                            tracing::debug!("update event emit error (window may be hidden): {e}");
                        }
                        if status.phase
                            == maekon_api_contracts::update::UpdatePhase::PendingApproval
                        {
                            if let Some(ref pending) = status.pending {
                                let version = &pending.latest_version;
                                if last_notified_version.as_deref() != Some(version) {
                                    last_notified_version = Some(version.clone());
                                    let _ =
                                        tauri_plugin_notification::NotificationExt::notification(
                                            &app,
                                        )
                                        .builder()
                                        .title("Update Available")
                                        .body(format!("Version {} is ready to install", version))
                                        .show();
                                }
                            }
                        }
                    }
                    // `Lagged` is recoverable: the receiver fell behind and dropped `n`
                    // events. Keep the bridge alive so update status changes continue to
                    // reach the frontend for the session.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("update event bridge lagged by {n} events, continuing");
                        continue;
                    }
                    // `Closed` is terminal: the sender was dropped, so the bridge exits.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    //! Regression guard for #52/#53: both event bridges previously used
    //! `while let Ok(event) = rx.recv().await`, which treats the *recoverable*
    //! `RecvError::Lagged(n)` as terminal and silently kills the bridge for the
    //! rest of the session. The production loops now `continue` on `Lagged` and
    //! only `break` on `Closed`.
    //!
    //! The bridge spawners themselves require a Tauri `AppHandle` (not
    //! constructible in a unit test), so this test reproduces the exact
    //! `match rx.recv().await` arm structure over a real `broadcast` channel and
    //! asserts the recovery invariant: a lagged receiver keeps consuming, and a
    //! closed channel terminates the loop.

    use tokio::sync::broadcast;

    /// A lagged receiver must keep consuming subsequent events instead of dying,
    /// and a closed channel must terminate the consumer loop.
    #[tokio::test]
    async fn lagged_receiver_recovers_and_closed_terminates() {
        // Small capacity so a burst forces the receiver to fall behind and the
        // next `recv()` observes `RecvError::Lagged`.
        let (tx, mut rx) = broadcast::channel::<u32>(2);

        let consumer = tokio::spawn(async move {
            let mut received = Vec::new();
            let mut lagged_events = 0u64;
            loop {
                match rx.recv().await {
                    Ok(value) => received.push(value),
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // Recoverable: mirror the production `continue` arm.
                        lagged_events += n;
                        continue;
                    }
                    // Terminal: mirror the production `break` arm.
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            (received, lagged_events)
        });

        // Overflow the capacity-2 channel to force a lag, then send one more
        // value that a *surviving* consumer must still observe.
        for value in 0..5u32 {
            tx.send(value).expect("send before close");
        }
        tx.send(42).expect("send post-lag sentinel");

        // Dropping the sender closes the channel and ends the loop.
        drop(tx);

        let (received, lagged_events) = consumer.await.expect("consumer task joins");

        assert!(
            lagged_events > 0,
            "the burst should have forced at least one Lagged error"
        );
        assert!(
            received.contains(&42),
            "consumer must keep receiving after a Lagged error (bridge stayed alive); got {received:?}"
        );
    }
}
