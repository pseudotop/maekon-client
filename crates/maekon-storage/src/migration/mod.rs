//! SQLite schema migrations for Maekon local storage.
//!
//! ## Directory Module Structure (ADR-003)
//!
//! - `mod.rs` — orchestrator (`run_migrations`, `get_version`, version constant)
//! - `v01_v08.rs` — foundation tables (events, frames, metrics, sessions, tags, edge intelligence)
//! - `v09_v18.rs` — tiered memory, vectors, sync, IVF index, coaching engine, trigram FTS, app_meta
//! - `v19_v21.rs` — app_meta, session audit log, AI sessions, gui_interactions type_confidence
//! - `v25.rs` — audit_log table for durable audit entry persistence
//! - `v26.rs` — ai_sessions title column for user-assigned display names
//! - `v27.rs` — habit_streaks table for daily regime habit tracking
//! - `v28.rs` — feedback tracking columns on local_suggestions for few-shot prompt construction
//! - `v29.rs` — automation_presets table for persistent custom preset storage
//! - `v30.rs` — frame_annotations table for user-created highlights, memos, arrows
//! - `v31_regime_manager_state.rs` — regime_manager_state singleton for
//!   RegimeManager persistence across restart (Phase 3 C3c/X6)
//! - `v32_audit_log_command_id_index.rs` — partial index on audit_log.command_id
//!   for O(log n) entries_by_command_id lookups (D25)
//! - `v33_suggestion_context_scope.rs` — context scope columns on suggestions
//!   for app/window/target-aware suggestion restore and dedupe
//! - `v34_memory_graph.rs` — memory_claims + memory_edges tables for the
//!   ADR-023 local symbolic memory-graph substrate
//! - `v35_memory_edge_unique.rs` — UNIQUE(src_id, dst_id, edge_type) on
//!   memory_edges so the `INSERT OR IGNORE` in add_edge/supersede_claim actually
//!   dedupes (ADR-023 belief-revision hygiene; collapses prior duplicates first)
//! - `v36_egress_ledger.rs` — egress_ledger table recording what left the device
//!   (or was policy-blocked) for compliance evidence (#4803, E20)
//! - `v37_audit_log_hash_chain.rs` — SHA-256 hash chain columns (seq/prev_hash/
//!   entry_hash) on audit_log making it tamper-evident (ADR-072 client mirror,
//!   #4834, E20)
//! - `v38_sync_tombstones.rs` — retained `sync_tombstones` outbox (id+HLC skeletons,
//!   no PII) carrying cross-device GDPR Art.17 erasure to offline-then-reconnecting
//!   peers; retained across erase like egress_ledger (#5174/#5178, E20)
//! - `v39_hlc_clock.rs` — `hlc_clock` singleton: persistent monotonic HLC clock floor
//!   for stamping local synced-table writes so sync actually propagates (F0/#5186, E20)
//! - `v41_cjk_bigram_shadow.rs` — rebuild `search_fts` with CJK bigram shadow column;
//!   switches tokenizer from `porter unicode61` to `unicode61` and adds an FTS-indexed
//!   `shadow` column containing CJK bigram expansions. Improves ja R@3 0→0.611,
//!   ko 0.286→0.611 (Option F, #5758).
//! - `v42_digest_processing_markers.rs` — digest downstream processing markers
//!   for idempotent ADR-023 claim promotion / belief revision catch-up (#7486).
//! - `v43_gui_interactions_drop_unused_columns.rs` — drops the never-populated
//!   `segment_id`/`element_text`/`element_type`/`bbox_json` columns from
//!   `gui_interactions`; the production writer only ever wrote `interaction_type`
//!   + `app_name` (#7678 D3).
//! - `v44_learning_persistence.rs` — restart-surviving feedback-learning tables:
//!   `feedback_scorer_tallies` (FeedbackScorer per-(type, source) counts, with a
//!   wall-clock `last_updated` decay anchor) and `regime_reaction_stats`
//!   (RegimeClassifier per-regime + aggregate reaction counts). Both erased with
//!   activity data like `coaching_effectiveness` (#7913 T2.1c).
//! - `v47_transcripts.rs` — `transcripts` table persisting PII-masked speech-to-
//!   text output (#8059). Indexed into `search_fts` for keyword search; swept by
//!   the range-delete / full-wipe / age-retention erasure paths. LOCAL-ONLY (no
//!   HLC/sync columns — deliberately off the cross-device sync surface).
//! - `v45_drop_search_trigram.rs` — drops the dead `search_trigram` FTS5 table
//!   (created by V18, never read/written in production; superseded by the V41
//!   `search_fts` CJK bigram shadow). Removes it from the GDPR erase loop too
//!   (retention.rs), so a post-drop `DELETE FROM search_trigram` can no longer
//!   error the erase transaction (#8056 P3).
//! - `v54_orphan_annotation_cleanup.rs` — one-time repair: deletes
//!   `frame_annotations` rows whose frame is already gone (#9736). Not a schema
//!   change; `frame_annotations` declares no FK, so pre-#9731 deletions left the
//!   rows behind and the GDPR export kept dumping them.
//! - `v55_effective_mapping_cache.rs` — non-authoritative, server-validated TMD
//!   mapping candidates used only for revalidation and offline explanation
//!   (#10358); included in the full local erasure sweep.
//! - `v56_wbs_xlsx_receipts.rs` — durable local-first output receipt spool;
//!   successful upload marks a row instead of deleting its local evidence.
//! - `v53_vault_mirror_state.rs` — `vault_mirror_state` (per-file content
//!   hash for the ADR-033 memory vault mirror's change detection; member of
//!   the GDPR erase sweep).
//! - `v52_skill_pack_activation.rs` — `skill_pack_catalog`,
//!   `skill_pack_activation` (singleton), and `skill_pack_activation_audit` for
//!   ADR-029 trusted Skill Pack activation. v51 is reserved for the
//!   MK-CONTEXT work-context ledger (#8587/#8589), landing concurrently
//!   (#8588).
//! - `v46_adaptive_scorer_state.rs` — singleton `adaptive_scorer_state` holding
//!   the coaching `AdaptiveScorer`'s online-logistic-regression weights (the LAST
//!   feedback-learning component that reset on restart). Erased with activity
//!   data like the sibling learning tables (#8058 P2-1).

#[cfg(test)]
mod tests;
mod v01_v08;
mod v09_v18;
mod v19_v21;
mod v22_v23;
mod v23_v24;
mod v25;
mod v26;
mod v27;
mod v28;
mod v29;
mod v30;
mod v31_regime_manager_state;
mod v32_audit_log_command_id_index;
mod v33_suggestion_context_scope;
mod v34_memory_graph;
mod v35_memory_edge_unique;
mod v36_egress_ledger;
mod v37_audit_log_hash_chain;
mod v38_sync_tombstones;
mod v39_hlc_clock;
mod v40_egress_recipient_count;
mod v41_cjk_bigram_shadow;
mod v42_digest_processing_markers;
mod v43_gui_interactions_drop_unused_columns;
mod v44_learning_persistence;
mod v45_drop_search_trigram;
mod v46_adaptive_scorer_state;
mod v47_transcripts;
mod v48_pomodoro_state;
mod v49_durable_tasks;
mod v50_extension_registry;
pub(crate) mod v51_work_context;
mod v52_skill_pack_activation;
mod v53_vault_mirror_state;
mod v54_orphan_annotation_cleanup;
mod v55_effective_mapping_cache;
mod v56_wbs_xlsx_receipts;

use rusqlite::Connection;
use tracing::{error, info, warn};

pub const CURRENT_VERSION: u32 = 56;

/// Keep at most this many pre-migration backups for a given DB; older ones are
/// pruned after each new backup so they cannot accumulate unbounded across the
/// lifetime of the install (#6830). 3 covers rollback across the most recent
/// migrations while bounding disk use.
const MAX_RETAINED_BACKUPS: usize = 3;

/// Prune pre-migration backups for `db_path`, keeping the `MAX_RETAINED_BACKUPS`
/// most recent (by mtime). Matches only THIS db's backups via the
/// `{stem}.backup.v` prefix (so it never touches the live `.db`/`-wal`/`-shm` or
/// a sibling db's backups). Best-effort: failures are logged, never fatal.
fn prune_old_backups(db_path: &std::path::Path) {
    let (Some(dir), Some(stem)) = (
        db_path.parent(),
        db_path.file_stem().and_then(|s| s.to_str()),
    ) else {
        return;
    };
    let prefix = format!("{stem}.backup.v");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut backups: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            // The char after `{stem}` must be `.` so "maekon" does not match a
            // sibling "maekon2.backup.v…".
            if !path.file_name()?.to_str()?.starts_with(&prefix) {
                return None;
            }
            let mtime = entry.metadata().ok()?.modified().ok()?;
            Some((mtime, path))
        })
        .collect();
    if backups.len() <= MAX_RETAINED_BACKUPS {
        return;
    }
    // Newest first; tie-break on path (the filename embeds version+timestamp) so
    // ordering is fully deterministic even when two backups share an mtime.
    backups.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    for (_, path) in backups.into_iter().skip(MAX_RETAINED_BACKUPS) {
        match std::fs::remove_file(&path) {
            Ok(()) => info!("pruned old DB backup: {}", path.display()),
            Err(e) => warn!("failed to prune old DB backup {}: {e}", path.display()),
        }
    }
}

/// Back up the database file before running schema migrations.
fn backup_if_needed(
    conn: &Connection,
    current_version: u32,
    target_version: u32,
) -> Option<std::path::PathBuf> {
    if current_version >= target_version {
        return None;
    }

    // conn.path() returns Option<&str> in rusqlite 0.38+
    let db_path_str = conn.path().filter(|p| !p.is_empty() && *p != ":memory:")?;
    let db_path = std::path::PathBuf::from(db_path_str);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup_path = db_path.with_extension(format!("backup.v{current_version}.{timestamp}"));

    // Merge any WAL-resident committed transactions into the main `.db` before
    // the file copy. In WAL mode a plain `fs::copy` of only the `.db` (without
    // the `-wal`/`-shm` sidecars) yields a VALID but STALE backup that silently
    // omits commits still held in the WAL — defeating the pre-migration safety
    // net if a restore is later attempted. Best-effort: on failure we still
    // attempt the copy but warn loudly (#6823).
    if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        warn!(
            "WAL checkpoint before migration backup failed (backup may omit recent commits): {e}"
        );
    }

    match std::fs::copy(&db_path, &backup_path) {
        Ok(bytes) => {
            info!(
                "DB backup created before migration v{current_version}→v{target_version}: {} ({bytes} bytes)",
                backup_path.display()
            );
            // #6830: bound accumulated backups (the just-created one is newest → retained).
            prune_old_backups(&db_path);
            Some(backup_path)
        }
        Err(e) => {
            warn!("DB backup failed (continuing with migration): {e}");
            None
        }
    }
}

/// Execute a single migration step inside a SAVEPOINT for rollback safety.
fn run_migration_step(
    conn: &Connection,
    version: u32,
    migrate_fn: fn(&Connection) -> Result<(), rusqlite::Error>,
) -> Result<(), rusqlite::Error> {
    let sp_name = format!("migration_v{version}");
    conn.execute_batch(&format!("SAVEPOINT {sp_name}"))?;
    match migrate_fn(conn) {
        Ok(()) => {
            conn.execute_batch(&format!("RELEASE SAVEPOINT {sp_name}"))?;
            Ok(())
        }
        Err(e) => {
            warn!("migration v{version} failed, rolling back: {e}");
            if let Err(rb_err) = conn.execute_batch(&format!("ROLLBACK TO SAVEPOINT {sp_name}")) {
                error!(
                    version,
                    "ROLLBACK TO SAVEPOINT failed — database may be in inconsistent state: {rb_err}"
                );
            }
            Err(e)
        }
    }
}

fn future_schema_error(current_version: u32) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
        Some(format!(
            "database schema version {current_version} is newer than this client supports ({CURRENT_VERSION}); upgrade the client or use a separate data directory"
        )),
    )
}

pub fn run_migrations(conn: &Connection) -> Result<(), rusqlite::Error> {
    run_migrations_to(conn, CURRENT_VERSION)
}

/// Run migrations only up to `target` (inclusive). Production always uses
/// [`run_migrations`] (target = [`CURRENT_VERSION`]); the bounded form exists
/// for the QC legacy-migration fixture (#9083), which needs a REAL
/// older-schema database instead of forging one by deleting newer-version
/// artifacts — a seal that silently broke every time `CURRENT_VERSION`
/// advanced (V49–V52).
pub fn run_migrations_to(conn: &Connection, target: u32) -> Result<(), rusqlite::Error> {
    if target > CURRENT_VERSION {
        return Err(future_schema_error(target));
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    let current = get_version(conn)?;
    info!("current schema version: {current}, target: {target}");

    if current > target {
        error!(
            current_schema_version = current,
            supported_schema_version = target,
            "database schema version is newer than the requested migration target"
        );
        return Err(future_schema_error(current));
    }

    if current < target && backup_if_needed(conn, current, target).is_none() {
        warn!("proceeding with migration without backup");
    }

    if target >= 1 && current < 1 {
        run_migration_step(conn, 1, v01_v08::migrate_v1)?;
    }
    if target >= 2 && current < 2 {
        run_migration_step(conn, 2, v01_v08::migrate_v2)?;
    }
    if target >= 3 && current < 3 {
        run_migration_step(conn, 3, v01_v08::migrate_v3)?;
    }
    if target >= 4 && current < 4 {
        run_migration_step(conn, 4, v01_v08::migrate_v4)?;
    }
    if target >= 5 && current < 5 {
        run_migration_step(conn, 5, v01_v08::migrate_v5)?;
    }
    if target >= 6 && current < 6 {
        run_migration_step(conn, 6, v01_v08::migrate_v6)?;
    }
    if target >= 7 && current < 7 {
        run_migration_step(conn, 7, v01_v08::migrate_v7)?;
    }
    if target >= 8 && current < 8 {
        run_migration_step(conn, 8, v01_v08::migrate_v8)?;
    }
    if target >= 9 && current < 9 {
        run_migration_step(conn, 9, v09_v18::migrate_v9)?;
    }
    if target >= 10 && current < 10 {
        run_migration_step(conn, 10, v09_v18::migrate_v10)?;
    }
    if target >= 11 && current < 11 {
        run_migration_step(conn, 11, v09_v18::migrate_v11)?;
    }
    if target >= 12 && current < 12 {
        run_migration_step(conn, 12, v09_v18::migrate_v12)?;
    }
    if target >= 13 && current < 13 {
        run_migration_step(conn, 13, v09_v18::migrate_v13)?;
    }
    if target >= 14 && current < 14 {
        run_migration_step(conn, 14, v09_v18::migrate_v14)?;
    }
    // V15 is reserved for Sync 3b (lan_peer_pins)
    if target >= 15 && current < 15 {
        run_migration_step(conn, 15, v09_v18::migrate_v15)?;
    }
    if target >= 16 && current < 16 {
        run_migration_step(conn, 16, v09_v18::migrate_v16)?;
    }
    if target >= 17 && current < 17 {
        run_migration_step(conn, 17, v09_v18::migrate_v17)?;
    }
    if target >= 18 && current < 18 {
        run_migration_step(conn, 18, v09_v18::migrate_v18)?;
    }
    if target >= 19 && current < 19 {
        run_migration_step(conn, 19, v09_v18::migrate_v19)?;
    }
    if target >= 20 && current < 20 {
        run_migration_step(conn, 20, v19_v21::migrate_v20)?;
    }
    if target >= 21 && current < 21 {
        run_migration_step(conn, 21, v19_v21::migrate_v21)?;
    }
    if target >= 22 && current < 22 {
        run_migration_step(conn, 22, v19_v21::migrate_v22)?;
    }
    if target >= 23 && current < 23 {
        run_migration_step(conn, 23, v22_v23::migrate_v23)?;
    }
    if target >= 24 && current < 24 {
        run_migration_step(conn, 24, v23_v24::migrate_v24)?;
    }
    if target >= 25 && current < 25 {
        run_migration_step(conn, 25, v25::migrate_v25)?;
    }
    if target >= 26 && current < 26 {
        run_migration_step(conn, 26, v26::migrate_v26)?;
    }
    if target >= 27 && current < 27 {
        run_migration_step(conn, 27, v27::migrate_v27)?;
    }
    if target >= 28 && current < 28 {
        run_migration_step(conn, 28, v28::migrate_v28)?;
    }
    if target >= 29 && current < 29 {
        run_migration_step(conn, 29, v29::migrate_v29)?;
    }
    if target >= 30 && current < 30 {
        run_migration_step(conn, 30, v30::migrate_v30)?;
    }
    if target >= 31 && current < 31 {
        run_migration_step(conn, 31, v31_regime_manager_state::migrate_v31)?;
    }
    if target >= 32 && current < 32 {
        run_migration_step(conn, 32, v32_audit_log_command_id_index::migrate_v32)?;
    }
    if target >= 33 && current < 33 {
        run_migration_step(conn, 33, v33_suggestion_context_scope::migrate_v33)?;
    }
    if target >= 34 && current < 34 {
        run_migration_step(conn, 34, v34_memory_graph::migrate_v34)?;
    }
    if target >= 35 && current < 35 {
        run_migration_step(conn, 35, v35_memory_edge_unique::migrate_v35)?;
    }
    if target >= 36 && current < 36 {
        run_migration_step(conn, 36, v36_egress_ledger::migrate_v36)?;
    }
    if target >= 37 && current < 37 {
        run_migration_step(conn, 37, v37_audit_log_hash_chain::migrate_v37)?;
    }
    if target >= 38 && current < 38 {
        run_migration_step(conn, 38, v38_sync_tombstones::migrate_v38)?;
    }
    if target >= 39 && current < 39 {
        run_migration_step(conn, 39, v39_hlc_clock::migrate_v39)?;
    }
    if target >= 40 && current < 40 {
        run_migration_step(conn, 40, v40_egress_recipient_count::migrate_v40)?;
    }
    if target >= 41 && current < 41 {
        run_migration_step(conn, 41, v41_cjk_bigram_shadow::migrate_v41)?;
    }
    if target >= 42 && current < 42 {
        run_migration_step(conn, 42, v42_digest_processing_markers::migrate_v42)?;
    }
    if target >= 43 && current < 43 {
        run_migration_step(
            conn,
            43,
            v43_gui_interactions_drop_unused_columns::migrate_v43,
        )?;
    }
    if target >= 44 && current < 44 {
        run_migration_step(conn, 44, v44_learning_persistence::migrate_v44)?;
    }
    if target >= 45 && current < 45 {
        run_migration_step(conn, 45, v45_drop_search_trigram::migrate_v45)?;
    }
    if target >= 46 && current < 46 {
        run_migration_step(conn, 46, v46_adaptive_scorer_state::migrate_v46)?;
    }
    if target >= 47 && current < 47 {
        run_migration_step(conn, 47, v47_transcripts::migrate_v47)?;
    }
    if target >= 48 && current < 48 {
        run_migration_step(conn, 48, v48_pomodoro_state::migrate_v48)?;
    }
    if target >= 49 && current < 49 {
        run_migration_step(conn, 49, v49_durable_tasks::migrate_v49)?;
    }
    if target >= 50 && current < 50 {
        run_migration_step(conn, 50, v50_extension_registry::migrate_v50)?;
    }
    if target >= 51 && current < 51 {
        run_migration_step(conn, 51, v51_work_context::migrate_v51)?;
    }
    if target >= 52 && current < 52 {
        run_migration_step(conn, 52, v52_skill_pack_activation::migrate_v52)?;
    }
    if target >= 53 && current < 53 {
        run_migration_step(conn, 53, v53_vault_mirror_state::migrate_v53)?;
    }
    // Keep every step independently gated by its own version. An install that
    // was already at V53 must still execute the one-time V54 data repair.
    if target >= 54 && current < 54 {
        run_migration_step(conn, 54, v54_orphan_annotation_cleanup::migrate_v54)?;
    }
    if target >= 55 && current < 55 {
        run_migration_step(conn, 55, v55_effective_mapping_cache::migrate_v55)?;
    }
    if target >= 56 && current < 56 {
        run_migration_step(conn, 56, v56_wbs_xlsx_receipts::migrate_v56)?;
    }

    Ok(())
}

fn get_version(conn: &Connection) -> Result<u32, rusqlite::Error> {
    let result: Result<u32, _> = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_version",
        [],
        |row| row.get(0),
    );
    result.or(Ok(0))
}
