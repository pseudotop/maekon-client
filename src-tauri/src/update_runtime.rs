use oneshim_core::config::UpdateConfig;
use oneshim_web::update_control::{PendingUpdateInfo, UpdateAction, UpdateControl, UpdatePhase};
use tokio::runtime::Handle;
use tracing::{debug, info};

use crate::update_coordinator;
use crate::updater::{UpdateCheckResult, Updater};

pub(crate) struct UpdateRuntimeBundle {
    pub(crate) update_control: UpdateControl,
    pub(crate) update_action_tx: tokio::sync::mpsc::UnboundedSender<UpdateAction>,
}

pub(crate) struct UpdateRuntimeBuilder<'a> {
    config: &'a UpdateConfig,
    runtime_handle: &'a Handle,
}

impl<'a> UpdateRuntimeBuilder<'a> {
    pub(crate) fn new(config: &'a UpdateConfig, runtime_handle: &'a Handle) -> Self {
        Self {
            config,
            runtime_handle,
        }
    }

    pub(crate) fn build_and_spawn(&self) -> UpdateRuntimeBundle {
        let runtime_auto_update = self.config.auto_install;
        let (update_action_tx, update_action_rx) =
            tokio::sync::mpsc::unbounded_channel::<UpdateAction>();
        let update_control = UpdateControl::new(
            update_action_tx.clone(),
            update_coordinator::initial_status(self.config, runtime_auto_update),
        );

        if self.config.enabled {
            // Fire-and-forget startup update check (non-blocking, 3s timeout).
            // Publishes to broadcast channel so the Tauri event bridge can
            // forward the result to the frontend immediately.
            let startup_config = self.config.clone();
            let startup_event_tx = update_control.event_tx.clone();
            let startup_state = update_control.state.clone();
            self.runtime_handle.spawn(async move {
                let updater = Updater::new(startup_config);
                match tokio::time::timeout(
                    std::time::Duration::from_secs(3),
                    updater.check_for_updates(),
                )
                .await
                {
                    Ok(Ok(UpdateCheckResult::Available {
                        current,
                        latest,
                        release,
                        download_url,
                        download_size,
                        ..
                    })) => {
                        info!("startup update check: v{latest} available");
                        // Write to shared state and publish to broadcast.
                        // send() is called while the write guard is held,
                        // matching the coordinator's run_check() pattern.
                        let mut guard = startup_state.write().await;
                        guard.phase = UpdatePhase::PendingApproval;
                        guard.message =
                            Some(format!("New version detected: {} -> {}", current, latest));
                        guard.pending = Some(PendingUpdateInfo {
                            current_version: current.to_string(),
                            latest_version: latest.to_string(),
                            release_url: release.html_url.clone(),
                            release_name: release.name.clone(),
                            published_at: release.published_at.clone(),
                            download_url,
                            release_notes: release.body.clone(),
                            download_size_bytes: download_size,
                        });
                        guard.touch();
                        if let Err(e) = startup_event_tx.send(guard.clone()) {
                            debug!("channel send failed: {e}");
                        }
                    }
                    Ok(Ok(UpdateCheckResult::UpToDate { .. })) => {
                        debug!("startup update check: up to date");
                    }
                    Ok(Err(e)) => {
                        debug!("startup update check failed: {e}");
                    }
                    Err(_) => {
                        debug!("startup update check: timed out");
                    }
                }
            });

            let update_config = self.config.clone();
            let update_state = update_control.state.clone();
            let update_status_tx = Some(update_control.event_tx.clone());
            self.runtime_handle.spawn(async move {
                update_coordinator::run_update_coordinator(
                    update_config,
                    update_state,
                    update_action_rx,
                    update_status_tx,
                    runtime_auto_update,
                )
                .await;
            });
        }

        UpdateRuntimeBundle {
            update_control,
            update_action_tx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the broadcast contract: when the startup check detects an
    /// Available update, it writes PendingApproval to shared state and
    /// publishes to the broadcast channel.
    ///
    /// Tests the state-write + broadcast pattern in isolation. The actual
    /// startup check integration (with real Updater + tokio runtime handle)
    /// is verified manually; coordinator tests cover the equivalent
    /// run_check() logic.
    #[tokio::test]
    async fn startup_check_publishes_to_broadcast_on_available() {
        let config = oneshim_core::config::UpdateConfig {
            enabled: true,
            auto_install: false,
            check_interval_hours: 24,
            ..Default::default()
        };

        let (action_tx, _action_rx) = tokio::sync::mpsc::unbounded_channel::<UpdateAction>();
        let control = UpdateControl::new(
            action_tx,
            crate::update_coordinator::initial_status(&config, false),
        );

        // Subscribe BEFORE spawning so we catch the event
        let mut rx = control.subscribe();
        let event_tx = control.event_tx.clone();
        let state = control.state.clone();

        // Simulate the startup check's Available path
        tokio::spawn(async move {
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::PendingApproval;
            guard.message = Some("New version detected: 0.1.0 -> 0.2.0".to_string());
            guard.pending = Some(PendingUpdateInfo {
                current_version: "0.1.0".to_string(),
                latest_version: "0.2.0".to_string(),
                release_url: "https://example.com".to_string(),
                release_name: Some("v0.2.0".to_string()),
                published_at: None,
                download_url: "https://example.com/download".to_string(),
                release_notes: None,
                download_size_bytes: None,
            });
            guard.touch();
            let _ = event_tx.send(guard.clone());
        });

        let status = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for broadcast")
            .expect("broadcast recv error");

        assert_eq!(status.phase, UpdatePhase::PendingApproval);
        assert!(status.pending.is_some());
        let pending = status.pending.unwrap();
        assert_eq!(pending.current_version, "0.1.0");
        assert_eq!(pending.latest_version, "0.2.0");
    }
}
