//! GDPR regression tests — transactional deletion, rollback, FTS5 coverage.
//!
//! Uses `SqliteStorage::open_in_memory(30)` to run against a fully-migrated
//! in-memory database without any file I/O.

use maekon_storage::sqlite::SqliteStorage;

/// Helper: insert sample data into core tables so we can verify deletion.
fn seed_sample_data(storage: &SqliteStorage) {
    let conn = storage.connection_arc();
    let guard = conn.retained_write_lock();

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

    // V34: ADR-023 memory-graph (durable activity-derived claims + evidence edges).
    guard
        .execute(
            "INSERT INTO memory_claims \
             (claim_id, kind, text, source, confidence, status, created_at, updated_at) \
             VALUES ('clm1', 'reflective', 'note', 'digest_highlight', 0.8, 'active', \
             1700000000, 1700000000)",
            [],
        )
        .expect("insert memory claim");
    guard
        .execute(
            "INSERT INTO memory_edges \
             (edge_id, src_id, dst_id, edge_type, confidence, evidence_ref, source, created_at) \
             VALUES ('edg1', 'clm1', 'seg1', 'evidence', 1.0, 'seg1', 'rule', 1700000000)",
            [],
        )
        .expect("insert memory edge");

    // #4478: V18-V31 user-data tables (audit_log/session_audit_log excluded).
    guard
        .execute(
            "INSERT INTO ai_sessions (session_id, provider, transport) \
             VALUES ('ai-sess-1', 'anthropic', 'http')",
            [],
        )
        .expect("insert ai session");
    guard
        .execute(
            "INSERT INTO ai_conversation_messages (session_id, role, content, seq) \
             VALUES ('ai-sess-1', 'user', 'secret prompt content', 0)",
            [],
        )
        .expect("insert ai message");
    guard
        .execute(
            "INSERT INTO frame_annotations \
             (annotation_id, frame_id, annotation_type, x, y, created_at) \
             VALUES ('ann-1', 1, 'note', 0.5, 0.5, '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("insert frame annotation");
    guard
        .execute(
            "INSERT INTO habit_streaks (regime_label, date, target_minutes) \
             VALUES ('Deep Focus', '2026-01-01', 120)",
            [],
        )
        .expect("insert habit streak");
    guard
        .execute(
            "INSERT INTO regime_manager_state (id, payload) VALUES (0, '{}')",
            [],
        )
        .expect("insert regime manager state");
    guard
        .execute(
            "INSERT INTO automation_presets (id, name, steps_json, created_at, updated_at) \
             VALUES ('preset-1', 'p', '[]', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("insert automation preset");
    guard
        .execute(
            "INSERT INTO feedback_retries (suggestion_id, feedback_type, next_retry_at) \
             VALUES ('sug-1', 'accept', '2026-01-01T00:00:00Z')",
            [],
        )
        .expect("insert feedback retry");
}

/// Helper: count rows in a table.
fn count_rows(storage: &SqliteStorage, table: &str) -> u64 {
    let conn = storage.connection_arc();
    let guard = conn.retained_write_lock();
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
    assert!(
        count_rows(&storage, "memory_claims") > 0,
        "memory_claims should be seeded"
    );
    assert!(
        count_rows(&storage, "memory_edges") > 0,
        "memory_edges should be seeded"
    );
    // #4478 user-data tables (sample — the rest are covered by tables_to_check).
    assert!(
        count_rows(&storage, "ai_conversation_messages") > 0,
        "ai_conversation_messages should be seeded"
    );
    assert!(
        count_rows(&storage, "frame_annotations") > 0,
        "frame_annotations should be seeded"
    );
    assert!(
        count_rows(&storage, "regime_manager_state") > 0,
        "regime_manager_state should be seeded"
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
        "search_trigram",
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
        // #4478: V18-V31 user-data tables (audit_log/session_audit_log excluded
        // pending a SOC2 retention decision; app_meta/schema_version are metadata).
        "ai_conversation_messages",
        "ai_sessions",
        "frame_annotations",
        "habit_streaks",
        "regime_manager_state",
        "automation_presets",
        "feedback_retries",
        // V34: ADR-023 memory-graph
        "memory_claims",
        "memory_edges",
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
        let mut guard = conn.retained_write_lock();
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
        let guard = conn.retained_write_lock();
        for i in 0..5 {
            guard
                .execute(
                    &format!(
                        // V41 schema: searchable_text is UNINDEXED; MATCH queries run against
                        // the `shadow` column. For ASCII text, cjk_bigram_shadow is a
                        // pass-through, so shadow == searchable_text (V41 parity).
                        "INSERT INTO search_fts (segment_id, content_type, searchable_text, shadow) \
                         VALUES ('seg{i}', 'segment', \
                                 'searchable content for segment number {i}', \
                                 'searchable content for segment number {i}')"
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
        let guard = conn.retained_write_lock();
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
        let guard = conn.retained_write_lock();
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

// ---------------------------------------------------------------------------
// Test: FTS5 SHADOW index segments are purged (no residual tokenized PII)
// ---------------------------------------------------------------------------
#[test]
fn fts5_shadow_index_purged_after_erasure() {
    // #4478 G2: `DELETE FROM search_fts` clears the `*_content` backing table (the
    // raw OCR/window-title text) but leaves tombstoned term postings — tokenized
    // user content — in the `*_data` index segments. The post-commit rebuild in
    // delete_all_data must compact `*_data` back to the empty-fresh baseline.
    let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
    {
        let conn = storage.connection_arc();
        let guard = conn.retained_write_lock();
        for i in 0..6 {
            guard
                .execute(
                    &format!(
                        "INSERT INTO search_fts (segment_id, content_type, searchable_text) \
                         VALUES ('seg{i}', 'segment', 'zsecretpii sensitive token {i}')"
                    ),
                    [],
                )
                .expect("seed search_fts");
            guard
                .execute(
                    &format!(
                        "INSERT INTO search_trigram (segment_id, content) \
                         VALUES ('seg{i}', 'zsecretpii sensitive token {i}')"
                    ),
                    [],
                )
                .expect("seed search_trigram");
        }
    }

    // Empty-fresh baseline for the `*_data` backing tables.
    let fresh = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
    let fresh_fts_data = count_rows(&fresh, "search_fts_data");
    let fresh_trigram_data = count_rows(&fresh, "search_trigram_data");

    // Sanity: indexing grew the `*_data` segments past the empty baseline.
    assert!(
        count_rows(&storage, "search_fts_data") > fresh_fts_data,
        "indexing should grow search_fts_data above the empty baseline"
    );

    storage.delete_all_data().expect("delete_all_data");

    // Raw content gone + logical rows gone + no MATCH.
    assert_eq!(count_rows(&storage, "search_fts"), 0);
    assert_eq!(
        count_rows(&storage, "search_fts_content"),
        0,
        "raw FTS text (*_content) must be purged"
    );
    {
        let conn = storage.connection_arc();
        let guard = conn.retained_write_lock();
        let m: i64 = guard
            .query_row(
                "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'zsecretpii'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        assert_eq!(m, 0, "no MATCH after erasure");
    }

    // `*_data` index segments compacted back to the empty-fresh baseline — no
    // residual tokenized term postings (this is what the post-commit rebuild fixes;
    // without it, the count stays inflated by the DELETE tombstones).
    assert!(
        count_rows(&storage, "search_fts_data") <= fresh_fts_data,
        "search_fts_data must be purged to the empty baseline (no residual postings)"
    );
    assert!(
        count_rows(&storage, "search_trigram_data") <= fresh_trigram_data,
        "search_trigram_data must be purged to the empty baseline"
    );
}

// ---------------------------------------------------------------------------
// #4834 + #4928 interaction: audit_log SHA-256 hash chain survives erasure
// ---------------------------------------------------------------------------
//
// `delete_all_data` intentionally excludes `audit_log` from ALL_TABLES (it is a
// retained table), so the audit chain must remain preserved and intact even
// after a full GDPR Art.17 erasure. The consent_revoked audit happens at erase
// time and the chain extends during/after the erase.
#[test]
fn audit_chain_survives_gdpr_delete_all_data() {
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

    let storage = SqliteStorage::open_in_memory(30).expect("in-memory sqlite");
    seed_sample_data(&storage);

    // Record 3 audit entries before erasure (forming a chain).
    for i in 0..3 {
        storage.save_audit_entry(&AuditEntry {
            entry_id: format!("erase-audit-{i}"),
            timestamp: chrono::Utc::now(),
            session_id: "s".to_string(),
            command_id: "c".to_string(),
            action_type: "a".to_string(),
            status: AuditStatus::Completed,
            details: Some("pre-erase".to_string()),
            execution_time_ms: Some(1),
        });
    }
    let before = storage.verify_audit_chain();
    assert!(before.ok, "pre-erase chain must be valid: {before:?}");
    assert_eq!(before.verified_count, 3);

    // Full GDPR erasure.
    storage.delete_all_data().expect("delete_all_data");

    // audit_log is retained, so the chain must survive and remain intact.
    let after = storage.verify_audit_chain();
    assert!(
        after.ok,
        "audit chain must survive GDPR erase intact (retained table): {after:?}"
    );
    assert_eq!(
        after.verified_count, 3,
        "audit_log rows are untouched by delete_all_data, so the chain is preserved"
    );

    // Appending a new audit entry after erasure must keep extending the chain.
    storage.save_audit_entry(&AuditEntry {
        entry_id: "post-erase".to_string(),
        timestamp: chrono::Utc::now(),
        session_id: "s".to_string(),
        command_id: "c".to_string(),
        action_type: "a".to_string(),
        status: AuditStatus::Completed,
        details: Some("post-erase append".to_string()),
        execution_time_ms: Some(1),
    });
    let extended = storage.verify_audit_chain();
    assert!(
        extended.ok,
        "post-erase append must keep chain valid: {extended:?}"
    );
    assert_eq!(
        extended.verified_count, 4,
        "the chain still extends after erase"
    );
}
