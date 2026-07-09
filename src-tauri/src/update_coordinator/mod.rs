mod check;
mod download;
mod executor;
mod install;
mod status;

#[cfg(test)]
mod tests;

// Public re-exports — preserves all call sites in update_runtime.rs unchanged.
pub use executor::UpdateExecutor;
pub use status::initial_status;

use crate::updater::Updater;
use maekon_core::config::UpdateConfig;
use maekon_web::update_control::{UpdateAction, UpdatePhase, UpdateStatus};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use tracing::{debug, info};

use status::broadcast_status;

pub async fn run_update_coordinator(
    config: UpdateConfig,
    state: Arc<RwLock<UpdateStatus>>,
    action_rx: mpsc::UnboundedReceiver<UpdateAction>,
    status_tx: Option<broadcast::Sender<UpdateStatus>>,
    auto_install: bool,
) {
    if !config.enabled {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Idle;
        guard.message = Some("Update feature is disabled".to_string());
        guard.pending = None;
        guard.touch();
        if let Some(tx) = &status_tx {
            if let Err(e) = tx.send(guard.clone()) {
                debug!("channel send failed: {e}");
            }
        }
        return;
    }

    let check_interval_hours = config.check_interval_hours;

    // D10 spawn-order guard (Phase 4): the update-check coordinator must not
    // start until app runtime launch has persisted installation_id. Current
    // launch order commits config + UUID before any update task spawns, but a
    // future regression would silently hide the device from rollout via D10's
    // defensive None handling in updater/mod.rs:check_for_updates_from.
    // Surface the invariant loudly in both debug and release builds.
    if config.installation_id.is_none() {
        // Loop 3 iter 1 fix (I-5): removed `debug_assert!(false, ...)` —
        // panicking dev builds on an invariant violation was a foot-gun for
        // test fixtures / Tauri dev runs and did not actually protect users
        // (release builds only get the tracing event).
        //
        // The single source of regression detection is the tracing::error!
        // below — captured as an OTel span event when the `telemetry`
        // feature is active. If a future counter API lands, add the
        // increment here (same namespace used symmetrically in Task 9's
        // rollback handler).
        tracing::error!(
            "update-check coordinator started with installation_id = None; \
             rollout gate will exclude this device until next launch"
        );
    }

    let updater = Updater::new(config);

    run_update_coordinator_with_executor(
        updater,
        state,
        action_rx,
        status_tx,
        auto_install,
        check_interval_hours,
    )
    .await;
}

pub async fn run_update_coordinator_with_executor<E: UpdateExecutor + 'static>(
    updater: E,
    state: Arc<RwLock<UpdateStatus>>,
    mut action_rx: mpsc::UnboundedReceiver<UpdateAction>,
    status_tx: Option<broadcast::Sender<UpdateStatus>>,
    auto_install: bool,
    check_interval_hours: u32,
) {
    // Track the downloaded file path between the two phases
    let mut downloaded_path: Option<PathBuf> = None;

    if let Some(tx) = &status_tx {
        let snapshot = state.read().await.clone();
        if let Err(e) = tx.send(snapshot) {
            debug!("channel send failed: {e}");
        }
    }

    // Initial check on startup
    if updater.should_check_for_updates() {
        check::run_check(
            &updater,
            &state,
            status_tx.as_ref(),
            auto_install,
            &mut downloaded_path,
        )
        .await;
    }

    // Periodic background re-check: use config's check_interval_hours,
    // clamped to minimum 1 hour to avoid API rate limits.
    let recheck_secs = (check_interval_hours.max(1) as u64) * 3600;
    let recheck_interval = std::time::Duration::from_secs(recheck_secs);
    let mut recheck_timer = tokio::time::interval(recheck_interval);
    recheck_timer.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            action = action_rx.recv() => {
                let Some(action) = action else { break };
                match action {
                    UpdateAction::CheckNow => {
                        check::run_check(
                            &updater,
                            &state,
                            status_tx.as_ref(),
                            auto_install,
                            &mut downloaded_path,
                        )
                        .await;
                    }
                    UpdateAction::Approve => {
                        let current_phase = state.read().await.phase.clone();
                        match current_phase {
                            UpdatePhase::PendingApproval => {
                                // Phase 1: start download
                                if let Err(e) = download::run_download(
                                    &updater,
                                    &state,
                                    status_tx.as_ref(),
                                    &mut downloaded_path,
                                )
                                .await
                                {
                                    status::emit_error(
                                        &state,
                                        status_tx.as_ref(),
                                        &format!("Download failed: {e}"),
                                    )
                                    .await;
                                } else if auto_install {
                                    // Auto-install: proceed to installation immediately
                                    if let Err(e) = install::run_install(
                                        &updater,
                                        &state,
                                        status_tx.as_ref(),
                                        &mut downloaded_path,
                                    )
                                    .await
                                    {
                                        status::emit_error(
                                            &state,
                                            status_tx.as_ref(),
                                            &format!("Auto-install failed: {e}"),
                                        )
                                        .await;
                                    }
                                }
                            }
                            UpdatePhase::ReadyToInstall => {
                                // Phase 2: install from downloaded file
                                if let Err(e) = install::run_install(
                                    &updater,
                                    &state,
                                    status_tx.as_ref(),
                                    &mut downloaded_path,
                                )
                                .await
                                {
                                    status::emit_error(
                                        &state,
                                        status_tx.as_ref(),
                                        &format!("Installation failed: {e}"),
                                    )
                                    .await;
                                }
                            }
                            _ => {
                                debug!("Approve action ignored in phase {:?}", current_phase);
                            }
                        }
                    }
                    UpdateAction::Defer => {
                        let current_phase = state.read().await.phase.clone();
                        match current_phase {
                            UpdatePhase::PendingApproval | UpdatePhase::ReadyToInstall => {
                                downloaded_path = None;
                                let mut guard = state.write().await;
                                guard.phase = UpdatePhase::Deferred;
                                guard.message = Some("Update was deferred".to_string());
                                guard.pending = None;
                                guard.download_progress = None;
                                guard.touch();
                                broadcast_status(status_tx.as_ref(), &guard);
                            }
                            _ => {
                                debug!("Defer action ignored in phase {:?}", current_phase);
                            }
                        }
                    }
                }
            }
            _ = recheck_timer.tick() => {
                // Skip re-check if an update is already pending, downloading, or installing
                let phase = state.read().await.phase.clone();
                if matches!(
                    phase,
                    UpdatePhase::Idle | UpdatePhase::Deferred | UpdatePhase::Error
                ) {
                    info!("periodic update re-check");
                    check::run_check(
                        &updater,
                        &state,
                        status_tx.as_ref(),
                        auto_install,
                        &mut downloaded_path,
                    )
                    .await;
                }
            }
        }
    }
}
