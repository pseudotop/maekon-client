use maekon_core::config::UpdateConfig;
use maekon_web::update_control::{UpdatePhase, UpdateStatus};
use tokio::sync::broadcast;
use tracing::debug;

use chrono::Utc;

pub fn initial_status(config: &UpdateConfig, auto_install: bool) -> UpdateStatus {
    UpdateStatus {
        enabled: config.enabled,
        auto_install,
        phase: UpdatePhase::Idle,
        message: None,
        pending: None,
        download_progress: None,
        rollback: None,
        revision: 0,
        updated_at: Utc::now().to_rfc3339(),
    }
}

/// Send an UpdateStatus snapshot to broadcast subscribers (if any).
pub(super) fn broadcast_status(
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    status: &UpdateStatus,
) {
    if let Some(tx) = status_tx {
        if let Err(e) = tx.send(status.clone()) {
            debug!("channel send failed: {e}");
        }
    }
}

/// Set the coordinator state to Error with a message.
pub(super) async fn emit_error(
    state: &std::sync::Arc<tokio::sync::RwLock<UpdateStatus>>,
    status_tx: Option<&broadcast::Sender<UpdateStatus>>,
    message: &str,
) {
    let mut guard = state.write().await;
    guard.phase = UpdatePhase::Error;
    guard.message = Some(message.to_string());
    guard.download_progress = None;
    guard.touch();
    broadcast_status(status_tx, &guard);
}
