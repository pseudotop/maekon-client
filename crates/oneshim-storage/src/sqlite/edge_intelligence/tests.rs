use chrono::Utc;
#[allow(deprecated)]
use oneshim_core::models::work_session::{
    AppCategory, FocusMetrics, Interruption, LocalSuggestion,
};
use oneshim_core::ports::web_storage::DigestStorage;

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

    storage.record_interruption_resume(int_id, "Code").unwrap();

    let pending = storage.get_pending_interruption().unwrap();
    assert!(pending.is_none());
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

#[test]
#[allow(deprecated)]
fn local_suggestion_persistence() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    let suggestion = LocalSuggestion::NeedFocusTime {
        communication_ratio: 0.6,
        suggested_focus_mins: 25,
    };

    let id = storage.save_local_suggestion(&suggestion).unwrap();
    assert!(id > 0);

    storage.mark_suggestion_shown(id).unwrap();
    storage.mark_suggestion_dismissed(id).unwrap();

    let suggestion2 = LocalSuggestion::TakeBreak {
        continuous_work_mins: 90,
    };
    let id2 = storage.save_local_suggestion(&suggestion2).unwrap();
    storage.mark_suggestion_acted(id2).unwrap();
}

#[test]
fn segments_for_date_query() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // Insert a test segment
    {
        let conn = storage.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, trigger_reason, dominant_category, event_count, avg_importance)
             VALUES ('seg-001', '2026-03-19T09:00:00Z', '2026-03-19T10:00:00Z', 3600, 'SCORE_HIGH', 'Development', 50, 0.8)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO activity_segments (id, start_time, end_time, duration_secs, trigger_reason, dominant_category, event_count, avg_importance)
             VALUES ('seg-002', '2026-03-20T09:00:00Z', '2026-03-20T10:00:00Z', 3600, 'SCORE_HIGH', 'Communication', 30, 0.5)",
            [],
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
