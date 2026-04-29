use crate::error::StorageError;
use oneshim_core::models::storage_records::SuggestionRecord;
use oneshim_core::models::suggestion::SuggestionSource;
#[allow(deprecated)]
use oneshim_core::models::work_session::LocalSuggestion;
use tracing::debug;

use super::super::{LocalSuggestionRecord, SqliteStorage};
use super::work_sessions::enum_to_sql_str;

/// Map a `local_suggestions` row to a `LocalSuggestionRecord`.
/// Shared by `list_recent_local_suggestions`, `list_local_suggestions_after_id`,
/// and `integration_query_impl`.
pub(crate) fn map_local_suggestion_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LocalSuggestionRecord> {
    let payload_str: String = row.get(2)?;
    let payload: serde_json::Value =
        serde_json::from_str(&payload_str).unwrap_or(serde_json::json!({}));

    Ok(LocalSuggestionRecord {
        id: row.get(0)?,
        suggestion_type: row.get(1)?,
        payload,
        created_at: row.get(3)?,
        shown_at: row.get(4)?,
        dismissed_at: row.get(5)?,
        acted_at: row.get(6)?,
    })
}

impl SqliteStorage {
    // --------------------------------------------------------
    // Unified suggestion persistence (sync version for FocusStorage trait)
    // --------------------------------------------------------

    /// Synchronously save a unified `Suggestion` to the V8 `suggestions` table.
    /// Returns the `suggestion_id` (UUID string).
    pub fn save_rule_suggestion_sync(
        &self,
        suggestion: &oneshim_core::models::suggestion::Suggestion,
    ) -> Result<String, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT OR REPLACE INTO suggestions \
             (suggestion_id, suggestion_type, source, content, priority, \
              confidence_score, relevance_score, is_actionable, reasoning, \
              created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                suggestion.suggestion_id,
                enum_to_sql_str(&suggestion.suggestion_type),
                enum_to_sql_str(&suggestion.source),
                suggestion.content,
                enum_to_sql_str(&suggestion.priority),
                suggestion.confidence_score,
                suggestion.relevance_score,
                suggestion.is_actionable as i32,
                suggestion.reasoning,
                suggestion.created_at.to_rfc3339(),
                suggestion.expires_at.map(|t| t.to_rfc3339()),
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save suggestion: {e}")))?;

        debug!(id = %suggestion.suggestion_id, "rule-based suggestion persisted to SQLite");
        Ok(suggestion.suggestion_id.clone())
    }

    /// Mark a unified suggestion as shown by its string suggestion_id.
    pub fn mark_unified_suggestion_shown(&self, suggestion_id: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE suggestions SET shown_at = datetime('now') WHERE suggestion_id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| StorageError::Internal(format!("suggestion shown record failure: {e}")))?;

        Ok(())
    }

    // --------------------------------------------------------
    // Legacy local_suggestions persistence (deprecated — kept for migration)
    // --------------------------------------------------------

    #[allow(deprecated)]
    pub fn save_local_suggestion(&self, suggestion: &LocalSuggestion) -> Result<i64, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let (suggestion_type, payload) = Self::serialize_suggestion(suggestion);

        conn.execute(
            "INSERT INTO local_suggestions (suggestion_type, payload) VALUES (?1, ?2)",
            rusqlite::params![suggestion_type, payload],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save local suggestion: {e}")))?;

        let id = conn.last_insert_rowid();
        debug!("suggestion save: id={}, type={}", id, suggestion_type);
        Ok(id)
    }

    pub fn mark_suggestion_shown(&self, suggestion_id: i64) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE local_suggestions SET shown_at = datetime('now') WHERE id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| StorageError::Internal(format!("suggestion display record failure: {e}")))?;

        Ok(())
    }

    pub fn mark_suggestion_dismissed(&self, suggestion_id: i64) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE local_suggestions SET acted_at = datetime('now') WHERE id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| StorageError::Internal(format!("suggestion execution record failure: {e}")))?;

        Ok(())
    }

    // --------------------------------------------------------
    // Unified V8 suggestions queries
    // --------------------------------------------------------

    /// List non-dismissed suggestions from the unified `suggestions` table,
    /// newest first, up to `limit` rows.
    pub fn list_suggestions(
        &self,
        limit: usize,
    ) -> Result<Vec<oneshim_core::models::storage_records::SuggestionRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, suggestion_id, suggestion_type, source, content, priority, \
                 confidence_score, relevance_score, is_actionable, reasoning, \
                 shown_at, dismissed_at, acted_at, created_at, expires_at \
                 FROM suggestions \
                 WHERE dismissed_at IS NULL \
                 ORDER BY created_at DESC \
                 LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("prepare failure: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok(oneshim_core::models::storage_records::SuggestionRecord {
                    id: row.get(0)?,
                    suggestion_id: row.get(1)?,
                    suggestion_type: row.get(2)?,
                    source: row.get(3)?,
                    content: row.get(4)?,
                    priority: row.get(5)?,
                    confidence_score: row.get(6)?,
                    relevance_score: row.get(7)?,
                    is_actionable: row.get::<_, i32>(8)? != 0,
                    reasoning: row.get(9)?,
                    shown_at: row.get(10)?,
                    dismissed_at: row.get(11)?,
                    acted_at: row.get(12)?,
                    created_at: row.get(13)?,
                    expires_at: row.get(14)?,
                    resurface_at: None,
                })
            })
            .map_err(|e| StorageError::Internal(format!("query failure: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    /// Dismiss a unified suggestion by its string `suggestion_id`.
    /// Returns `true` if a row was updated, `false` otherwise.
    pub fn dismiss_unified_suggestion(&self, suggestion_id: &str) -> Result<bool, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let changed = conn
            .execute(
                "UPDATE suggestions SET dismissed_at = datetime('now') WHERE suggestion_id = ?1 AND dismissed_at IS NULL",
                rusqlite::params![suggestion_id],
            )
            .map_err(|e| StorageError::Internal(format!("dismiss failure: {e}")))?;

        Ok(changed > 0)
    }

    pub fn list_recent_local_suggestions(
        &self,
        cutoff: &str,
        limit: usize,
    ) -> Result<Vec<LocalSuggestionRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

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

    /// Check whether LLM_SERVER suggestions exist within the given lookback
    /// window. Used by the analysis loop to suppress local analysis when the
    /// server is actively sending suggestions.
    pub fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let sql = "SELECT COUNT(*) FROM suggestions \
             WHERE source = ?1 \
             AND created_at > datetime('now', ?2)";
        let count: i64 = conn
            .query_row(
                sql,
                rusqlite::params![
                    SuggestionSource::LLM_SERVER_STR,
                    format!("-{lookback_secs} seconds")
                ],
                |row| row.get(0),
            )
            .map_err(|e| StorageError::Internal(format!("query failure: {e}")))?;

        Ok(count > 0)
    }

    // --------------------------------------------------------
    // Queue persistence (V23): save/list by state
    // --------------------------------------------------------

    /// Save suggestion with explicit state for queue persistence.
    pub fn save_suggestion_with_state(
        &self,
        suggestion: &oneshim_core::models::suggestion::Suggestion,
        state: &str,
        resurface_at: Option<&str>,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT OR REPLACE INTO suggestions \
             (suggestion_id, suggestion_type, source, content, priority, \
              confidence_score, relevance_score, is_actionable, reasoning, \
              created_at, expires_at, state, resurface_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                suggestion.suggestion_id,
                enum_to_sql_str(&suggestion.suggestion_type),
                suggestion.source.as_sql_str(),
                suggestion.content,
                enum_to_sql_str(&suggestion.priority),
                suggestion.confidence_score,
                suggestion.relevance_score,
                suggestion.is_actionable as i32,
                suggestion.reasoning,
                suggestion.created_at.to_rfc3339(),
                suggestion.expires_at.map(|d| d.to_rfc3339()),
                state,
                resurface_at,
            ],
        )
        .map_err(|e| {
            StorageError::Internal(format!("Failed to save suggestion with state: {e}"))
        })?;

        debug!(id = %suggestion.suggestion_id, state, "suggestion persisted with state");
        Ok(())
    }

    /// List suggestions by state for queue restoration.
    pub fn list_suggestions_by_state(
        &self,
        state: &str,
        limit: usize,
    ) -> Result<Vec<SuggestionRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, suggestion_id, suggestion_type, source, content, priority, \
                 confidence_score, relevance_score, is_actionable, reasoning, \
                 shown_at, dismissed_at, acted_at, created_at, expires_at, resurface_at \
                 FROM suggestions WHERE state = ?1 \
                 ORDER BY created_at DESC LIMIT ?2",
            )
            .map_err(|e| StorageError::Internal(format!("prepare failure: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![state, limit as i64], |row| {
                Ok(SuggestionRecord {
                    id: row.get(0)?,
                    suggestion_id: row.get(1)?,
                    suggestion_type: row.get(2)?,
                    source: row.get(3)?,
                    content: row.get(4)?,
                    priority: row.get(5)?,
                    confidence_score: row.get(6)?,
                    relevance_score: row.get(7)?,
                    is_actionable: row.get::<_, i32>(8)? != 0,
                    reasoning: row.get(9)?,
                    shown_at: row.get(10)?,
                    dismissed_at: row.get(11)?,
                    acted_at: row.get(12)?,
                    created_at: row.get(13)?,
                    expires_at: row.get(14)?,
                    resurface_at: row.get(15)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("query failure: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    /// Persist a pending feedback for retry after app restart.
    pub fn save_pending_feedback(
        &self,
        record: &oneshim_core::models::storage_records::PendingFeedbackRecord,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;
        conn.execute(
            "INSERT OR REPLACE INTO feedback_retries \
             (suggestion_id, feedback_type, comment, attempts, next_retry_at) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                record.suggestion_id,
                record.feedback_type,
                record.comment,
                record.attempts,
                record.next_retry_at,
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to save pending feedback: {e}")))?;
        Ok(())
    }

    /// List pending feedbacks for retry queue restoration on startup.
    pub fn list_pending_feedbacks(
        &self,
        limit: usize,
    ) -> Result<Vec<oneshim_core::models::storage_records::PendingFeedbackRecord>, StorageError>
    {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, suggestion_id, feedback_type, comment, attempts, next_retry_at, created_at \
                 FROM feedback_retries ORDER BY created_at ASC LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("prepare failure: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok(
                    oneshim_core::models::storage_records::PendingFeedbackRecord {
                        id: Some(row.get(0)?),
                        suggestion_id: row.get(1)?,
                        feedback_type: row.get(2)?,
                        comment: row.get(3)?,
                        attempts: row.get(4)?,
                        next_retry_at: row.get(5)?,
                        created_at: row.get(6)?,
                    },
                )
            })
            .map_err(|e| StorageError::Internal(format!("query failure: {e}")))?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(|e| StorageError::Internal(format!("row failure: {e}")))?);
        }
        Ok(records)
    }

    /// Delete orphaned feedback retries older than `max_age_days`.
    /// Returns the number of rows deleted.
    pub fn cleanup_old_feedback_retries(&self, max_age_days: u32) -> Result<usize, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;
        let cutoff = format!("-{max_age_days} days");
        let deleted = conn
            .execute(
                "DELETE FROM feedback_retries WHERE created_at < datetime('now', ?1)",
                rusqlite::params![cutoff],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to cleanup old feedback retries: {e}"))
            })?;
        Ok(deleted)
    }

    /// Delete a pending feedback after successful retry or exhaustion.
    pub fn delete_pending_feedback(&self, suggestion_id: &str) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;
        conn.execute(
            "DELETE FROM feedback_retries WHERE suggestion_id = ?1",
            rusqlite::params![suggestion_id],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to delete pending feedback: {e}")))?;
        Ok(())
    }

    /// Daily suggestion stats for the last N days, grouped by type and source.
    pub fn suggestion_daily_stats(
        &self,
        days: u32,
    ) -> Result<Vec<oneshim_core::models::storage_records::DailyStatRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;
        let cutoff = format!("-{days} days");
        let mut stmt = conn
            .prepare(
                "SELECT SUBSTR(created_at, 1, 10) as day, \
                 COUNT(*) as total, \
                 SUM(CASE WHEN acted_at IS NOT NULL THEN 1 ELSE 0 END) as acted, \
                 suggestion_type, source \
                 FROM suggestions \
                 WHERE created_at >= datetime('now', ?1) \
                 GROUP BY day, suggestion_type, source \
                 ORDER BY day DESC",
            )
            .map_err(|e| StorageError::Internal(format!("prepare failure: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![cutoff], |row| {
                Ok(oneshim_core::models::storage_records::DailyStatRecord {
                    day: row.get(0)?,
                    total: row.get(1)?,
                    acted: row.get(2)?,
                    suggestion_type: row.get(3)?,
                    source: row.get(4)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("query failure: {e}")))?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(|e| StorageError::Internal(format!("row failure: {e}")))?);
        }
        Ok(records)
    }

    #[allow(deprecated)]
    fn serialize_suggestion(suggestion: &LocalSuggestion) -> (String, String) {
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
