// OOS-TBD: ADR-013 file split — baselined past the 900-line giant
// threshold while growing for #9700; split per ADR-003 when next touched.
// ADR-013: maintenance module split (was 1533 lines)
// Responsibilities:
//   backup.rs    — backup upsert/list helpers (tags, frames, events)
//   stats.rs     — storage statistics and frame file path queries
//   retention.rs — time-range and full-table data deletion (GDPR)
//   export.rs    — data export queries (events, metrics, frames, search)
//   vacuum.rs    — SQLite VACUUM, WAL checkpoint, FTS5, ANALYZE

mod app_deletion;
mod backup;
mod export;
mod frame_dependents;
mod retention;
mod stats;
mod vacuum;

#[cfg(test)]
mod work_context_erasure_tests;

#[cfg(test)]
mod tests {
    use super::super::*;
    use maekon_core::types::TimeWindow;

    // ── Helper: insert test data via sync upsert methods ────────────

    fn insert_events(storage: &SqliteStorage, timestamps: &[&str]) {
        for (i, ts) in timestamps.iter().enumerate() {
            storage
                .upsert_backup_event(
                    &format!("evt-{i}"),
                    "WindowChange",
                    ts,
                    Some("Code"),
                    Some("test.rs"),
                )
                .unwrap();
        }
    }

    fn insert_frame(storage: &SqliteStorage, id: i64, timestamp: &str) {
        storage
            .upsert_backup_frame(
                id, timestamp, "manual", "Code", "main.rs", 0.5, 1920, 1080, None,
            )
            .unwrap();
    }

    fn seed_frame_dependents(storage: &SqliteStorage, frame_id: i64) {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO tags (id, name, color, created_at) VALUES (?1, ?2, '#3b82f6', '2025-06-01T00:00:00Z')",
            rusqlite::params![frame_id, format!("tag-{frame_id}")],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO frame_tags (frame_id, tag_id, created_at) VALUES (?1, ?1, '2025-06-01T00:00:00Z')",
            rusqlite::params![frame_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO frame_annotations (annotation_id, frame_id, annotation_type, x, y, text, created_at)
             VALUES (?1, ?2, 'memo', 0.1, 0.1, 'private note', '2025-06-01T00:00:00Z')",
            rusqlite::params![format!("ann-{frame_id}"), frame_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO interruptions (interrupted_at, from_app, from_category, to_app, to_category, snapshot_frame_id)
             VALUES ('2025-06-01T10:00:00Z', 'Code', 'dev', 'Slack', 'chat', ?1)",
            rusqlite::params![frame_id],
        )
        .unwrap();
    }

    fn count_rows(storage: &SqliteStorage, sql: &str) -> i64 {
        let conn = storage.conn.test_lock();
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    /// #9721: `frame_annotations` declares no FK (v30.rs:9), so SQLite's engine
    /// cannot remove it with its frame — and the rows carry the user's own memo
    /// text on a path whose whole purpose is deletion.
    ///
    /// `frame_tags` (CASCADE) and `interruptions` (SET NULL) are asserted too,
    /// but the engine already handles those: they are a scoping check, not a
    /// regression guard for this fix. See #9735.
    #[test]
    fn deleting_frames_in_a_range_removes_the_annotations_the_fk_engine_cannot() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_frame(&storage, 1, "2025-06-01T10:00:00Z");
        seed_frame_dependents(&storage, 1);
        // A frame OUTSIDE the window, with its own dependents. Without this the
        // assertions below pass even if the cleanup dropped its WHERE clause and
        // wiped the tables wholesale.
        insert_frame(&storage, 2, "2025-07-01T10:00:00Z");
        seed_frame_dependents(&storage, 2);

        let window =
            TimeWindow::from_rfc3339_pair("2025-06-01T00:00:00Z", "2025-06-02T00:00:00Z").unwrap();
        let counts = storage
            .delete_data_in_range(&window, false, true, false, false, false)
            .unwrap();
        assert_eq!(counts.frames_deleted, 1);

        // The deleted frame's dependents are gone.
        assert_eq!(
            count_rows(
                &storage,
                "SELECT COUNT(*) FROM frame_annotations WHERE frame_id = 1"
            ),
            0,
            "the user's memo must not outlive the deletion that removed its frame"
        );
        assert_eq!(
            count_rows(
                &storage,
                "SELECT COUNT(*) FROM frame_tags WHERE frame_id = 1"
            ),
            0,
            "CASCADE (SQLite's own): the relation goes with its frame"
        );
        assert_eq!(
            count_rows(
                &storage,
                "SELECT COUNT(*) FROM interruptions WHERE snapshot_frame_id = 1"
            ),
            0,
            "SET NULL (SQLite's own): the snapshot reference is cleared"
        );

        // The surviving frame keeps everything — this is what proves the
        // cleanup is SCOPED rather than a table-wide wipe.
        assert_eq!(
            count_rows(
                &storage,
                "SELECT COUNT(*) FROM frame_annotations WHERE frame_id = 2"
            ),
            1,
            "an out-of-range frame's annotation must survive"
        );
        assert_eq!(
            count_rows(
                &storage,
                "SELECT COUNT(*) FROM frame_tags WHERE frame_id = 2"
            ),
            1,
            "an out-of-range frame's relation must survive"
        );
        assert_eq!(
            count_rows(
                &storage,
                "SELECT COUNT(*) FROM interruptions WHERE snapshot_frame_id = 2"
            ),
            1,
            "an out-of-range frame's snapshot reference must survive"
        );

        assert_eq!(
            count_rows(&storage, "SELECT COUNT(*) FROM interruptions"),
            2,
            "SET NULL must not delete the interruption rows themselves"
        );
        assert_eq!(
            count_rows(&storage, "SELECT COUNT(*) FROM tags"),
            2,
            "tags are not frame dependents"
        );
    }

    fn insert_metric(storage: &SqliteStorage, timestamp: &str) {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO system_metrics (timestamp, cpu_usage, memory_used, memory_total, disk_used, disk_total, network_upload, network_download)
             VALUES (?1, 45.5, 8589934592, 17179869184, 107374182400, 536870912000, 1000, 5000)",
            rusqlite::params![timestamp],
        )
        .unwrap();
    }

    // ── maybe_vacuum ────────────────────────────────────────────────

    #[test]
    fn maybe_vacuum_fresh_db_returns_false() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let vacuumed = storage.maybe_vacuum(10).unwrap();
        assert!(
            !vacuumed,
            "fresh DB has no freelist pages, should skip VACUUM"
        );
    }

    #[test]
    fn maybe_vacuum_after_bulk_delete() {
        // In-memory databases may not accumulate freelist pages the same way
        // as disk databases, so we just verify the method runs without error.
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vacuum_test.db");
        let storage = SqliteStorage::open(&db_path, 30, None).unwrap();

        // Insert and delete bulk data to create freelist pages.
        for i in 0..500 {
            storage
                .upsert_backup_event(
                    &format!("bulk-{i}"),
                    "WindowChange",
                    "2025-06-01T00:00:00Z",
                    Some("App"),
                    Some("Title"),
                )
                .unwrap();
        }
        storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap();

        // With threshold 0, any freelist pages will trigger VACUUM.
        // Contract: returns true when VACUUM actually ran (freelist > 0 after bulk delete).
        let vacuumed = storage
            .maybe_vacuum(0)
            .expect("maybe_vacuum on disk DB must not error (#5594)");
        assert!(
            vacuumed,
            "threshold=0 after bulk insert+delete must produce freelist pages, triggering VACUUM"
        );
    }

    // ── wal_checkpoint_passive ──────────────────────────────────────

    #[test]
    fn wal_checkpoint_passive_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // WAL checkpoint on an in-memory DB is a no-op PRAGMA that must not error.
        // In-memory DBs use journal_mode=memory, so there is no WAL file to checkpoint;
        // Ok-only IS the whole observable contract here (#5594).
        storage
            .wal_checkpoint_passive()
            .expect("wal_checkpoint_passive must not error on fresh in-memory DB (#5594)");
    }

    // ── wal_checkpoint_truncate ─────────────────────────────────────

    #[test]
    fn wal_checkpoint_truncate_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // TRUNCATE checkpoint on in-memory DB: no WAL file exists, PRAGMA is a no-op.
        // Ok-only IS the whole observable contract here (#5594).
        storage
            .wal_checkpoint_truncate()
            .expect("wal_checkpoint_truncate must not error on fresh in-memory DB (#5594)");
    }

    // ── run_analyze ─────────────────────────────────────────────────

    #[test]
    fn run_analyze_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // ANALYZE on an empty DB updates sqlite_stat tables but produces no user-visible row.
        // The only cheaply observable contract is that the call completes without error (#5594).
        storage
            .run_analyze()
            .expect("run_analyze must not error on fresh in-memory DB (#5594)");
    }

    #[test]
    fn run_analyze_with_conn_on_fresh_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let conn = storage.conn.test_lock();
        // ANALYZE via the pre-held connection guard: same contract as run_analyze —
        // no cheap observable row effect on an empty DB; Ok-only is the full contract (#5594).
        SqliteStorage::run_analyze_with_conn(&conn)
            .expect("run_analyze_with_conn must not error on fresh in-memory DB (#5594)");
    }

    // ── get_storage_stats_summary ───────────────────────────────────

    #[test]
    fn stats_summary_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let stats = storage.get_storage_stats_summary().unwrap();

        assert_eq!(stats.event_count, 0);
        assert_eq!(stats.frame_count, 0);
        assert_eq!(stats.metric_count, 0);
        assert!(stats.oldest_data_date.is_none());
        assert!(stats.newest_data_date.is_none());
        assert!(stats.page_size > 0, "page_size should be positive");
    }

    #[test]
    fn stats_summary_after_inserts() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(&storage, &["2025-06-01T10:00:00Z", "2025-06-02T12:00:00Z"]);
        insert_frame(&storage, 1, "2025-06-01T11:00:00Z");
        insert_metric(&storage, "2025-06-03T08:00:00Z");

        let stats = storage.get_storage_stats_summary().unwrap();
        assert_eq!(stats.event_count, 2);
        assert_eq!(stats.frame_count, 1);
        assert_eq!(stats.metric_count, 1);
        assert!(stats.oldest_data_date.is_some());
        assert!(stats.newest_data_date.is_some());
    }

    // ── delete_data_in_range ────────────────────────────────────────

    #[test]
    fn delete_range_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let counts = storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                true,
                true,
                true,
                true,
            )
            .unwrap();

        assert_eq!(counts.events_deleted, 0);
        assert_eq!(counts.frames_deleted, 0);
        assert_eq!(counts.metrics_deleted, 0);
        assert_eq!(counts.process_snapshots_deleted, 0);
        assert_eq!(counts.idle_periods_deleted, 0);
    }

    #[test]
    fn delete_range_removes_matching_events() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(
            &storage,
            &[
                "2025-06-01T10:00:00Z",
                "2025-06-15T10:00:00Z",
                "2025-07-01T10:00:00Z",
            ],
        );

        // Delete only June events
        let counts = storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap();

        assert_eq!(counts.events_deleted, 2);

        // July event should remain — verify via count_events_in_range
        let remaining = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[test]
    fn delete_range_selective_flags() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let ts = "2025-06-15T10:00:00Z";

        insert_events(&storage, &[ts]);
        insert_frame(&storage, 1, ts);
        insert_metric(&storage, ts);

        // Delete only events, not frames or metrics
        let counts = storage
            .delete_data_in_range(
                &TimeWindow::from_rfc3339_pair("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
                    .expect("trusted test bounds"),
                true,
                false,
                false,
                false,
                false,
            )
            .unwrap();

        assert_eq!(counts.events_deleted, 1);
        assert_eq!(counts.frames_deleted, 0);

        // Frames and metrics should still exist
        let frames = storage
            .list_frame_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert_eq!(frames.len(), 1);

        let metrics = storage
            .list_metric_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert_eq!(metrics.len(), 1);
    }

    // ── delete_all_data ─────────────────────────────────────────────

    #[test]
    fn delete_all_data_clears_everything() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(&storage, &["2025-06-01T10:00:00Z", "2025-06-02T10:00:00Z"]);
        insert_frame(&storage, 1, "2025-06-01T11:00:00Z");
        insert_metric(&storage, "2025-06-01T12:00:00Z");

        storage.delete_all_data().unwrap();

        let stats = storage.get_storage_stats_summary().unwrap();
        assert_eq!(stats.event_count, 0);
        assert_eq!(stats.frame_count, 0);
        assert_eq!(stats.metric_count, 0);
    }

    #[test]
    fn delete_all_data_on_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // Line 308: delete_all_data on an already-empty DB returns Ok(()) — no error.
        storage
            .delete_all_data()
            .expect("delete_all_data should succeed on empty DB");
        // Read-back: counts must all be zero after deleting from an already-empty DB.
        let stats = storage.get_storage_stats_summary().unwrap();
        assert_eq!(
            stats.event_count, 0,
            "event_count must be 0 after delete_all_data on empty DB"
        );
        assert_eq!(
            stats.frame_count, 0,
            "frame_count must be 0 after delete_all_data on empty DB"
        );
        assert_eq!(
            stats.metric_count, 0,
            "metric_count must be 0 after delete_all_data on empty DB"
        );
    }

    // ── list_event_exports ──────────────────────────────────────────

    #[test]
    fn list_event_exports_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let result = storage
            .list_event_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_event_exports_extracts_json_fields() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_events(&storage, &["2025-06-15T10:00:00Z"]);

        let records = storage
            .list_event_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].app_name.as_deref(), Some("Code"));
        assert_eq!(records[0].window_title.as_deref(), Some("test.rs"));
    }

    // ── count_events_in_range (exercised as event-query alternative) ─

    #[test]
    fn count_events_in_range_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let count = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn count_events_in_range_filters_by_range() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_events(
            &storage,
            &[
                "2025-03-01T10:00:00Z",
                "2025-06-15T10:00:00Z",
                "2025-09-01T10:00:00Z",
            ],
        );

        let count = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    // ── list_metric_exports ─────────────────────────────────────────

    #[test]
    fn list_metric_exports_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let exports = storage
            .list_metric_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert!(exports.is_empty());
    }

    #[test]
    fn list_metric_exports_filters_by_range() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_metric(&storage, "2025-03-01T10:00:00Z");
        insert_metric(&storage, "2025-06-15T10:00:00Z");

        let exports = storage
            .list_metric_exports("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
            .unwrap();
        assert_eq!(exports.len(), 1);
        assert!(exports[0].cpu_usage > 40.0);
    }

    // ── list_frame_exports ──────────────────────────────────────────

    #[test]
    fn list_frame_exports_empty() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let exports = storage
            .list_frame_exports("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
            .unwrap();
        assert!(exports.is_empty());
    }

    #[test]
    fn list_frame_exports_filters_by_range() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        insert_frame(&storage, 1, "2025-03-01T10:00:00Z");
        insert_frame(&storage, 2, "2025-06-15T10:00:00Z");
        insert_frame(&storage, 3, "2025-09-01T10:00:00Z");

        let exports = storage
            .list_frame_exports("2025-06-01T00:00:00Z", "2025-06-30T23:59:59Z")
            .unwrap();
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].app_name, "Code");
        assert!((exports[0].importance - 0.5).abs() < f32::EPSILON);
    }

    // ── fts_merge ───────────────────────────────────────────────────

    #[test]
    fn fts_merge_runs_without_error() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // FTS_AVAILABLE is set true after migrations in open_in_memory.
        // Contract: fts_merge completes without error and the FTS5 shadow tables
        // remain intact (search_fts_config still holds its configuration rows).
        // The merge command itself writes to *_data shadow pages; on an empty index
        // there is nothing to merge so the observable state is "table still queryable",
        // verified via a direct row-count on the config shadow table (#5594).
        storage
            .fts_merge(64)
            .expect("fts_merge must not error when FTS is available (#5594)");

        let conn = storage.conn.test_lock();
        let config_rows: i64 = conn
            .query_row("SELECT count(*) FROM search_fts_config", [], |row| {
                row.get(0)
            })
            .expect("search_fts_config must be queryable after fts_merge");
        assert!(
            config_rows > 0,
            "FTS5 config shadow table must retain rows after fts_merge (table intact)"
        );
    }

    #[test]
    fn fts_merge_skipped_when_unavailable() {
        // Temporarily set FTS_AVAILABLE to false
        let prev = FTS_AVAILABLE.load(std::sync::atomic::Ordering::Relaxed);
        FTS_AVAILABLE.store(false, std::sync::atomic::Ordering::Relaxed);

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // Restore the flag before calling — open_in_memory resets it to true,
        // so we set it again after opening.
        FTS_AVAILABLE.store(false, std::sync::atomic::Ordering::Relaxed);

        // Line 462: fts_merge no-ops and returns Ok(()) when FTS_AVAILABLE=false.
        // The Ok-only form is justified here (#5594): the no-op path returns early
        // before any DB access, so there is no observable write to read back — the
        // contract is solely "no error propagated to the maintenance scheduler".
        storage
            .fts_merge(64)
            .expect("should no-op when FTS unavailable");

        FTS_AVAILABLE.store(prev, std::sync::atomic::Ordering::Relaxed);
    }

    // ── fts_optimize ────────────────────────────────────────────────

    #[test]
    fn fts_optimize_runs_without_error() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        // Contract: fts_optimize merges all b-tree segments into one.
        // On an empty index the only observable guarantee is that the FTS5 shadow
        // tables remain intact after the optimize command (#5594).
        storage
            .fts_optimize()
            .expect("fts_optimize must not error when FTS is available (#5594)");

        let conn = storage.conn.test_lock();
        let config_rows: i64 = conn
            .query_row("SELECT count(*) FROM search_fts_config", [], |row| {
                row.get(0)
            })
            .expect("search_fts_config must be queryable after fts_optimize");
        assert!(
            config_rows > 0,
            "FTS5 config shadow table must retain rows after fts_optimize (table intact)"
        );
    }

    #[test]
    fn fts_optimize_skipped_when_unavailable() {
        let prev = FTS_AVAILABLE.load(std::sync::atomic::Ordering::Relaxed);
        FTS_AVAILABLE.store(false, std::sync::atomic::Ordering::Relaxed);

        let storage = SqliteStorage::open_in_memory(30).unwrap();
        FTS_AVAILABLE.store(false, std::sync::atomic::Ordering::Relaxed);

        // Line 500: fts_optimize no-ops and returns Ok(()) when FTS_AVAILABLE=false.
        // Justified as Ok-only (#5594): the early-return guard issues no SQL, so
        // there is no write-side effect to verify — the sole contract is non-error.
        storage
            .fts_optimize()
            .expect("should no-op when FTS unavailable");

        FTS_AVAILABLE.store(prev, std::sync::atomic::Ordering::Relaxed);
    }

    // ── Backup upsert helpers ───────────────────────────────────────

    #[test]
    fn upsert_backup_event_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .upsert_backup_event(
                "evt-100",
                "Idle",
                "2025-08-01T09:00:00Z",
                Some("Finder"),
                Some("Desktop"),
            )
            .unwrap();

        // Verify the event was persisted via count_events_in_range
        let count = storage
            .count_events_in_range(
                &TimeWindow::from_rfc3339_pair("2025-08-01T00:00:00Z", "2025-08-01T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Verify event_id and event_type via direct SQL
        let conn = storage.conn.test_lock();
        let (eid, etype): (String, String) = conn
            .query_row(
                "SELECT event_id, event_type FROM events WHERE event_id = 'evt-100'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(eid, "evt-100");
        assert_eq!(etype, "Idle");
    }

    #[test]
    fn upsert_backup_frame_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .upsert_backup_frame(
                42,
                "2025-08-01T09:00:00Z",
                "smart",
                "Safari",
                "Google",
                0.9,
                2560,
                1440,
                Some("Hello World"),
            )
            .unwrap();

        let exports = storage
            .list_frame_exports("2025-08-01T00:00:00Z", "2025-08-01T23:59:59Z")
            .unwrap();
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].id, 42);
        assert_eq!(exports[0].trigger_type, "smart");
        assert_eq!(exports[0].ocr_text.as_deref(), Some("Hello World"));
    }

    // ── list_frame_file_paths_in_range ──────────────────────────────

    #[test]
    fn list_frame_file_paths_empty_db() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let paths = storage
            .list_frame_file_paths_in_range(
                &TimeWindow::from_rfc3339_pair("2025-01-01T00:00:00Z", "2025-12-31T23:59:59Z")
                    .expect("trusted test bounds"),
            )
            .unwrap();
        assert!(paths.is_empty());
    }

    // ── search_events ───────────────────────────────────────────────

    #[test]
    fn count_search_events_searches_data_column() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_events(&storage, &["2025-06-01T10:00:00Z"]);

        let count = storage.count_search_events("%Code%").unwrap();
        assert_eq!(count, 1);

        let count = storage.count_search_events("%nonexistent%").unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn search_events_returns_matching_rows() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_events(&storage, &["2025-06-01T10:00:00Z"]);

        let rows = storage.search_events("%Code%", 10, 0).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].app_name.as_deref(), Some("Code"));
        assert_eq!(rows[0].window_title.as_deref(), Some("test.rs"));
    }

    // ── Backup tag helpers ──────────────────────────────────────────

    #[test]
    fn backup_tag_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        storage
            .upsert_backup_tag(1, "work", "#3b82f6", "2025-06-01T00:00:00Z")
            .unwrap();
        storage
            .upsert_backup_tag(2, "personal", "#ef4444", "2025-06-01T00:00:00Z")
            .unwrap();

        let tags = storage.list_backup_tags().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(tags[0].name, "work");
        assert_eq!(tags[1].name, "personal");
    }

    /// #9700: restore merges into a live table, so the three collision shapes
    /// must each resolve to a real, correct tag id — the caller remaps
    /// `frame_tags` through the returned value.
    ///
    /// Before this, `INSERT OR IGNORE` swallowed both collisions and the caller
    /// wrote the ARCHIVE's id, so a restore silently mis-attached or orphaned
    /// relations while reporting every tag as restored.
    #[test]
    fn restoring_a_tag_returns_the_id_it_actually_occupies() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // 1. Free id, unseen name -> keeps the archived id.
        let fresh = storage
            .upsert_backup_tag(7, "work", "#3b82f6", "2025-06-01T00:00:00Z")
            .unwrap();
        assert_eq!(fresh, Some(7), "an unclaimed id must be honoured");

        // 2. Name already present under a different archived id -> that row IS
        //    this tag. Returning the existing id is what stops the relation
        //    from dangling.
        let same_name = storage
            .upsert_backup_tag(99, "work", "#ef4444", "2025-06-02T00:00:00Z")
            .unwrap();
        assert_eq!(
            same_name,
            Some(7),
            "an existing name must map onto its own row"
        );
        assert_eq!(
            storage.list_backup_tags().unwrap().len(),
            1,
            "no duplicate row may be created for a name that already exists"
        );

        // 3. Id taken by a DIFFERENT tag -> insert under a fresh id and report
        //    it, so the relation follows this tag rather than the squatter.
        let relocated = storage
            .upsert_backup_tag(7, "personal", "#10b981", "2025-06-03T00:00:00Z")
            .unwrap();
        assert_ne!(relocated, Some(7), "a taken id must not be reused");
        let relocated = relocated.expect("a real write must report its id");

        let tags = storage.list_backup_tags().unwrap();
        assert_eq!(tags.len(), 2);
        let personal = tags.iter().find(|t| t.name == "personal").unwrap();
        assert_eq!(
            personal.id, relocated,
            "the reported id must be where the tag really landed"
        );
        // The pre-existing tag is untouched — the squatter was not overwritten.
        assert_eq!(tags.iter().find(|t| t.id == 7).unwrap().name, "work");
    }

    /// #9700 review: during a GDPR erase the write funnel SKIPS the insert.
    /// The return value must say "nothing was written" rather than hand back an
    /// id — the caller remaps `frame_tags` through it, and an invented id is
    /// exactly the mis-attachment this change removes. `i64::default()` is `0`,
    /// which this codebase also uses as a live tag sentinel, so `Option` is what
    /// makes the skip unambiguous.
    #[test]
    fn a_skipped_write_reports_none_rather_than_an_id() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage.set_deletion_flag(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            true,
        )));

        let result = storage
            .upsert_backup_tag(7, "work", "#3b82f6", "2025-06-01T00:00:00Z")
            .unwrap();

        assert_eq!(result, None, "an erase-skipped write must not report an id");
        assert!(
            storage.list_backup_tags().unwrap().is_empty(),
            "nothing may be written while the deletion flag is set"
        );
    }

    /// The test above exercises the SYNC inherent twin, where `None` is a
    /// literal in `run(None, ...)`. The ASYNC twin is the production path — the
    /// one that was returning `0` — and it derives `None` from
    /// `Option::<i64>::default()` via `with_conn`. Pin that axis too: the
    /// realistic regression is someone switching it to
    /// `with_conn_skip(Some(id), ...)` or changing the return type, which the
    /// sync test would not catch.
    #[tokio::test]
    async fn a_skipped_async_write_also_reports_none() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage.set_deletion_flag(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            true,
        )));

        let result = storage
            .upsert_backup_tag_async(
                7,
                "work".to_string(),
                "#3b82f6".to_string(),
                "2025-06-01T00:00:00Z".to_string(),
            )
            .await
            .unwrap();

        assert_eq!(
            result, None,
            "the production twin must report a skipped write as None, not an id"
        );
        assert!(
            storage.list_backup_tags().unwrap().is_empty(),
            "nothing may be written while the deletion flag is set"
        );
    }

    /// #9708: frames have no natural identity key, so a taken id means "a
    /// different frame", never "the same frame". Relocate and report where it
    /// landed — dropping it silently (the old behaviour) both over-counted the
    /// restore and left the caller writing relations against an id belonging to
    /// an unrelated local screenshot.
    #[test]
    fn restoring_a_frame_relocates_rather_than_dropping_on_id_collision() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_frame(&storage, 1, "2025-06-01T10:00:00Z");

        let landed = storage
            .upsert_backup_frame(
                1,
                "2025-01-01T09:00:00Z",
                "manual",
                "archived-app",
                "archived-title",
                0.9,
                200,
                200,
                None,
            )
            .unwrap()
            .expect("a real write must report its id");

        assert_ne!(landed, 1, "a taken id must not be reused");

        // A free id is honoured as-is.
        let free = storage
            .upsert_backup_frame(
                4242,
                "2025-01-02T09:00:00Z",
                "manual",
                "app",
                "title",
                0.5,
                100,
                100,
                None,
            )
            .unwrap();
        assert_eq!(free, Some(4242), "an unclaimed id must be honoured");
    }

    /// #9722: the last of the four restore writes to report the erase barrier
    /// honestly. `events` used to return `Ok(())` on skip, so the caller counted
    /// rows that were never written — the same over-count #9700/#9708 removed
    /// for tags and frames.
    #[test]
    fn a_skipped_event_write_reports_false() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage.set_deletion_flag(std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            true,
        )));

        let landed = storage
            .upsert_backup_event("evt-1", "focus", "2025-06-01T10:00:00Z", None, None)
            .unwrap();

        assert!(!landed, "an erase-skipped write must not report success");
        assert_eq!(
            count_rows(&storage, "SELECT COUNT(*) FROM events"),
            0,
            "nothing may be written while the deletion flag is set"
        );
    }

    /// #9735: CI holds the premise that FK enforcement is ON.
    ///
    /// Flipping this silently breaks things in both directions. Turned OFF, the
    /// frame/tag dependent cleanups start leaking again (#9721). Left ON while
    /// the code believes otherwise, inserts are rejected underneath a comment
    /// promising they cannot be — which is what this issue was. The point is
    /// less the value than making explicit what this crate reasons from.
    #[test]
    fn foreign_key_enforcement_is_on() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let conn = storage.conn.test_lock();
        let enabled: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("read PRAGMA foreign_keys");
        assert_eq!(
            enabled, 1,
            "this crate assumes FK enforcement — configure_connection turns it on \
             explicitly. Turning it off means amending ADR-028 B3 and every \
             comment that reasons from it, in the same change."
        );
    }

    #[test]
    fn backup_frame_tag_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Create prerequisite frame and tag
        insert_frame(&storage, 1, "2025-06-01T10:00:00Z");
        storage
            .upsert_backup_tag(10, "important", "#f59e0b", "2025-06-01T00:00:00Z")
            .unwrap();

        storage
            .upsert_backup_frame_tag(1, 10, "2025-06-01T10:00:00Z")
            .unwrap();

        let links = storage.list_backup_frame_tags().unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].frame_id, 1);
        assert_eq!(links[0].tag_id, 10);
    }

    // ── Closed-closed boundary regression tests (Phase 3 Task 5) ─────
    //
    // Per spec §5.1 and NG6: TimeWindow is closed-closed [start, end] —
    // both bounds are INCLUDED. These tests guard against accidental drift
    // to half-open semantics during future refactors.
    //
    // Note: Test fixture timestamps use the canonical `+00:00` form
    // because `TimeWindow::to_sql_pair()` uses `chrono::DateTime::to_rfc3339()`
    // which emits `+00:00` (not `Z`) for UTC. Lexicographic SQL comparison
    // requires matching formats at the exact boundary. Production code goes
    // through the same `to_rfc3339()` path on insert, so this fixture choice
    // mirrors real-world data.

    #[test]
    fn count_frames_in_range_includes_both_boundaries() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let t1 = "2026-04-01T00:00:00+00:00";
        let t2 = "2026-04-25T00:00:00+00:00";
        insert_frame(&storage, 1, t1); // exactly at start
        insert_frame(&storage, 2, "2026-04-15T00:00:00+00:00"); // middle
        insert_frame(&storage, 3, t2); // exactly at end
        let window = TimeWindow::from_rfc3339_pair(t1, t2).expect("trusted test bounds");
        assert_eq!(storage.count_frames_in_range(&window).unwrap(), 3);
    }

    #[test]
    fn count_events_in_range_includes_both_boundaries() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let t1 = "2026-04-01T00:00:00+00:00";
        let t2 = "2026-04-25T00:00:00+00:00";
        insert_events(&storage, &[t1, "2026-04-15T00:00:00+00:00", t2]);
        let window = TimeWindow::from_rfc3339_pair(t1, t2).expect("trusted test bounds");
        assert_eq!(storage.count_events_in_range(&window).unwrap(), 3);
    }

    #[test]
    fn delete_data_in_range_respects_delete_flags() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let t1 = "2026-04-01T00:00:00+00:00";
        let t2 = "2026-04-25T00:00:00+00:00";
        let middle = "2026-04-15T00:00:00+00:00";
        // Seed one of each: event + frame + metric (process and idle have
        // no convenient sync helper here; their delete flags are still
        // exercised via unrelated tests above).
        insert_events(&storage, &[middle]);
        insert_frame(&storage, 1, middle);
        insert_metric(&storage, middle);

        let window = TimeWindow::from_rfc3339_pair(t1, t2).expect("trusted test bounds");
        // delete_events=true, all others false
        let counts = storage
            .delete_data_in_range(&window, true, false, false, false, false)
            .unwrap();

        assert_eq!(counts.events_deleted, 1);
        assert_eq!(counts.frames_deleted, 0);
        assert_eq!(counts.metrics_deleted, 0);
        assert_eq!(counts.process_snapshots_deleted, 0);
        assert_eq!(counts.idle_periods_deleted, 0);

        // Frames + metrics should remain
        let remaining_frames = storage.count_frames_in_range(&window).unwrap();
        assert_eq!(remaining_frames, 1);
    }

    // ── range-delete transaction atomicity (#6) ─────────────────────

    /// Insert a single `system_metrics_hourly` rollup row directly.
    fn insert_hourly(storage: &SqliteStorage, hour_key: &str) {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO system_metrics_hourly (hour, cpu_avg, cpu_max, memory_avg, memory_max, sample_count)
             VALUES (?1, 10.0, 20.0, 1000, 2000, 5)",
            rusqlite::params![hour_key],
        )
        .unwrap();
    }

    fn count_hourly(storage: &SqliteStorage) -> i64 {
        let conn = storage.conn.test_lock();
        conn.query_row("SELECT COUNT(*) FROM system_metrics_hourly", [], |r| {
            r.get(0)
        })
        .unwrap()
    }

    #[test]
    fn delete_data_in_range_rolls_back_on_midway_failure() {
        // Regression (#6): the multi-table range-delete must run inside ONE
        // transaction so a mid-way failure does not leave the DB partially
        // deleted. We force the LAST delete (idle_periods) to abort via a
        // trigger; the earlier events/frames/metrics deletes must then roll back.
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let middle = "2026-04-15T00:00:00+00:00";
        insert_events(&storage, &[middle]);
        insert_frame(&storage, 1, middle);
        // #9721: the annotation cleanup runs inside the same transaction, so a
        // mid-way abort must roll it back too.
        seed_frame_dependents(&storage, 1);
        insert_metric(&storage, middle);
        {
            // Seed one idle_periods row so the aborting DELETE has a target row,
            // and install a trigger that raises on any DELETE from idle_periods.
            let conn = storage.conn.test_lock();
            conn.execute(
                "INSERT INTO idle_periods (start_time, end_time, duration_secs) VALUES (?1, ?1, 0)",
                rusqlite::params![middle],
            )
            .unwrap();
            conn.execute_batch(
                "CREATE TRIGGER abort_idle_delete BEFORE DELETE ON idle_periods
                 BEGIN SELECT RAISE(ABORT, 'forced delete failure'); END;",
            )
            .unwrap();
        }

        let window =
            TimeWindow::from_rfc3339_pair("2026-04-01T00:00:00+00:00", "2026-04-25T00:00:00+00:00")
                .expect("trusted test bounds");

        // All flags true → events/frames/metrics deletes succeed, idle delete aborts.
        // The RAISE(ABORT) on idle_periods is mapped to
        // StorageError::Internal("idle record delete failure: …"); assert the
        // variant AND message so the rollback is proven to be triggered by the
        // forced idle-delete failure, not some earlier unrelated error.
        let result = storage.delete_data_in_range(&window, true, true, true, true, true);
        let err = result.expect_err("aborting idle_periods delete must surface as an error");
        assert!(
            matches!(&err, crate::error::StorageError::Internal(msg) if msg.contains("idle record delete failure")),
            "aborted idle delete must surface StorageError::Internal(\"idle record delete failure: …\"), got {err:?}"
        );

        // Everything must be intact — the transaction rolled back atomically.
        assert_eq!(
            storage.count_events_in_range(&window).unwrap(),
            1,
            "events must be rolled back (not partially deleted)"
        );
        assert_eq!(
            storage.count_frames_in_range(&window).unwrap(),
            1,
            "frames must be rolled back"
        );
        let metrics = storage
            .list_metric_exports("2026-01-01T00:00:00Z", "2026-12-31T23:59:59Z")
            .unwrap();
        assert_eq!(metrics.len(), 1, "metrics must be rolled back");
        assert_eq!(
            count_rows(&storage, "SELECT COUNT(*) FROM frame_annotations"),
            1,
            "the #9721 annotation cleanup must roll back with the parent delete"
        );
    }

    // ── hourly-rollup boundary row (#7) ─────────────────────────────

    #[test]
    fn delete_data_in_range_deletes_boundary_hourly_rollup() {
        // Regression (#7): the rollup-delete bounds must match the stored
        // hour-bucket key format (`%Y-%m-%dT%H:00:00Z`). Previously the raw
        // RFC3339 bound (`...+00:00`) sorted differently from the stored `Z`
        // key, so when the window `to` landed on an hour boundary the boundary
        // rollup row was orphaned ('Z' > '+' lexically).
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // Bucket at the exact upper boundary hour, and one inside the window.
        insert_hourly(&storage, "2026-04-15T10:00:00Z");
        insert_hourly(&storage, "2026-04-15T12:00:00Z"); // == window `to` hour
                                                         // A bucket strictly after the window — must survive.
        insert_hourly(&storage, "2026-04-15T13:00:00Z");
        assert_eq!(count_hourly(&storage), 3);

        // `to` lands exactly on the 12:00 hour boundary (the orphan trigger case):
        // to_rfc3339() => "2026-04-15T12:00:00+00:00", stored key "…12:00:00Z".
        let window =
            TimeWindow::from_rfc3339_pair("2026-04-15T09:00:00+00:00", "2026-04-15T12:00:00+00:00")
                .expect("trusted test bounds");

        storage
            .delete_data_in_range(&window, false, false, true, false, false)
            .unwrap();

        // The 10:00 + the 12:00 boundary buckets are deleted; only the 13:00
        // bucket (strictly after the window) survives.
        assert_eq!(
            count_hourly(&storage),
            1,
            "boundary rollup row at the `to` hour must be deleted, not orphaned"
        );
        let remaining = storage
            .list_hourly_metrics_since("2026-04-15T00:00:00Z")
            .unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].hour, "2026-04-15T13:00:00Z");
    }
}
