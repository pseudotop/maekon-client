//! Legacy `local_suggestions` table persistence (deprecated — kept for migration).

use tracing::debug;

use crate::error::StorageError;
#[allow(deprecated)]
use maekon_core::models::work_session::LocalSuggestion;

use super::super::super::LocalSuggestionRecord;
use super::super::super::SqliteStorage;
use super::row_mapper::map_local_suggestion_row;

impl SqliteStorage {
    #[allow(deprecated)]
    pub fn save_local_suggestion(&self, suggestion: &LocalSuggestion) -> Result<i64, StorageError> {
        let (suggestion_type, payload) = Self::serialize_suggestion(suggestion);

        // Write — write_lock (skip when deletion_flag is set → id 0; local_suggestions ∈ ALL_TABLES).
        self.conn.write_lock().run(0i64, |conn| {
            conn.execute(
                "INSERT INTO local_suggestions (suggestion_type, payload) VALUES (?1, ?2)",
                rusqlite::params![suggestion_type, payload],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save local suggestion: {e}")))?;

            let id = conn.last_insert_rowid();
            debug!("suggestion save: id={}, type={}", id, suggestion_type);
            Ok(id)
        })
    }

    pub fn mark_suggestion_shown(&self, suggestion_id: i64) -> Result<(), StorageError> {
        // Write — write_lock (skip when deletion_flag is set).
        self.conn.write_lock().run((), |conn| {
            Self::mark_suggestion_shown_inner(conn, suggestion_id)
        })
    }

    /// Async `mark_suggestion_shown` over the write funnel (ADR-026 PR-5).
    pub(crate) async fn mark_suggestion_shown_async(
        &self,
        suggestion_id: i64,
    ) -> Result<(), StorageError> {
        self.with_conn(move |conn| Self::mark_suggestion_shown_inner(conn, suggestion_id))
            .await
    }

    fn mark_suggestion_shown_inner(
        conn: &rusqlite::Connection,
        suggestion_id: i64,
    ) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE local_suggestions SET shown_at = datetime('now') WHERE id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| StorageError::Internal(format!("suggestion display record failure: {e}")))?;

        Ok(())
    }

    pub fn mark_suggestion_dismissed(&self, suggestion_id: i64) -> Result<(), StorageError> {
        // Write — write_lock (skip when deletion_flag is set).
        self.conn.write_lock().run((), |conn| {
            Self::mark_suggestion_dismissed_inner(conn, suggestion_id)
        })
    }

    /// Async `mark_suggestion_dismissed` over the write funnel (ADR-026 PR-5).
    pub(crate) async fn mark_suggestion_dismissed_async(
        &self,
        suggestion_id: i64,
    ) -> Result<(), StorageError> {
        self.with_conn(move |conn| Self::mark_suggestion_dismissed_inner(conn, suggestion_id))
            .await
    }

    fn mark_suggestion_dismissed_inner(
        conn: &rusqlite::Connection,
        suggestion_id: i64,
    ) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE local_suggestions SET dismissed_at = datetime('now') WHERE id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| {
            StorageError::Internal(format!("Failed to record suggestion dismissal: {e}"))
        })?;

        Ok(())
    }

    pub fn mark_suggestion_acted(&self, suggestion_id: i64) -> Result<(), StorageError> {
        // Write — write_lock (skip when deletion_flag is set).
        self.conn.write_lock().run((), |conn| {
            Self::mark_suggestion_acted_inner(conn, suggestion_id)
        })
    }

    /// Async `mark_suggestion_acted` over the write funnel (ADR-026 PR-5).
    pub(crate) async fn mark_suggestion_acted_async(
        &self,
        suggestion_id: i64,
    ) -> Result<(), StorageError> {
        self.with_conn(move |conn| Self::mark_suggestion_acted_inner(conn, suggestion_id))
            .await
    }

    fn mark_suggestion_acted_inner(
        conn: &rusqlite::Connection,
        suggestion_id: i64,
    ) -> Result<(), StorageError> {
        conn.execute(
            "UPDATE local_suggestions SET acted_at = datetime('now') WHERE id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| StorageError::Internal(format!("suggestion execution record failure: {e}")))?;

        Ok(())
    }

    pub fn list_recent_local_suggestions(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, StorageError> {
        // Read — read_lock (deletion_flag irrelevant).
        let read = self.conn.read_lock();
        Self::list_recent_local_suggestions_inner(read.conn(), cutoff, limit)
    }

    /// Async `list_recent_local_suggestions` over the read funnel (ADR-026 PR-5).
    pub(crate) async fn list_recent_local_suggestions_async(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, StorageError> {
        // owned move into the Send + 'static closure.
        let cutoff = cutoff.to_string();
        self.with_conn_read(move |conn| {
            Self::list_recent_local_suggestions_inner(conn, &cutoff, limit)
        })
        .await
    }

    fn list_recent_local_suggestions_inner(
        conn: &rusqlite::Connection,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT id, suggestion_type, payload, created_at, shown_at, dismissed_at, acted_at
                 FROM local_suggestions
                 WHERE created_at >= ?1
                 ORDER BY created_at DESC
                 LIMIT ?2",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(
                rusqlite::params![cutoff, limit as i64],
                map_local_suggestion_row,
            )
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    pub fn list_local_suggestions_after_id(
        &self,
        after_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, StorageError> {
        // Read — read_lock (deletion_flag irrelevant).
        let read = self.conn.read_lock();
        let conn = read.conn();

        let sql = if after_id.is_some() {
            "SELECT id, suggestion_type, payload, created_at, shown_at, dismissed_at, acted_at
             FROM local_suggestions
             WHERE id > ?1
             ORDER BY id ASC
             LIMIT ?2"
        } else {
            "SELECT id, suggestion_type, payload, created_at, shown_at, dismissed_at, acted_at
             FROM local_suggestions
             ORDER BY id ASC
             LIMIT ?1"
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = if let Some(after_id) = after_id {
            stmt.query_map(
                rusqlite::params![after_id, limit as i64],
                map_local_suggestion_row,
            )
        } else {
            stmt.query_map(rusqlite::params![limit as i64], map_local_suggestion_row)
        }
        .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    /// Private: serialize a `LocalSuggestion` to `(type_str, payload_json)`.
    #[allow(deprecated)]
    pub(super) fn serialize_suggestion(suggestion: &LocalSuggestion) -> (String, String) {
        match suggestion {
            LocalSuggestion::NeedFocusTime {
                communication_ratio,
                suggested_focus_mins,
            } => (
                "NeedFocusTime".to_string(),
                serde_json::json!({
                    "communication_ratio": communication_ratio,
                    "suggested_focus_mins": suggested_focus_mins,
                })
                .to_string(),
            ),
            LocalSuggestion::TakeBreak {
                continuous_work_mins,
            } => (
                "TakeBreak".to_string(),
                serde_json::json!({
                    "continuous_work_mins": continuous_work_mins,
                })
                .to_string(),
            ),
            LocalSuggestion::RestoreContext {
                interrupted_app,
                interrupted_at,
                snapshot_frame_id,
            } => (
                "RestoreContext".to_string(),
                serde_json::json!({
                    "interrupted_app": interrupted_app,
                    "interrupted_at": interrupted_at.to_rfc3339(),
                    "snapshot_frame_id": snapshot_frame_id,
                })
                .to_string(),
            ),
            LocalSuggestion::PatternDetected {
                pattern_description,
                confidence,
            } => (
                "PatternDetected".to_string(),
                serde_json::json!({
                    "pattern_description": pattern_description,
                    "confidence": confidence,
                })
                .to_string(),
            ),
            LocalSuggestion::ExcessiveCommunication {
                today_communication_mins,
                avg_communication_mins,
            } => (
                "ExcessiveCommunication".to_string(),
                serde_json::json!({
                    "today_communication_mins": today_communication_mins,
                    "avg_communication_mins": avg_communication_mins,
                })
                .to_string(),
            ),
        }
    }
}
