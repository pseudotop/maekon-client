use crate::updater::UpdateError;
use maekon_web::update_control::{UpdatePhase, UpdateStatus};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::runtime::{Handle, RuntimeFlavor};
use tokio::sync::{broadcast, RwLock};
use tracing::error;

use super::executor::UpdateExecutor;
use super::status::broadcast_status;

/// Phase 2: Install a previously downloaded update.
pub(super) async fn run_install<E: UpdateExecutor>(
    updater: &E,
    state: &Arc<RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    downloaded_path: &mut Option<PathBuf>,
) -> Result<(), UpdateError> {
    let path = match downloaded_path.take() {
        Some(p) => p,
        None => {
            return Err(UpdateError::Install(
                "No downloaded file available for installation".to_string(),
            ));
        }
    };

    let version_label = {
        let guard = state.read().await;
        guard
            .pending
            .as_ref()
            .map(|p| p.latest_version.clone())
            .unwrap_or_else(|| "unknown".to_string())
    };

    // Transition to Installing
    {
        let mut guard = state.write().await;
        guard.phase = UpdatePhase::Installing;
        guard.message = Some(format!("Installing version {}", version_label));
        guard.download_progress = None;
        guard.touch();
        broadcast_status(status_tx, &guard);
    }

    // install_and_restart performs blocking FS I/O (extract_tar_gz / extract_zip /
    // std::fs::copy). On the app's multi-thread runtime, block_in_place avoids
    // stalling an async worker without requiring a 'static executor bound. The
    // current-thread runtime used by focused tests cannot call block_in_place, so
    // it falls back to a direct call instead of panicking.
    let install_result = if Handle::current().runtime_flavor() == RuntimeFlavor::MultiThread {
        tokio::task::block_in_place(|| updater.install_and_restart(&path))
    } else {
        updater.install_and_restart(&path)
    };

    match install_result {
        Ok(()) => {
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Updated;
            guard.message = Some(format!("Updated to version {}", version_label));
            guard.pending = None;
            guard.download_progress = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
            Ok(())
        }
        Err(e) => {
            error!("Update installation failed: {}", e);
            let mut guard = state.write().await;
            guard.phase = UpdatePhase::Error;
            guard.message = Some(format!("Update installation failed: {}", e));
            guard.download_progress = None;
            guard.touch();
            broadcast_status(status_tx, &guard);
            Err(e)
        }
    }
}
