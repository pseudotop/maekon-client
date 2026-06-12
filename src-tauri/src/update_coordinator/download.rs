use crate::updater::UpdateError;
use maekon_api_contracts::update::DownloadProgress;
use maekon_web::update_control::{UpdatePhase, UpdateStatus};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, watch, RwLock};
use tracing::info;

use super::executor::UpdateExecutor;
use super::status::broadcast_status;

/// Phase 1: Download the update with streaming progress.
pub(super) async fn run_download<E: UpdateExecutor>(
    updater: &E,
    state: &Arc<RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    downloaded_path: &mut Option<PathBuf>,
) -> Result<(), UpdateError> {
    let pending = {
        let guard = state.read().await;
        guard.pending.clone()
    };

    let Some(pending) = pending else {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Idle;
        guard.message = Some("No pending update to download".to_string());
        guard.download_progress = None;
        guard.touch();
        broadcast_status(status_tx, &guard);
        return Ok(());
    };

    // Transition to Downloading
    {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Downloading;
        guard.message = Some(format!("Downloading version {}", pending.latest_version));
        guard.download_progress = Some(DownloadProgress {
            bytes_downloaded: 0,
            total_bytes: 0,
            percent: 0.0,
        });
        guard.touch();
        broadcast_status(status_tx, &guard);
    }

    // Create progress watch channel and spawn the download
    let (progress_tx, mut progress_rx) = watch::channel(DownloadProgress {
        bytes_downloaded: 0,
        total_bytes: 0,
        percent: 0.0,
    });

    let download_url = pending.download_url.clone();
    let download_future = updater.download_update_with_progress(&download_url, progress_tx);
    tokio::pin!(download_future);

    // Forward progress to broadcast subscribers via periodic polling
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(250));
    tick.tick().await; // consume immediate first tick

    let result = loop {
        tokio::select! {
            biased;
            result = &mut download_future => {
                // Download finished — push final progress snapshot
                let final_progress = progress_rx.borrow().clone();
                {
                    let mut guard = state.write().await;
                    guard.download_progress = Some(final_progress);
                    guard.touch();
                    broadcast_status(status_tx, &guard);
                }
                break result;
            }
            _ = tick.tick() => {
                if progress_rx.has_changed().unwrap_or(false) {
                    progress_rx.mark_changed();
                    let snap = progress_rx.borrow_and_update().clone();
                    let mut guard = state.write().await;
                    guard.download_progress = Some(snap);
                    guard.touch();
                    broadcast_status(status_tx, &guard);
                }
            }
        }
    };

    match result {
        Ok(path) => {
            info!("Download completed: {:?}", path);
            *downloaded_path = Some(path);
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::ReadyToInstall;
            guard.message = Some(format!(
                "Download complete. Ready to install version {}",
                pending.latest_version
            ));
            guard.download_progress = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
            Ok(())
        }
        Err(e) => {
            *downloaded_path = None;
            Err(e)
        }
    }
}
