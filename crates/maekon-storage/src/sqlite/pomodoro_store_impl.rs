//! SQLite adapter for the durable Pomodoro session port (#8218).

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::pomodoro::PomodoroSession;
use maekon_core::ports::pomodoro_store::PomodoroStorePort;
use rusqlite::OptionalExtension;

use super::SqliteStorage;
use crate::error::StorageError;

#[async_trait]
impl PomodoroStorePort for SqliteStorage {
    async fn load_pomodoro_session(&self) -> Result<Option<PomodoroSession>, CoreError> {
        self.with_conn_read(|conn| {
            let json = conn
                .query_row(
                    "SELECT session_json FROM pomodoro_state WHERE slot = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| StorageError::Internal(format!("pomodoro load failed: {e}")))?;
            json.map(|value| {
                serde_json::from_str(&value).map_err(|e| {
                    StorageError::Internal(format!("pomodoro state decode failed: {e}"))
                })
            })
            .transpose()
        })
        .await
        .map_err(Into::into)
    }

    async fn save_pomodoro_session(&self, session: &PomodoroSession) -> Result<(), CoreError> {
        let session = session.clone();
        let json = serde_json::to_string(&session)
            .map_err(|e| StorageError::Internal(format!("pomodoro state encode failed: {e}")))?;
        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO pomodoro_state
                 (slot, session_json, started_at, completed_at, updated_at)
                 VALUES (1, ?1, ?2, ?3, datetime('now'))
                 ON CONFLICT(slot) DO UPDATE SET
                   session_json = excluded.session_json,
                   started_at = excluded.started_at,
                   completed_at = excluded.completed_at,
                   updated_at = excluded.updated_at",
                rusqlite::params![
                    json,
                    session.started_at.to_rfc3339(),
                    session.completed_at.map(|value| value.to_rfc3339()),
                ],
            )
            .map_err(|e| StorageError::Internal(format!("pomodoro save failed: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::pomodoro::PomodoroStatus;

    #[tokio::test]
    async fn session_roundtrips_and_terminal_transition_overwrites() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        assert!(storage.load_pomodoro_session().await.unwrap().is_none());

        let mut session = PomodoroSession::new("pomo-test".to_string(), 25, 5);
        storage.save_pomodoro_session(&session).await.unwrap();
        let restored = storage.load_pomodoro_session().await.unwrap().unwrap();
        assert_eq!(restored.id, "pomo-test");
        assert_eq!(restored.status, PomodoroStatus::Running);

        session.status = PomodoroStatus::Cancelled;
        session.completed_at = Some(chrono::Utc::now());
        storage.save_pomodoro_session(&session).await.unwrap();
        let restored = storage.load_pomodoro_session().await.unwrap().unwrap();
        assert_eq!(restored.status, PomodoroStatus::Cancelled);
        assert!(restored.completed_at.is_some());
    }

    #[tokio::test]
    async fn full_erasure_removes_pomodoro_state() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let session = PomodoroSession::new("erase-me".to_string(), 25, 5);
        storage.save_pomodoro_session(&session).await.unwrap();

        storage.delete_all_data().unwrap();

        assert!(storage.load_pomodoro_session().await.unwrap().is_none());
    }
}
