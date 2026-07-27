//! Port for persisting the current Pomodoro session.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::pomodoro::PomodoroSession;

/// Durable store for the single current/most-recent Pomodoro session.
#[async_trait]
pub trait PomodoroStorePort: Send + Sync {
    /// Load the current/most-recent session, if one has been created.
    async fn load_pomodoro_session(&self) -> Result<Option<PomodoroSession>, CoreError>;

    /// Atomically replace the durable current session.
    async fn save_pomodoro_session(&self, session: &PomodoroSession) -> Result<(), CoreError>;
}
