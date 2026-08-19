//! `local_suggestions` table persistence — legacy schema (predates the unified
//! `suggestions` table, V8+). The table is intentionally KEPT, not dead: it has
//! three live readers —
//! [`FewShotStorage`](crate::sqlite::few_shot_storage_impl) (few-shot prompt
//! construction from stored feedback), `LocalSuggestionQueryPort`
//! (`integration_query_impl.rs`, consumed by
//! `LocalSuggestionIntegrationSource` in `src-tauri`), and
//! `WebStorage::list_recent_local_suggestions` (dashboard REST feed via
//! `maekon-web`'s `focus_service`). Only the rule-based `LocalSuggestion` enum
//! writer path (`save_local_suggestion` / `serialize_suggestion`) was dead —
//! deleted 2026-07 (#7733). New RuleBased/LlmLocal suggestions flow through the
//! unified `suggestions` table (`save_rule_suggestion_sync`) instead; rows
//! already persisted here remain readable through the functions below.

use crate::error::StorageError;

use super::super::super::LocalSuggestionRecord;
use super::super::super::SqliteStorage;
use super::row_mapper::map_local_suggestion_row;

impl SqliteStorage {
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
}
