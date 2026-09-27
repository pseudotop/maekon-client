//! ChangeExtractor implementation for SQLite.
//!
//! Queries activity_segments, regimes, regime_overrides, embedding_vectors,
//! suggestions, and trigger_params_snapshots for rows modified since a
//! given HLC watermark. Respects SyncConfig data minimization flags.

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension};
use std::sync::Arc;
use tracing::debug;

use crate::error::StorageError;
use crate::sqlite::hlc_clock::ERASURE_HLC_META_KEY;
use crate::sqlite::GuardedConnection;
use crate::sync_table_descriptor as table_descriptor;
use maekon_core::config::SyncConfig;
use maekon_core::error::CoreError;
use maekon_core::models::sync::{ChangeSet, ChangeSetKind, Tombstone};
use maekon_core::ports::change_extractor::ChangeExtractor;
use maekon_core::sync::Hlc;

/// SQLite-backed ChangeExtractor adapter.
pub struct SqliteSyncExtractor {
    conn: Arc<GuardedConnection>,
    device_id: String,
    device_name: String,
    sync_config: SyncConfig,
}

impl SqliteSyncExtractor {
    pub fn new(
        conn: Arc<GuardedConnection>,
        device_id: String,
        device_name: String,
        sync_config: SyncConfig,
    ) -> Self {
        Self {
            conn,
            device_id,
            device_name,
            sync_config,
        }
    }

    /// Backfill origin_device_id for pre-sync rows (empty string -> local device_id).
    /// Called once on first extraction. Idempotent.
    ///
    /// #4928 round-3 (FIX A): this function issues an `UPDATE` against all 6 ALL_TABLES
    /// tables, so it **must go through the `write_lock()` funnel** (no bypassing via the
    /// read path). When an erasure is in progress the funnel skips it (`Skipped` → rows
    /// unchanged), so a DB about to be wiped is not backfilled — which is semantically
    /// correct and avoids smuggling a mutation through the read funnel.
    fn backfill_origin_device_id(conn: &Connection, device_id: &str) -> Result<u64, StorageError> {
        // ⚠️ GDPR SYNC GUARD (#4478 G3) — authoritative cross-device sync table set.
        // Adding a table here replicates its rows to LAN peers; a contributor MUST:
        //   1. add `hlc_wall_ms`/`hlc_counter`/`origin_device_id` columns (a schema
        //      migration — the HLC extractor below requires them);
        //   2. add it to `handle_deletion_event` in `sync_merger.rs` so a device-wide
        //      erasure (revoke_consent) propagates the delete to peers;
        //   3. note that erasure cross-device convergence is moving to a retained
        //      `sync_tombstones` outbox (schema landed V38/#5178; producer+apply in
        //      #5179/#5180, epic #5174) — until that wires up, a local `delete_all_data`
        //      can still be RE-HYDRATED from a peer that was offline at erasure time;
        //   4. add a PII opt-in flag (cf. `include_content_activities` /
        //      `include_embedding_text`) if the table carries user content.
        // Do NOT add `memory_claims`/`memory_edges` (ADR-023) — they are intentionally
        // device-local; syncing LLM-enriched claims would need all of the above plus a
        // PII gate the extractor does not have.
        //
        // Single source of truth for the 6 synced tables — see
        // `table_descriptor::ALL_TABLE_NAMES` (#7742).
        let tables = table_descriptor::ALL_TABLE_NAMES;
        let mut total = 0u64;
        for table in &tables {
            let sql =
                format!("UPDATE {table} SET origin_device_id = ?1 WHERE origin_device_id = ''");
            let updated = conn
                .execute(&sql, rusqlite::params![device_id])
                .map_err(|e| {
                    StorageError::Internal(format!("backfill origin_device_id on {table}: {e}"))
                })?;
            total += updated as u64;
        }
        if total > 0 {
            debug!("backfilled origin_device_id on {total} rows");
        }
        Ok(total)
    }

    /// Query a single table for rows with HLC > watermark, returning JSON values.
    ///
    /// When `self_origin` is `Some(device_id)` the query is additionally constrained
    /// to `origin_device_id = device_id` so only rows authored by THIS device are
    /// emitted. This is the PUSH scope: the LAN `/sync/push` receiver contract (#5211)
    /// rejects any data row whose `origin_device_id` differs from the authenticated
    /// pusher (`first_row_origin_mismatch` in `lan_server`), so re-pushing peer-origin
    /// rows received via merge would be refused and would create cross-device echo
    /// loops (#6247). When `self_origin` is `None` every origin is served — the PULL
    /// scope, where a relay device may legitimately forward another peer's rows.
    fn query_table_changes(
        conn: &Connection,
        table: &str,
        columns: &str,
        since: &Hlc,
        self_origin: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, StorageError> {
        // The HLC>since predicate is identical in both scopes; the self-origin variant
        // just ANDs an equality on origin_device_id (bound as ?4 when present).
        let origin_filter = if self_origin.is_some() {
            " AND origin_device_id = ?4"
        } else {
            ""
        };
        let sql = format!(
            "SELECT {columns} FROM {table} \
             WHERE ((hlc_wall_ms > ?1) \
                OR (hlc_wall_ms = ?1 AND hlc_counter > ?2) \
                OR (hlc_wall_ms = ?1 AND hlc_counter = ?2 AND origin_device_id > ?3))\
                {origin_filter} \
             ORDER BY hlc_wall_ms, hlc_counter"
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| StorageError::Internal(format!("prepare query for {table}: {e}")))?;

        // rusqlite needs a homogeneous param slice; build it with the optional 4th bind.
        let mut params: Vec<&dyn rusqlite::ToSql> =
            vec![&since.wall_ms, &since.counter, &since.device_id];
        if let Some(device_id) = self_origin.as_ref() {
            params.push(device_id);
        }
        let rows = stmt
            .query_map(params.as_slice(), |row| {
                let json_str: String = row.get(0)?;
                Ok(json_str)
            })
            .map_err(|e| StorageError::Internal(format!("query {table}: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            let json_str =
                row.map_err(|e| StorageError::Internal(format!("row read {table}: {e}")))?;
            let value: serde_json::Value = serde_json::from_str(&json_str)
                .map_err(|e| StorageError::Internal(format!("json parse {table}: {e}")))?;
            results.push(value);
        }
        Ok(results)
    }

    /// Emit `sync_tombstones` (V38, #5174 S2) rows with HLC > `since` — the retained
    /// erasure outbox flowing through the normal sync stream so offline peers converge.
    /// Same HLC>since predicate as the live tables; rows are content-free skeletons.
    fn query_tombstones_since(
        conn: &Connection,
        since: &Hlc,
    ) -> Result<Vec<Tombstone>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at \
                 FROM sync_tombstones \
                 WHERE (hlc_wall_ms > ?1) \
                    OR (hlc_wall_ms = ?1 AND hlc_counter > ?2) \
                    OR (hlc_wall_ms = ?1 AND hlc_counter = ?2 AND origin_device_id > ?3) \
                 ORDER BY hlc_wall_ms, hlc_counter",
            )
            .map_err(|e| StorageError::Internal(format!("prepare tombstone query: {e}")))?;
        let rows = stmt
            .query_map(
                rusqlite::params![since.wall_ms, since.counter, &since.device_id],
                |row| {
                    Ok(Tombstone {
                        table_name: row.get(0)?,
                        row_id: row.get(1)?,
                        origin_device_id: row.get(2)?,
                        hlc_wall_ms: row.get(3)?,
                        hlc_counter: row.get(4)?,
                        deleted_at: row.get(5)?,
                    })
                },
            )
            .map_err(|e| StorageError::Internal(format!("query sync_tombstones: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(
                r.map_err(|e| StorageError::Internal(format!("row read sync_tombstones: {e}")))?,
            );
        }
        Ok(out)
    }

    /// Find the maximum HLC across all syncable tables.
    fn compute_max_hlc(conn: &Connection) -> Result<Hlc, StorageError> {
        // Mirror of the sync table set — see the GDPR SYNC GUARD on
        // `backfill_origin_device_id` before adding any table (#4478 G3).
        let tables = table_descriptor::ALL_TABLE_NAMES;

        let mut max = Hlc::default();
        for table in &tables {
            let sql = format!(
                "SELECT hlc_wall_ms, hlc_counter, origin_device_id \
                 FROM {table} \
                 ORDER BY hlc_wall_ms DESC, hlc_counter DESC, origin_device_id DESC \
                 LIMIT 1"
            );
            let candidate: Option<Hlc> = conn
                .query_row(&sql, [], |row| {
                    Ok(Hlc {
                        wall_ms: row.get(0)?,
                        counter: row.get(1)?,
                        device_id: row.get(2)?,
                    })
                })
                .optional()
                .map_err(|e| StorageError::Internal(format!("max HLC query on {table}: {e}")))?;
            if let Some(candidate) = candidate.filter(|candidate| candidate > &max) {
                max = candidate;
            }
        }

        // sync_tombstones (V38, #5174 S2): the retained erasure outbox must also raise the
        // watermark so a tombstone-only changeset (post-wipe, the 6 live tables empty)
        // advances the peer's watermark PAST the tombstones (B2). NOT added to the `tables`
        // array above — that array drives `backfill_origin_device_id`, which must never
        // touch the retained outbox.
        let candidate: Option<Hlc> = conn
            .query_row(
                "SELECT hlc_wall_ms, hlc_counter, origin_device_id \
                 FROM sync_tombstones \
                 ORDER BY hlc_wall_ms DESC, hlc_counter DESC, origin_device_id DESC \
                 LIMIT 1",
                [],
                |row| {
                    Ok(Hlc {
                        wall_ms: row.get(0)?,
                        counter: row.get(1)?,
                        device_id: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|e| {
                StorageError::Internal(format!("max HLC query on sync_tombstones: {e}"))
            })?;
        if let Some(candidate) = candidate.filter(|candidate| candidate > &max) {
            max = candidate;
        }

        Ok(max)
    }

    /// Synchronous core of changeset extraction, shared by the pull-serving
    /// (`get_changes_since`, all-origin) and push (`get_local_changes_since`,
    /// self-origin) paths. Runs entirely under a blocking thread + read lock.
    ///
    /// `self_origin` selects the row scope (#6247):
    /// * `true`  — only rows authored by THIS device (`origin_device_id = device_id`)
    ///   on the 6 live data tables. This is the PUSH scope and matches the LAN
    ///   `/sync/push` receiver contract (#5211): a peer may only push self-origin
    ///   data rows, so re-sending peer-origin rows received via merge would be
    ///   rejected and cause cross-device echo/loops.
    /// * `false` — every origin (the PULL scope), where a relay device may forward
    ///   another peer's rows to an offline receiver.
    ///
    /// Tombstones are ALWAYS emitted all-origin regardless of `self_origin`: the
    /// receiver contract explicitly exempts them (they keep the erased row's original
    /// origin) so a relay peer can carry a content-free erasure for an offline
    /// receiver. Scoping them to self would break offline-peer erasure convergence.
    fn extract_changeset_blocking(
        conn: &Connection,
        since: &Hlc,
        device_id: &str,
        device_name: &str,
        sync_config: &SyncConfig,
        self_origin: bool,
    ) -> Result<ChangeSet, StorageError> {
        let include_llm_summary = sync_config.include_llm_summary;
        // Self-origin push scope binds origin_device_id = this device; pull serves all.
        let origin_scope: Option<&str> = if self_origin { Some(device_id) } else { None };

        // --- Per-table JSON extraction queries ---
        // Each `TableDescriptor::extractor_select_expr` builds a self-contained
        // `json_object(...)` SELECT projection — the single source for each table's
        // column list, shared with `sync_merger`'s write side (#7742). Data-minimization
        // gating (`llm_summary` / `content_activities_json` / embedding `original_text`)
        // is `ExtractorGate` data on the descriptor, not hand-built here; `trigger_reason`
        // (NOT NULL) is always emitted so the peer's merge INSERT satisfies the
        // constraint (#5202).
        let segments = Self::query_table_changes(
            conn,
            table_descriptor::ACTIVITY_SEGMENTS.data_table,
            &table_descriptor::ACTIVITY_SEGMENTS.extractor_select_expr(sync_config),
            since,
            origin_scope,
        )?;

        // regimes (LWW, includes tombstone columns)
        let regimes = Self::query_table_changes(
            conn,
            table_descriptor::REGIMES.data_table,
            &table_descriptor::REGIMES.extractor_select_expr(sync_config),
            since,
            origin_scope,
        )?;

        // regime_overrides (append-only)
        let overrides = Self::query_table_changes(
            conn,
            table_descriptor::REGIME_OVERRIDES.data_table,
            &table_descriptor::REGIME_OVERRIDES.extractor_select_expr(sync_config),
            since,
            origin_scope,
        )?;

        // embedding_vectors (LWW, includes tombstone; `original_text` respects
        // include_embedding_text via the descriptor's `ExtractorGate`)
        let mut embeddings = Self::query_table_changes(
            conn,
            table_descriptor::EMBEDDING_VECTORS.data_table,
            &table_descriptor::EMBEDDING_VECTORS.extractor_select_expr(sync_config),
            since,
            origin_scope,
        )?;
        // #5210: a SEGMENT_SUMMARY embedding's text AND vector both represent the
        // llm_summary screen-activity narrative. When include_llm_summary is off, exclude
        // those rows entirely (not just their original_text) so include_embedding_text is
        // not a backdoor that re-exposes the gated narrative. They regenerate locally. This
        // is a whole-ROW filter (not a column gate), so it stays here rather than in the
        // shared descriptor.
        if !include_llm_summary {
            embeddings.retain(|e| {
                e.get("content_type").and_then(|v| v.as_str()) != Some("SEGMENT_SUMMARY")
            });
        }

        // suggestions (LWW, monotonic status merge)
        let suggestions = Self::query_table_changes(
            conn,
            table_descriptor::SUGGESTIONS.data_table,
            &table_descriptor::SUGGESTIONS.extractor_select_expr(sync_config),
            since,
            origin_scope,
        )?;

        // trigger_params_snapshots (append-only)
        let param_snapshots = Self::query_table_changes(
            conn,
            table_descriptor::TRIGGER_PARAMS_SNAPSHOTS.data_table,
            &table_descriptor::TRIGGER_PARAMS_SNAPSHOTS.extractor_select_expr(sync_config),
            since,
            origin_scope,
        )?;

        // sync_tombstones (V38, #5174 S2): the retained erasure outbox rides the normal
        // changeset stream so an offline peer converges on its next pull. Pure read.
        // ALWAYS all-origin (see the doc comment): the receiver exempts tombstones from
        // the self-origin check so relay peers carry content-free erasures.
        let tombstones = Self::query_tombstones_since(conn, since)?;

        // Compute the new watermark as the DB-GLOBAL max HLC tuple
        // (compute_max_hlc scans each whole syncable table with no `> since`
        // filter — it is NOT the max of just this batch). This DB-global
        // monotonicity is load-bearing: the watermark is the egress-ledger
        // dedup_key for a CrossDeviceSync push (#5147), so two distinct pushes
        // always get distinct keys and only an exact same-batch re-push
        // collapses. Do NOT "fix" this toward batch-max — it would let two
        // different egresses share a record_id and silently drop an audit row.
        // It is origin-agnostic by design so both scopes share one watermark, but
        // preserves origin_device_id as the HLC tie-breaker.
        let watermark = Self::compute_max_hlc(conn)?;

        Ok(ChangeSet {
            kind: ChangeSetKind::Data,
            origin_device_id: device_id.to_string(),
            origin_device_name: device_name.to_string(),
            watermark,
            segments,
            regimes,
            overrides,
            embeddings,
            suggestions,
            param_snapshots,
            preferences: Vec::new(), // deferred to Phase 3b
            tombstones,
        })
    }

    /// Shared async wrapper for the two extraction scopes. Runs backfill through the
    /// write funnel, then extracts under a read lock on a blocking thread. `self_origin`
    /// is forwarded to `extract_changeset_blocking` (true = push/self-origin, #6247).
    async fn extract_with_scope(
        &self,
        since: &Hlc,
        self_origin: bool,
    ) -> Result<ChangeSet, CoreError> {
        let conn = self.conn.clone();
        let since = since.clone();
        let device_id = self.device_id.clone();
        let device_name = self.device_name.clone();
        let sync_config = self.sync_config.clone();

        tokio::task::spawn_blocking(move || {
            // #4928 round-3 (FIX A): backfill is an ALL_TABLES UPDATE, so route it through
            // the write_lock funnel (no smuggling a mutation through the read path). When an
            // erasure is in progress the funnel skips it, so rows about to be wiped are not
            // backfilled — semantically harmless.
            conn.write_lock()
                .run(0u64, |c| Self::backfill_origin_device_id(c, &device_id))?;

            // The extraction that follows is a pure read path — read_lock (independent of
            // deletion_flag) is sufficient.
            let read = conn.read_lock();
            Self::extract_changeset_blocking(
                read.conn(),
                &since,
                &device_id,
                &device_name,
                &sync_config,
                self_origin,
            )
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
        .map_err(CoreError::from)
    }
}

#[async_trait]
impl ChangeExtractor for SqliteSyncExtractor {
    async fn get_changes_since(&self, since: &Hlc) -> Result<ChangeSet, CoreError> {
        // All-origin (pull-serving) scope.
        self.extract_with_scope(since, false).await
    }

    async fn get_local_changes_since(&self, since: &Hlc) -> Result<ChangeSet, CoreError> {
        // Self-origin (push) scope — see the trait doc + #6247.
        self.extract_with_scope(since, true).await
    }

    async fn local_watermark(&self) -> Result<Hlc, CoreError> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            // Read — read_lock (independent of deletion_flag).
            let read = conn.read_lock();
            Self::compute_max_hlc(read.conn())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
        .map_err(CoreError::from)
    }

    async fn persisted_erasure_hlc(&self) -> Result<Option<Hlc>, CoreError> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<Hlc>, StorageError> {
            // Read — read_lock (independent of deletion_flag). Even while an erasure is in
            // progress (flag set), the retained anchor must be read to stamp the
            // DeletionEvent watermark, so read_lock is correct.
            let read = conn.read_lock();
            let value: Option<String> = read
                .conn()
                .query_row(
                    "SELECT value FROM app_meta WHERE key = ?1",
                    rusqlite::params![ERASURE_HLC_META_KEY],
                    |r| r.get::<_, String>(0),
                )
                .optional()
                .map_err(|e| StorageError::Internal(format!("read erasure anchor: {e}")))?;
            Ok(value.and_then(|v| serde_json::from_str::<Hlc>(&v).ok()))
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
        .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sqlite::SqliteStorage;

    fn setup() -> (SqliteStorage, String) {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (device_id, _) = storage.ensure_device_identity("Test Device").unwrap();
        (storage, device_id)
    }

    #[tokio::test]
    async fn empty_db_returns_empty_changeset() {
        let (storage, device_id) = setup();
        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );
        let cs = extractor.get_changes_since(&Hlc::default()).await.unwrap();
        assert!(cs.is_empty());
        assert_eq!(cs.kind, ChangeSetKind::Data);
    }

    #[tokio::test]
    async fn local_watermark_returns_default_on_empty_db() {
        let (storage, device_id) = setup();
        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );
        let wm = extractor.local_watermark().await.unwrap();
        assert_eq!(wm.wall_ms, 0);
        assert_eq!(wm.counter, 0);
    }

    #[tokio::test]
    async fn local_watermark_preserves_origin_device_tie_breaker() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            for (id, origin) in [("seg-low", "aa-peer"), ("seg-high", "zz-peer")] {
                guard
                    .execute(
                        "INSERT INTO activity_segments \
                     (id, start_time, end_time, duration_secs, trigger_reason, \
                      dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES (?1, '2026-01-01T00:00:00', '2026-01-01T01:00:00', \
                             3600, 'timer', 'Development', 777, 2, ?2)",
                        rusqlite::params![id, origin],
                    )
                    .unwrap();
            }
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            "local-a".to_string(),
            "Test".to_string(),
            SyncConfig::default(),
        );
        let wm = extractor.local_watermark().await.unwrap();
        assert_eq!(wm.wall_ms, 777);
        assert_eq!(wm.counter, 2);
        assert_eq!(
            wm.device_id, "zz-peer",
            "watermark must keep the max origin_device_id tie-breaker"
        );
    }

    #[tokio::test]
    async fn backfill_sets_origin_device_id() {
        let (storage, device_id) = setup();
        // Insert a segment with empty origin_device_id (simulating pre-V14 data)
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-1', '2026-01-01T00:00:00', '2026-01-01T01:00:00', \
                         3600, 'timer', 'Development', 100, 1, '')",
                    [],
                )
                .unwrap();
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id.clone(),
            "Test".to_string(),
            SyncConfig::default(),
        );
        let cs = extractor.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(cs.segments.len(), 1);

        // Verify backfill happened
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        let origin: String = guard
            .query_row(
                "SELECT origin_device_id FROM activity_segments WHERE id = 'seg-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(origin, device_id);
    }

    /// #4928 round-3 (FIX A): backfill goes through the write_lock funnel, so it self-skips
    /// when deletion_flag is set — the origin_device_id of rows about to be wiped is not
    /// changed.
    #[tokio::test]
    async fn backfill_self_skips_when_deletion_flag_set() {
        use std::sync::atomic::Ordering;

        let (storage, device_id) = setup();
        // Insert a pre-V14 row (empty origin_device_id).
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-skip', '2026-01-01T00:00:00', '2026-01-01T01:00:00', \
                         3600, 'timer', 'Development', 100, 1, '')",
                    [],
                )
                .unwrap();
        }

        // deletion_flag set → the backfill write_lock must be skipped.
        storage
            .connection_arc()
            .deletion_flag()
            .store(true, Ordering::Release);

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id.clone(),
            "Test".to_string(),
            SyncConfig::default(),
        );
        // get_changes_since itself must succeed (reads are not skipped).
        let _ = extractor.get_changes_since(&Hlc::default()).await.unwrap();

        // origin_device_id must still be the empty string (proves the backfill was skipped).
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        let origin: String = guard
            .query_row(
                "SELECT origin_device_id FROM activity_segments WHERE id = 'seg-skip'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            origin, "",
            "when deletion_flag is set the backfill UPDATE must be skipped, leaving the row unchanged"
        );
    }

    #[tokio::test]
    async fn watermark_filters_old_rows() {
        let (storage, device_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            // Row with HLC (100, 1)
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-old', '2026-01-01T00:00:00', '2026-01-01T01:00:00', \
                         3600, 'timer', 'Development', 100, 1, ?1)",
                    rusqlite::params![device_id],
                )
                .unwrap();
            // Row with HLC (200, 0)
            guard
                .execute(
                    "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, \
                  dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                 VALUES ('seg-new', '2026-01-02T00:00:00', '2026-01-02T01:00:00', \
                         3600, 'timer', 'Communication', 200, 0, ?1)",
                    rusqlite::params![device_id],
                )
                .unwrap();
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );

        // Watermark at (150, 0) should only return seg-new
        let since = Hlc {
            wall_ms: 150,
            counter: 0,
            device_id: "".to_string(),
        };
        let cs = extractor.get_changes_since(&since).await.unwrap();
        assert_eq!(cs.segments.len(), 1);
        assert_eq!(cs.segments[0]["id"], "seg-new");
    }

    #[tokio::test]
    async fn erase_captures_tombstones_and_extractor_emits_them() {
        // S2 end-to-end: erase folds a content-free tombstone skeleton into the retained
        // outbox + persists the erasure_hlc anchor; the extractor then emits the skeleton
        // into the normal changeset stream so an offline peer converges.
        let (storage, device_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-e', '2026-01-01', '2026-01-01', 3600, 'timer', 'Dev', 100, 1, ?1)",
                    rusqlite::params![device_id],
                )
                .unwrap();
        }

        storage.delete_all_data().unwrap();

        {
            let conn = storage.connection_arc();
            let g = conn.test_lock();
            let segs: i64 = g
                .query_row("SELECT COUNT(*) FROM activity_segments", [], |r| r.get(0))
                .unwrap();
            assert_eq!(segs, 0, "live row hard-deleted");
            let (tw, origin): (i64, String) = g
                .query_row(
                    "SELECT hlc_wall_ms, origin_device_id FROM sync_tombstones \
                     WHERE table_name='activity_segments' AND row_id='seg-e'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert!(tw > 0, "tombstone stamped with the erasure HLC");
            assert_eq!(origin, device_id, "origin-scoped to the local device");
            let anchor: i64 = g
                .query_row(
                    "SELECT COUNT(*) FROM app_meta WHERE key='sync.erasure_hlc'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(anchor, 1, "erasure_hlc anchor persisted (retained)");
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id.clone(),
            "Test".to_string(),
            SyncConfig::default(),
        );
        let cs = extractor.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(cs.tombstones.len(), 1, "tombstone emitted");
        assert_eq!(cs.tombstones[0].table_name, "activity_segments");
        assert_eq!(cs.tombstones[0].row_id, "seg-e");
        assert!(
            !cs.is_empty(),
            "a tombstone-only changeset must be non-empty (push gate)"
        );

        // #5181: the SyncEngine reads this anchor to bound the device-wide DeletionEvent.
        let anchor = extractor.persisted_erasure_hlc().await.unwrap();
        let anchor = anchor.expect("erasure anchor present after erase");
        assert!(anchor.wall_ms > 0, "anchor carries the erasure HLC");
        assert_eq!(
            anchor.device_id, device_id,
            "anchor stamped by the local device"
        );
    }

    #[tokio::test]
    async fn persisted_erasure_hlc_none_before_any_erase() {
        // #5181: with no erase yet, the anchor is absent → the engine falls back to now()
        // (effectively unbounded), never panicking.
        let (storage, device_id) = setup();
        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );
        assert!(extractor.persisted_erasure_hlc().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn extractor_emits_trigger_reason() {
        // #5202: the emitted segment row must carry trigger_reason (NOT NULL on the peer).
        let (storage, device_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-tr', '2026-01-01', '2026-01-01', 3600, 'manual', 'Dev', 100, 1, ?1)",
                    rusqlite::params![device_id],
                )
                .unwrap();
        }
        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );
        let cs = extractor.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(cs.segments.len(), 1);
        assert_eq!(cs.segments[0]["trigger_reason"], "manual");
    }

    #[tokio::test]
    async fn llm_summary_gated_by_include_flag() {
        // #5174 privacy: llm_summary (an LLM narrative of screen activity) must only sync
        // when include_llm_summary is opted in — the default-off path omits it entirely.
        let (storage, device_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                    trigger_reason, dominant_category, llm_summary, llm_summary_status_json, hlc_wall_ms, hlc_counter, \
                     origin_device_id) VALUES ('seg-s', '2026-01-01', '2026-01-01', 3600, \
                     'timer', 'Dev', 'reviewed the Q2 headcount spreadsheet', ?2, 100, 1, ?1)",
                    rusqlite::params![device_id, r#"{"provider_class":"external_api"}"#],
                )
                .unwrap();
        }

        // Default config (include_llm_summary = false): the narrative is NOT emitted.
        let default_ex = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id.clone(),
            "Test".to_string(),
            SyncConfig::default(),
        );
        let cs = default_ex.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(cs.segments.len(), 1);
        assert!(
            cs.segments[0].get("llm_summary").is_none(),
            "llm_summary must be absent when not opted in"
        );
        assert!(
            cs.segments[0].get("llm_summary_status_json").is_none(),
            "summary provenance must be absent when its narrative is not opted in"
        );

        // Opted in: the narrative IS emitted.
        let cfg = SyncConfig {
            include_llm_summary: true,
            ..SyncConfig::default()
        };
        let opted_ex =
            SqliteSyncExtractor::new(storage.connection_arc(), device_id, "Test".to_string(), cfg);
        let cs2 = opted_ex.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(
            cs2.segments[0]["llm_summary"],
            "reviewed the Q2 headcount spreadsheet"
        );
        assert_eq!(
            cs2.segments[0]["llm_summary_status_json"],
            r#"{"provider_class":"external_api"}"#
        );
    }

    #[tokio::test]
    async fn segment_summary_embedding_excluded_when_llm_summary_off() {
        // #5210: include_embedding_text must not be a backdoor for the llm_summary narrative.
        // A SEGMENT_SUMMARY embedding (text+vector = the narrative) is excluded when
        // include_llm_summary is off; a CONTENT_ACTIVITY embedding still syncs.
        let (storage, device_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            for (seg, ct, w) in [
                ("seg-sum", "SEGMENT_SUMMARY", 100),
                ("seg-act", "CONTENT_ACTIVITY", 101),
            ] {
                guard
                    .execute(
                        "INSERT INTO embedding_vectors (segment_id, content_type, original_text, \
                         vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
                         VALUES (?1, ?2, 'narrative', x'0102', 'm1', '2026-01-01', ?3, 0, ?4)",
                        rusqlite::params![seg, ct, w, device_id],
                    )
                    .unwrap();
            }
        }

        // include_embedding_text ON, include_llm_summary OFF → SEGMENT_SUMMARY excluded.
        let cfg_off = SyncConfig {
            include_embedding_text: true,
            include_llm_summary: false,
            ..SyncConfig::default()
        };
        let ex_off = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id.clone(),
            "T".to_string(),
            cfg_off,
        );
        let cs = ex_off.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(
            cs.embeddings.len(),
            1,
            "only the non-summary embedding syncs"
        );
        assert_eq!(cs.embeddings[0]["content_type"], "CONTENT_ACTIVITY");

        // include_llm_summary ON → both sync.
        let cfg_on = SyncConfig {
            include_embedding_text: true,
            include_llm_summary: true,
            ..SyncConfig::default()
        };
        let ex_on =
            SqliteSyncExtractor::new(storage.connection_arc(), device_id, "T".to_string(), cfg_on);
        let cs2 = ex_on.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(
            cs2.embeddings.len(),
            2,
            "both embeddings sync when llm_summary is on"
        );
    }

    #[tokio::test]
    async fn push_excludes_peer_origin_rows_but_pull_includes_them() {
        // #6247: the PUSH path (get_local_changes_since) must emit ONLY self-origin data
        // rows, because the LAN /sync/push receiver (#5211) rejects any data row whose
        // origin is not the authenticated pusher — re-sending a peer-origin row received
        // via merge would fail the push AND echo the row back to its author (loop). The
        // PULL-serving path (get_changes_since) keeps all-origin so a relay can forward
        // another peer's rows to an offline receiver.
        let (storage, device_id) = setup();
        let peer_id = "peer-device-c";
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            // A row authored locally (self-origin).
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-self', '2026-01-01', '2026-01-01', 3600, 'timer', 'Dev', 100, 1, ?1)",
                    rusqlite::params![device_id],
                )
                .unwrap();
            // A row received from a peer via merge (peer-origin).
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES ('seg-peer', '2026-01-02', '2026-01-02', 3600, 'timer', 'Comm', 200, 1, ?1)",
                    rusqlite::params![peer_id],
                )
                .unwrap();
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id.clone(),
            "Test".to_string(),
            SyncConfig::default(),
        );

        // PUSH scope: only the self-origin row.
        let push_cs = extractor
            .get_local_changes_since(&Hlc::default())
            .await
            .unwrap();
        assert_eq!(
            push_cs.segments.len(),
            1,
            "push must carry only the self-origin row"
        );
        assert_eq!(push_cs.segments[0]["id"], "seg-self");
        assert_eq!(push_cs.segments[0]["origin_device_id"], device_id);

        // PULL-serving scope: both rows (relay may forward the peer's row).
        let pull_cs = extractor.get_changes_since(&Hlc::default()).await.unwrap();
        assert_eq!(
            pull_cs.segments.len(),
            2,
            "pull-serving must carry every origin so a relay can forward peer rows"
        );

        // Both scopes share the DB-global watermark (compute_max_hlc is origin-agnostic),
        // so the push watermark still advances past the peer row and the next push does
        // not re-extract it.
        assert_eq!(push_cs.watermark, pull_cs.watermark);
        assert_eq!(push_cs.watermark.wall_ms, 200);
    }

    #[tokio::test]
    async fn push_still_emits_peer_origin_tombstones_for_relay() {
        // #6247 / #5174: tombstones are content-free erasure carriers exempt from the
        // self-origin receiver check, so even on the self-origin PUSH path a relay device
        // must still forward a peer-origin tombstone to help an offline receiver converge.
        let (storage, device_id) = setup();
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            // A tombstone authored by ANOTHER device (as if relayed/merged in).
            guard
                .execute(
                    "INSERT INTO sync_tombstones \
                     (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
                     VALUES ('activity_segments', 'seg-erased', 'origin-other', 300, 0, \
                             '2026-02-01T00:00:00Z')",
                    [],
                )
                .unwrap();
        }

        let extractor = SqliteSyncExtractor::new(
            storage.connection_arc(),
            device_id,
            "Test".to_string(),
            SyncConfig::default(),
        );

        let push_cs = extractor
            .get_local_changes_since(&Hlc::default())
            .await
            .unwrap();
        assert_eq!(
            push_cs.tombstones.len(),
            1,
            "push must still relay a peer-origin tombstone"
        );
        assert_eq!(push_cs.tombstones[0].row_id, "seg-erased");
        assert_eq!(push_cs.tombstones[0].origin_device_id, "origin-other");
        assert!(
            push_cs.segments.is_empty(),
            "no self-origin data rows present"
        );
    }
}
