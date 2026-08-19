use super::*;
use chrono::{Duration, Utc};
use maekon_core::models::activity::{ProcessSnapshot, ProcessSnapshotEntry, SessionStats};
use maekon_core::models::event::{ContextEvent, Event};
use maekon_core::models::system::{NetworkInfo, SystemMetrics};
use maekon_core::ports::storage::{MetricsStorage, StorageService};
use std::sync::atomic::Ordering;

use super::test_utils::make_user_event;
use crate::error::StorageError;

fn make_context_event() -> Event {
    Event::Context(ContextEvent {
        app_name: "Firefox".to_string(),
        window_title: "MAEKON".to_string(),
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
        let conn = storage.conn.test_lock();
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

    // Contract: UPDATE matching 0 rows succeeds; pending queue is unaffected.
    let ids = vec!["nonexistent1".to_string(), "nonexistent2".to_string()];
    storage
        .mark_as_sent(&ids)
        .await
        .expect("mark_as_sent with nonexistent IDs must not error (#5594)");

    // Verify the call had no side-effects on the (empty) pending queue.
    let pending = storage.get_pending_events(100).await.unwrap();
    assert!(
        pending.is_empty(),
        "no events inserted, pending must stay empty after marking nonexistent IDs"
    );

    // Contract: empty slice hits the early-return Ok(()) path without touching the DB.
    storage
        .mark_as_sent(&[])
        .await
        .expect("mark_as_sent with empty slice must not error (#5594)");
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
        let conn = storage.conn.test_lock();
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
    assert!(
        matches!(
            storage.create_tag("unique", "#ffffff").unwrap_err(),
            StorageError::Internal(_)
        ),
        "duplicate tag name must yield StorageError::Internal (UNIQUE constraint)"
    );
}

#[test]
fn add_tag_to_frame_idempotent() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    {
        let conn = storage.conn.test_lock();
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
    use maekon_core::models::daily_digest::{
        DailyDigest, DailyInsight, DailyStatistics, DigestHighlight, HighlightType,
    };

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
    use maekon_core::models::daily_digest::{DailyDigest, DailyStatistics};

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
fn digest_processing_marker_roundtrip() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let kind = "daily_claim_promotion";
    let period_key = "2026-06-30";

    assert!(
        !storage
            .has_digest_processing_marker(kind, period_key)
            .unwrap(),
        "marker should be absent before save"
    );

    storage
        .save_digest_processing_marker(kind, period_key, Utc::now())
        .unwrap();

    assert!(
        storage
            .has_digest_processing_marker(kind, period_key)
            .unwrap(),
        "marker should be present after save"
    );
}

#[test]
fn daily_digest_get_nonexistent_returns_none() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let result = storage.get_daily_digest("2020-01-01").unwrap();
    assert!(result.is_none());
}

/// ADR-026 PR-final — Digest async-safety guard (deferred review follow-up).
///
/// The `DigestStorage` async body has **no `maekon-web` entrypoint**, so its
/// async path is not exercised by any handler `#[tokio::test]`. If a future
/// change reintroduced a `block_in_place` bridge inside that async body, it
/// would silently re-block the runtime — and on a single-threaded runtime it
/// would outright **panic** (`block_in_place` requires the multi-thread
/// scheduler).
///
/// This test runs on the **default `#[tokio::test]` current-thread runtime**
/// and drives the async trait method `DigestStorage::get_daily_digest` (which
/// must route through `with_conn_read` → `spawn_blocking`, never
/// `block_in_place`). Fully-qualified trait syntax is required because the
/// inherent sync twin `SqliteStorage::get_daily_digest` shadows the trait
/// method by name. A `block_in_place` reintroduction would either panic or
/// hang; the 5s `timeout` guards the hang case and the current-thread runtime
/// guards the panic case.
#[tokio::test]
async fn digest_async_path_runs_on_current_thread_runtime() {
    use maekon_core::ports::web_storage::DigestStorage;
    use std::time::Duration;

    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // Drive the async trait method (NOT the sync inherent twin) so the
    // `spawn_blocking` funnel is what gets exercised. On the current-thread
    // runtime, any `block_in_place` in this body would panic.
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        DigestStorage::get_daily_digest(&storage, "2020-01-01"),
    )
    .await;

    let inner = result.expect("digest async path must complete within 5s (no block_in_place hang)");
    assert!(
        inner.expect("digest async query must not error").is_none(),
        "no digest exists for 2020-01-01"
    );

    // Also guard a list-style async read so the regression net covers more of
    // the `DigestStorage` async surface than a single accessor.
    let listed = tokio::time::timeout(
        Duration::from_secs(5),
        DigestStorage::list_daily_digests(&storage, 10),
    )
    .await
    .expect("digest list async path must complete within 5s")
    .expect("digest list async query must not error");
    assert!(listed.is_empty(), "empty in-memory store lists no digests");
}

#[test]
fn ensure_device_identity_generates_uuid_on_first_call() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let (device_id, device_name) = storage.ensure_device_identity("Test Machine").unwrap();

    assert!(!device_id.is_empty());
    // device_id is a server-wire field kept as UUID v4 (ADR-022 exemption).
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
        let conn = storage.conn.test_lock();

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
        let conn = storage.conn.test_lock();

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

#[test]
fn enforce_all_retention_prunes_old_coaching_events() {
    // `coaching_events` (V17) is INSERT-only; without an age window it grows
    // unbounded. enforce_all_retention must prune rows older than 90 days
    // (keyed on `shown_at`) while keeping recent ones.
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    {
        let conn = storage.conn.test_lock();

        // Old coaching event (shown 100 days ago) — should be pruned.
        conn.execute(
            "INSERT INTO coaching_events
                (event_id, trigger_type, profile_name, message_template, shown_at)
             VALUES ('coach-old', 'LongSession', 'default', 'Take a break', datetime('now', '-100 days'))",
            [],
        )
        .unwrap();

        // Recent coaching event (shown 1 day ago) — should be kept.
        conn.execute(
            "INSERT INTO coaching_events
                (event_id, trigger_type, profile_name, message_template, shown_at)
             VALUES ('coach-new', 'LongSession', 'default', 'Take a break', datetime('now', '-1 day'))",
            [],
        )
        .unwrap();
    }

    let deleted = storage.enforce_all_retention().unwrap();
    assert!(
        deleted >= 1,
        "expected the old coaching_event to be pruned, got {deleted}"
    );

    let conn = storage.conn.test_lock();
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM coaching_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        remaining, 1,
        "exactly the recent coaching_event should remain"
    );
    let kept_id: String = conn
        .query_row("SELECT event_id FROM coaching_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        kept_id, "coach-new",
        "the recent event must be the survivor"
    );
}

#[test]
fn gc_sync_tombstones_bounds_outbox_at_max_retention_90() {
    // #5174 S5/R4: the retained erasure-tombstone outbox is GC'd at max(retention, 90)d.
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    {
        let conn = storage.conn.test_lock();
        // Old, producer-local format (SQLite `datetime('now')`) — past the 90d horizon.
        conn.execute(
            "INSERT INTO sync_tombstones (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('activity_segments', 'old-local', 'dev-a', 100, 0, datetime('now', '-120 days'))",
            [],
        ).unwrap();
        // Defensive: a non-production ISO-8601 `…Z` variant must also normalize + GC
        // (production stores the `datetime('now')` space-format; this proves robustness).
        conn.execute(
            "INSERT INTO sync_tombstones (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('regimes', 'old-wire', 'dev-b', 100, 0, '2020-01-01T00:00:00Z')",
            [],
        ).unwrap();
        // Recent — inside the horizon, must SURVIVE.
        conn.execute(
            "INSERT INTO sync_tombstones (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('suggestions', 'recent', 'dev-a', 200, 0, datetime('now', '-10 days'))",
            [],
        ).unwrap();
    }

    // retention=30 → horizon = max(30,90) = 90 days.
    let gc = storage.gc_sync_tombstones(30).unwrap();
    assert_eq!(gc, 2, "both old tombstones (local + wire format) GC'd");

    let conn = storage.conn.test_lock();
    let remaining: i64 = conn
        .query_row("SELECT COUNT(*) FROM sync_tombstones", [], |r| r.get(0))
        .unwrap();
    assert_eq!(remaining, 1, "recent tombstone survives the horizon");
    let survivor: String = conn
        .query_row("SELECT row_id FROM sync_tombstones", [], |r| r.get(0))
        .unwrap();
    assert_eq!(survivor, "recent");
}

#[test]
fn gc_sync_tombstones_honors_retention_beyond_90() {
    // A configured retention > 90 extends the horizon (keep tombstones ≥ as long as data).
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    {
        let conn = storage.conn.test_lock();
        // 120 days old: GC'd at horizon 90, but SPARED at horizon 180.
        conn.execute(
            "INSERT INTO sync_tombstones (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('activity_segments', 't-120', 'dev-a', 100, 0, datetime('now', '-120 days'))",
            [],
        ).unwrap();
    }
    // retention=180 → horizon = max(180,90) = 180 → the 120d-old row survives.
    let gc = storage.gc_sync_tombstones(180).unwrap();
    assert_eq!(
        gc, 0,
        "retention>90 keeps tombstones younger than the larger horizon"
    );
}

// ── Subtask A: configure_connection PRAGMA parity ────────────────

#[test]
fn in_memory_applies_cache_size_pragma() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let conn = storage.conn.test_lock();
    let cache_size: i64 = conn
        .query_row("PRAGMA cache_size", [], |row| row.get(0))
        .unwrap();
    assert_eq!(cache_size, 8000, "in-memory DB should have cache_size=8000");
}

#[test]
fn in_memory_applies_temp_store_pragma() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let conn = storage.conn.test_lock();
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
    let conn = storage.conn.test_lock();
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
    let conn = storage.conn.test_lock();
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
    let conn = storage.conn.test_lock();
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
    let conn = storage.conn.test_lock();
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

/// #4801 R2: on success `set_meta_checked` must return Ok and the value must be stored.
#[test]
fn set_meta_checked_stores_value_and_returns_ok() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // Call set_meta_checked when the key does not exist yet.
    storage
        .set_meta_checked("pending_local_erase", "1")
        .expect("set_meta_checked must not fail on an in-memory DB");

    // The value must be stored correctly.
    assert_eq!(
        storage.get_meta("pending_local_erase"),
        Some("1".to_string()),
        "the value must be readable via get_meta after set_meta_checked"
    );
}

/// #5156: the `ErasurePropagationStore` impl round-trips through `app_meta`.
#[test]
fn erasure_propagation_store_roundtrip() {
    use maekon_core::ports::erasure_propagation_store::ErasurePropagationStore;
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    assert_eq!(
        storage.last_pushed_erasure_id(),
        None,
        "no erasure recorded initially"
    );

    storage
        .record_pushed_erasure_id("2026-06-05T08:11:24.336923+00:00")
        .expect("record must succeed on an in-memory DB");
    assert_eq!(
        storage.last_pushed_erasure_id(),
        Some("2026-06-05T08:11:24.336923+00:00".to_string()),
        "the recorded erasure id round-trips through app_meta"
    );

    // A genuinely distinct erasure overwrites the marker.
    storage
        .record_pushed_erasure_id("2026-06-06T00:00:00+00:00")
        .unwrap();
    assert_eq!(
        storage.last_pushed_erasure_id(),
        Some("2026-06-06T00:00:00+00:00".to_string()),
        "a new erasure id replaces the previous one"
    );
}

/// #4801 R2: `set_meta_checked` must update an existing value via OR REPLACE semantics.
#[test]
fn set_meta_checked_overwrites_existing_key() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.set_meta_checked("k", "v1").unwrap();
    storage.set_meta_checked("k", "v2").unwrap();

    assert_eq!(storage.get_meta("k"), Some("v2".to_string()));
}

/// #4801 R2: on success `delete_meta_checked` must return Ok and the key must be deleted.
#[test]
fn delete_meta_checked_removes_key_and_returns_ok() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    storage.set_meta_checked("marker", "1").unwrap();
    assert!(storage.get_meta("marker").is_some());

    storage
        .delete_meta_checked("marker")
        .expect("delete_meta_checked must not fail on an in-memory DB");

    assert_eq!(
        storage.get_meta("marker"),
        None,
        "the key must be deleted after delete_meta_checked"
    );
}

/// #4801 R2: deleting a non-existent key via `delete_meta_checked` must still return Ok.
#[test]
fn delete_meta_checked_nonexistent_key_returns_ok() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // Deleting a missing key — SQL DELETE is not an error when no row matches, so it must be Ok.
    // Line 1038: delete_meta_checked on a missing key returns Ok(()) — no error raised.
    storage
        .delete_meta_checked("does_not_exist")
        .expect("deleting a non-existent key must return Ok");
    // Read-back: key must still be absent (idempotent delete, no phantom insertion).
    assert_eq!(
        storage.get_meta("does_not_exist"),
        None,
        "key must remain absent after delete_meta_checked on a non-existent key"
    );
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
fn sqlcipher_plaintext_db_with_encryption_key_fails_closed() {
    use crate::encryption::EncryptionKey;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("plain.db");

    // Create an unencrypted database first
    {
        let storage = SqliteStorage::open(&db_path, 30, None).unwrap();
        storage.set_meta("hello", "world");
    }

    // Reopen with an encryption key — must not fall back to plaintext.
    let key = EncryptionKey::from_bytes([0x42; 32]);
    let msg = match SqliteStorage::open(&db_path, 30, Some(&key)) {
        Ok(_) => panic!("plaintext DB must fail closed when encryption is requested"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("plaintext SQLite database"),
        "expected a plaintext fail-closed error, got: {msg}"
    );
}

#[test]
fn sqlcipher_wrong_key_fails_closed_not_plaintext() {
    use crate::encryption::EncryptionKey;
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("encrypted.db");

    // Create an encrypted database with key A and write data.
    {
        let storage =
            SqliteStorage::open(&db_path, 30, Some(&EncryptionKey::from_bytes([0x42; 32])))
                .expect("create encrypted DB with key A");
        storage.set_meta("secret", "value");
    }

    // #6438 (F8): reopening the encrypted DB with the WRONG key must FAIL CLOSED. The
    // on-disk file is SQLCipher-encrypted (no plaintext magic header), so the key
    // verification failure is a wrong/rotated key — NOT a legacy-unencrypted migration —
    // and must not silently reopen as plaintext.
    let wrong = EncryptionKey::from_bytes([0x99; 32]);
    let msg = match SqliteStorage::open(&db_path, 30, Some(&wrong)) {
        Ok(_) => panic!("wrong key must fail closed, not reopen as plaintext"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("wrong or rotated key"),
        "expected a fail-closed wrong-key error, got: {msg}"
    );
}

#[test]
fn is_plaintext_sqlite_detects_magic_header() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();

    // A real plaintext SQLite file starts with the 16-byte magic header.
    let plain = dir.path().join("plain.bin");
    std::fs::File::create(&plain)
        .unwrap()
        .write_all(b"SQLite format 3\0trailing data")
        .unwrap();
    assert!(
        super::is_plaintext_sqlite(&plain),
        "the SQLite magic header must be detected as plaintext"
    );

    // Encrypted-looking bytes (no magic) must NOT be treated as plaintext.
    let enc = dir.path().join("enc.bin");
    std::fs::File::create(&enc)
        .unwrap()
        .write_all(&[0xAB_u8; 64])
        .unwrap();
    assert!(
        !super::is_plaintext_sqlite(&enc),
        "a non-magic file (encrypted / wrong key) must not be treated as plaintext"
    );

    // Empty and missing files are not legacy plaintext DBs.
    let empty = dir.path().join("empty.bin");
    std::fs::File::create(&empty).unwrap();
    assert!(
        !super::is_plaintext_sqlite(&empty),
        "an empty file is not a legacy plaintext DB"
    );
    assert!(
        !super::is_plaintext_sqlite(&dir.path().join("missing.bin")),
        "a missing file is not a legacy plaintext DB"
    );
}

// ── PR3 underlying-impl gap coverage ───────────────────────────────
//
// Phase 5-D8 PR3 audit identified 4 underlying SqliteStorage methods
// lacking coverage in any sibling file. (2 more — save_rule_suggestion_sync
// + mark_unified_suggestion_shown — deferred to follow-up; require heavy
// Suggestion fixture.)

#[test]
fn update_coaching_event_personalized_updates_existing() {
    use maekon_core::models::coaching::CoachingEventRow;

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
    use maekon_core::models::work_session::AppCategory;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let session = storage
        .start_work_session("VSCode", AppCategory::Development)
        .unwrap();

    storage.add_deep_work_secs(session.id, 30).unwrap();
    storage.add_deep_work_secs(session.id, 45).unwrap();

    // Verify via direct SQL that the deep_work_secs column reflects the sum.
    let conn = storage.conn.test_lock();
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
    use maekon_core::models::work_session::AppCategory;

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

    let conn = storage.conn.test_lock();
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
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

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
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

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

// ── recent_audit_entries (#4819 — OSS local audit-log export) ─────────────

#[test]
fn recent_audit_entries_returns_all_newest_first() {
    use chrono::Utc;
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite open");

    // Record 5 entries with different command_ids at descending timestamps.
    for i in 0..5_i64 {
        let entry = AuditEntry {
            entry_id: format!("rae-{i}"),
            timestamp: Utc::now() - chrono::Duration::milliseconds(i),
            session_id: "s".to_string(),
            command_id: format!("cmd-{}", i % 2),
            action_type: "test".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(10),
        };
        storage.save_audit_entry(&entry);
    }

    let results = storage.recent_audit_entries(100);
    assert_eq!(results.len(), 5, "all rows regardless of command_id");
    for w in results.windows(2) {
        assert!(
            w[0].timestamp >= w[1].timestamp,
            "expected newest-first ordering"
        );
    }
}

#[test]
fn recent_audit_entries_respects_limit_clamp() {
    use chrono::Utc;
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    // #7600: a single fixed base timestamp with a 1-second-per-row gap, instead
    // of calling `Utc::now()` once per iteration with millisecond offsets. The
    // old pattern could tie on fast hosts (repeated `Utc::now()` calls are not
    // guaranteed to advance at sub-millisecond granularity), making the
    // `ORDER BY timestamp DESC` tiebreak — and thus which row lands first —
    // nondeterministic (observed as `rae-lim-1` racing `rae-lim-0`). A coarse,
    // strictly decreasing offset from one captured base removes any dependency
    // on wall-clock call timing.
    let base = Utc::now();
    for i in 0..10_i64 {
        let entry = AuditEntry {
            entry_id: format!("rae-lim-{i}"),
            timestamp: base - chrono::Duration::seconds(i),
            session_id: "s".to_string(),
            command_id: "cmd".to_string(),
            action_type: "test".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(5),
        };
        storage.save_audit_entry(&entry);
    }
    let results = storage.recent_audit_entries(3);
    assert_eq!(results.len(), 3, "limit clamps row count");
    // The most recent entry (smallest i) must come first.
    assert_eq!(results[0].entry_id, "rae-lim-0");
}

#[test]
fn recent_audit_entries_empty_when_no_rows() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    assert!(storage.recent_audit_entries(100).is_empty());
}

#[test]
fn audit_stats_aggregate_terminal_statuses_from_storage() {
    use chrono::Utc;
    use maekon_core::models::audit::{AuditEntry, AuditStatus};

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    let statuses = [
        AuditStatus::Completed,
        AuditStatus::Failed,
        AuditStatus::Denied,
        AuditStatus::Timeout,
        AuditStatus::Started,
    ];
    for (index, status) in statuses.into_iter().enumerate() {
        storage.save_audit_entry(&AuditEntry {
            entry_id: format!("audit-stats-{index}"),
            timestamp: Utc::now(),
            session_id: "system.privacy".to_string(),
            command_id: "system.privacy".to_string(),
            action_type: "test".to_string(),
            status,
            details: None,
            execution_time_ms: None,
        });
    }

    let stats = storage.audit_stats();

    assert_eq!(stats.total, 4, "Started is not a terminal execution");
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.failed, 1);
    assert_eq!(stats.denied, 1);
    assert_eq!(stats.timeout, 1);
}

#[test]
fn audit_stats_are_empty_when_storage_has_no_rows() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    let stats = storage.audit_stats();
    assert_eq!(stats.total, 0);
    assert_eq!(stats.completed, 0);
}

// ── Egress audit ledger (V36, #4803/E20) ────────────────────────────

fn make_egress_record(record_id: &str, disposition: &str) -> EgressLedgerRecord {
    EgressLedgerRecord {
        record_id: record_id.to_string(),
        event_type: "Context".to_string(),
        event_id: Some("evt-1".to_string()),
        byte_count: 128,
        recipient_count: 1,
        destination: "server.batch_upload".to_string(),
        disposition: disposition.to_string(),
        consent_state: "telemetry=true".to_string(),
        occurred_at: Utc::now().to_rfc3339(),
    }
}

#[test]
fn record_egress_round_trip() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    let rec = make_egress_record("egr-1", "uploaded");
    storage.record_egress(&rec).expect("record_egress");

    let rows = storage.recent_egress(10);
    assert_eq!(rows.len(), 1);
    let got = &rows[0];
    assert_eq!(got.record_id, "egr-1");
    assert_eq!(got.event_type, "Context");
    assert_eq!(got.event_id.as_deref(), Some("evt-1"));
    assert_eq!(got.byte_count, 128);
    assert_eq!(got.destination, "server.batch_upload");
    assert_eq!(got.disposition, "uploaded");
    assert_eq!(got.consent_state, "telemetry=true");
}

#[test]
fn record_egress_unique_dedupes() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    storage
        .record_egress(&make_egress_record("egr-dup", "uploaded"))
        .expect("first insert");
    // Re-inserting the same record_id must be ignored via UNIQUE + INSERT OR IGNORE.
    storage
        .record_egress(&make_egress_record("egr-dup", "blocked"))
        .expect("second insert (ignored)");

    let rows = storage.recent_egress(10);
    assert_eq!(rows.len(), 1, "the same record_id must not be stored twice");
    // The first insert (disposition='uploaded') is preserved.
    assert_eq!(rows[0].disposition, "uploaded");
}

#[test]
fn egress_between_filters_by_occurred_at() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");

    let old_ts = (Utc::now() - Duration::days(10)).to_rfc3339();
    let mid_ts = (Utc::now() - Duration::days(5)).to_rfc3339();
    let new_ts = Utc::now().to_rfc3339();

    for (id, ts) in [
        ("egr-old", &old_ts),
        ("egr-mid", &mid_ts),
        ("egr-new", &new_ts),
    ] {
        let mut rec = make_egress_record(id, "uploaded");
        rec.occurred_at = ts.clone();
        storage.record_egress(&rec).expect("record_egress");
    }

    let from = (Utc::now() - Duration::days(7)).to_rfc3339();
    let to = (Utc::now() + Duration::days(1)).to_rfc3339();
    let rows = storage.egress_between(&from, &to);
    // Only mid + new fall within the range (old is excluded at -10 days).
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].record_id, "egr-mid", "ascending order");
    assert_eq!(rows[1].record_id, "egr-new");
}

#[test]
fn recent_egress_empty_when_no_rows() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    assert!(storage.recent_egress(100).is_empty());
}

#[test]
fn egress_ledger_survives_gdpr_delete_all_data() {
    // #4803/E20 + xcut: egress_ledger is deliberately absent from ALL_TABLES, so it must
    // be preserved as compliance evidence even after a GDPR Art.17 full erasure
    // (delete_all_data) — the same processing-record rationale as audit_log; it holds
    // only PII-free metadata.
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    storage
        .record_egress(&make_egress_record("egr-survives", "uploaded"))
        .expect("record_egress");
    assert_eq!(storage.recent_egress(10).len(), 1);

    storage.delete_all_data().expect("delete_all_data");

    let rows = storage.recent_egress(10);
    assert_eq!(
        rows.len(),
        1,
        "egress_ledger rows must survive a GDPR full erasure (compliance evidence)"
    );
    assert_eq!(rows[0].record_id, "egr-survives");
}

#[test]
fn sync_tombstones_survives_gdpr_delete_all_data() {
    // #5174/#5178/E20: sync_tombstones is the cross-device deletion-convergence outbox.
    // It is deliberately absent from ALL_TABLES, so its skeleton (id+HLC) must survive a
    // GDPR Art.17 full erasure — that way a peer that was offline can later receive the
    // tombstone and converge. (S1 covers schema + preservation only; the automatic id-capture
    // INSERT on erase is S2/#5179.) Here a raw INSERT proves the preservation.
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO sync_tombstones \
             (table_name, row_id, origin_device_id, hlc_wall_ms, hlc_counter, deleted_at) \
             VALUES ('activity_segments','seg-survives','dev-a',1000,0,'2026-06-05T00:00:00Z')",
            [],
        )
        .expect("insert tombstone");
    }

    storage.delete_all_data().expect("delete_all_data");

    let surviving: i64 = {
        let conn = storage.conn.test_lock();
        conn.query_row(
            "SELECT COUNT(*) FROM sync_tombstones WHERE row_id = 'seg-survives'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        surviving, 1,
        "the sync_tombstones skeleton must survive a GDPR full erasure (offline-peer convergence carrier)"
    );
}

#[test]
fn hlc_clock_is_erased_by_gdpr_delete_all_data() {
    // F0/#5186: hlc_clock is activity-timing metadata (wall_ms) and is therefore subject to
    // GDPR Art.17 erasure — in contrast to sync_tombstones/egress_ledger (preserved), it is
    // included in ALL_TABLES and must be emptied by delete_all_data. (Re-seeded from the
    // retained anchor on restart.)
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    {
        let conn = storage.conn.test_lock();
        // Bump the clock to a non-zero value (to make it clear data was present).
        conn.execute(
            "UPDATE hlc_clock SET last_wall_ms = 12345, last_counter = 6 WHERE id = 0",
            [],
        )
        .expect("bump clock");
    }

    storage.delete_all_data().expect("delete_all_data");

    let remaining: i64 = {
        let conn = storage.conn.test_lock();
        conn.query_row("SELECT COUNT(*) FROM hlc_clock", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        remaining, 0,
        "hlc_clock must be emptied by a GDPR full erasure (activity-timing metadata, in erasure scope)"
    );
}

// ── audit_log SHA-256 hash chain (V37, #4834/E20) ────────────────────────────

fn make_audit_entry(
    entry_id: &str,
    details: Option<&str>,
) -> maekon_core::models::audit::AuditEntry {
    use maekon_core::models::audit::{AuditEntry, AuditStatus};
    AuditEntry {
        entry_id: entry_id.to_string(),
        timestamp: Utc::now(),
        session_id: "sess-chain".to_string(),
        command_id: "cmd-chain".to_string(),
        action_type: "MouseClick".to_string(),
        status: AuditStatus::Completed,
        details: details.map(|s| s.to_string()),
        execution_time_ms: Some(7),
    }
}

/// Read the chain row's (seq, prev_hash, entry_hash) by entry_id.
fn chain_fields(
    storage: &SqliteStorage,
    entry_id: &str,
) -> (Option<i64>, Option<String>, Option<String>) {
    let conn = storage.conn.test_lock();
    conn.query_row(
        "SELECT seq, prev_hash, entry_hash FROM audit_log WHERE entry_id = ?1",
        [entry_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}

#[test]
fn save_audit_entry_genesis_on_first_then_links() {
    use crate::audit_chain::GENESIS_PREV_HASH;

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");

    storage.save_audit_entry(&make_audit_entry("c-0", Some("first")));
    let (seq0, prev0, hash0) = chain_fields(&storage, "c-0");
    assert_eq!(seq0, Some(0), "first row seq is 0");
    assert_eq!(
        prev0.as_deref(),
        Some(GENESIS_PREV_HASH),
        "first row prev_hash is genesis"
    );
    let hash0 = hash0.expect("entry_hash present");

    storage.save_audit_entry(&make_audit_entry("c-1", Some("second")));
    let (seq1, prev1, _hash1) = chain_fields(&storage, "c-1");
    assert_eq!(seq1, Some(1), "second row seq is 1");
    assert_eq!(
        prev1.as_deref(),
        Some(hash0.as_str()),
        "second row prev_hash must equal the previous entry_hash"
    );
}

#[test]
fn verify_audit_chain_clean_is_ok() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    for i in 0..5 {
        storage.save_audit_entry(&make_audit_entry(&format!("ok-{i}"), Some("d")));
    }
    let report = storage.verify_audit_chain();
    assert!(report.ok, "clean chain should be ok: {report:?}");
    assert_eq!(report.first_seq, Some(0));
    assert_eq!(report.last_seq, Some(4));
    assert_eq!(report.verified_count, 5);
    assert_eq!(report.legacy_unchained_count, 0);
    assert!(report.first_break.is_none());
}

/// #7600: the async wrapper (used by the `GET /audit/verify` HTTP handler via
/// `AuditChainVerifierPort`) must agree with the sync `verify_audit_chain` it
/// delegates to — same underlying `verify_audit_chain_impl`, just offloaded
/// onto `spawn_blocking`.
#[tokio::test]
async fn verify_audit_chain_async_matches_sync_report() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    for i in 0..5 {
        storage.save_audit_entry(&make_audit_entry(&format!("ok-{i}"), Some("d")));
    }
    let sync_report = storage.verify_audit_chain();
    let async_report = storage.verify_audit_chain_async().await;
    assert_eq!(sync_report, async_report);
    assert!(
        async_report.ok,
        "clean chain should be ok: {async_report:?}"
    );
    assert_eq!(async_report.verified_count, 5);
}

/// The `AuditChainVerifierPort` trait impl must reach the same real logic —
/// not a stub — when invoked through a `dyn` port reference, mirroring how
/// `AppState.core.audit_chain_verifier` is consumed by the HTTP handler.
#[tokio::test]
async fn audit_chain_verifier_port_impl_reaches_real_verification() {
    use maekon_core::ports::audit_chain_verifier::AuditChainVerifierPort;
    use std::sync::Arc;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"));
    storage.save_audit_entry(&make_audit_entry("via-port", Some("d")));

    let verifier: Arc<dyn AuditChainVerifierPort> = storage.clone();
    let report = verifier.verify_audit_chain().await;

    assert!(
        report.ok,
        "port-mediated verification should be ok: {report:?}"
    );
    assert_eq!(report.verified_count, 1);
}

#[test]
fn verify_audit_chain_empty_is_ok() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    let report = storage.verify_audit_chain();
    assert!(report.ok);
    assert_eq!(report.verified_count, 0);
    assert_eq!(report.first_seq, None);
}

#[test]
fn verify_audit_chain_detects_tampered_details() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    for i in 0..4 {
        storage.save_audit_entry(&make_audit_entry(&format!("t-{i}"), Some("orig")));
    }
    // Tamper directly with the details of the seq=2 row (leaving entry_hash) → recompute mismatch.
    {
        let conn = storage.conn.test_lock();
        conn.execute(
            "UPDATE audit_log SET details = 'TAMPERED' WHERE seq = 2",
            [],
        )
        .unwrap();
    }
    let report = storage.verify_audit_chain();
    assert!(!report.ok, "tamper must be detected");
    let brk = report.first_break.expect("break present");
    assert_eq!(brk.seq, 2, "the tamper must be detected at seq=2");
    assert!(brk.reason.contains("tampered") || brk.reason.contains("mismatch"));
}

#[test]
fn verify_audit_chain_detects_middle_delete_gap() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    for i in 0..4 {
        storage.save_audit_entry(&make_audit_entry(&format!("g-{i}"), Some("d")));
    }
    // Delete the seq=1 row → break in seq continuity (gap).
    {
        let conn = storage.conn.test_lock();
        conn.execute("DELETE FROM audit_log WHERE seq = 1", [])
            .unwrap();
    }
    let report = storage.verify_audit_chain();
    assert!(!report.ok, "gap must be detected");
    let brk = report.first_break.expect("break present");
    assert_eq!(brk.seq, 2, "the gap must be detected at seq=2");
    assert!(brk.reason.contains("gap"), "reason = {}", brk.reason);
}

#[test]
fn verify_audit_chain_detects_timestamp_reorder() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    for i in 0..3 {
        storage.save_audit_entry(&make_audit_entry(&format!("ts-{i}"), Some("d")));
    }
    // Tamper with the seq=1 row's timestamp → canonical input changes → recompute mismatch.
    {
        let conn = storage.conn.test_lock();
        conn.execute(
            "UPDATE audit_log SET timestamp = '2099-01-01T00:00:00+00:00' WHERE seq = 1",
            [],
        )
        .unwrap();
    }
    let report = storage.verify_audit_chain();
    assert!(!report.ok, "timestamp tamper must be detected");
    assert_eq!(report.first_break.expect("break").seq, 1);
}

#[test]
fn verify_audit_chain_detects_broken_link() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    for i in 0..3 {
        storage.save_audit_entry(&make_audit_entry(&format!("lk-{i}"), Some("d")));
    }
    // Tamper with the seq=2 row's prev_hash to an arbitrary value → link integrity violation.
    {
        let conn = storage.conn.test_lock();
        conn.execute(
            "UPDATE audit_log SET prev_hash = 'deadbeef' WHERE seq = 2",
            [],
        )
        .unwrap();
    }
    let report = storage.verify_audit_chain();
    assert!(!report.ok);
    assert_eq!(report.first_break.expect("break").seq, 2);
}

#[test]
fn save_audit_entry_duplicate_id_does_not_gap_chain() {
    // When INSERT OR IGNORE is a no-op for a duplicate entry_id, seq must not be consumed.
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    storage.save_audit_entry(&make_audit_entry("dup", Some("first")));
    // Re-record the same entry_id → INSERT OR IGNORE no-op.
    storage.save_audit_entry(&make_audit_entry("dup", Some("ignored")));
    // New entry_id → seq must be 1 (the duplicate does not consume a seq).
    storage.save_audit_entry(&make_audit_entry("next", Some("d")));

    let (seq_dup, _, _) = chain_fields(&storage, "dup");
    let (seq_next, _, _) = chain_fields(&storage, "next");
    assert_eq!(seq_dup, Some(0));
    assert_eq!(
        seq_next,
        Some(1),
        "the duplicate does not consume a seq, so the chain is gap-free"
    );

    let report = storage.verify_audit_chain();
    assert!(report.ok, "chain must remain valid: {report:?}");
    assert_eq!(report.verified_count, 2);
}

#[test]
fn save_audit_entry_concurrent_chain_is_gap_free() {
    use std::sync::Arc;
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"));
    let mut handles = Vec::new();
    for t in 0..4 {
        let s = Arc::clone(&storage);
        handles.push(std::thread::spawn(move || {
            for i in 0..10 {
                s.save_audit_entry(&make_audit_entry(&format!("cc-{t}-{i}"), Some("d")));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let report = storage.verify_audit_chain();
    assert!(report.ok, "concurrent chain must be valid: {report:?}");
    assert_eq!(
        report.verified_count, 40,
        "all 40 entries are incorporated gap-free"
    );
    assert_eq!(report.first_seq, Some(0));
    assert_eq!(report.last_seq, Some(39));
}

#[test]
fn legacy_null_chain_rows_excluded_and_counted() {
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    // Inject a legacy row with NULL chain columns directly, as if written before v37.
    {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO audit_log \
             (entry_id, timestamp, session_id, command_id, action_type, status) \
             VALUES ('legacy-a', '2026-01-01T00:00:00+00:00', 's', 'c', 'a', 'Completed')",
            [],
        )
        .unwrap();
    }
    // Then a normal write → starts a new chain from genesis.
    storage.save_audit_entry(&make_audit_entry("post-1", Some("d")));

    let report = storage.verify_audit_chain();
    assert!(
        report.ok,
        "legacy rows must be excluded from chain verification: {report:?}"
    );
    assert_eq!(report.legacy_unchained_count, 1);
    assert_eq!(report.verified_count, 1);
    assert_eq!(
        report.first_seq,
        Some(0),
        "the post-migration write starts from genesis (0)"
    );
}

#[test]
fn save_audit_entry_appends_chain_even_with_deletion_flag_set() {
    // #4928 interaction: even inside the erase window (deletion_flag set), audit_log must keep
    // appending via retained_write_lock and the chain must keep extending.
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    storage.save_audit_entry(&make_audit_entry("pre-erase", Some("d")));

    // Set deletion_flag (simulates consent revoke / entering erase).
    let flag = Arc::new(AtomicBool::new(true));
    storage
        .connection_arc()
        .set_deletion_flag(Arc::clone(&flag));
    assert!(flag.load(Ordering::Acquire));

    // A consent_revoked audit occurs during erase — it must append because it is the retained path.
    storage.save_audit_entry(&make_audit_entry("during-erase", Some("consent_revoked")));

    let report = storage.verify_audit_chain();
    assert!(
        report.ok,
        "the chain must stay valid even with deletion_flag set: {report:?}"
    );
    assert_eq!(
        report.verified_count, 2,
        "audit_log keeps appending even with deletion_flag set (retained path)"
    );
    let (seq_during, _, _) = chain_fields(&storage, "during-erase");
    assert_eq!(
        seq_during,
        Some(1),
        "a write during erase also extends the chain"
    );
}

/// Regression (#6168): `save_session_audit_entry` must actually persist a row to
/// `session_audit_log`. Before the fix the only production `AuditLogPort` impl
/// left `record_session_event` as a no-op, so the table got zero production
/// writes — the entire AI-conversation session audit trail was silently dropped.
#[test]
fn save_session_audit_entry_persists_row() {
    use maekon_core::models::ai_session::{SessionAuditCategory, SessionAuditEntry};

    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    let entry = SessionAuditEntry {
        timestamp: Utc::now(),
        session_id: "sess-6168".to_string(),
        category: SessionAuditCategory::ToolUse,
        event_type: "inbound".to_string(),
        provider: "claude".to_string(),
        payload: Some(serde_json::json!({ "role": "user" })),
        duration_ms: Some(42),
    };
    storage.save_session_audit_entry(&entry);

    let conn = storage.conn.test_lock();
    let (session_id, category, event_type, provider, payload, duration): (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<i64>,
    ) = conn
        .query_row(
            "SELECT session_id, category, event_type, provider, payload, duration_ms \
             FROM session_audit_log WHERE session_id = ?1",
            ["sess-6168"],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .expect("session audit row must be persisted");

    assert_eq!(session_id, "sess-6168");
    // `SessionAuditCategory` serializes `snake_case` — ToolUse → "tool_use".
    assert_eq!(category, "tool_use");
    assert_eq!(event_type, "inbound");
    assert_eq!(provider, "claude");
    assert_eq!(duration, Some(42));
    let payload_json: serde_json::Value =
        serde_json::from_str(&payload.expect("payload stored")).expect("payload is valid json");
    assert_eq!(payload_json["role"], "user");
}

/// #7946: WHY upload producers must pair the persisted id with the payload —
/// egress filtering can change id-relevant fields (a Context event's window
/// title feeds the id), so an id derived from the FILTERED payload would
/// stamp the wrong (or no) row as sent.
#[test]
fn storage_event_id_diverges_when_title_is_redacted() {
    use maekon_core::models::event::{ContextEvent, Event};

    let original = Event::Context(ContextEvent {
        app_name: "Mail".to_string(),
        window_title: "Quarterly numbers — draft".to_string(),
        prev_app_name: None,
        timestamp: chrono::Utc::now(),
        ..Default::default()
    });
    let redacted = match &original {
        Event::Context(ctx) => Event::Context(ContextEvent {
            window_title: "[REDACTED_WINDOW_TITLE]".to_string(),
            ..ctx.clone()
        }),
        _ => unreachable!(),
    };

    let id_original = crate::sqlite::storage_event_id(&original);
    let id_redacted = crate::sqlite::storage_event_id(&redacted);
    assert_ne!(
        id_original, id_redacted,
        "title redaction changes the derived id — producers must capture the id BEFORE egress filtering"
    );
}

#[test]
fn vault_mirror_state_is_erased_by_gdpr_delete_all_data() {
    // #9465/ADR-033 §1.4 regression guard (fail-before/pass-after at the
    // hash-state layer): an erase-surviving vault_mirror_state row would make
    // the next mirror cycle see "hash matches" and silently skip regenerating
    // files Art.17 Phase-3 just deleted. The table must be in ALL_TABLES.
    let storage = SqliteStorage::open_in_memory(30).expect("sqlite");
    {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO vault_mirror_state (file_name, content_hash, updated_at)
             VALUES ('claims.md', 'h1', 1), ('daily/2026-07-29.md', 'h2', 2)",
            [],
        )
        .expect("seed vault hashes");
    }

    storage.delete_all_data().expect("delete_all_data");

    let remaining: i64 = {
        let conn = storage.conn.test_lock();
        conn.query_row("SELECT COUNT(*) FROM vault_mirror_state", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(
        remaining, 0,
        "vault_mirror_state must be emptied by GDPR delete_all_data (ADR-033 §1.4)"
    );
}
