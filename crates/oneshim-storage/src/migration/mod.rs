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

use rusqlite::Connection;
use tracing::{error, info, warn};

pub(crate) const CURRENT_VERSION: u32 = 32;

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

    match std::fs::copy(&db_path, &backup_path) {
        Ok(bytes) => {
            info!(
                "DB backup created before migration v{current_version}→v{CURRENT_VERSION}: {} ({bytes} bytes)",
                backup_path.display()
            );
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
