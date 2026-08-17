//! Tests for schema migrations.

use super::*;
use rusqlite::Connection;

#[test]
fn upgrade_from_exact_v53_runs_v54_repair_before_v55() {
    let conn = Connection::open_in_memory().unwrap();
    run_migrations_to(&conn, 53).unwrap();
    conn.execute(
        "INSERT INTO frame_annotations
         (annotation_id, frame_id, annotation_type, x, y, text, created_at)
         VALUES ('orphan-after-v53', 999, 'memo', 0.1, 0.1, 'private',
                 '2026-08-15T00:00:00Z')",
        [],
    )
    .unwrap();

    run_migrations_to(&conn, 55).unwrap();

    let orphan_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM frame_annotations WHERE annotation_id = 'orphan-after-v53'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(orphan_count, 0, "V54 repair must not be skipped from V53");
    let versions: Vec<u32> = conn
        .prepare("SELECT version FROM schema_version WHERE version >= 54 ORDER BY version")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(versions, vec![54, 55]);
}

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

    // V36: egress_ledger table (egress audit ledger, #4803).
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='egress_ledger'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "egress_ledger table should exist at CURRENT_VERSION"
    );

    // V37: audit_log hash-chain columns (seq/prev_hash/entry_hash) + partial unique index (#4834).
    let chain_cols: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(audit_log)").unwrap();
        stmt.query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect()
    };
    for c in ["seq", "prev_hash", "entry_hash"] {
        assert!(
            chain_cols.iter().any(|x| x == c),
            "audit_log.{c} chain column should exist at CURRENT_VERSION"
        );
    }
    let idx_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_audit_log_seq'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        idx_count, 1,
        "idx_audit_log_seq partial unique index should exist at CURRENT_VERSION"
    );

    // V38: sync_tombstones retained outbox table + HLC index (#5174/#5178).
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sync_tombstones'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "sync_tombstones table should exist at CURRENT_VERSION"
    );
    let idx_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='index' AND name='idx_sync_tombstones_hlc'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        idx_count, 1,
        "idx_sync_tombstones_hlc index should exist at CURRENT_VERSION"
    );

    // V39: hlc_clock singleton (persistent monotonic HLC floor, #5186).
    let (tbl, rows): (i64, i64) = conn
        .query_row(
            "SELECT \
               (SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='hlc_clock'), \
               (SELECT COUNT(*) FROM hlc_clock)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(tbl, 1, "hlc_clock table should exist at CURRENT_VERSION");
    assert_eq!(rows, 1, "hlc_clock singleton row should be seeded");

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

    // V14 - INT8 quantization column exists
    let has_int8: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('embedding_vectors') WHERE name='vector_int8'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_int8, 1);

    // V14 - sync tables
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

    // V14 - HLC column on activity_segments
    let has_hlc: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('activity_segments') WHERE name='hlc_wall_ms'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(has_hlc, 1);

    // V15 - lan_peer_pins table
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

    // V16 - idx_ivf_assign_cluster index
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

    // V18 created `search_trigram`, but V45 (#8056) drops it as dead schema
    // (superseded by the V41 `search_fts` CJK bigram shadow). At CURRENT_VERSION
    // it must NOT exist.
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='search_trigram'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "search_trigram must be dropped by V45");

    // V19 - app_meta table
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_meta'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);

    // V34 - memory_claims + memory_edges (ADR-023 substrate)
    for table in ["memory_claims", "memory_edges", "digest_processing_markers"] {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "table {table} should exist after migrations");
    }

    // V47 - transcripts table + timestamp index (#8059).
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='transcripts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "transcripts table should exist at CURRENT_VERSION"
    );
    let idx_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='index' AND name='idx_transcripts_timestamp'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        idx_count, 1,
        "idx_transcripts_timestamp index should exist at CURRENT_VERSION"
    );

    // V48 - durable singleton Pomodoro state (#8218).
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='pomodoro_state'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "pomodoro_state table should exist at CURRENT_VERSION"
    );

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

    // Now run migrations -- should create backup since version 0 < CURRENT_VERSION
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
fn backup_includes_uncheckpointed_wal_commits() {
    // Regression (#6823): in WAL mode the pre-migration backup must include
    // commits still resident in the `-wal` file (not yet checkpointed into the
    // main `.db`). Without a WAL checkpoint before the file copy, the backup is
    // a valid but STALE database missing the most recent commits.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("wal.db");

    let conn = Connection::open(&db_path).unwrap();
    // Enable WAL and confirm it is active (file-backed DBs only).
    let mode: String = conn
        .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        mode.to_lowercase(),
        "wal",
        "WAL mode must be active for this regression test"
    );

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        CREATE TABLE canary (id INTEGER PRIMARY KEY, marker TEXT NOT NULL);",
    )
    .unwrap();
    // Commit a row that lives in the WAL and is NOT yet checkpointed into `.db`.
    conn.execute(
        "INSERT INTO canary (id, marker) VALUES (1, 'wal-resident')",
        [],
    )
    .unwrap();

    // Pre-migration backup (version 0 < CURRENT_VERSION).
    let backup_path =
        backup_if_needed(&conn, 0, CURRENT_VERSION).expect("backup should be created");

    // Open the backup file independently and verify the WAL-resident row is
    // present (i.e. the WAL was checkpointed into the `.db` before the copy).
    let backup_conn = Connection::open(&backup_path).unwrap();
    let marker: Result<String, _> =
        backup_conn.query_row("SELECT marker FROM canary WHERE id = 1", [], |row| {
            row.get(0)
        });
    assert_eq!(
        marker.ok().as_deref(),
        Some("wal-resident"),
        "backup must include committed rows still resident in the WAL (checkpoint before copy)"
    );
}

#[test]
fn backup_skipped_for_in_memory_db() {
    let conn = Connection::open_in_memory().unwrap();
    let result = backup_if_needed(&conn, 0, CURRENT_VERSION);
    assert!(result.is_none(), "in-memory DB should not produce backup");
}

#[test]
fn backup_skipped_when_already_current() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("current.db");
    let conn = Connection::open(&db_path).unwrap();
    let result = backup_if_needed(&conn, CURRENT_VERSION, CURRENT_VERSION);
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
    run_migrations(&conn).unwrap(); // running twice must not error
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

#[test]
fn prune_old_backups_keeps_only_the_most_recent() {
    // #6830: with more than MAX_RETAINED_BACKUPS backups present, prune keeps the
    // newest MAX_RETAINED_BACKUPS (by mtime) and removes the rest, leaving the
    // live db and unrelated files untouched.
    use std::time::Duration;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("app.db");
    std::fs::write(&db_path, b"live-db").unwrap();
    // The live WAL/SHM sidecars — deleting these would CORRUPT the database, so
    // they must never match the backup prefix (safety-critical).
    std::fs::write(dir.path().join("app.db-wal"), b"wal").unwrap();
    std::fs::write(dir.path().join("app.db-shm"), b"shm").unwrap();
    // An unrelated file + a sibling-db backup that must NOT be matched.
    std::fs::write(dir.path().join("notes.txt"), b"x").unwrap();
    std::fs::write(dir.path().join("app2.backup.v1.100"), b"sibling").unwrap();

    // Create MAX_RETAINED_BACKUPS + 2 backups with strictly increasing mtimes so
    // ordering is deterministic regardless of filename-vs-mtime.
    let total = MAX_RETAINED_BACKUPS + 2;
    let mut paths = Vec::new();
    for i in 0..total {
        let p = db_path.with_extension(format!("backup.v{i}.{}", 1000 + i));
        std::fs::write(&p, format!("backup-{i}")).unwrap();
        // Stagger mtimes: later i = newer.
        let mtime = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(2000 + i as u64);
        filetime_set(&p, mtime);
        paths.push(p);
    }

    prune_old_backups(&db_path);

    let surviving: Vec<_> = paths.iter().filter(|p| p.exists()).collect();
    assert_eq!(
        surviving.len(),
        MAX_RETAINED_BACKUPS,
        "exactly MAX_RETAINED_BACKUPS backups must survive"
    );
    // The newest MAX_RETAINED_BACKUPS (highest i) are the survivors.
    for (i, p) in paths.iter().enumerate() {
        let expected = i >= total - MAX_RETAINED_BACKUPS;
        assert_eq!(p.exists(), expected, "backup {i} retention mismatch");
    }
    // Untouched: live db + its WAL/SHM sidecars + unrelated + sibling-db backup.
    assert!(db_path.exists(), "live db must not be pruned");
    assert!(
        dir.path().join("app.db-wal").exists(),
        "live -wal sidecar must not be pruned (deleting it corrupts the db)"
    );
    assert!(
        dir.path().join("app.db-shm").exists(),
        "live -shm sidecar must not be pruned"
    );
    assert!(dir.path().join("notes.txt").exists());
    assert!(
        dir.path().join("app2.backup.v1.100").exists(),
        "a sibling db's backups must not be matched by the `app.` prefix"
    );
}

/// Set a file's mtime via a second write + an explicit timestamp. `filetime` is
/// not a dependency, so emulate ordering by writing files in sequence with a
/// real sleep-free monotonic guarantee: we instead set times through the std
/// API available on the platform.
fn filetime_set(path: &std::path::Path, mtime: std::time::SystemTime) {
    // `std::fs` has no portable set-mtime; use a File + set_modified (Rust 1.75+).
    let f = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(mtime).unwrap();
}
