//! Pending feedback retry persistence: save, list, delete, cleanup.

use super::super::super::SqliteStorage;
use crate::error::StorageError;

impl SqliteStorage {
    /// Persist a pending feedback for retry after app restart.
    pub fn save_pending_feedback(
        &self,
        record: &maekon_core::models::storage_records::PendingFeedbackRecord,
    ) -> Result<(), StorageError> {
        // Write — write_lock (skipped when deletion_flag is set, feedback_retries ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
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
        })
    }

    /// List pending feedbacks for retry queue restoration on startup.
    pub fn list_pending_feedbacks(
        &self,
        limit: usize,
    ) -> Result<Vec<maekon_core::models::storage_records::PendingFeedbackRecord>, StorageError>
    {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        let conn = read.conn();
        let mut stmt = conn
            .prepare(
                "SELECT id, suggestion_id, feedback_type, comment, attempts, next_retry_at, created_at \
                 FROM feedback_retries ORDER BY created_at ASC LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("prepare failure: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok(
                    maekon_core::models::storage_records::PendingFeedbackRecord {
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
        let cutoff = format!("-{max_age_days} days");
        // Write — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run(0usize, |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM feedback_retries WHERE created_at < datetime('now', ?1)",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to cleanup old feedback retries: {e}"))
                })?;
            Ok(deleted)
        })
    }

    /// Delete a pending feedback after successful retry or exhaustion.
    pub fn delete_pending_feedback(&self, suggestion_id: &str) -> Result<(), StorageError> {
        // Write — write_lock (skipped when deletion_flag is set).
        self.conn.write_lock().run((), |conn| {
            conn.execute(
                "DELETE FROM feedback_retries WHERE suggestion_id = ?1",
                rusqlite::params![suggestion_id],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to delete pending feedback: {e}"))
            })?;
            Ok(())
        })
    }
}
