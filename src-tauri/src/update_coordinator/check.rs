use crate::updater::{UpdateCheckResult, UpdateError};
use maekon_web::update_control::{PendingUpdateInfo, UpdatePhase, UpdateStatus};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use tracing::{info, warn};

use super::executor::UpdateExecutor;
use super::status::broadcast_status;

pub(super) async fn run_check<E: UpdateExecutor>(
    updater: &E,
    state: &Arc<RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    auto_install: bool,
    downloaded_path: &mut Option<PathBuf>,
) {
    {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Checking;
        guard.message = Some("Checking for new version".to_string());
        guard.pending = None;
        guard.download_progress = None;
        guard.touch();
        broadcast_status(status_tx, &guard);
    }

    let result = updater.check_for_updates().await;
    match result {
        Ok(UpdateCheckResult::UpToDate { current }) => {
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Idle;
            guard.message = Some(format!("Already on latest version: {}", current));
            guard.pending = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
        }
        Ok(UpdateCheckResult::Available {
            current,
            latest,
            release,
            download_url,
            download_size,
            ..
        }) => {
            {
                let mut guard = state.write().await;
                guard.phase = UpdatePhase::PendingApproval;
                guard.message = Some(format!("New version detected: {} -> {}", current, latest));
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
                broadcast_status(status_tx, &guard);
            }

            if auto_install {
                info!("Auto-update mode enabled: downloading and installing");
                // Phase 1: Download
                if let Err(e) =
                    super::download::run_download(updater, state, status_tx, downloaded_path).await
                {
                    super::status::emit_error(
                        state,
                        status_tx,
                        &format!("Auto-download failed: {e}"),
                    )
                    .await;
                    return;
                }
                // Phase 2: Install
                if let Err(e) =
                    super::install::run_install(updater, state, status_tx, downloaded_path).await
                {
                    super::status::emit_error(
                        state,
                        status_tx,
                        &format!("Auto-install failed: {e}"),
                    )
                    .await;
                }
            }
        }
        Err(UpdateError::Disabled) => {
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Idle;
            guard.message = Some("Update feature is disabled".to_string());
            guard.pending = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
        }
        Err(e) => {
            warn!("Failed to check for updates: {}", e);
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Error;
            guard.message = Some(format!("Failed to check for updates: {}", e));
            guard.pending = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
        }
    }

    if let Err(e) = updater.save_last_check_time() {
        warn!("Failed to persist last update check timestamp: {}", e);
    }
}
