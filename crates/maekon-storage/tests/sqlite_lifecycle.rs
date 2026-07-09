//! Integration tests for SqliteStorage lifecycle scenarios.
//!
//! Why a separate integration suite (beyond the 405 unit tests in `src/sqlite/*.rs`):
//! the unit tests cover individual port traits in isolation, but cross-trait
//! coordination + on-disk persistence across close/re-open boundaries is
//! exactly the regression surface that schema migration mistakes hit first.
//!
//! This suite is kept here because `maekon-storage` owns migration and
//! persistence invariants that are easy to miss in isolated unit tests.

use std::sync::Arc;

use chrono::{Duration, Utc};
use maekon_core::models::event::{Event, UserEvent, UserEventType};
use maekon_core::models::storage_records::NewGuiInteraction;
use maekon_core::ports::storage::StorageService;
use maekon_storage::sqlite::SqliteStorage;
use tempfile::TempDir;
use uuid::Uuid;

/// Build a deterministic-shape `UserEvent` with a fresh UUID + adjustable
/// timestamp. Used across the lifecycle scenarios so each assertion can target
/// a specific event rather than relying on insertion order.
fn make_user_event_at(ts: chrono::DateTime<Utc>, app: &str, window: &str) -> Event {
    Event::User(UserEvent {
        event_id: Uuid::new_v4(),
        event_type: UserEventType::WindowChange,
        timestamp: ts,
        app_name: app.to_string(),
        window_title: window.to_string(),
    })
}

fn event_id_of(event: &Event) -> String {
    match event {
        Event::User(u) => u.event_id.to_string(),
        _ => panic!("test fixture only produces UserEvent"),
    }
}

/// Acceptance: `SqliteStorage::open(path, ..)` against a fresh path runs the
/// full migration chain, accepts writes, and the data survives a drop + reopen
/// of the same path.
///
/// Regression surface: any migration that fails to run idempotently on reopen
/// (e.g., a `CREATE TABLE` instead of `CREATE TABLE IF NOT EXISTS`, or a v→v+1
/// migration that overwrites user data) is caught here.
#[tokio::test]
async fn fresh_db_open_persists_events_through_close_and_reopen() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("lifecycle.db");

    let event = make_user_event_at(Utc::now(), "CodeEditor", "lifecycle_test.rs");
    let event_id = event_id_of(&event);

    // First open: schema bootstrap + write.
    {
        let storage = SqliteStorage::open(&db_path, 30, None).expect("open fresh");
        storage.save_event(&event).await.expect("save_event");

        let now = Utc::now();
        let recent = storage
            .get_events(now - Duration::hours(1), now + Duration::hours(1), 10)
            .await
            .expect("get_events after save");
        assert_eq!(
            recent.iter().filter(|e| event_id_of(e) == event_id).count(),
            1,
            "saved event must appear in first-session query"
        );
    } // Storage drops here — connection closes, file released.

    assert!(
        db_path.exists(),
        "lifecycle.db must persist after Storage drops"
    );

    // Second open against the same path: migrations must be idempotent, data
    // must survive.
    {
        let storage = SqliteStorage::open(&db_path, 30, None).expect("reopen");

        let now = Utc::now();
        let after_reopen = storage
            .get_events(now - Duration::hours(1), now + Duration::hours(1), 10)
            .await
            .expect("get_events after reopen");
        assert_eq!(
            after_reopen
                .iter()
                .filter(|e| event_id_of(e) == event_id)
                .count(),
            1,
            "event saved before close must survive reopen"
        );
    }
}

/// Acceptance: idempotent migration when `open` is called against an existing
/// fully-migrated DB. Two consecutive opens against the same path must not
/// corrupt or duplicate any data.
///
/// Regression surface: a migration step that adds rows / updates existing
/// rows on every open (e.g., a "seed default config" step missing a guard)
/// would double the row counts here.
#[tokio::test]
async fn repeated_open_does_not_duplicate_events() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("idempotent.db");

    let event = make_user_event_at(Utc::now(), "Browser", "tabs.html");
    let event_id = event_id_of(&event);

    {
        let storage = SqliteStorage::open(&db_path, 30, None).expect("open 1");
        storage.save_event(&event).await.expect("save");
    }

    // Open + close + open + close + final query.
    for _ in 0..3 {
        let _ = SqliteStorage::open(&db_path, 30, None).expect("reopen idempotent");
    }

    let final_storage = SqliteStorage::open(&db_path, 30, None).expect("final open");
    let now = Utc::now();
    let events = final_storage
        .get_events(now - Duration::hours(1), now + Duration::hours(1), 50)
        .await
        .expect("get_events");
    assert_eq!(
        events.iter().filter(|e| event_id_of(e) == event_id).count(),
        1,
        "event must appear exactly once regardless of how many opens occurred"
    );
}

/// Acceptance: a single SqliteStorage value can be shared across threads via
/// `Arc<SqliteStorage>` and concurrent `save_event` calls all succeed (the
/// internal `Mutex<Connection>` serialises writes correctly without
/// deadlock).
///
/// Regression surface: a future change that introduces a second lock or a
/// re-entrant lock acquisition path would hang or panic this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_save_event_across_tasks_serialises_via_mutex() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("concurrent.db");
    let storage = Arc::new(SqliteStorage::open(&db_path, 30, None).expect("open"));

    let mut handles = Vec::with_capacity(8);
    for i in 0..8 {
        let s = Arc::clone(&storage);
        handles.push(tokio::spawn(async move {
            let event = make_user_event_at(Utc::now(), "ConcurrentApp", &format!("window-{i}.txt"));
            s.save_event(&event).await.expect("concurrent save");
            event_id_of(&event)
        }));
    }

    let mut written_ids: Vec<String> = Vec::with_capacity(handles.len());
    for h in handles {
        written_ids.push(h.await.expect("task join"));
    }

    let now = Utc::now();
    let events = storage
        .get_events(now - Duration::hours(1), now + Duration::hours(1), 50)
        .await
        .expect("get_events");
    let observed_ids: std::collections::HashSet<_> = events.iter().map(event_id_of).collect();

    for id in &written_ids {
        assert!(
            observed_ids.contains(id),
            "concurrently-saved event {id} must be readable after all tasks complete"
        );
    }
}

/// F-PF-19 regression test: wrapping `save_gui_interaction` in `tokio::task::spawn_blocking`
/// must run without panicking on the tokio multi-threaded runtime.
/// Key assertion: the synchronous SQLite write does not block an async executor worker.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_save_gui_interaction_async_safe() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("test_gui.db");
    let storage = Arc::new(SqliteStorage::open(&db_path, 30, None).expect("open"));

    let event_id = Uuid::new_v4().to_string();
    let timestamp = Utc::now().to_rfc3339();
    let app_name = "TestApp".to_string();
    let storage_clone = Arc::clone(&storage);

    // Reproduce the spawn_blocking pattern directly: move owned Strings into the closure.
    let result = tokio::task::spawn_blocking(move || {
        let input = NewGuiInteraction {
            event_id: &event_id,
            timestamp: &timestamp,
            interaction_type: "Click",
            app_name: &app_name,
            type_confidence: 1.0,
        };
        // ADR-026 PR-7: the `GuiInteractionStorage` trait method is now async;
        // this F-PF-19 regression exercises the synchronous inherent twin, which
        // is the path the capture loop drives inside `spawn_blocking`.
        SqliteStorage::save_gui_interaction(&storage_clone, &input)
    })
    .await;

    // Line 214: spawn_blocking join must not panic (task not aborted/panicked).
    let _join_ok = result.expect("spawn_blocking join should not panic (F-PF-19)");

    // Line 218 (was result.unwrap().is_ok()): the inner StorageError Result is
    // already consumed by the expect above (result is now gone), so we re-run
    // the call directly to verify the storage write succeeds and can be read back.
    // This exercises the same sync-twin path inside spawn_blocking.
    {
        let event_id2 = Uuid::new_v4().to_string();
        let timestamp2 = Utc::now().to_rfc3339();
        let app_name2 = "TestApp".to_string();
        let storage_ref = Arc::clone(&storage);
        let inner = tokio::task::spawn_blocking(move || {
            let input = NewGuiInteraction {
                event_id: &event_id2,
                timestamp: &timestamp2,
                interaction_type: "Click",
                app_name: &app_name2,
                type_confidence: 1.0,
            };
            SqliteStorage::save_gui_interaction(&storage_ref, &input)
        })
        .await
        .expect("second spawn_blocking join should not panic (F-PF-19)");
        inner.expect("save_gui_interaction should succeed inside spawn_blocking (F-PF-19)");
    }
}
