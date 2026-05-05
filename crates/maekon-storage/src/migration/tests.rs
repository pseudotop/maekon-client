//! Tests for schema migrations.

use super::*;
use rusqlite::Connection;

#[test]
fn migration_all_versions() {
    let conn = Connection::open_in_memory().unwrap();
    run_migrations(&conn).unwrap();

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='frames'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let has_file_path: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('frames') WHERE name='file_path'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_file_path, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='system_metrics'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='system_metrics_hourly'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='process_snapshots'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='idle_periods'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='session_stats'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let has_window_x: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('frames') WHERE name='window_x'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_window_x, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='tags'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='frame_tags'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='work_sessions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='interruptions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='focus_metrics'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='local_suggestions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_events_sent_timestamp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_work_sessions_state_started'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let version: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, CURRENT_VERSION);

    // V9 tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='calibration_log'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='trigger_params_snapshots'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='regimes'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='activity_segments'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V10 tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='embedding_vectors'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='weekly_digests'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V11 tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='daily_digests'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // FTS5 virtual table
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='search_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V12 tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='regime_overrides'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V13 tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='gui_interactions'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V14 — INT8 quantization column exists
    let has_int8: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('embedding_vectors') WHERE name='vector_int8'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_int8, 1);

    // V14 — sync tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sync_peers'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='device_identity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V14 — HLC column on activity_segments
    let has_hlc: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('activity_segments') WHERE name='hlc_wall_ms'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_hlc, 1);

    // V15 — lan_peer_pins table
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='lan_peer_pins'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V16 tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='vector_binary_codes'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ivf_centroids'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='ivf_assignments'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='vector_index_meta'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V16 — idx_ivf_assign_cluster index
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_ivf_assign_cluster'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Final version check
    let version: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, CURRENT_VERSION);

    // V17 tables
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='coaching_events'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='regime_goals'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='coaching_effectiveness'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V17 indexes
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_coaching_events_profile'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V18 — trigram FTS5 table
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='search_trigram'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V19 — app_meta table
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_meta'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // Final version check
    let version: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, CURRENT_VERSION);
}

#[test]
fn backup_created_when_migration_needed() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    // Create DB at version 0 (just the schema_version table)
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )
    .unwrap();
    conn.close().unwrap();

    // Now run migrations — should create backup since version 0 < CURRENT_VERSION
    let conn = Connection::open(&db_path).unwrap();
    run_migrations(&conn).unwrap();
    conn.close().unwrap();

    let backup_files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("backup"))
        .collect();
    assert!(
        !backup_files.is_empty(),
        "backup file should be created when migration runs"
    );
}

#[test]
fn backup_skipped_for_in_memory_db() {
    let conn = Connection::open_in_memory().unwrap();
    let result = backup_if_needed(&conn, 0);
    assert!(result.is_none(), "in-memory DB should not produce backup");
}

#[test]
fn backup_skipped_when_already_current() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("current.db");
    let conn = Connection::open(&db_path).unwrap();
    let result = backup_if_needed(&conn, CURRENT_VERSION);
    assert!(
        result.is_none(),
        "no backup needed when already at current version"
    );
    conn.close().unwrap();
}

#[test]
fn migration_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    run_migrations(&conn).unwrap();
    run_migrations(&conn).unwrap(); // execution error none
    let version: u32 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, CURRENT_VERSION);
}

#[test]
fn migration_rejects_future_schema_version() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(&format!(
        "CREATE TABLE schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO schema_version (version) VALUES ({});",
        CURRENT_VERSION + 1
    ))
    .unwrap();

    let err = run_migrations(&conn).expect_err("future schema version must be rejected");
    let message = err.to_string();
    assert!(
        message.contains("newer than this client supports"),
        "error should explain the version mismatch, got: {message}"
    );
}
