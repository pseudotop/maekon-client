use chrono::Utc;
use maekon_core::models::work_session::{AppCategory, FocusMetrics, Interruption};

use super::super::SqliteStorage;

#[test]
fn work_session_lifecycle() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let session = storage
        .start_work_session("Code", AppCategory::Development)
        .unwrap();
    assert!(session.id > 0);
    assert_eq!(session.category, AppCategory::Development);

    let active = storage.get_active_work_session().unwrap();
    assert!(active.is_some());

    storage.end_work_session(session.id).unwrap();

    let active = storage.get_active_work_session().unwrap();
    assert!(active.is_none());
}

#[test]
fn interruption_tracking() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let session = storage
        .start_work_session("Code", AppCategory::Development)
        .unwrap();
    let _ = session; // session ID
    let interruption = Interruption::new(
        0,
        "Code".to_string(),
        "Slack".to_string(),
        None, // snapshot_frame_id
    );

    let int_id = storage.record_interruption(&interruption).unwrap();
    assert!(int_id > 0);

    let pending = storage.get_pending_interruption().unwrap();
    assert!(pending.is_some());

    let resumed = storage
        .resume_interruption(int_id, "Code", Utc::now())
        .unwrap()
        .expect("pending interruption should resume once");
    assert_eq!(resumed.id, int_id);
    assert_eq!(resumed.from_app, "Code");
    assert_eq!(resumed.resumed_to_app.as_deref(), Some("Code"));

    // Simulate a caller crash after commit but before clearing its tracker:
    // retrying the exact ID must not return a second resume snapshot.
    let duplicate = storage
        .resume_interruption(int_id, "Code", Utc::now())
        .unwrap();
    assert!(duplicate.is_none(), "double resume must be a no-op");

    let pending = storage.get_pending_interruption().unwrap();
    assert!(pending.is_none());
}

/// #8578: resume targets the requested pending ID, not the newest row, and
/// returns the pre-update context together with the committed resume fields.
#[test]
fn interruption_resume_is_exact_id_transaction() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let first = Interruption::new(
        0,
        "Visual Studio Code".to_string(),
        "Slack".to_string(),
        None,
    );
    let second = Interruption::new(
        0,
        "Terminal".to_string(),
        "Microsoft Teams".to_string(),
        None,
    );
    let first_id = storage.record_interruption(&first).unwrap();
    let second_id = storage.record_interruption(&second).unwrap();
    assert!(second_id > first_id);

    let resumed_at = Utc::now();
    let resumed = storage
        .resume_interruption(first_id, "Visual Studio Code", resumed_at)
        .unwrap()
        .expect("the exact first row should resume");
    assert_eq!(resumed.id, first_id);
    assert_eq!(resumed.from_app, "Visual Studio Code");
    assert_eq!(resumed.to_app, "Slack");
    assert_eq!(resumed.resumed_at, Some(resumed_at));
    assert_eq!(
        resumed.resumed_to_app.as_deref(),
        Some("Visual Studio Code")
    );

    let pending = storage
        .get_pending_interruption()
        .unwrap()
        .expect("newer unrelated row must remain pending");
    assert_eq!(pending.id, second_id);
    assert_eq!(pending.from_app, "Terminal");
}

/// #8578: an unknown ID cannot mutate whichever interruption is pending.
#[test]
fn unknown_interruption_resume_preserves_pending_row() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let interruption = Interruption::new(
        0,
        "Visual Studio Code".to_string(),
        "Slack".to_string(),
        None,
    );
    let int_id = storage.record_interruption(&interruption).unwrap();

    let resumed = storage
        .resume_interruption(int_id + 10_000, "Visual Studio Code", Utc::now())
        .unwrap();
    assert!(resumed.is_none());

    let pending = storage
        .get_pending_interruption()
        .unwrap()
        .expect("unknown ID must not consume a pending row");
    assert_eq!(pending.id, int_id);
}

#[test]
fn focus_metrics_lifecycle() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let metrics = storage.get_or_create_today_focus_metrics().unwrap();
    assert_eq!(metrics.deep_work_secs, 0);

    // increment_focus_metrics(date, total_active_secs, deep_work_secs, communication_secs, context_switches, interruption_count)
    let today = Utc::now().format("%Y-%m-%d").to_string();
    storage
        .increment_focus_metrics(&today, 300, 200, 100, 5, 2)
        .unwrap();

    let updated = storage.get_or_create_today_focus_metrics().unwrap();
    assert_eq!(updated.total_active_secs, 300);
    assert_eq!(updated.deep_work_secs, 200);
    assert_eq!(updated.communication_secs, 100);
    assert_eq!(updated.context_switches, 5);
    assert_eq!(updated.interruption_count, 2);

    let full_metrics = FocusMetrics::new(updated.period.start, updated.period.end)
        .expect("trusted test bounds — period from get_or_create_today");
    storage.update_focus_metrics(&today, &full_metrics).unwrap();
}

/// #7733: `save_local_suggestion` (the deprecated `LocalSuggestion` enum writer)
/// was deleted as dead code, but the `local_suggestions` table itself is a live
/// read path (`FewShotStorage`, `LocalSuggestionQueryPort`,
/// `WebStorage::list_recent_local_suggestions`). This test now writes fixture
/// rows via raw SQL — the same shape the (now-deleted) writer used to produce —
/// to keep `mark_suggestion_shown`/`mark_suggestion_dismissed`/`mark_suggestion_acted`
/// covered end-to-end against real legacy-shaped rows.
#[test]
fn local_suggestion_persistence() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let id: i64 = {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO local_suggestions (suggestion_type, payload) VALUES (?1, ?2)",
            rusqlite::params![
                "NeedFocusTime",
                serde_json::json!({
                    "communication_ratio": 0.6,
                    "suggested_focus_mins": 25,
                })
                .to_string()
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    };
    assert!(id > 0);

    storage.mark_suggestion_shown(id).unwrap();
    storage.mark_suggestion_dismissed(id).unwrap();

    let id2: i64 = {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO local_suggestions (suggestion_type, payload) VALUES (?1, ?2)",
            rusqlite::params![
                "TakeBreak",
                serde_json::json!({ "continuous_work_mins": 90 }).to_string()
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    };
    storage.mark_suggestion_acted(id2).unwrap();
}

#[test]
fn segments_for_date_query() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // #5664: `get_segments_for_date` interprets the date in the MACHINE-LOCAL
    // timezone. Derive fixture instants from the same production window helper
    // so this test is green on any machine/CI timezone (hardcoded `T09:00Z`
    // fixtures broke on UTC-10-and-westward machines).
    let mid_of = |date: &str| -> String {
        let (from, _) =
            crate::sqlite::web_storage_impl::suggestion_digest_storage::local_date_utc_window(
                date,
                &chrono::Local,
            )
            .unwrap();
        let start = chrono::DateTime::parse_from_rfc3339(&from).unwrap();
        (start + chrono::Duration::hours(1)).to_rfc3339()
    };
    let day1_start = mid_of("2026-03-19");
    let day2_start = mid_of("2026-03-20");

    // Insert a test segment
    {
        let conn = storage.conn.test_lock();
        conn.execute(
            "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, trigger_reason, dominant_category, event_count, avg_importance)
             VALUES ('seg-001', ?1, ?1, 3600, 'SCORE_HIGH', 'Development', 50, 0.8)",
            rusqlite::params![day1_start],
        ).unwrap();
        conn.execute(
            "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, trigger_reason, dominant_category, event_count, avg_importance)
             VALUES ('seg-002', ?1, ?1, 3600, 'SCORE_HIGH', 'Communication', 30, 0.5)",
            rusqlite::params![day2_start],
        ).unwrap();
    }

    let segments = storage.get_segments_for_date("2026-03-19").unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].segment_id, "seg-001");
    assert_eq!(segments[0].dominant_category, "Development");
    assert_eq!(segments[0].duration_secs, 3600);

    // Different date returns different segment
    let segments2 = storage.get_segments_for_date("2026-03-20").unwrap();
    assert_eq!(segments2.len(), 1);
    assert_eq!(segments2[0].segment_id, "seg-002");

    // Non-existent date returns empty
    let empty = storage.get_segments_for_date("2020-01-01").unwrap();
    assert!(empty.is_empty());
}

#[test]
fn app_category_parsing() {
    assert_eq!(
        SqliteStorage::parse_app_category("Communication"),
        AppCategory::Communication
    );
    assert_eq!(
        SqliteStorage::parse_app_category("Development"),
        AppCategory::Development
    );
    assert_eq!(
        SqliteStorage::parse_app_category("Unknown"),
        AppCategory::Other
    );
}
