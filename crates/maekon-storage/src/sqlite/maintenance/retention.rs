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

            // V48 (#8218): the Pomodoro session is focus/activity state. A
            // user-requested activity-range deletion removes a session that
            // started inside the same closed interval.
            tx.execute(
                "DELETE FROM pomodoro_state WHERE started_at >= ?1 AND started_at <= ?2",
                rusqlite::params![from, to],
            )
            .map_err(|e| StorageError::Internal(format!("pomodoro delete failure: {e}")))?;
        }

        if delete_frames {
            // #9721: `frame_annotations` declares no FK, so SQLite's engine
            // cannot clean it up when the frame goes — and the rows carry the
            // user's own memo text on a path whose purpose is deletion.
            // `frame_tags` (CASCADE) and `interruptions` (SET NULL) DO have
            // FKs and are handled by the engine; see #9735 for the widespread
            // comments in this crate that wrongly claim otherwise.
            //
            // Runs before the parent delete, in the same transaction.
            super::frame_dependents::delete_frame_dependents_in_range(&tx, from, to)?;

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

        // #8045 B1: derived-data cascade. `activity_segments` (LLM summaries),
        // `embedding_vectors`, and `local_suggestions` are DERIVED from the raw
        // events/frames of a window, but the range delete above only touched the
        // raw tables — so a user deleting a sensitive period previously kept its
        // LLM summaries / embeddings / suggestions (they were only pruned by
        // separate age-based sweeps). Cascade them here, inside the SAME
        // transaction, for the overlapping window. Gated on the raw content they
        // are derived from (`events`/`frames`); a metrics/idle/process-only
        // delete leaves them untouched (they carry no screen/window content).
        if delete_events || delete_frames {
            Self::cascade_derivatives_in_range(&tx, from, to, &mut counts)?;
            Self::cascade_transcripts_in_range(&tx, from, to, &mut counts)?;
        }

        tx.commit()
            .map_err(|e| StorageError::Internal(format!("Failed to commit range deletion: {e}")))?;

        Ok(counts)
    }

    /// Delete voice transcripts (V47, #8059) overlapping `[from, to]` — plus
    /// their paired `search_fts` index rows — inside the caller's range-delete
    /// transaction.
    ///
    /// Transcripts are voice-activity content, so they are removed alongside the
    /// derivative cascade on `delete_events || delete_frames` (any screen/voice
    /// content delete); a metrics/idle/process-only delete leaves them untouched.
    /// They are LOCAL-ONLY (absent from `sync_table_descriptor::ALL_TABLE_NAMES`),
    /// so — unlike the synced derivative tables — no suppression tombstone is
    /// needed: a never-synced row cannot be resurrected by a peer.
    ///
    /// The `search_fts` rows are removed FIRST (the subquery still sees the
    /// transcript rows) and scoped to `content_type = 'transcript'` so a
    /// coincidental `segment_id` collision can never drop a real segment's index
    /// entry.
    fn cascade_transcripts_in_range(
        tx: &rusqlite::Transaction<'_>,
        from: &str,
        to: &str,
        counts: &mut DeletedRangeCounts,
    ) -> Result<(), StorageError> {
        tx.execute(
            "DELETE FROM search_fts \
             WHERE content_type = 'transcript' \
               AND segment_id IN ( \
                   SELECT id FROM transcripts WHERE timestamp >= ?1 AND timestamp <= ?2 \
               )",
            rusqlite::params![from, to],
        )
        .map_err(|e| StorageError::Internal(format!("transcript FTS range delete failure: {e}")))?;

        counts.transcripts_deleted = tx
            .execute(
                "DELETE FROM transcripts WHERE timestamp >= ?1 AND timestamp <= ?2",
                rusqlite::params![from, to],
            )
            .map_err(|e| StorageError::Internal(format!("transcript range delete failure: {e}")))?
            as u64;

        Ok(())
    }

    /// Deletes the derived-data rows (`activity_segments`, `embedding_vectors`,
    /// `local_suggestions`) overlapping `[from, to]`, inside the caller's
    /// range-delete transaction (#8045 B1).
    ///
    /// `activity_segments` and `embedding_vectors` are cross-device-synced
    /// tables, so — exactly like the age-based sweeps (#8043) — a plain DELETE
    /// with no tombstone would let a peer that still holds a locally-authored
    /// row re-push it on the next sync (peer resurrection → GDPR Art. 5(1)(e)).
    /// A LOCAL-origin suppression tombstone is captured for each aged-into-range
    /// row BEFORE the DELETE, using the IDENTICAL predicate so the tombstone set
    /// matches the removed rows exactly. `local_suggestions` is NOT in the synced
    /// set (`sync_table_descriptor::ALL_TABLE_NAMES`), so it needs no tombstone.
    ///
    /// Embeddings are keyed on their own `timestamp`; a SEGMENT_SUMMARY embedding
    /// shares the segment's window, so the two window filters remove the matched
    /// segment and its embedding together.
    fn cascade_derivatives_in_range(
        tx: &rusqlite::Transaction<'_>,
        from: &str,
        to: &str,
        counts: &mut DeletedRangeCounts,
    ) -> Result<(), StorageError> {
        // Read the local device id once for the tombstone capture below. `None`
        // (sync never ran) => no tombstone needed (a never-synced row was never
        // delivered to any peer, so it cannot be resurrected).
        let local_device = crate::sync_retention_tombstone::local_device_id(tx);

        // activity_segments (LLM summaries): matched on `start_time`.
        let seg_predicate =
            format!("start_time >= '{from}' AND start_time <= '{to}' AND start_time IS NOT NULL");
        if let Some(local) = local_device.as_deref() {
            crate::sync_retention_tombstone::capture_local_origin_retention_tombstones(
                tx,
                "activity_segments",
                &seg_predicate,
                local,
            )?;
        }
        counts.activity_segments_deleted = tx
            .execute(
                "DELETE FROM activity_segments \
                 WHERE start_time >= ?1 AND start_time <= ?2 AND start_time IS NOT NULL",
                rusqlite::params![from, to],
            )
            .map_err(|e| StorageError::Internal(format!("segment range delete failure: {e}")))?
            as u64;

        // embedding_vectors: matched on `timestamp`.
        let emb_predicate = format!("timestamp >= '{from}' AND timestamp <= '{to}'");
        if let Some(local) = local_device.as_deref() {
            crate::sync_retention_tombstone::capture_local_origin_retention_tombstones(
                tx,
                "embedding_vectors",
                &emb_predicate,
                local,
            )?;
        }
        counts.embedding_vectors_deleted = tx
            .execute(
                "DELETE FROM embedding_vectors WHERE timestamp >= ?1 AND timestamp <= ?2",
                rusqlite::params![from, to],
            )
            .map_err(|e| StorageError::Internal(format!("embedding range delete failure: {e}")))?
            as u64;

        // local_suggestions (NOT synced → no tombstone): matched on `created_at`.
        counts.local_suggestions_deleted = tx
            .execute(
                "DELETE FROM local_suggestions WHERE created_at >= ?1 AND created_at <= ?2",
                rusqlite::params![from, to],
            )
            .map_err(|e| {
                StorageError::Internal(format!("local suggestion range delete failure: {e}"))
            })? as u64;

        Ok(())
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
            // (V18 `search_trigram` was dropped by V45 (#8056) — no longer erased
            //  here; a DELETE on the now-absent table would error this txn.)
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
            // V44: feedback-learning state (#7913 T2.1c) — user-derived learning,
            // erased with activity data like coaching_effectiveness.
            "feedback_scorer_tallies",
            "regime_reaction_stats",
            // V46: coaching adaptive-scorer weights (#8058 P2-1) — user-derived
            // learning, erased with activity data like the sibling learning tables.
            "adaptive_scorer_state",
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
            // V42: digest downstream processing markers are user-derived activity
            // processing state, not retained system metadata.
            "digest_processing_markers",
            // V47 (#8059): voice transcripts are user activity content, erased with
            // the rest of the user's data. Their paired `search_fts` index rows are
            // cleared by the whole-table `DELETE FROM search_fts` above (search_fts
            // is in ALL_TABLES) + the post-commit FTS rebuild, so no separate
            // transcript-FTS cleanup is needed here. LOCAL-ONLY table — no tombstone.
            "transcripts",
            // V48 (#8218): local focus-session state. No sync tombstone; it is
            // fully erased with the rest of the user's activity data.
            "pomodoro_state",
            // V49 (#8577, ADR-028): durable task lifecycle. LOCAL-ONLY tables — no
            // sync tombstone. Child before parent (FK is inert, but keep the
            // convention): blocker edges + receipts, then to-dos + source refs,
            // then the candidate root. Full erasure deletes all task data; the
            // retained audit/egress records carry category + outcome only.
            "todo_blockers",
            "task_transition_receipts",
            "todo_items",
            "task_source_refs",
            "task_candidates",
            // V51 (#8587, ADR-030 §12): work-context ledger. FULL LOCAL ERASURE
            // (this path) deletes EVERY plane INCLUDING the content-free suppression
            // tombstones — after credentials/cursors/connector are gone, no
            // acquisition path can resurrect them (§12; Amendment B3 distinguishes
            // this from uninstall, which RETAINS tombstones). LOCAL-ONLY tables — no
            // sync tombstone. Child before parent (FK inert): projection/raw/conflict
            // /tombstone/cursor/epoch reference or scope the envelope, so they go
            // before the envelopes themselves.
            "work_context_raw_blobs",
            "work_context_projections",
            "work_context_conflicts",
            "work_context_tombstones",
            "work_context_cursors",
            "work_context_access_epochs",
            "work_context_envelopes",
            // V52 (#8588, ADR-029): trusted Skill Pack catalog + activation.
            // LOCAL-ONLY tables — no sync tombstone. Child before parent (FK is
            // inert, but keep the convention): the singleton activation points at
            // a catalog row, so it goes first.
            //
            // NOTE: `skill_pack_activation_audit` (V52) is deliberately RETAINED,
            // mirroring `audit_log`/`egress_ledger`. It holds NO user content by
            // construction — the table has no body/context column at all, only the
            // skill id, version, body DIGEST, decision and capability outcomes.
            // Retention rests on the same GDPR Art. 17(3) processing-record basis
            // as `audit_log`. Intentionally absent from ALL_TABLES; do NOT add it.
            "skill_pack_activation",
            "skill_pack_catalog",
            // V53 (#9465, ADR-033 §1.4): vault mirror hash state. MUST be
            // erased with everything else — an erase-surviving hash row would
            // make the next mirror cycle silently skip regenerating files the
            // Art.17 Phase-3 just deleted (the vault files themselves are
            // deleted by the erase orchestrators via
            // `MemoryVaultWriterPort::erase_generated_files`, not here).
            "vault_mirror_state",
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
        // already cleared the FTS5 `*_content` backing table (the raw OCR/window-
        // title text), but a `DELETE` leaves tombstoned term postings — tokenized
        // user content — in the `*_data` index segments until the index is merged.
        // Rebuild the index from the now-empty content so no tokenized content
        // lingers. Best-effort + POST-commit: the raw text is already erased
        // transactionally, so a rebuild failure must NOT roll back (and thereby
        // fail) the erasure itself. (`search_trigram` was dropped by V45 (#8056),
        // so it is no longer rebuilt here.)
        for fts in ["search_fts"] {
            if let Err(e) = conn.execute(&format!("INSERT INTO {fts}({fts}) VALUES('rebuild')"), [])
            {
                tracing::warn!("FTS5 index rebuild after GDPR erasure failed for {fts}: {e}");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod range_cascade_tests {
    use super::SqliteStorage;
    use maekon_core::types::TimeWindow;

    const IN_RANGE: &str = "2026-01-15T12:00:00+00:00";
    const OUT_OF_RANGE: &str = "2026-02-15T12:00:00+00:00";

    fn window() -> TimeWindow {
        TimeWindow::from_rfc3339_pair("2026-01-01T00:00:00+00:00", "2026-01-31T23:59:59+00:00")
            .unwrap()
    }

    /// Seed one in-range + one out-of-range row into each of the three derived
    /// tables, all local-origin so the synced ones are tombstone-eligible.
    fn seed_derivatives(storage: &SqliteStorage, local: &str) {
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        for (id, ts) in [("seg-in", IN_RANGE), ("seg-out", OUT_OF_RANGE)] {
            guard
                .execute(
                    "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
                     trigger_reason, dominant_category, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES (?1, ?2, ?2, 1, 'timer', 'Dev', 100, 1, ?3)",
                    rusqlite::params![id, ts, local],
                )
                .unwrap();
        }
        for (seg, ts) in [("seg-in", IN_RANGE), ("seg-out", OUT_OF_RANGE)] {
            guard
                .execute(
                    "INSERT INTO embedding_vectors (segment_id, content_type, original_text, \
                     vector, model_id, timestamp, hlc_wall_ms, hlc_counter, origin_device_id) \
                     VALUES (?1, 'screen', 'txt', x'0102', 'm1', ?2, 200, 2, ?3)",
                    rusqlite::params![seg, ts, local],
                )
                .unwrap();
        }
        for ts in [IN_RANGE, OUT_OF_RANGE] {
            guard
                .execute(
                    "INSERT INTO local_suggestions (suggestion_type, payload, created_at) \
                     VALUES ('tip', '{}', ?1)",
                    rusqlite::params![ts],
                )
                .unwrap();
        }
    }

    fn count(storage: &SqliteStorage, sql: &str) -> i64 {
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        guard.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn range_delete_cascades_to_derivatives_with_tombstones() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        seed_derivatives(&storage, &local);

        // delete_events=true triggers the derivative cascade.
        let counts = storage
            .delete_data_in_range(&window(), true, false, false, false, false)
            .unwrap();

        assert_eq!(counts.activity_segments_deleted, 1);
        assert_eq!(counts.embedding_vectors_deleted, 1);
        assert_eq!(counts.local_suggestions_deleted, 1);

        // Only the in-range derived rows are gone; out-of-range rows survive.
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id = 'seg-in'"
            ),
            0
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM activity_segments WHERE id = 'seg-out'"
            ),
            1,
            "an out-of-range segment must survive"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM embedding_vectors WHERE segment_id = 'seg-out'"
            ),
            1
        );
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM local_suggestions"),
            1,
            "only the in-range local suggestion is removed"
        );

        // Suppression tombstones captured for the two SYNCED derived tables so a
        // peer cannot resurrect them; the composite embedding key is used.
        let seg_tomb = count(
            &storage,
            "SELECT COUNT(*) FROM sync_tombstones \
             WHERE table_name = 'activity_segments' AND row_id = 'seg-in'",
        );
        assert_eq!(seg_tomb, 1, "in-range segment must leave a tombstone");
        let emb_key = format!("seg-in{}m1", '\u{1f}');
        let emb_tomb = {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .query_row(
                    "SELECT COUNT(*) FROM sync_tombstones \
                     WHERE table_name = 'embedding_vectors' AND row_id = ?1",
                    rusqlite::params![emb_key],
                    |r| r.get::<_, i64>(0),
                )
                .unwrap()
        };
        assert_eq!(
            emb_tomb, 1,
            "in-range embedding must leave a composite tombstone"
        );
        // local_suggestions is NOT a synced table → no tombstone.
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM sync_tombstones WHERE table_name = 'local_suggestions'"
            ),
            0,
            "local_suggestions is not synced and must not be tombstoned"
        );
    }

    // ── transcript erasure cascade (V47, #8059) ──────────────────────────

    fn seed_transcript(storage: &SqliteStorage, id: &str, ts: &str, text: &str) {
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        guard
            .execute(
                "INSERT INTO transcripts (id, timestamp, duration_secs, source, language, text) \
                 VALUES (?1, ?2, 2.0, 'whisper', 'en', ?3)",
                rusqlite::params![id, ts, text],
            )
            .unwrap();
        // Mirror the live save path's FTS index write so cleanup can be verified.
        guard
            .execute(
                "INSERT INTO search_fts (segment_id, content_type, searchable_text, shadow) \
                 VALUES (?1, 'transcript', ?2, '')",
                rusqlite::params![id, text],
            )
            .unwrap();
    }

    #[test]
    fn range_delete_removes_transcripts_and_their_fts_rows() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        seed_transcript(&storage, "tr-in", IN_RANGE, "in range transcript");
        seed_transcript(&storage, "tr-out", OUT_OF_RANGE, "out of range transcript");
        // A non-transcript FTS row that must never be touched by transcript cleanup.
        {
            let conn = storage.connection_arc();
            let guard = conn.test_lock();
            guard
                .execute(
                    "INSERT INTO search_fts (segment_id, content_type, searchable_text, shadow) \
                     VALUES ('seg-keep', 'segment', 'keep me', '')",
                    [],
                )
                .unwrap();
        }

        // delete_events=true triggers the content cascade (transcripts included).
        let counts = storage
            .delete_data_in_range(&window(), true, false, false, false, false)
            .unwrap();
        assert_eq!(counts.transcripts_deleted, 1);
        assert!(
            counts.total() >= 1,
            "the transcript delete must contribute to the reported total"
        );

        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM transcripts WHERE id = 'tr-in'"
            ),
            0,
            "the in-range transcript must be deleted"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM transcripts WHERE id = 'tr-out'"
            ),
            1,
            "an out-of-range transcript must survive"
        );
        // Paired FTS row for the deleted transcript is gone; the out-of-range
        // transcript's and the unrelated segment's FTS rows survive.
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM search_fts WHERE segment_id = 'tr-in'"
            ),
            0,
            "the deleted transcript's FTS row must be removed"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM search_fts WHERE segment_id = 'tr-out'"
            ),
            1,
            "the surviving transcript's FTS row must remain"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM search_fts WHERE segment_id = 'seg-keep'"
            ),
            1,
            "a non-transcript FTS row must never be touched by transcript cleanup"
        );
    }

    #[test]
    fn metrics_only_range_delete_leaves_transcripts_untouched() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        seed_transcript(&storage, "tr-in", IN_RANGE, "in range transcript");

        // delete_events=delete_frames=false → no content cascade.
        let counts = storage
            .delete_data_in_range(&window(), false, false, true, false, false)
            .unwrap();
        assert_eq!(counts.transcripts_deleted, 0);
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM transcripts"),
            1,
            "a metrics-only range delete must not remove transcripts"
        );
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM search_fts WHERE segment_id = 'tr-in'"
            ),
            1,
            "a metrics-only range delete must not touch transcript FTS rows"
        );
    }

    #[test]
    fn full_wipe_removes_transcripts() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        seed_transcript(&storage, "tr-1", IN_RANGE, "wipe me");

        storage.delete_all_data().unwrap();

        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM transcripts"),
            0,
            "delete_all_data must clear transcripts (ALL_TABLES membership)"
        );
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM search_fts"),
            0,
            "the whole-table search_fts wipe must clear the transcript FTS row too"
        );
    }

    #[test]
    fn metrics_only_range_delete_leaves_derivatives_untouched() {
        // A metrics/idle/process-only delete (delete_events=delete_frames=false)
        // must not cascade — derived screen/window content is unrelated.
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let (local, _) = storage.ensure_device_identity("Local").unwrap();
        seed_derivatives(&storage, &local);

        let counts = storage
            .delete_data_in_range(&window(), false, false, true, false, false)
            .unwrap();

        assert_eq!(counts.activity_segments_deleted, 0);
        assert_eq!(counts.embedding_vectors_deleted, 0);
        assert_eq!(counts.local_suggestions_deleted, 0);
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM activity_segments"),
            2,
            "no segment may be deleted by a metrics-only range delete"
        );
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM sync_tombstones"),
            0,
            "no tombstone may be captured when no derived row is deleted"
        );
    }
}
