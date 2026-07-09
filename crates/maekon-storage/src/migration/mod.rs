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

use rusqlite::Connection;
use tracing::{error, info, warn};

pub(crate) const CURRENT_VERSION: u32 = 44;

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
fn backup_if_needed(conn: &Connection, current_version: u32) -> Option<std::path::PathBuf> {
    if current_version >= CURRENT_VERSION {
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
                "DB backup created before migration v{current_version}→v{CURRENT_VERSION}: {} ({bytes} bytes)",
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
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    let current = get_version(conn)?;
    info!("current schema version: {current}, target: {CURRENT_VERSION}");

    if current > CURRENT_VERSION {
        error!(
            current_schema_version = current,
            supported_schema_version = CURRENT_VERSION,
            "database schema version is newer than this client supports"
        );
        return Err(future_schema_error(current));
    }

    if current < CURRENT_VERSION && backup_if_needed(conn, current).is_none() {
        warn!("proceeding with migration without backup");
    }

    if current < 1 {
        run_migration_step(conn, 1, v01_v08::migrate_v1)?;
    }
    if current < 2 {
        run_migration_step(conn, 2, v01_v08::migrate_v2)?;
    }
    if current < 3 {
        run_migration_step(conn, 3, v01_v08::migrate_v3)?;
    }
    if current < 4 {
        run_migration_step(conn, 4, v01_v08::migrate_v4)?;
    }
    if current < 5 {
        run_migration_step(conn, 5, v01_v08::migrate_v5)?;
    }
    if current < 6 {
        run_migration_step(conn, 6, v01_v08::migrate_v6)?;
    }
    if current < 7 {
        run_migration_step(conn, 7, v01_v08::migrate_v7)?;
    }
    if current < 8 {
        run_migration_step(conn, 8, v01_v08::migrate_v8)?;
    }
    if current < 9 {
        run_migration_step(conn, 9, v09_v18::migrate_v9)?;
    }
    if current < 10 {
        run_migration_step(conn, 10, v09_v18::migrate_v10)?;
    }
    if current < 11 {
        run_migration_step(conn, 11, v09_v18::migrate_v11)?;
    }
    if current < 12 {
        run_migration_step(conn, 12, v09_v18::migrate_v12)?;
    }
    if current < 13 {
        run_migration_step(conn, 13, v09_v18::migrate_v13)?;
    }
    if current < 14 {
        run_migration_step(conn, 14, v09_v18::migrate_v14)?;
    }
    // V15 is reserved for Sync 3b (lan_peer_pins)
    if current < 15 {
        run_migration_step(conn, 15, v09_v18::migrate_v15)?;
    }
    if current < 16 {
        run_migration_step(conn, 16, v09_v18::migrate_v16)?;
    }
    if current < 17 {
        run_migration_step(conn, 17, v09_v18::migrate_v17)?;
    }
    if current < 18 {
        run_migration_step(conn, 18, v09_v18::migrate_v18)?;
    }
    if current < 19 {
        run_migration_step(conn, 19, v09_v18::migrate_v19)?;
    }
    if current < 20 {
        run_migration_step(conn, 20, v19_v21::migrate_v20)?;
    }
    if current < 21 {
        run_migration_step(conn, 21, v19_v21::migrate_v21)?;
    }
    if current < 22 {
        run_migration_step(conn, 22, v19_v21::migrate_v22)?;
    }
    if current < 23 {
        run_migration_step(conn, 23, v22_v23::migrate_v23)?;
    }
    if current < 24 {
        run_migration_step(conn, 24, v23_v24::migrate_v24)?;
    }
    if current < 25 {
        run_migration_step(conn, 25, v25::migrate_v25)?;
    }
    if current < 26 {
        run_migration_step(conn, 26, v26::migrate_v26)?;
    }
    if current < 27 {
        run_migration_step(conn, 27, v27::migrate_v27)?;
    }
    if current < 28 {
        run_migration_step(conn, 28, v28::migrate_v28)?;
    }
    if current < 29 {
        run_migration_step(conn, 29, v29::migrate_v29)?;
    }
    if current < 30 {
        run_migration_step(conn, 30, v30::migrate_v30)?;
    }
    if current < 31 {
        run_migration_step(conn, 31, v31_regime_manager_state::migrate_v31)?;
    }
    if current < 32 {
        run_migration_step(conn, 32, v32_audit_log_command_id_index::migrate_v32)?;
    }
    if current < 33 {
        run_migration_step(conn, 33, v33_suggestion_context_scope::migrate_v33)?;
    }
    if current < 34 {
        run_migration_step(conn, 34, v34_memory_graph::migrate_v34)?;
    }
    if current < 35 {
        run_migration_step(conn, 35, v35_memory_edge_unique::migrate_v35)?;
    }
    if current < 36 {
        run_migration_step(conn, 36, v36_egress_ledger::migrate_v36)?;
    }
    if current < 37 {
        run_migration_step(conn, 37, v37_audit_log_hash_chain::migrate_v37)?;
    }
    if current < 38 {
        run_migration_step(conn, 38, v38_sync_tombstones::migrate_v38)?;
    }
    if current < 39 {
        run_migration_step(conn, 39, v39_hlc_clock::migrate_v39)?;
    }
    if current < 40 {
        run_migration_step(conn, 40, v40_egress_recipient_count::migrate_v40)?;
    }
    if current < 41 {
        run_migration_step(conn, 41, v41_cjk_bigram_shadow::migrate_v41)?;
    }
    if current < 42 {
        run_migration_step(conn, 42, v42_digest_processing_markers::migrate_v42)?;
    }
    if current < 43 {
        run_migration_step(
            conn,
            43,
            v43_gui_interactions_drop_unused_columns::migrate_v43,
        )?;
    }
    if current < 44 {
        run_migration_step(conn, 44, v44_learning_persistence::migrate_v44)?;
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
