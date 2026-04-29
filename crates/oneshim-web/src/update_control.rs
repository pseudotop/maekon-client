pub use oneshim_api_contracts::update::{
    DownloadProgress, PendingUpdateInfo, RollbackInfo, RollbackReason, UpdateAction, UpdatePhase,
    UpdateStatus,
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_increments_revision() {
        let mut status = UpdateStatus::default();
        let initial_revision = status.revision;
        status.touch();
        assert_eq!(status.revision, initial_revision + 1);
    }

    #[test]
    fn default_status_has_timestamp() {
        let status = UpdateStatus::default();
        assert!(!status.updated_at.is_empty());
    }

    #[tokio::test]
    async fn set_rolled_back_transitions_phase_and_broadcasts() {
        let (action_tx, _action_rx) = mpsc::unbounded_channel();
        let control = UpdateControl::new(action_tx, UpdateStatus::default());
        let mut subscriber = control.subscribe();

        let info = RollbackInfo {
            from_version: "0.5.0-rc.1".to_string(),
            from_published_at: Some("2026-05-01T00:00:00Z".to_string()),
            to_version: "0.4.40".to_string(),
            to_published_at: Some("2026-04-20T00:00:00Z".to_string()),
            reason: RollbackReason::RepeatedStartupFailure,
            rolled_back_at: chrono::Utc::now().to_rfc3339(),
        };

        let snapshot = control.set_rolled_back(info.clone()).await;
        assert_eq!(snapshot.phase, UpdatePhase::RolledBack);
        assert_eq!(snapshot.rollback.as_ref().unwrap(), &info);
        assert!(snapshot.message.is_some());
        assert!(snapshot.revision > 0);

        let broadcast = subscriber.recv().await.expect("broadcast delivered");
        assert_eq!(broadcast.phase, UpdatePhase::RolledBack);
        assert_eq!(
            broadcast.rollback.as_ref().unwrap().from_version,
            "0.5.0-rc.1"
        );
    }
}

#[derive(Clone)]
pub struct UpdateControl {
    pub state: Arc<RwLock<UpdateStatus>>,
    pub action_tx: mpsc::UnboundedSender<UpdateAction>,
    pub event_tx: broadcast::Sender<UpdateStatus>,
}

impl UpdateControl {
    pub fn new(action_tx: mpsc::UnboundedSender<UpdateAction>, initial: UpdateStatus) -> Self {
        let (event_tx, _) = broadcast::channel(64);
        Self {
            state: Arc::new(RwLock::new(initial)),
            action_tx,
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UpdateStatus> {
        self.event_tx.subscribe()
    }

    /// Phase 4 D11: transition the status to `RolledBack` with the provided
    /// RollbackInfo. Acquires the state lock, replaces phase + rollback
    /// fields, touches the revision, and broadcasts the new status.
    ///
    /// Returns the updated `UpdateStatus`. Callers that need to also inspect
    /// what changed can use the return value directly; subscribers receive
    /// it via the broadcast channel.
    pub async fn set_rolled_back(&self, info: RollbackInfo) -> UpdateStatus {
        let mut guard = self.state.write().await;
        guard.phase = UpdatePhase::RolledBack;
        guard.rollback = Some(info);
        guard.message = Some("Previous install reverted after repeated startup failures".into());
        guard.touch();
        let snapshot = guard.clone();
        drop(guard);
        let _ = self.event_tx.send(snapshot.clone());
        snapshot
    }
}
