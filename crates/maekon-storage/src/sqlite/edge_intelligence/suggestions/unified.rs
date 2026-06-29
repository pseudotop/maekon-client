//! Unified V8 `suggestions` table: save, list, dismiss, mark-acted, shown.

use tracing::debug;

use crate::error::StorageError;
use crate::sqlite::hlc_clock::HlcClock;
use maekon_core::models::storage_records::SuggestionRecord;
use maekon_core::models::suggestion::SuggestionSource;

use super::super::super::SqliteStorage;
use super::super::work_sessions::enum_to_sql_str;
use super::context_columns::suggestion_context_columns;

/// D2 (#5186): stamp a suggestion INSERT with a monotonic HLC so it propagates via
/// cross-device sync — EXCEPT `LlmServer`-source rows, which carry server-synthesized
/// prose (content/reasoning reflecting device-local context) that must NOT cross-broadcast
/// between a user's own devices (privacy review; gated behind `cross_device_sync` consent
/// regardless). Returns `(hlc_wall_ms, hlc_counter, origin_device_id)` — zeros for a
/// skipped `LlmServer` row so it stays at the degenerate `(0,0,'')` (never propagates).
pub(crate) fn suggestion_stamp(
    clock: &HlcClock,
    conn: &rusqlite::Connection,
    source: &SuggestionSource,
) -> rusqlite::Result<(i64, i64, String)> {
    if matches!(source, SuggestionSource::LlmServer) {
        return Ok((0, 0, String::new()));
    }
    let h = clock.next(conn)?;
    Ok((h.wall_ms as i64, h.counter as i64, h.device_id))
}

impl SqliteStorage {
    // --------------------------------------------------------
    // Unified suggestion persistence (sync version for FocusStorage trait)
    // --------------------------------------------------------

    /// Synchronously save a unified `Suggestion` to the V8 `suggestions` table.
    /// Returns the `suggestion_id` (UUID string).
    pub fn save_rule_suggestion_sync(
        &self,
        suggestion: &maekon_core::models::suggestion::Suggestion,
    ) -> Result<String, StorageError> {
        let (context_app, context_window, context_target_id) =
            suggestion_context_columns(suggestion);

        // Write — write_lock (skip when deletion_flag is set → empty id; suggestions ∈ ALL_TABLES).
        self.conn.write_lock().run(String::new(), |conn| {
            // F0/#5186 + D2: stamp a monotonic HLC so this row syncs (LlmServer → skip).
            let (hw, hc, hd) = suggestion_stamp(&self.clock, conn, &suggestion.source)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            conn.execute(
                "INSERT OR REPLACE INTO suggestions \
             (suggestion_id, suggestion_type, source, content, priority, \
              confidence_score, relevance_score, is_actionable, reasoning, \
              created_at, expires_at, context_app, context_window, context_target_id, \
              hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
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
                    context_app,
                    context_window,
                    context_target_id,
                    hw,
                    hc,
                    hd,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save suggestion: {e}")))?;

            debug!(id = %suggestion.suggestion_id, "rule-based suggestion persisted to SQLite");
            Ok(suggestion.suggestion_id.clone())
        })
    }

    /// Async `save_rule_suggestion_sync` over the write funnel (ADR-026 PR-2).
    ///
    /// Isolated via `spawn_blocking` so the parking_lot guard is held only on a
    /// blocking-pool thread (never across an `.await`). Preserves the #4928 erase
    /// barrier (write_lock re-checks `deletion_flag || erasing` → returns an empty
    /// id on skip).
    pub(crate) async fn save_rule_suggestion_async(
        &self,
        suggestion: &maekon_core::models::suggestion::Suggestion,
    ) -> Result<String, StorageError> {
        // owned move into the Send + 'static closure; context columns borrow from
        // the suggestion, so compute them inside the closure on the owned clone.
        let suggestion = suggestion.clone();
        let clock = self.clock.clone();
        self.with_conn_skip(String::new(), move |conn| {
            let (context_app, context_window, context_target_id) =
                suggestion_context_columns(&suggestion);
            // F0/#5186 + D2: stamp a monotonic HLC so this row syncs (LlmServer → skip).
            let (hw, hc, hd) = suggestion_stamp(&clock, conn, &suggestion.source)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            conn.execute(
                "INSERT OR REPLACE INTO suggestions \
             (suggestion_id, suggestion_type, source, content, priority, \
              confidence_score, relevance_score, is_actionable, reasoning, \
              created_at, expires_at, context_app, context_window, context_target_id, \
              hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
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
                    context_app,
                    context_window,
                    context_target_id,
                    hw,
                    hc,
                    hd,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save suggestion: {e}")))?;

            debug!(id = %suggestion.suggestion_id, "rule-based suggestion persisted to SQLite");
            Ok(suggestion.suggestion_id.clone())
        })
        .await
    }

    /// Async `mark_unified_suggestion_shown` over the write funnel (ADR-026 PR-2;
    /// shared `_inner` body added in PR-6).
    pub(crate) async fn mark_unified_suggestion_shown_async(
        &self,
        suggestion_id: &str,
    ) -> Result<bool, StorageError> {
        // owned move into the Send + 'static closure.
        let suggestion_id = suggestion_id.to_string();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            Self::mark_unified_suggestion_shown_inner(conn, &suggestion_id, &clock)
        })
        .await
    }

    /// Mark a unified suggestion as shown by its string suggestion_id.
    /// Returns `true` if a row was updated, `false` otherwise.
    pub fn mark_unified_suggestion_shown(&self, suggestion_id: &str) -> Result<bool, StorageError> {
        // Write — write_lock (skip when deletion_flag is set → false).
        self.conn.write_lock().run(false, |conn| {
            Self::mark_unified_suggestion_shown_inner(conn, suggestion_id, &self.clock)
        })
    }

    fn mark_unified_suggestion_shown_inner(
        conn: &rusqlite::Connection,
        suggestion_id: &str,
        clock: &HlcClock,
    ) -> Result<bool, StorageError> {
        // F0/#5186: lifecycle status is a synced column → bump HLC so it propagates.
        // D2: the CASE leaves `LlmServer` rows at their existing HLC so they never start
        // propagating (server-synthesized prose must not cross-broadcast).
        let hlc = clock
            .next(conn)
            .map_err(|e| StorageError::Internal(format!("hlc stamp (shown): {e}")))?;
        let changed = conn
            .execute(
                "UPDATE suggestions SET shown_at = COALESCE(shown_at, datetime('now')), \
                 hlc_wall_ms = CASE WHEN source = 'LLM_SERVER' THEN hlc_wall_ms ELSE ?2 END, \
                 hlc_counter = CASE WHEN source = 'LLM_SERVER' THEN hlc_counter ELSE ?3 END, \
                 origin_device_id = CASE WHEN source = 'LLM_SERVER' THEN origin_device_id ELSE ?4 END \
                 WHERE suggestion_id = ?1",
                rusqlite::params![suggestion_id, hlc.wall_ms, hlc.counter, hlc.device_id],
            )
            .map_err(|e| StorageError::Internal(format!("suggestion shown record failure: {e}")))?;

        Ok(changed > 0)
    }

    // --------------------------------------------------------
    // Unified V8 suggestions queries
    // --------------------------------------------------------

    /// List active suggestions from the unified `suggestions` table, newest
    /// first, up to `limit` rows.
    pub fn list_suggestions(
        &self,
        limit: usize,
    ) -> Result<Vec<maekon_core::models::storage_records::SuggestionRecord>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        // Read — read_lock (deletion_flag irrelevant).
        let read = self.conn.read_lock();
        Self::list_suggestions_inner(read.conn(), limit)
    }

    /// Async `list_suggestions` over the read funnel (ADR-026 PR-6).
    pub(crate) async fn list_suggestions_async(
        &self,
        limit: usize,
    ) -> Result<Vec<maekon_core::models::storage_records::SuggestionRecord>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_conn_read(move |conn| Self::list_suggestions_inner(conn, limit))
            .await
    }

    fn list_suggestions_inner(
        conn: &rusqlite::Connection,
        limit: usize,
    ) -> Result<Vec<maekon_core::models::storage_records::SuggestionRecord>, StorageError> {
        let fetch_limit = limit.saturating_mul(4).max(limit) as i64;

        let mut stmt = conn
            .prepare(
                "SELECT id, suggestion_id, suggestion_type, source, content, priority, \
                 confidence_score, relevance_score, is_actionable, reasoning, \
                 shown_at, dismissed_at, acted_at, created_at, expires_at, \
                 context_app, context_window, context_target_id \
                 FROM suggestions \
                 WHERE dismissed_at IS NULL \
                   AND acted_at IS NULL \
                   AND state = 'pending' \
                   AND (expires_at IS NULL OR datetime(expires_at) > datetime('now')) \
                 ORDER BY created_at DESC \
                 LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("prepare failure: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![fetch_limit], |row| {
                Ok(maekon_core::models::storage_records::SuggestionRecord {
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
                    context_app: row.get(15)?,
                    context_window: row.get(16)?,
                    context_target_id: row.get(17)?,
                    resurface_at: None,
                })
            })
            .map_err(|e| StorageError::Internal(format!("query failure: {e}")))?;

        let mut records = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let record =
                row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?;
            let normalized_content = record
                .content
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            let dedupe_key = format!(
                "{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
                record.suggestion_type.to_ascii_uppercase(),
                record.source.to_ascii_uppercase(),
                record
                    .context_app
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase(),
                record
                    .context_window
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase(),
                record
                    .context_target_id
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase(),
                normalized_content
            );
            if seen.insert(dedupe_key) {
                records.push(record);
                if records.len() >= limit {
                    break;
                }
            }
        }
        Ok(records)
    }

    /// Dismiss a unified suggestion by its string `suggestion_id`.
    /// Returns `true` if a row was updated, `false` otherwise.
    pub fn dismiss_unified_suggestion(&self, suggestion_id: &str) -> Result<bool, StorageError> {
        // Write — write_lock (skip when deletion_flag is set → false).
        self.conn.write_lock().run(false, |conn| {
            Self::dismiss_unified_suggestion_inner(conn, suggestion_id, &self.clock)
        })
    }

    /// Async `dismiss_unified_suggestion` over the write funnel (ADR-026 PR-6).
    pub(crate) async fn dismiss_unified_suggestion_async(
        &self,
        suggestion_id: &str,
    ) -> Result<bool, StorageError> {
        // owned move into the Send + 'static closure.
        let suggestion_id = suggestion_id.to_string();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            Self::dismiss_unified_suggestion_inner(conn, &suggestion_id, &clock)
        })
        .await
    }

    fn dismiss_unified_suggestion_inner(
        conn: &rusqlite::Connection,
        suggestion_id: &str,
        clock: &HlcClock,
    ) -> Result<bool, StorageError> {
        // F0/#5186 + D2 (see shown_inner): bump HLC so the dismissal propagates, except
        // for `LlmServer` rows (CASE leaves their HLC untouched).
        let hlc = clock
            .next(conn)
            .map_err(|e| StorageError::Internal(format!("hlc stamp (dismiss): {e}")))?;
        let changed = conn
            .execute(
                "UPDATE suggestions SET dismissed_at = datetime('now'), \
                 hlc_wall_ms = CASE WHEN source = 'LLM_SERVER' THEN hlc_wall_ms ELSE ?2 END, \
                 hlc_counter = CASE WHEN source = 'LLM_SERVER' THEN hlc_counter ELSE ?3 END, \
                 origin_device_id = CASE WHEN source = 'LLM_SERVER' THEN origin_device_id ELSE ?4 END \
                 WHERE suggestion_id = ?1 AND dismissed_at IS NULL",
                rusqlite::params![suggestion_id, hlc.wall_ms, hlc.counter, hlc.device_id],
            )
            .map_err(|e| StorageError::Internal(format!("dismiss failure: {e}")))?;

        Ok(changed > 0)
    }

    /// Mark a unified suggestion as acted by its string `suggestion_id`.
    /// Returns `true` if a row was updated, `false` otherwise.
    pub fn mark_unified_suggestion_acted(&self, suggestion_id: &str) -> Result<bool, StorageError> {
        // Write — write_lock (skip when deletion_flag is set → false).
        self.conn.write_lock().run(false, |conn| {
            Self::mark_unified_suggestion_acted_inner(conn, suggestion_id, &self.clock)
        })
    }

    /// Async `mark_unified_suggestion_acted` over the write funnel (ADR-026 PR-6).
    pub(crate) async fn mark_unified_suggestion_acted_async(
        &self,
        suggestion_id: &str,
    ) -> Result<bool, StorageError> {
        // owned move into the Send + 'static closure.
        let suggestion_id = suggestion_id.to_string();
        let clock = self.clock.clone();
        self.with_conn(move |conn| {
            Self::mark_unified_suggestion_acted_inner(conn, &suggestion_id, &clock)
        })
        .await
    }

    fn mark_unified_suggestion_acted_inner(
        conn: &rusqlite::Connection,
        suggestion_id: &str,
        clock: &HlcClock,
    ) -> Result<bool, StorageError> {
        // F0/#5186 + D2 (see shown_inner): bump HLC so the action propagates, except for
        // `LlmServer` rows (CASE leaves their HLC untouched).
        let hlc = clock
            .next(conn)
            .map_err(|e| StorageError::Internal(format!("hlc stamp (acted): {e}")))?;
        let changed = conn
            .execute(
                "UPDATE suggestions SET acted_at = datetime('now'), \
                 hlc_wall_ms = CASE WHEN source = 'LLM_SERVER' THEN hlc_wall_ms ELSE ?2 END, \
                 hlc_counter = CASE WHEN source = 'LLM_SERVER' THEN hlc_counter ELSE ?3 END, \
                 origin_device_id = CASE WHEN source = 'LLM_SERVER' THEN origin_device_id ELSE ?4 END \
                 WHERE suggestion_id = ?1 AND acted_at IS NULL",
                rusqlite::params![suggestion_id, hlc.wall_ms, hlc.counter, hlc.device_id],
            )
            .map_err(|e| StorageError::Internal(format!("suggestion acted record failure: {e}")))?;

        Ok(changed > 0)
    }

    /// Check whether LLM_SERVER suggestions exist within the given lookback
    /// window. Used by the analysis loop to suppress local analysis when the
    /// server is actively sending suggestions.
    pub fn has_recent_server_suggestions(&self, lookback_secs: u64) -> Result<bool, StorageError> {
        // Read — read_lock (deletion_flag irrelevant).
        let read = self.conn.read_lock();
        Self::has_recent_server_suggestions_inner(read.conn(), lookback_secs)
    }

    /// Async `has_recent_server_suggestions` over the read funnel (ADR-026 PR-6).
    pub(crate) async fn has_recent_server_suggestions_async(
        &self,
        lookback_secs: u64,
    ) -> Result<bool, StorageError> {
        self.with_conn_read(move |conn| {
            Self::has_recent_server_suggestions_inner(conn, lookback_secs)
        })
        .await
    }

    fn has_recent_server_suggestions_inner(
        conn: &rusqlite::Connection,
        lookback_secs: u64,
    ) -> Result<bool, StorageError> {
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

    /// Daily suggestion stats for the last N days, grouped by type and source.
    pub fn suggestion_daily_stats(
        &self,
        days: u32,
    ) -> Result<Vec<maekon_core::models::storage_records::DailyStatRecord>, StorageError> {
        // Read — read_lock (deletion_flag irrelevant).
        let read = self.conn.read_lock();
        let conn = read.conn();
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
                Ok(maekon_core::models::storage_records::DailyStatRecord {
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

    /// List all suggestions regardless of state or expiry, ordered by
    /// `created_at DESC`, up to `limit` rows.  No dedup — history is a ledger.
    ///
    /// Used by the History/Stats/DailyStats fallback paths (#5699) when the
    /// in-memory SuggestionManager is not running (standalone / offline mode).
    pub fn list_recent_suggestions(
        &self,
        limit: usize,
    ) -> Result<Vec<SuggestionRecord>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // Read path — no deletion_flag filter needed for reads.
        let read = self.conn.read_lock();
        Self::list_recent_suggestions_inner(read.conn(), limit)
    }

    fn list_recent_suggestions_inner(
        conn: &rusqlite::Connection,
        limit: usize,
    ) -> Result<Vec<SuggestionRecord>, StorageError> {
        // Select the same 19 columns as list_suggestions_by_state (including
        // resurface_at) but with NO state/acted/dismissed/expiry filter — every
        // row regardless of lifecycle status is returned.  Uses the
        // idx_suggestions_created index that orders by created_at DESC.
        let mut stmt = conn
            .prepare(
                "SELECT id, suggestion_id, suggestion_type, source, content, priority, \
                 confidence_score, relevance_score, is_actionable, reasoning, \
                 shown_at, dismissed_at, acted_at, created_at, expires_at, resurface_at, \
                 context_app, context_window, context_target_id \
                 FROM suggestions \
                 ORDER BY created_at DESC LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("prepare failure: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
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
                    context_app: row.get(16)?,
                    context_window: row.get(17)?,
                    context_target_id: row.get(18)?,
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

    /// List suggestions by state for queue restoration (newest-created first).
    pub fn list_suggestions_by_state(
        &self,
        state: &str,
        limit: usize,
    ) -> Result<Vec<SuggestionRecord>, StorageError> {
        self.list_suggestions_by_state_ordered(state, limit, "ORDER BY created_at DESC")
    }

    /// #6938: List `deferred` suggestions ordered by SOONEST `resurface_at` first.
    ///
    /// The generic `list_suggestions_by_state` orders by `created_at DESC`, but for
    /// restoring snoozed (deferred) suggestions at launch the correct ranking key is
    /// `resurface_at` (when the snooze expires), NOT the suggestion's origin time —
    /// they are decoupled (an old-created suggestion snoozed for 30min has an old
    /// `created_at` but an imminent `resurface_at`). With `created_at DESC + LIMIT`,
    /// a deferred backlog over the limit dropped the suggestions about to resurface.
    /// Order by `resurface_at ASC` (NULLs last) so the soonest-resurfacing are kept.
    pub fn list_deferred_suggestions_by_resurface(
        &self,
        limit: usize,
    ) -> Result<Vec<SuggestionRecord>, StorageError> {
        self.list_suggestions_by_state_ordered(
            "deferred",
            limit,
            "ORDER BY resurface_at IS NULL, resurface_at ASC",
        )
    }

    /// Shared query for state-filtered suggestion reads. `order_clause` is a fixed,
    /// caller-supplied SQL fragment (never user input) appended before `LIMIT ?2`.
    fn list_suggestions_by_state_ordered(
        &self,
        state: &str,
        limit: usize,
        order_clause: &str,
    ) -> Result<Vec<SuggestionRecord>, StorageError> {
        // Read — read_lock (deletion_flag irrelevant).
        let read = self.conn.read_lock();
        let conn = read.conn();

        let query = format!(
            "SELECT id, suggestion_id, suggestion_type, source, content, priority, \
             confidence_score, relevance_score, is_actionable, reasoning, \
             shown_at, dismissed_at, acted_at, created_at, expires_at, resurface_at, \
             context_app, context_window, context_target_id \
             FROM suggestions WHERE state = ?1 \
             {order_clause} LIMIT ?2"
        );
        let mut stmt = conn
            .prepare(&query)
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
                    context_app: row.get(16)?,
                    context_window: row.get(17)?,
                    context_target_id: row.get(18)?,
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

    /// Save suggestion with explicit state for queue persistence.
    pub fn save_suggestion_with_state(
        &self,
        suggestion: &maekon_core::models::suggestion::Suggestion,
        state: &str,
        resurface_at: Option<&str>,
    ) -> Result<(), StorageError> {
        let (context_app, context_window, context_target_id) =
            suggestion_context_columns(suggestion);

        // Write — write_lock (skip when deletion_flag is set; suggestions ∈ ALL_TABLES).
        self.conn.write_lock().run((), |conn| {
            // F0/#5186 + D2: stamp a monotonic HLC so this row syncs (LlmServer → skip).
            let (hw, hc, hd) = suggestion_stamp(&self.clock, conn, &suggestion.source)
                .map_err(|e| StorageError::Internal(format!("hlc stamp: {e}")))?;
            conn.execute(
                "INSERT OR REPLACE INTO suggestions \
             (suggestion_id, suggestion_type, source, content, priority, \
              confidence_score, relevance_score, is_actionable, reasoning, \
              created_at, expires_at, state, resurface_at, \
              context_app, context_window, context_target_id, \
              hlc_wall_ms, hlc_counter, origin_device_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
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
                    context_app,
                    context_window,
                    context_target_id,
                    hw,
                    hc,
                    hd,
                ],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to save suggestion with state: {e}"))
            })?;

            debug!(id = %suggestion.suggestion_id, state, "suggestion persisted with state");
            Ok(())
        })
    }
}
