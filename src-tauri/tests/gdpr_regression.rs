//! GDPR regression tests — transactional deletion, rollback, FTS5 coverage.
//!
//! Uses `SqliteStorage::open_in_memory(30)` to run against a fully-migrated
//! in-memory database without any file I/O.

use oneshim_storage::sqlite::SqliteStorage;

/// Helper: insert sample data into core tables so we can verify deletion.
fn seed_sample_data(storage: &SqliteStorage) {
    let conn = storage.connection_arc();
    let guard = conn.lock().expect("lock");

    // V1 tables
    guard
        .execute(
            "INSERT INTO events (event_id, event_type, timestamp, data) \
             VALUES ('e1', 'context', '2026-01-01T00:00:00Z', '{}')",
            [],
        )
        .expect("insert event");
    guard
        .execute(
            "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, \
             importance, resolution_w, resolution_h, has_image) \
             VALUES ('2026-01-01T00:00:00Z', 'manual', 'App', 'Win', 0.5, 1920, 1080, 0)",
            [],
        )
        .expect("insert frame");
    guard
        .execute(
            "INSERT INTO system_metrics (timestamp, cpu_usage, memory_used, memory_total, \
             disk_used, disk_total) \
             VALUES ('2026-01-01T00:00:00Z', 25.0, 4096, 16384, 100000, 500000)",
            [],
        )
        .expect("insert metric");
    guard
        .execute(
            "INSERT INTO process_snapshots (timestamp, snapshot_data) \
             VALUES ('2026-01-01T00:00:00Z', '[]')",
            [],
        )
        .expect("insert process snapshot");
    guard
        .execute(
            "INSERT INTO idle_periods (start_time, end_time, duration_secs) \
             VALUES ('2026-01-01T00:00:00Z', '2026-01-01T00:05:00Z', 300)",
            [],
        )
        .expect("insert idle period");
    guard
        .execute(
            "INSERT INTO tags (name, color, created_at) \
             VALUES ('test-tag', '#ff0000', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("insert tag");

    // V8-V10 tables
    guard
        .execute(
            "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, \
             trigger_reason, dominant_category) \
             VALUES ('seg1', '2026-01-01T00:00:00Z', '2026-01-01T00:30:00Z', 1800, \
             'timer', 'coding')",
            [],
        )
        .expect("insert segment");
    guard
        .execute(
            "INSERT INTO regimes (id, label, detected_at, last_seen_at, dominant_category) \
             VALUES ('r1', 'focus', '2026-01-01T00:00:00Z', '2026-01-01T00:30:00Z', 'coding')",
            [],
        )
        .expect("insert regime");

    // V11: FTS5 virtual table (columns: segment_id, content_type, searchable_text)
    guard
        .execute(
            "INSERT INTO search_fts (segment_id, content_type, searchable_text) \
             VALUES ('seg1', 'segment', 'important meeting notes about quarterly review')",
            [],
        )
        .expect("insert FTS5 row");

    // V13: GUI interactions
    guard
        .execute(
            "INSERT INTO gui_interactions (event_id, segment_id, timestamp, interaction_type, app_name) \
             VALUES ('gui1', 'seg1', '2026-01-01T00:00:00Z', 'click', 'Firefox')",
            [],
        )
        .expect("insert gui interaction");

    // V17: coaching tables
    guard
        .execute(
            "INSERT INTO coaching_events (event_id, trigger_type, profile_name, \
             message_template, shown_at, regime_id) \
             VALUES ('ce1', 'break_reminder', 'default', \
             'Take a break!', '2026-01-01T00:30:00Z', 'r1')",
            [],
        )
        .expect("insert coaching event");
}

/// Helper: count rows in a table.
fn count_rows(storage: &SqliteStorage, table: &str) -> u64 {
    let conn = storage.connection_arc();
    let guard = conn.lock().expect("lock");
    guard
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0) as u64
}

// ---------------------------------------------------------------------------
// Test 1: delete_all_data clears all tables
// ---------------------------------------------------------------------------
#[test]
fn delete_all_data_clears_all_tables() {
    let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
    seed_sample_data(&storage);

    // Verify data was seeded
    assert!(
        count_rows(&storage, "events") > 0,
        "events should be seeded"
    );
    assert!(
        count_rows(&storage, "frames") > 0,
        "frames should be seeded"
    );
    assert!(
        count_rows(&storage, "system_metrics") > 0,
        "metrics should be seeded"
    );
    assert!(count_rows(&storage, "tags") > 0, "tags should be seeded");
    assert!(
        count_rows(&storage, "activity_segments") > 0,
        "segments should be seeded"
    );
    assert!(
        count_rows(&storage, "search_fts") > 0,
        "FTS5 should be seeded"
    );
    assert!(
        count_rows(&storage, "gui_interactions") > 0,
        "gui_interactions should be seeded"
    );
    assert!(
        count_rows(&storage, "coaching_events") > 0,
        "coaching_events should be seeded"
    );

    // Execute GDPR deletion
    storage.delete_all_data().expect("delete_all_data");

    // ALL known tables must be empty after deletion
    let tables_to_check = [
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
        "activity_segments",
        "calibration_log",
        "daily_digests",
        "weekly_digests",
        "embedding_vectors",
        "regime_overrides",
        "regimes",
        "trigger_params_snapshots",
        "search_fts",
        "vector_binary_codes",
        "vector_index_meta",
        "ivf_centroids",
        "ivf_assignments",
        "gui_interactions",
        "device_identity",
        "sync_peers",
        "lan_peer_pins",
        "coaching_events",
        "regime_goals",
        "coaching_effectiveness",
    ];

    for table in tables_to_check {
        assert_eq!(
            count_rows(&storage, table),
            0,
            "table '{table}' should be empty after delete_all_data"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2: Transaction rollback on simulated failure
// ---------------------------------------------------------------------------
#[test]
fn transaction_rollback_preserves_data_on_failure() {
    let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
    seed_sample_data(&storage);

    let events_before = count_rows(&storage, "events");
    let frames_before = count_rows(&storage, "frames");
    assert!(events_before > 0);
    assert!(frames_before > 0);

    // Simulate a transaction that partially deletes then fails.
    // We do this by directly using the connection to show that a rolled-back
    // transaction preserves all data.
    {
        let conn = storage.connection_arc();
        let mut guard = conn.lock().expect("lock");
        let tx = guard.transaction().expect("begin tx");

        // Delete events (succeeds)
        tx.execute("DELETE FROM events", [])
            .expect("delete events in tx");

        // Verify events are deleted within the transaction
        let in_tx_count: i64 = tx
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(in_tx_count, 0, "events deleted within transaction");

        // Drop the transaction without committing — this triggers auto-rollback
        drop(tx);
    }

    // After rollback, data should be intact
    assert_eq!(
        count_rows(&storage, "events"),
        events_before,
        "events should be restored after rollback"
    );
    assert_eq!(
        count_rows(&storage, "frames"),
        frames_before,
        "frames should be intact after rollback"
    );
}

// ---------------------------------------------------------------------------
// Test 3: FTS5 table cleared within transaction
// ---------------------------------------------------------------------------
#[test]
fn fts5_table_cleared_within_transaction() {
    let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");

    // Insert multiple FTS5 rows
    {
        let conn = storage.connection_arc();
        let guard = conn.lock().expect("lock");
        for i in 0..5 {
            guard
                .execute(
                    &format!(
                        "INSERT INTO search_fts (segment_id, content_type, searchable_text) \
                         VALUES ('seg{i}', 'segment', 'searchable content for segment number {i}')"
                    ),
                    [],
                )
                .expect("insert FTS5 row");
        }
    }

    assert_eq!(
        count_rows(&storage, "search_fts"),
        5,
        "FTS5 should have 5 rows"
    );

    // Verify FTS5 search works before deletion
    {
        let conn = storage.connection_arc();
        let guard = conn.lock().expect("lock");
        let fts_count: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'searchable'",
                [],
                |row| row.get(0),
            )
            .expect("FTS5 MATCH query");
        assert_eq!(fts_count, 5, "FTS5 MATCH should find all 5 rows");
    }

    // Execute GDPR deletion — FTS5 must be included in transaction
    storage.delete_all_data().expect("delete_all_data");

    // FTS5 table should be empty
    assert_eq!(
        count_rows(&storage, "search_fts"),
        0,
        "FTS5 table should be empty after GDPR deletion"
    );

    // FTS5 MATCH query should return 0 results
    {
        let conn = storage.connection_arc();
        let guard = conn.lock().expect("lock");
        let fts_count: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'searchable'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        assert_eq!(
            fts_count, 0,
            "FTS5 MATCH should return 0 after GDPR deletion"
        );
    }
}
