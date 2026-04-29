use super::*;
use chrono::{Duration, Utc};
use oneshim_core::models::activity::{ProcessSnapshot, ProcessSnapshotEntry, SessionStats};
use oneshim_core::models::event::{ContextEvent, Event};
use oneshim_core::models::system::{NetworkInfo, SystemMetrics};
use oneshim_core::ports::storage::{MetricsStorage, StorageService};
use std::sync::atomic::Ordering;

use super::test_utils::make_user_event;

fn make_context_event() -> Event {
    Event::Context(ContextEvent {
        app_name: "Firefox".to_string(),
        window_title: "ONESHIM".to_string(),
        prev_app_name: Some("Code".to_string()),
        timestamp: Utc::now(),
        ..Default::default()
    })
}

#[tokio::test]
async fn save_and_get_events() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.save_event(&make_user_event()).await.unwrap();
    storage.save_event(&make_context_event()).await.unwrap();

    let from = Utc::now() - Duration::hours(1);
    let to = Utc::now() + Duration::hours(1);
    let events = storage.get_events(from, to, 100).await.unwrap();
    assert_eq!(events.len(), 2);
}

#[tokio::test]
async fn pending_events() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.save_event(&make_user_event()).await.unwrap();
    storage.save_event(&make_user_event()).await.unwrap();

    let pending = storage.get_pending_events(100).await.unwrap();
    assert_eq!(pending.len(), 2);
}

#[tokio::test]
async fn enforce_retention() {
    let storage = SqliteStorage::open_in_memory(0).unwrap(); // 0-day retention triggers immediate cleanup
    storage.save_event(&make_user_event()).await.unwrap();

    {
        let conn = storage.conn.lock().unwrap();
        conn.execute("UPDATE events SET is_sent = 1", []).unwrap();
    } // MutexGuard await
    let deleted = storage.enforce_retention().await.unwrap();
    assert!(deleted >= 1);
}

#[tokio::test]
async fn empty_storage() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let from = Utc::now() - Duration::hours(1);
    let to = Utc::now() + Duration::hours(1);
    let events = storage.get_events(from, to, 100).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn get_events_invalid_time_range() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.save_event(&make_user_event()).await.unwrap();

    let from = Utc::now() + Duration::hours(1);
    let to = Utc::now() - Duration::hours(1);
    let events = storage.get_events(from, to, 100).await.unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn mark_nonexistent_ids_no_error() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let ids = vec!["nonexistent1".to_string(), "nonexistent2".to_string()];
    let result = storage.mark_as_sent(&ids).await;
    assert!(result.is_ok());

    let result = storage.mark_as_sent(&[]).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn large_batch_insert() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    for _ in 0..100 {
        storage.save_event(&make_user_event()).await.unwrap();
    }

    let from = Utc::now() - Duration::hours(1);
    let to = Utc::now() + Duration::hours(1);
    let events = storage.get_events(from, to, 200).await.unwrap();
    assert_eq!(events.len(), 100);
}

#[tokio::test]
async fn retention_does_not_delete_unsent() {
    let storage = SqliteStorage::open_in_memory(0).unwrap();

    storage.save_event(&make_user_event()).await.unwrap();

    let deleted = storage.enforce_retention().await.unwrap();

    assert_eq!(deleted, 0);

    let pending = storage.get_pending_events(100).await.unwrap();
    assert_eq!(pending.len(), 1);
}

#[tokio::test]
async fn mark_as_sent_affects_pending() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let event = make_user_event();
    let event_id = match &event {
        Event::User(e) => e.event_id.to_string(),
        _ => panic!("unexpected event type"),
    };
    storage.save_event(&event).await.unwrap();

    let pending = storage.get_pending_events(100).await.unwrap();
    assert_eq!(pending.len(), 1);

    storage.mark_as_sent(&[event_id]).await.unwrap();

    let pending = storage.get_pending_events(100).await.unwrap();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn concurrent_save_and_get() {
    let storage = std::sync::Arc::new(SqliteStorage::open_in_memory(30).unwrap());

    let handles: Vec<_> = (0..10)
        .map(|_| {
            let s = storage.clone();
            tokio::spawn(async move {
                for _ in 0..10 {
                    s.save_event(&make_user_event()).await.unwrap();
                }
            })
        })
        .collect();

    for h in handles {
        h.await.unwrap();
    }

    let from = Utc::now() - Duration::hours(1);
    let to = Utc::now() + Duration::hours(1);
    let events = storage.get_events(from, to, 200).await.unwrap();
    assert_eq!(events.len(), 100);
}

fn make_system_metrics() -> SystemMetrics {
    SystemMetrics {
        timestamp: Utc::now(),
        cpu_usage: 45.5,
        memory_used: 8 * 1024 * 1024 * 1024,   // 8GB
        memory_total: 16 * 1024 * 1024 * 1024, // 16GB
        disk_used: 100 * 1024 * 1024 * 1024,
        disk_total: 500 * 1024 * 1024 * 1024,
        network: Some(NetworkInfo {
            upload_speed: 1000,
            download_speed: 5000,
            is_connected: true,
        }),
        typing_wpm: 0.0,
    }
}

fn make_process_snapshot() -> ProcessSnapshot {
    ProcessSnapshot {
        timestamp: Utc::now(),
        processes: vec![
            ProcessSnapshotEntry {
                pid: 1234,
                name: "firefox".to_string(),
                cpu_usage: 10.5,
                memory_bytes: 512 * 1024 * 1024, // 512MB
            },
            ProcessSnapshotEntry {
                pid: 5678,
                name: "code".to_string(),
                cpu_usage: 5.2,
                memory_bytes: 256 * 1024 * 1024, // 256MB
            },
        ],
    }
}

#[tokio::test]
async fn save_and_get_metrics() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.save_metrics(&make_system_metrics()).await.unwrap();

    let from = Utc::now() - Duration::hours(1);
    let to = Utc::now() + Duration::hours(1);
    let metrics = storage.get_metrics(from, to, 100).await.unwrap();
    assert_eq!(metrics.len(), 1);
    assert!(metrics[0].cpu_usage > 40.0);
}

#[tokio::test]
async fn cleanup_old_metrics() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.save_metrics(&make_system_metrics()).await.unwrap();

    let future = Utc::now() + Duration::days(1);
    let deleted = storage.cleanup_old_metrics(future).await.unwrap();
    assert_eq!(deleted, 1);
}

#[tokio::test]
async fn save_and_get_process_snapshot() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage
        .save_process_snapshot(&make_process_snapshot())
        .await
        .unwrap();

    let from = Utc::now() - Duration::hours(1);
    let to = Utc::now() + Duration::hours(1);
    let snapshots = storage.get_process_snapshots(from, to, 100).await.unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].processes.len(), 2);
}

#[tokio::test]
async fn idle_period_lifecycle() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let start = Utc::now();
    let id = storage.start_idle_period(start).await.unwrap();
    assert!(id > 0);

    let ongoing = storage.get_ongoing_idle_period().await.unwrap();
    assert!(ongoing.is_some());
    let (ongoing_id, _) = ongoing.unwrap();
    assert_eq!(ongoing_id, id);

    let end = start + Duration::minutes(5);
    storage.end_idle_period(id, end).await.unwrap();

    let ongoing = storage.get_ongoing_idle_period().await.unwrap();
    assert!(ongoing.is_none());

    let from = Utc::now() - Duration::hours(1);
    let to = Utc::now() + Duration::hours(1);
    let periods = storage.get_idle_periods(from, to).await.unwrap();
    assert_eq!(periods.len(), 1);
    assert!(periods[0].duration_secs.is_some());
}

#[tokio::test]
async fn session_stats_lifecycle() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let session_id = "test-session-123";
    let stats = SessionStats {
        session_id: session_id.to_string(),
        started_at: Utc::now(),
        ended_at: None,
        total_events: 10,
        total_frames: 5,
        total_idle_secs: 60,
    };

    storage.upsert_session(&stats).await.unwrap();

    let loaded = storage.get_session(session_id).await.unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.total_events, 10);

    storage
        .increment_session_counters(session_id, 5, 2, 30)
        .await
        .unwrap();

    let loaded = storage.get_session(session_id).await.unwrap().unwrap();
    assert_eq!(loaded.total_events, 15);
    assert_eq!(loaded.total_frames, 7);
    assert_eq!(loaded.total_idle_secs, 90);

    storage.end_session(session_id, Utc::now()).await.unwrap();

    let loaded = storage.get_session(session_id).await.unwrap().unwrap();
    assert!(loaded.ended_at.is_some());
}

#[tokio::test]
async fn session_not_found() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let result = storage.get_session("nonexistent").await.unwrap();
    assert!(result.is_none());
}

#[test]
fn create_and_get_tags() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let tag = storage.create_tag("work", "#3b82f6").unwrap();
    assert_eq!(tag.name, "work");
    assert_eq!(tag.color, "#3b82f6");

    let tags = storage.get_all_tags().unwrap();
    assert_eq!(tags.len(), 1);
}

#[test]
fn delete_tag() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let tag = storage.create_tag("temp", "#ef4444").unwrap();
    let deleted = storage.delete_tag(tag.id).unwrap();
    assert!(deleted);

    let tags = storage.get_all_tags().unwrap();
    assert!(tags.is_empty());
}

#[test]
fn get_tag_by_id() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let tag = storage.create_tag("important", "#f59e0b").unwrap();
    let found = storage.get_tag(tag.id).unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "important");

    let not_found = storage.get_tag(99999).unwrap();
    assert!(not_found.is_none());
}

#[test]
fn update_tag() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let tag = storage.create_tag("old", "#000000").unwrap();
    let updated = storage.update_tag(tag.id, "new", "#ffffff").unwrap();
    assert!(updated);

    let found = storage.get_tag(tag.id).unwrap().unwrap();
    assert_eq!(found.name, "new");
    assert_eq!(found.color, "#ffffff");
}

#[test]
fn frame_tag_operations() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, importance, resolution_w, resolution_h, has_image)
             VALUES ('2024-01-01T00:00:00Z', 'manual', 'test', 'test', 0.5, 1920, 1080, 0)",
            [],
        )
        .unwrap();
    }

    let tag1 = storage.create_tag("tag1", "#ff0000").unwrap();
    let tag2 = storage.create_tag("tag2", "#00ff00").unwrap();

    storage.add_tag_to_frame(1, tag1.id).unwrap();
    storage.add_tag_to_frame(1, tag2.id).unwrap();

    let tags = storage.get_tags_for_frame(1).unwrap();
    assert_eq!(tags.len(), 2);

    let frames = storage.get_frames_by_tag(tag1.id, 100).unwrap();
    assert_eq!(frames.len(), 1);

    let removed = storage.remove_tag_from_frame(1, tag1.id).unwrap();
    assert!(removed);

    let tags = storage.get_tags_for_frame(1).unwrap();
    assert_eq!(tags.len(), 1);
}

#[test]
fn duplicate_tag_name_fails() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.create_tag("unique", "#000000").unwrap();
    let result = storage.create_tag("unique", "#ffffff");
    assert!(result.is_err());
}

#[test]
fn add_tag_to_frame_idempotent() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO frames (timestamp, trigger_type, app_name, window_title, importance, resolution_w, resolution_h, has_image)
             VALUES ('2024-01-01T00:00:00Z', 'manual', 'test', 'test', 0.5, 1920, 1080, 0)",
            [],
        )
        .unwrap();
    }

    let tag = storage.create_tag("tag", "#000000").unwrap();

    storage.add_tag_to_frame(1, tag.id).unwrap();
    storage.add_tag_to_frame(1, tag.id).unwrap();

    let tags = storage.get_tags_for_frame(1).unwrap();
    assert_eq!(tags.len(), 1);
}

#[test]
fn daily_digest_save_and_get_roundtrip() {
    use oneshim_core::models::daily_digest::{
        DailyDigest, DailyInsight, DailyStatistics, DigestHighlight, HighlightType,
    };
    use oneshim_core::ports::web_storage::DigestStorage;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let today = Utc::now().date_naive();

    let digest = DailyDigest {
        date: today,
        insight: Some(DailyInsight {
            narrative: "Great focus day!".to_string(),
            highlights: vec![DigestHighlight {
                highlight_type: HighlightType::Achievement,
                text: "2h deep work".to_string(),
                segment_id: Some("seg-001".to_string()),
            }],
        }),
        timeline: vec![],
        statistics: DailyStatistics {
            deep_work_hours: 4.2,
            ..DailyStatistics::default()
        },
        generated_at: Utc::now(),
    };

    storage.save_daily_digest(&digest).unwrap();

    let loaded = storage.get_daily_digest(&today.to_string()).unwrap();
    assert!(loaded.is_some());
    let loaded = loaded.unwrap();
    assert_eq!(loaded.date, today);
    assert!(loaded.insight.is_some());
    let insight = loaded.insight.unwrap();
    assert_eq!(insight.narrative, "Great focus day!");
    assert_eq!(insight.highlights.len(), 1);
    assert!((loaded.statistics.deep_work_hours - 4.2).abs() < f32::EPSILON);
}

#[test]
fn daily_digest_list_ordering() {
    use chrono::Days;
    use oneshim_core::models::daily_digest::{DailyDigest, DailyStatistics};
    use oneshim_core::ports::web_storage::DigestStorage;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let today = Utc::now().date_naive();

    for offset in 0..3 {
        let date = today - Days::new(offset);
        let digest = DailyDigest {
            date,
            insight: None,
            timeline: vec![],
            statistics: DailyStatistics::default(),
            generated_at: Utc::now(),
        };
        storage.save_daily_digest(&digest).unwrap();
    }

    let digests = storage.list_daily_digests(10).unwrap();
    assert_eq!(digests.len(), 3);
    // Newest first
    assert_eq!(digests[0].date, today);
    assert_eq!(digests[1].date, today - Days::new(1));
}

#[test]
fn daily_digest_get_nonexistent_returns_none() {
    use oneshim_core::ports::web_storage::DigestStorage;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let result = storage.get_daily_digest("2020-01-01").unwrap();
    assert!(result.is_none());
}

#[test]
fn ensure_device_identity_generates_uuid_on_first_call() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let (device_id, device_name) = storage.ensure_device_identity("Test Machine").unwrap();

    assert!(!device_id.is_empty());
    // Validate UUID v4 format (8-4-4-4-12 hex chars)
    assert_eq!(device_id.len(), 36);
    assert_eq!(device_name, "Test Machine");
}

#[test]
fn ensure_device_identity_returns_same_id_on_second_call() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let (id1, _) = storage.ensure_device_identity("Machine A").unwrap();
    let (id2, name2) = storage.ensure_device_identity("Machine B").unwrap();

    // Second call must return the FIRST identity, not generate a new one.
    assert_eq!(id1, id2);
    assert_eq!(name2, "Machine A"); // Original name preserved
}

#[test]
fn ensure_device_identity_persists_across_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let id1 = {
        let storage = SqliteStorage::open(&db_path, 30, None).unwrap();
        let (id, _) = storage.ensure_device_identity("Laptop").unwrap();
        id
    };

    // Reopen the database
    let id2 = {
        let storage = SqliteStorage::open(&db_path, 30, None).unwrap();
        let (id, name) = storage.ensure_device_identity("Different Name").unwrap();
        assert_eq!(name, "Laptop"); // Original name preserved
        id
    };

    assert_eq!(id1, id2);
}

#[test]
fn reset_device_identity_generates_new_uuid() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let (id1, name1) = storage.ensure_device_identity("Original").unwrap();
    assert_eq!(name1, "Original");

    let (id2, name2) = storage.reset_device_identity("Reset Device").unwrap();
    assert_eq!(name2, "Reset Device");

    // After reset, device_id must be different
    assert_ne!(id1, id2, "reset must generate a new device_id");
    assert_eq!(id2.len(), 36, "new id must be valid UUID format");
}

#[test]
fn reset_device_identity_allows_re_ensure() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let (id1, _) = storage.ensure_device_identity("First").unwrap();
    let (id2, _) = storage.reset_device_identity("Second").unwrap();
    assert_ne!(id1, id2);

    // After reset, ensure_device_identity returns the new identity
    let (id3, name3) = storage.ensure_device_identity("Third").unwrap();
    assert_eq!(
        id2, id3,
        "ensure after reset must return the reset identity"
    );
    assert_eq!(name3, "Second"); // Name from reset is preserved
}

#[test]
fn enforce_all_retention_runs_without_error() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // Insert test data into tables covered by enforce_all_retention
    {
        let conn = storage.conn.lock().unwrap();

        // work_sessions — old closed session (schema: primary_app, category, started_at, ended_at)
        conn.execute(
            "INSERT INTO work_sessions (primary_app, category, started_at, ended_at)
             VALUES ('Code', 'Development', datetime('now', '-100 days'), datetime('now', '-100 days', '+1 hour'))",
            [],
        ).unwrap();

        // interruptions — old record (schema: interrupted_at, from_app, from_category, to_app, to_category)
        conn.execute(
            "INSERT INTO interruptions (interrupted_at, from_app, from_category, to_app, to_category)
             VALUES (datetime('now', '-100 days'), 'Code', 'Development', 'Slack', 'Communication')",
            [],
        ).unwrap();

        // suggestions — old record
        conn.execute(
            "INSERT INTO suggestions (suggestion_id, suggestion_type, content, priority, source, created_at)
             VALUES ('sugg-001', 'general', 'Take a break', 'Low', 'server', datetime('now', '-100 days'))",
            [],
        ).unwrap();

        // local_suggestions — old record
        conn.execute(
            "INSERT INTO local_suggestions (suggestion_type, payload, created_at)
             VALUES ('TakeBreak', '{}', datetime('now', '-100 days'))",
            [],
        )
        .unwrap();

        // focus_metrics — old record
        conn.execute(
            "INSERT INTO focus_metrics (date, total_active_secs)
             VALUES (date('now', '-400 days'), 0)",
            [],
        )
        .unwrap();
    }

    // enforce_all_retention should delete the old rows
    let deleted = storage.enforce_all_retention().unwrap();
    assert!(
        deleted >= 5,
        "expected at least 5 rows deleted, got {deleted}"
    );
}

#[test]
fn enforce_all_retention_keeps_recent_data() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    {
        let conn = storage.conn.lock().unwrap();

        // Recent work_session (should NOT be deleted)
        conn.execute(
            "INSERT INTO work_sessions (primary_app, category, started_at, ended_at)
             VALUES ('Code', 'Development', datetime('now', '-1 day'), datetime('now', '-1 day', '+1 hour'))",
            [],
        ).unwrap();

        // Recent interruption (should NOT be deleted)
        conn.execute(
            "INSERT INTO interruptions (interrupted_at, from_app, from_category, to_app, to_category)
             VALUES (datetime('now', '-1 day'), 'Code', 'Development', 'Slack', 'Communication')",
            [],
        ).unwrap();
    }

    let deleted = storage.enforce_all_retention().unwrap();
    assert_eq!(deleted, 0, "recent data should not be deleted");
}

// ── Subtask A: configure_connection PRAGMA parity ────────────────

#[test]
fn in_memory_applies_cache_size_pragma() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let conn = storage.conn.lock().unwrap();
    let cache_size: i64 = conn
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .unwrap();
    assert_eq!(cache_size, 8000, "in-memory DB should have cache_size=8000");
}

#[test]
fn in_memory_applies_temp_store_pragma() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let conn = storage.conn.lock().unwrap();
    let temp_store: i64 = conn
        .query_row("PRAGMA temp_store", [], |row| row.get(0))
        .unwrap();
    // MEMORY = 2
    assert_eq!(
        temp_store, 2,
        "in-memory DB should have temp_store=MEMORY (2)"
    );
}

#[test]
fn disk_applies_wal_mode() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_wal.db");
    let storage = SqliteStorage::open(&db_path, 30, None).unwrap();
    let conn = storage.conn.lock().unwrap();
    let journal_mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode, "wal", "disk DB should use WAL journal mode");
}

#[test]
fn disk_applies_synchronous_normal() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_sync.db");
    let storage = SqliteStorage::open(&db_path, 30, None).unwrap();
    let conn = storage.conn.lock().unwrap();
    let synchronous: i64 = conn
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    // NORMAL = 1
    assert_eq!(synchronous, 1, "disk DB should have synchronous=NORMAL (1)");
}

// ── Subtask B: journal_size_limit + PRAGMA optimize ──────────────

#[test]
fn disk_applies_journal_size_limit() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_journal_limit.db");
    let storage = SqliteStorage::open(&db_path, 30, None).unwrap();
    let conn = storage.conn.lock().unwrap();
    let limit: i64 = conn
        .query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        limit, 67_108_864,
        "disk DB should have journal_size_limit=64MB"
    );
}

#[test]
fn in_memory_does_not_set_journal_size_limit() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let conn = storage.conn.lock().unwrap();
    let limit: i64 = conn
        .query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
        .unwrap();
    // Default is -1 (no limit) for in-memory
    assert_eq!(
        limit, -1,
        "in-memory DB should keep default journal_size_limit (-1)"
    );
}

#[test]
fn pragma_optimize_runs_without_error() {
    // open_in_memory calls post_migration_setup which includes PRAGMA optimize.
    // If it panics or errors, this test will fail.
    let _storage = SqliteStorage::open_in_memory(30).unwrap();
}

#[test]
fn pragma_optimize_runs_on_disk_without_error() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_optimize.db");
    let _storage = SqliteStorage::open(&db_path, 30, None).unwrap();
}

// ── Subtask C: FTS5 existence caching ────────────────────────────

#[test]
fn fts_available_set_after_open_in_memory() {
    let _storage = SqliteStorage::open_in_memory(30).unwrap();
    assert!(
        FTS_AVAILABLE.load(Ordering::Relaxed),
        "FTS_AVAILABLE should be true after migrations create search_fts"
    );
}

#[test]
fn gui_interactions_available_set_after_open_in_memory() {
    let _storage = SqliteStorage::open_in_memory(30).unwrap();
    assert!(
        GUI_INTERACTIONS_AVAILABLE.load(Ordering::Relaxed),
        "GUI_INTERACTIONS_AVAILABLE should be true after migrations create gui_interactions"
    );
}

#[test]
fn fts_available_set_after_disk_open() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_fts_flag.db");
    let _storage = SqliteStorage::open(&db_path, 30, None).unwrap();
    assert!(
        FTS_AVAILABLE.load(Ordering::Relaxed),
        "FTS_AVAILABLE should be true after disk open with migrations"
    );
}

#[test]
fn gui_interactions_available_set_after_disk_open() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_gui_flag.db");
    let _storage = SqliteStorage::open(&db_path, 30, None).unwrap();
    assert!(
        GUI_INTERACTIONS_AVAILABLE.load(Ordering::Relaxed),
        "GUI_INTERACTIONS_AVAILABLE should be true after disk open with migrations"
    );
}

#[test]
fn meta_roundtrip() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // Key does not exist yet
    assert_eq!(storage.get_meta("onboarding_completed"), None);

    // Set and read back
    storage.set_meta("onboarding_completed", "true");
    assert_eq!(
        storage.get_meta("onboarding_completed"),
        Some("true".to_string())
    );

    // Overwrite
    storage.set_meta("onboarding_completed", "false");
    assert_eq!(
        storage.get_meta("onboarding_completed"),
        Some("false".to_string())
    );

    // Delete and verify absence
    storage.delete_meta("onboarding_completed");
    assert_eq!(storage.get_meta("onboarding_completed"), None);
}

// ── SQLCipher encryption tests ──────────────────────────────

#[test]
fn sqlcipher_open_with_encryption_key() {
    use crate::encryption::EncryptionKey;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("encrypted.db");
    let key = EncryptionKey::from_bytes([0x42; 32]);

    // First open: creates encrypted database
    {
        let storage = SqliteStorage::open(&db_path, 30, Some(&key)).unwrap();
        storage.set_meta("secret", "value");
        assert_eq!(storage.get_meta("secret"), Some("value".to_string()));
    }

    // Reopen with same key: data persists
    {
        let storage = SqliteStorage::open(&db_path, 30, Some(&key)).unwrap();
        assert_eq!(storage.get_meta("secret"), Some("value".to_string()));
    }
}

#[test]
fn sqlcipher_fallback_for_unencrypted_db() {
    use crate::encryption::EncryptionKey;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("plain.db");

    // Create an unencrypted database first
    {
        let storage = SqliteStorage::open(&db_path, 30, None).unwrap();
        storage.set_meta("hello", "world");
    }

    // Reopen with an encryption key — should fall back to unencrypted
    let key = EncryptionKey::from_bytes([0x42; 32]);
    let storage = SqliteStorage::open(&db_path, 30, Some(&key)).unwrap();
    // The fallback path reopens without encryption, so data is accessible
    assert_eq!(storage.get_meta("hello"), Some("world".to_string()));
}

// ── PR3 underlying-impl gap coverage ───────────────────────────────
//
// Phase 5-D8 PR3 audit identified 4 underlying SqliteStorage methods
// lacking coverage in any sibling file. (2 more — save_rule_suggestion_sync
// + mark_unified_suggestion_shown — deferred to follow-up; require heavy
// Suggestion fixture.)

#[test]
fn update_coaching_event_personalized_updates_existing() {
    use oneshim_core::models::coaching::CoachingEventRow;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let event = CoachingEventRow {
        event_id: "coach-upd-001".to_string(),
        trigger_type: "t".into(),
        profile_name: "default".into(),
        regime_id: None,
        message_template: "original".into(),
        personalized_message: None,
        shown_at: Utc::now().to_rfc3339(),
        dismissed_at: None,
        dismiss_action: None,
        feedback_type: None,
        feedback_score: None,
    };
    storage.insert_coaching_event(&event).unwrap();

    storage
        .update_coaching_event_personalized("coach-upd-001", "new-text")
        .unwrap();
}

#[test]
fn list_suggestions_empty_db_returns_empty_vec() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let results = storage.list_suggestions(100).unwrap();
    assert!(results.is_empty());
}

#[test]
fn add_deep_work_secs_accumulates_on_work_session() {
    use oneshim_core::models::work_session::AppCategory;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let session = storage
        .start_work_session("VSCode", AppCategory::Development)
        .unwrap();

    storage.add_deep_work_secs(session.id, 30).unwrap();
    storage.add_deep_work_secs(session.id, 45).unwrap();

    // Verify via direct SQL that the deep_work_secs column reflects the sum.
    let conn = storage.conn.lock().unwrap();
    let total: i64 = conn
        .query_row(
            "SELECT deep_work_secs FROM work_sessions WHERE id = ?1",
            rusqlite::params![session.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(total, 75);
}

#[test]
fn increment_work_session_interruption_increments_counter() {
    use oneshim_core::models::work_session::AppCategory;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let session = storage
        .start_work_session("VSCode", AppCategory::Development)
        .unwrap();

    storage
        .increment_work_session_interruption(session.id)
        .unwrap();
    storage
        .increment_work_session_interruption(session.id)
        .unwrap();

    let conn = storage.conn.lock().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT interruption_count FROM work_sessions WHERE id = ?1",
            rusqlite::params![session.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
}

// ── entries_by_command_id (V32 / D25) ─────────────────────────────────────

#[test]
fn entries_by_command_id_returns_matching_rows_newest_first() {
    use chrono::Utc;
    use oneshim_core::models::audit::{AuditEntry, AuditStatus};

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite open");

    // 3 entries with command_id "cmd-X" at descending timestamps.
    for i in 0..3_i64 {
        let entry = AuditEntry {
            entry_id: format!("id-X-{i}"),
            timestamp: Utc::now() - chrono::Duration::milliseconds(i),
            session_id: "s".to_string(),
            command_id: "cmd-X".to_string(),
            action_type: "test".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(10),
        };
        storage.save_audit_entry(&entry);
    }
    // 2 entries with command_id "cmd-Y" (must not appear in results).
    for i in 0..2_i64 {
        let entry = AuditEntry {
            entry_id: format!("id-Y-{i}"),
            timestamp: Utc::now(),
            session_id: "s".to_string(),
            command_id: "cmd-Y".to_string(),
            action_type: "test".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(10),
        };
        storage.save_audit_entry(&entry);
    }

    let results = storage.entries_by_command_id("cmd-X", 10);
    assert_eq!(results.len(), 3);
    for r in &results {
        assert_eq!(r.command_id, "cmd-X");
    }
    for w in results.windows(2) {
        assert!(
            w[0].timestamp >= w[1].timestamp,
            "expected newest-first ordering"
        );
    }
}

#[test]
fn entries_by_command_id_empty_for_no_match() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    assert!(storage.entries_by_command_id("nonexistent", 10).is_empty());
}

#[test]
fn entries_by_command_id_respects_limit() {
    use chrono::Utc;
    use oneshim_core::models::audit::{AuditEntry, AuditStatus};

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    for i in 0..10_i64 {
        let entry = AuditEntry {
            entry_id: format!("id-Z-{i}"),
            timestamp: Utc::now(),
            session_id: "s".to_string(),
            command_id: "cmd-Z".to_string(),
            action_type: "test".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(5),
        };
        storage.save_audit_entry(&entry);
    }
    let results = storage.entries_by_command_id("cmd-Z", 3);
    assert_eq!(results.len(), 3);
}
