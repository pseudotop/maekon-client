use crate::error::StorageError;
use maekon_core::types::TimeWindow;

use super::super::{DeletedRangeCounts, SqliteStorage};

impl SqliteStorage {
    #[allow(clippy::too_many_arguments)]
    pub fn delete_data_in_range(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, StorageError> {
        let (from, to) = window.to_sql_pair();

        // A user-specified range delete is an ALL_TABLES write, so it goes through
        // write_lock (skipped when deletion_flag is set — harmless because erase
        // already wipes everything).
        // `run_mut` so the multi-table delete runs inside a single transaction.
        self.conn
            .write_lock()
            .run_mut(DeletedRangeCounts::default(), |conn| {
                Self::delete_data_in_range_inner(
                    conn,
                    &from,
                    &to,
                    delete_events,
                    delete_frames,
                    delete_metrics,
                    delete_processes,
                    delete_idle,
                )
            })
    }

    /// Async `delete_data_in_range` over the write funnel (ADR-026 PR-4).
    ///
    /// Routes through `with_conn` (`write_lock`, re-checks `deletion_flag ||
    /// erasing`); on erase-skip it returns `DeletedRangeCounts::default()`,
    /// which is harmless because the full erase already wipes every table.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn delete_data_in_range_async(
        &self,
        window: &TimeWindow,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, StorageError> {
        // owned move into the Send + 'static closure.
        let (from, to) = window.to_sql_pair();
        // `with_conn_mut` so the multi-table delete runs inside a single
        // transaction (mirrors the sync `run_mut` path above).
        self.with_conn_mut(move |conn| {
            Self::delete_data_in_range_inner(
                conn,
                &from,
                &to,
                delete_events,
                delete_frames,
                delete_metrics,
                delete_processes,
                delete_idle,
            )
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    fn delete_data_in_range_inner(
        conn: &mut rusqlite::Connection,
        from: &str,
        to: &str,
        delete_events: bool,
        delete_frames: bool,
        delete_metrics: bool,
        delete_processes: bool,
        delete_idle: bool,
    ) -> Result<DeletedRangeCounts, StorageError> {
        let mut counts = DeletedRangeCounts::default();

        // Wrap the multi-table (up to 6 statements) delete in ONE transaction so a
        // mid-way failure rolls the whole range-delete back instead of leaving the
        // database partially deleted. Mirrors `delete_all_data_inner`'s tx pattern.
        let tx = conn
            .transaction()
            .map_err(|e| StorageError::Internal(format!("Failed to begin transaction: {e}")))?;

        if delete_events {
            counts.events_deleted = tx
                .execute(
                    "DELETE FROM events WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("event delete failure: {e}")))?
                as u64;
        }

        if delete_frames {
            counts.frames_deleted = tx
                .execute(
                    "DELETE FROM frames WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("frame delete failure: {e}")))?
                as u64;
        }

        if delete_metrics {
            counts.metrics_deleted = tx
                .execute(
                    "DELETE FROM system_metrics WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("Failed to delete metrics: {e}")))?
                as u64;

            // `system_metrics_hourly.hour` is stored as an hour-truncated,
            // Z-suffixed bucket key (`%Y-%m-%dT%H:00:00Z`), NOT a full RFC3339
            // timestamp. Re-formatting the parsed bounds to that exact key makes
            // the lexical comparison match the stored column format. The previous
            // raw `from`/`to` (RFC3339, `+00:00` offset) sorted differently from
            // the `Z` key — e.g. when `to` landed on an hour boundary, the boundary
            // rollup row (`...HH:00:00Z`) compared GREATER than the bound
            // (`...HH:00:00+00:00`, since 'Z' > '+') and was orphaned. We truncate
            // both bounds DOWN to the hour: the lower bucket covering `from` and the
            // upper bucket covering `to` are both inclusive (the bucket at the `to`
            // hour aggregates samples up to `to`), matching the closed-closed
            // `[from, to]` raw-metric delete above.
            let (hour_from, hour_to) = Self::hourly_bucket_bounds(from, to);
            tx.execute(
                "DELETE FROM system_metrics_hourly WHERE hour >= ?1 AND hour <= ?2",
                rusqlite::params![hour_from, hour_to],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to delete hourly metrics: {e}")))?;
        }

        if delete_processes {
            counts.process_snapshots_deleted = tx
                .execute(
                    "DELETE FROM process_snapshots WHERE timestamp >= ?1 AND timestamp <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to delete process snapshots: {e}"))
                })? as u64;
        }

        if delete_idle {
            counts.idle_periods_deleted = tx
                .execute(
                    "DELETE FROM idle_periods WHERE start_time >= ?1 AND start_time <= ?2",
                    rusqlite::params![from, to],
                )
                .map_err(|e| StorageError::Internal(format!("idle record delete failure: {e}")))?
                as u64;
        }

        tx.commit()
            .map_err(|e| StorageError::Internal(format!("Failed to commit range deletion: {e}")))?;

        Ok(counts)
    }

    /// Map the RFC3339 range bounds (`from`, `to`) to the hour-bucket key range
    /// used by `system_metrics_hourly.hour`. Both bounds are truncated DOWN to
    /// the start of their hour and Z-suffixed via [`super::super::hour_bucket_key`]
    /// so the comparison matches the stored column format exactly. On a parse
    /// failure the raw bound is returned unchanged (best-effort; the raw-metric
    /// delete already used the same string).
    fn hourly_bucket_bounds(from: &str, to: &str) -> (String, String) {
        use chrono::{DateTime, Utc};
        let bucket = |s: &str| {
            DateTime::parse_from_rfc3339(s)
                .map(|dt| super::super::hour_bucket_key(dt.with_timezone(&Utc)))
                .unwrap_or_else(|_| s.to_string())
        };
        (bucket(from), bucket(to))
    }

    /// Atomically delete all user data from every known table inside a single
    /// SQLite transaction. On any failure the transaction auto-rolls-back so
    /// the database is never left in a partially-deleted state (GDPR compliance).
    pub fn delete_all_data(&self) -> Result<(), StorageError> {
        // The erase body itself — `lock_for_erase` (no deletion_flag re-check). It is
        // called while the flag is already set, so using write_lock would skip itself
        // and the wipe would never happen. The retained tables
        // (audit_log/egress_ledger/app_meta/schema_version) are deliberately excluded
        // from ALL_TABLES.
        self.conn
            .lock_for_erase()
            .run_mut(|conn| Self::delete_all_data_inner(conn, &self.clock))
    }

    /// Async `delete_all_data` for the `StorageMaintenanceStorage` port
    /// (ADR-026 PR-4).
    ///
    /// This is the **erase body itself**, so — unlike every other converted
    /// method — it MUST NOT route through the `with_conn`/`write_lock` funnel:
    /// `delete_all_data` is invoked while `deletion_flag` is already set, and
    /// `write_lock` would re-check that flag and *skip* the wipe. We therefore
    /// offload the synchronous, `lock_for_erase`-based `delete_all_data` onto
    /// the `spawn_blocking` pool directly (cloning the shared
    /// `Arc<GuardedConnection>`), so the `parking_lot` guard is still acquired
    /// on a blocking-pool thread and never held across an `.await`, while the
    /// #4928 erase barrier (retained-guard, no flag re-check) is preserved
    /// exactly. This mirrors the existing `src-tauri` consent-erase call site,
    /// which already wraps the sync method in `spawn_blocking`.
    pub(crate) async fn delete_all_data_async(&self) -> Result<(), StorageError> {
        let conn = self.conn.clone();
        let clock = self.clock.clone();
        tokio::task::spawn_blocking(move || {
            conn.lock_for_erase()
                .run_mut(|c| Self::delete_all_data_inner(c, &clock))
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    /// Shared erase-transaction body, callable from both the sync
    /// `delete_all_data` and the async `delete_all_data_async` over the same
    /// `lock_for_erase` retained guard. `clock` is `SqliteStorage::clock` — used to capture
    /// the erasure HLC + populate the retained `sync_tombstones` outbox inside this txn (S2).
    fn delete_all_data_inner(
        conn: &mut rusqlite::Connection,
        clock: &crate::sqlite::hlc_clock::HlcClock,
    ) -> Result<(), StorageError> {
        // All tables created by V1-V17 migrations (excluding schema_version).
        // Order: child/referencing tables before parent tables to avoid FK issues
        // if foreign keys are ever enabled.
        const ALL_TABLES: &[&str] = &[
            // V1-V7
            "events",
            "frames",
            "system_metrics",
            "system_metrics_hourly",
            "process_snapshots",
            "idle_periods",
            "session_stats",
            "work_sessions",
            "interruptions",
            "focus_metrics",
            "suggestions",
            "local_suggestions",
            "frame_tags",
            "tags",
            // V8-V10
            "activity_segments",
            "calibration_log",
            "daily_digests",
            "weekly_digests",
            "embedding_vectors",
            "regime_overrides",
            "regimes",
            "trigger_params_snapshots",
            // V11: FTS5 virtual table
            "search_fts",
            // V18: Korean trigram FTS5 table
            "search_trigram",
            // V12-V14
            "vector_binary_codes",
            "vector_index_meta",
            "ivf_centroids",
            "ivf_assignments",
            "gui_interactions",
            "device_identity",
            "sync_peers",
            // V15-V16
            "lan_peer_pins",
            // V17: coaching
            "coaching_events",
            "regime_goals",
            "coaching_effectiveness",
            // V18-V31 user-data tables (#4478): close the pre-existing right-to-erasure
            // gap. Child before parent: ai_conversation_messages CASCADEs from ai_sessions.
            // NOTE: `audit_log` / `session_audit_log` are deliberately RETAINED (NOT
            // erased) — the security audit trail is kept under a GDPR Art. 17(3)(b)/(e)
            // legal-obligation / legitimate-interest basis (SOC2 security-monitoring
            // retention outweighs erasure). This is a ratified exemption (#4478), not a
            // coverage gap. `app_meta` / `schema_version` are system metadata and must
            // NOT be erased.
            //
            // NOTE: `egress_ledger` (V36, #4803/E20) is ALSO deliberately RETAINED — it
            // is the egress compliance-evidence record (what left / was blocked from
            // leaving the device). It holds NO PII: only an event_type *category*,
            // byte_count, destination, disposition, consent_state and occurred_at — never
            // the event payload. Retention rests on the same GDPR Art. 17(3) processing-
            // record basis as `audit_log` (mirrors it). Intentionally absent from
            // ALL_TABLES; do NOT add it.
            //
            // NOTE: `sync_tombstones` (V38, #5174/#5178/E20) is ALSO deliberately RETAINED
            // — it is the cross-device erasure-convergence outbox. It holds NO PII: only a
            // table_name, the deleted row_id, origin_device_id, the erasure HLC and
            // deleted_at — never the row payload. The skeleton must survive THIS wipe so
            // the extractor can keep propagating the tombstone to offline-then-reconnecting
            // peers (otherwise the offline peer never erases). S2 (#5179) folds the
            // erase-time id-capture INSERT into this same transaction (before the DELETEs).
            // Intentionally absent from ALL_TABLES; do NOT add it.
            "ai_conversation_messages",
            "ai_sessions",
            "frame_annotations",
            "habit_streaks",
            "regime_manager_state",
            "automation_presets",
            "feedback_retries",
            // V34: ADR-023 memory-graph (child before parent: edges reference claims).
            "memory_edges",
            "memory_claims",
            // V39 (F0/#5186): the local HLC clock floor is activity-timing metadata
            // (wall_ms) → IN GDPR Art.17 erasure scope, so it IS erased here (unlike the
            // retained `app_meta["sync.erasure_hlc"]` anchor). Restart re-seeds it from that
            // anchor in `post_migration_setup`; within-session post-erase writes self-heal
            // the singleton via `HlcClock::next`'s UPSERT.
            "hlc_clock",
        ];

        let tx = conn
            .transaction()
            .map_err(|e| StorageError::Internal(format!("Failed to begin transaction: {e}")))?;

        // #5174 S2 (erase-time tombstone outbox): capture the erasure HLC ONCE (a monotonic
        // clock tick over the held erase transaction — NOT Hlc::now), persist it to the
        // RETAINED app_meta anchor, and populate the retained `sync_tombstones` outbox with
        // content-free id+HLC skeletons of THIS device's synced rows — all inside this one
        // `lock_for_erase` transaction so it is atomic with the wipe (B1/B3). The live rows
        // are still hard-deleted by the loop below; only skeletons survive, so an
        // offline-then-reconnecting peer converges later via the normal sync stream.
        let erasure_hlc = clock
            .next(&tx)
            .map_err(|e| StorageError::Internal(format!("erasure HLC tick: {e}")))?;
        let erasure_hlc_json = serde_json::to_string(&erasure_hlc)
            .map_err(|e| StorageError::Internal(format!("serialize erasure_hlc: {e}")))?;
        // Persist the anchor via a RAW `tx.execute` on the held transaction — NOT
        // `set_meta`/`set_meta_checked` (they re-acquire `retained_write_lock`, which would
        // deadlock the non-reentrant connection mutex already held by `lock_for_erase`).
        // `app_meta` is retained (absent from ALL_TABLES), so the anchor survives the wipe;
        // F0's `seed_from_db` reads it back on restart.
        tx.execute(
            "INSERT OR REPLACE INTO app_meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![
                crate::sqlite::hlc_clock::ERASURE_HLC_META_KEY,
                erasure_hlc_json
            ],
        )
        .map_err(|e| StorageError::Internal(format!("persist erasure_hlc anchor: {e}")))?;

        // Capture tombstone skeletons BEFORE the rows are hard-deleted, origin-scoped to the
        // local device (B3 — the local device_id is exactly `erasure_hlc.device_id`, stamped
        // by `HlcClock::next`). The `row_id` SQL expression differs per table: the PK column
        // for most, but `embedding_vectors.id` is a per-device autoincrement (the merger
        // re-inserts peers' embeddings with a fresh local id), so embeddings use the
        // cross-device-stable composite `segment_id || US || model_id` (US = char(31)).
        const SYNCED_TOMBSTONE_SOURCES: [(&str, &str); 6] = [
            ("activity_segments", "id"),
            ("regimes", "id"),
            ("regime_overrides", "override_id"),
            ("embedding_vectors", "segment_id || char(31) || model_id"),
            ("suggestions", "suggestion_id"),
            ("trigger_params_snapshots", "id"),
        ];
        for (table, pk) in SYNCED_TOMBSTONE_SOURCES {
            tx.execute(
                &format!(
                    "INSERT OR REPLACE INTO sync_tombstones \
                     (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
                     SELECT '{table}', CAST({pk} AS TEXT), origin_device_id, ?1, ?2, datetime('now') \
                     FROM {table} WHERE origin_device_id = ?3"
                ),
                rusqlite::params![
                    erasure_hlc.wall_ms,
                    erasure_hlc.counter,
                    &erasure_hlc.device_id
                ],
            )
            .map_err(|e| StorageError::Internal(format!("capture tombstones for {table}: {e}")))?;
        }

        for table in ALL_TABLES {
            tx.execute(&format!("DELETE FROM {table}"), [])
                .map_err(|e| {
                    StorageError::Internal(format!("GDPR delete failed on table '{table}': {e}"))
                })?;
        }

        tx.commit()
            .map_err(|e| StorageError::Internal(format!("Failed to commit GDPR deletion: {e}")))?;

        // GDPR defense-in-depth (#4478 G2): the committed `DELETE FROM search_fts`
        // / `search_trigram` already cleared the FTS5 `*_content` backing tables
        // (the raw OCR/window-title text), but a `DELETE` leaves tombstoned term
        // postings — tokenized user content — in the `*_data` index segments until
        // the index is merged. Rebuild the index from the now-empty content so no
        // tokenized content lingers. Best-effort + POST-commit: the raw text is
        // already erased transactionally, so a rebuild failure must NOT roll back
        // (and thereby fail) the erasure itself.
        for fts in ["search_fts", "search_trigram"] {
            if let Err(e) = conn.execute(&format!("INSERT INTO {fts}({fts}) VALUES('rebuild')"), [])
            {
                tracing::warn!("FTS5 index rebuild after GDPR erasure failed for {fts}: {e}");
            }
        }

        Ok(())
    }
}
