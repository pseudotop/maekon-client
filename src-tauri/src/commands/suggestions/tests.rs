use super::feedback::{find_suggestion_explain_payload, submit_suggestion_feedback_to_runtime};
use super::queries::{pending_suggestions_snapshot, suggestion_history_snapshot};
use super::replay::{suggestion_replay_log_context, validate_suggestion_replay_payload};
use super::types::SuggestionReplayEventPayload;
use chrono::Utc;
use maekon_core::models::suggestion::{
    Priority, Suggestion, SuggestionContextScope, SuggestionSource, SuggestionType,
};
use maekon_storage::sqlite::SqliteStorage;
use std::sync::Arc;

use crate::runtime_state::SuggestionRuntimeState;

fn sample_suggestion(id: &str) -> Suggestion {
    Suggestion {
        suggestion_id: id.to_string(),
        suggestion_type: SuggestionType::WorkGuidance,
        content: "Review the current GUI before accepting the action.".to_string(),
        priority: Priority::Medium,
        confidence_score: 0.82,
        relevance_score: 0.91,
        is_actionable: true,
        created_at: Utc::now(),
        expires_at: None,
        source: SuggestionSource::LlmLocal,
        reasoning: Some("The active window has a reviewable UI state.".to_string()),
        context_scope: Some(SuggestionContextScope {
            app_name: Some("Calculator".to_string()),
            window_title: Some("Calculator".to_string()),
            target_id: Some("calculator-display-result".to_string()),
        }),
    }
}

#[tokio::test]
async fn pending_suggestions_fall_back_to_storage_without_manager() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
    storage
        .save_rule_suggestion_sync(&sample_suggestion("storage-suggestion-1"))
        .expect("save suggestion");
    let suggestion_state = SuggestionRuntimeState::default();

    let suggestions = pending_suggestions_snapshot(&suggestion_state, &storage)
        .await
        .expect("fallback suggestions");

    assert_eq!(suggestions.len(), 1);
    assert_eq!(suggestions[0].id, "storage-suggestion-1");
    assert_eq!(suggestions[0].source, "local");
    assert_eq!(
        suggestions[0]
            .context_scope
            .as_ref()
            .and_then(|scope| scope.app_name.as_deref()),
        Some("Calculator")
    );
    assert_eq!(
        suggestions[0]
            .context_scope
            .as_ref()
            .and_then(|scope| scope.target_id.as_deref()),
        Some("calculator-display-result")
    );
}

/// E20-24 (#4816): the OSS local pipeline end-to-end. A locally-generated
/// suggestion pushed into the manager's queue (exactly what the analysis producer
/// wire does) must surface through the SAME IPC read path used in production —
/// from the LIVE QUEUE, not the SQLite fallback — with NO server. This is the
/// regression guard for the dead-end the issue describes: before this change the
/// pipeline was `#[cfg(feature = "server")]` and the producer never fed the queue,
/// so an OSS build could not see or act on its own suggestions.
#[cfg(feature = "local-suggestions")]
#[tokio::test]
async fn pending_suggestions_surface_locally_generated_via_live_queue() {
    use maekon_suggestion::deferred::DeferredManager;
    use maekon_suggestion::feedback::FeedbackSender;
    use maekon_suggestion::feedback_retry::FeedbackRetryQueue;
    use maekon_suggestion::history::SuggestionHistory;
    use maekon_suggestion::queue::SuggestionQueue;
    use maekon_suggestion::scorer::FeedbackScorer;
    use tokio::sync::Mutex;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));

    // Build the local SuggestionManager exactly as the OSS `build_suggestion_manager`
    // (local variant) does: no network, FeedbackSender backed by the no-op
    // LocalApiClient.
    let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
    let api: Arc<dyn maekon_core::ports::api_client::ApiClient> =
        Arc::new(crate::local_api_client::LocalApiClient);
    let feedback = Arc::new(FeedbackSender::new_with_sink(api, None));
    let manager = Arc::new(crate::suggestion_manager::SuggestionManager::new(
        queue.clone(),
        Arc::new(Mutex::new(SuggestionHistory::new(100))),
        feedback,
        Arc::new(Mutex::new(FeedbackScorer::new())),
        Arc::new(Mutex::new(DeferredManager::new(50))),
        Arc::new(Mutex::new(FeedbackRetryQueue::new(100, 5))),
        storage.clone(),
    ));

    // Producer wire: a locally-generated suggestion enters the live queue (the
    // SAME Arc the manager — and therefore the IPC read — sees). NOTE: nothing is
    // written to SQLite, so a non-empty result can only come from the live queue.
    let pushed = queue.lock().await.push(sample_suggestion("local-gen-1"));
    assert!(
        pushed,
        "queue should accept the locally-generated suggestion"
    );

    let state = SuggestionRuntimeState::new(Some(manager), None);
    let suggestions = pending_suggestions_snapshot(&state, &storage)
        .await
        .expect("pending suggestions");

    assert_eq!(
        suggestions.len(),
        1,
        "the locally-generated suggestion must surface from the live queue (no server, no SQLite fallback)"
    );
    assert_eq!(suggestions[0].id, "local-gen-1");
}

#[tokio::test]
async fn feedback_falls_back_to_storage_without_manager() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
    storage
        .save_rule_suggestion_sync(&sample_suggestion("storage-feedback-1"))
        .expect("save suggestion");
    let suggestion_state = SuggestionRuntimeState::default();

    submit_suggestion_feedback_to_runtime(
        &suggestion_state,
        &storage,
        "storage-feedback-1",
        "accept",
        None,
    )
    .await
    .expect("storage feedback fallback");

    let suggestions = pending_suggestions_snapshot(&suggestion_state, &storage)
        .await
        .expect("fallback suggestions");
    assert!(suggestions
        .iter()
        .all(|suggestion| suggestion.id != "storage-feedback-1"));
}

#[tokio::test]
async fn reject_and_defer_feedback_fall_back_to_storage_without_manager() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
    storage
        .save_rule_suggestion_sync(&sample_suggestion("storage-reject-1"))
        .expect("save reject suggestion");
    storage
        .save_rule_suggestion_sync(&sample_suggestion("storage-defer-1"))
        .expect("save defer suggestion");
    let suggestion_state = SuggestionRuntimeState::default();

    submit_suggestion_feedback_to_runtime(
        &suggestion_state,
        &storage,
        "storage-reject-1",
        "reject",
        None,
    )
    .await
    .expect("reject storage fallback");
    submit_suggestion_feedback_to_runtime(
        &suggestion_state,
        &storage,
        "storage-defer-1",
        "defer",
        Some(30),
    )
    .await
    .expect("defer storage fallback");

    let suggestions = pending_suggestions_snapshot(&suggestion_state, &storage)
        .await
        .expect("fallback suggestions");
    assert!(suggestions
        .iter()
        .all(|suggestion| suggestion.id != "storage-reject-1"));
    assert!(suggestions
        .iter()
        .all(|suggestion| suggestion.id != "storage-defer-1"));
    assert_eq!(
        storage
            .list_suggestions_by_state("deferred", 10)
            .expect("deferred suggestions")
            .len(),
        1
    );
}

#[tokio::test]
async fn explain_payload_falls_back_to_storage_without_manager() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
    storage
        .save_rule_suggestion_sync(&sample_suggestion("storage-explain-1"))
        .expect("save suggestion");
    let suggestion_state = SuggestionRuntimeState::default();

    let (content, reasoning) =
        find_suggestion_explain_payload(&suggestion_state, &storage, "storage-explain-1")
            .await
            .expect("storage explain fallback");

    assert_eq!(
        content,
        "Review the current GUI before accepting the action."
    );
    assert_eq!(
        reasoning.as_deref(),
        Some("The active window has a reviewable UI state.")
    );
}

#[test]
fn suggestion_replay_event_accepts_metadata_only_payload() {
    let payload = SuggestionReplayEventPayload {
        event_name: "suggestion.replay.proposal_visible".to_string(),
        phase: "proposal_visible".to_string(),
        suggestion_id: Some("calculator-result-rum-replay-proposal".to_string()),
        target_id: Some("display-result".to_string()),
        surface_placement: "window-side-panel".to_string(),
        app_name: Some("Calculator".to_string()),
        window_title: Some("Calculator".to_string()),
        action: None,
        audit_ready: true,
        raw_context_included: false,
    };

    let ack = validate_suggestion_replay_payload(&payload).expect("metadata-only payload");

    assert!(ack.recorded);
    assert_eq!(
        ack.trace_id,
        "suggestion-replay-proposal_visible-calculator-result-rum-replay-proposal"
    );
}

#[test]
fn suggestion_replay_event_rejects_raw_context_and_unknown_phase() {
    let mut payload = SuggestionReplayEventPayload {
        event_name: "suggestion.replay.proposal_visible".to_string(),
        phase: "proposal_visible".to_string(),
        suggestion_id: Some("calculator-result-rum-replay-proposal".to_string()),
        target_id: Some("display-result".to_string()),
        surface_placement: "window-side-panel".to_string(),
        app_name: Some("Calculator".to_string()),
        window_title: Some("Calculator".to_string()),
        action: None,
        audit_ready: true,
        raw_context_included: true,
    };

    let err = validate_suggestion_replay_payload(&payload).expect_err("raw context rejected");
    assert_eq!(err.code, "validation.invalid_arguments");

    payload.raw_context_included = false;
    payload.phase = "raw_screen_attached".to_string();
    let err = validate_suggestion_replay_payload(&payload).expect_err("unknown phase rejected");
    assert_eq!(err.code, "validation.invalid_arguments");
}

#[test]
fn suggestion_replay_log_context_records_presence_without_raw_values() {
    let payload = SuggestionReplayEventPayload {
        event_name: "suggestion.replay.proposal_visible".to_string(),
        phase: "proposal_visible".to_string(),
        suggestion_id: Some("calculator-result-rum-replay-proposal".to_string()),
        target_id: Some("display-result".to_string()),
        surface_placement: "window-side-panel".to_string(),
        app_name: Some("Private Notes".to_string()),
        window_title: Some("alice@example.com invoice".to_string()),
        action: None,
        audit_ready: true,
        raw_context_included: false,
    };

    let log_context = suggestion_replay_log_context(&payload);

    assert!(log_context.app_name_present);
    assert!(log_context.window_title_present);
}

// ---------------------------------------------------------------------------
// #5699 — SQLite fallback tests for history / stats / daily / deferred
// ---------------------------------------------------------------------------

/// Build a `Suggestion` with a distinct `SuggestionType`, `Priority`, and optional
/// timestamps already applied (acted/dismissed/resurface recorded separately via
/// the storage API after saving).
fn sample_suggestion_with_type(id: &str, stype: SuggestionType) -> Suggestion {
    Suggestion {
        suggestion_id: id.to_string(),
        suggestion_type: stype,
        content: format!("Content for {id}"),
        priority: Priority::High,
        confidence_score: 0.75,
        relevance_score: 0.80,
        is_actionable: true,
        created_at: Utc::now(),
        expires_at: None,
        source: SuggestionSource::LlmLocal,
        reasoning: None,
        context_scope: None,
    }
}

/// (1) History fallback: 3 rows (acted / dismissed / pending), assert feedback
/// labels and created_at DESC order. (#5699)
#[tokio::test]
async fn history_fallback_returns_rows_with_correct_feedback_labels() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));

    // Save three suggestions; then mark two of them acted / dismissed.
    let s_acted = sample_suggestion("hist-acted-1");
    let s_rejected = sample_suggestion("hist-rejected-1");
    let s_pending = sample_suggestion("hist-pending-1");

    storage
        .save_rule_suggestion_sync(&s_acted)
        .expect("save acted");
    storage
        .save_rule_suggestion_sync(&s_rejected)
        .expect("save rejected");
    storage
        .save_rule_suggestion_sync(&s_pending)
        .expect("save pending");

    storage
        .mark_unified_suggestion_acted("hist-acted-1")
        .expect("mark acted");
    storage
        .dismiss_unified_suggestion("hist-rejected-1")
        .expect("dismiss rejected");

    let state = SuggestionRuntimeState::default();
    let results = suggestion_history_snapshot(&state, &storage, Some(50))
        .await
        .expect("history fallback");

    assert_eq!(results.len(), 3, "all 3 rows should be returned");

    // Feedback labels derive from lifecycle columns.
    let by_id: std::collections::HashMap<_, _> = results
        .iter()
        .map(|r| (r.suggestion.id.as_str(), r.feedback.as_deref()))
        .collect();

    assert_eq!(by_id["hist-acted-1"], Some("accepted"));
    assert_eq!(by_id["hist-rejected-1"], Some("rejected"));
    assert_eq!(by_id["hist-pending-1"], None);
}

/// (2) History manager-first guard: when the manager is Some (even empty) the
/// SQLite rows must NOT be returned — prevents double-counting. (#5699)
#[cfg(feature = "local-suggestions")]
#[tokio::test]
async fn history_manager_present_ignores_sqlite_rows() {
    use maekon_suggestion::deferred::DeferredManager;
    use maekon_suggestion::feedback::FeedbackSender;
    use maekon_suggestion::feedback_retry::FeedbackRetryQueue;
    use maekon_suggestion::history::SuggestionHistory;
    use maekon_suggestion::queue::SuggestionQueue;
    use maekon_suggestion::scorer::FeedbackScorer;
    use tokio::sync::Mutex;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
    // Write a row to SQLite — but we expect the manager path to return empty.
    storage
        .save_rule_suggestion_sync(&sample_suggestion("sqlite-row-1"))
        .expect("save");

    // Build a live manager with an empty history.
    let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
    let api: Arc<dyn maekon_core::ports::api_client::ApiClient> =
        Arc::new(crate::local_api_client::LocalApiClient);
    let feedback = Arc::new(FeedbackSender::new_with_sink(api, None));
    let manager = Arc::new(crate::suggestion_manager::SuggestionManager::new(
        queue,
        Arc::new(Mutex::new(SuggestionHistory::new(100))),
        feedback,
        Arc::new(Mutex::new(FeedbackScorer::new())),
        Arc::new(Mutex::new(DeferredManager::new(50))),
        Arc::new(Mutex::new(FeedbackRetryQueue::new(100, 5))),
        storage.clone(),
    ));

    let state = SuggestionRuntimeState::new(Some(manager), None);
    let results = suggestion_history_snapshot(&state, &storage, Some(50))
        .await
        .expect("manager path");

    assert!(
        results.is_empty(),
        "manager path (empty history) must win — SQLite rows must not leak"
    );
}

/// (3) Stats fallback: row counts, acceptance_rate fraction, by_type key
/// matches manager path convention, by_source 'local' present. (#5699)
#[tokio::test]
async fn stats_fallback_counts_and_type_key_parity() {
    use super::mapping::storage_type_key;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));

    // 2 WorkGuidance from LlmLocal; mark 1 acted.
    let s1 = sample_suggestion_with_type("stats-wg-1", SuggestionType::WorkGuidance);
    let s2 = sample_suggestion_with_type("stats-wg-2", SuggestionType::WorkGuidance);
    storage.save_rule_suggestion_sync(&s1).expect("save s1");
    storage.save_rule_suggestion_sync(&s2).expect("save s2");
    storage
        .mark_unified_suggestion_acted("stats-wg-1")
        .expect("act");

    let rows = storage.list_recent_suggestions(100).expect("read");
    // Build the tuples exactly as the fallback path does.
    let tuples: Vec<_> = rows
        .iter()
        .map(|r| {
            let type_key = storage_type_key(&r.suggestion_type);
            let source_key = super::mapping::storage_source_label_pub(&r.source);
            let feedback = super::mapping::storage_feedback_label(r);
            (type_key, source_key, feedback)
        })
        .collect();

    // total_shown = 2, accepted = 1, acceptance_rate = 0.5
    let type_key_expected = format!("{:?}", SuggestionType::WorkGuidance).to_lowercase();
    assert_eq!(type_key_expected, "workguidance");

    let accepted = tuples
        .iter()
        .filter(|(_, _, f)| f.as_deref() == Some("accepted"))
        .count();
    assert_eq!(accepted, 1);
    assert_eq!(tuples.len(), 2);

    let has_local = tuples.iter().any(|(_, src, _)| src == "local");
    assert!(has_local, "LlmLocal source should map to 'local'");
}

/// (4) Daily stats fallback: grouping by date prefix and 90-day cap. (#5699)
#[tokio::test]
async fn daily_stats_fallback_groups_by_date_prefix() {
    use super::mapping::storage_feedback_label;

    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
    let s1 = sample_suggestion("daily-1");
    let s2 = sample_suggestion("daily-2");
    storage.save_rule_suggestion_sync(&s1).expect("save");
    storage.save_rule_suggestion_sync(&s2).expect("save");
    storage
        .mark_unified_suggestion_acted("daily-1")
        .expect("act");

    let rows = storage.list_recent_suggestions(100).expect("read");

    // Verify date prefix is valid (len >= 10 and format YYYY-MM-DD).
    for r in &rows {
        assert!(
            r.created_at.len() >= 10,
            "created_at too short: {}",
            r.created_at
        );
        let date = &r.created_at[..10];
        assert_eq!(date.chars().nth(4), Some('-'), "date separator at pos 4");
        assert_eq!(date.chars().nth(7), Some('-'), "date separator at pos 7");
    }

    // Verify acted_at driven feedback label.
    let acted_row = rows.iter().find(|r| r.suggestion_id == "daily-1").unwrap();
    assert_eq!(
        storage_feedback_label(acted_row).as_deref(),
        Some("accepted")
    );

    let pending_row = rows.iter().find(|r| r.suggestion_id == "daily-2").unwrap();
    assert_eq!(storage_feedback_label(pending_row), None);
}

/// (5) Deferred fallback: `list_suggestions_by_state("deferred")` + resurface_at
/// parse + remaining_minutes > 0.  Malformed resurface_at row must be skipped. (#5699)
#[tokio::test]
async fn deferred_fallback_parses_resurface_at_and_skips_malformed() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));

    let future = Utc::now() + chrono::Duration::hours(2);
    let future_str = future.to_rfc3339();

    // Good deferred row.
    storage
        .save_suggestion_with_state(
            &sample_suggestion("def-good-1"),
            "deferred",
            Some(&future_str),
        )
        .expect("save deferred");

    // Malformed resurface_at — must be skipped, not panic.
    storage
        .save_suggestion_with_state(
            &sample_suggestion("def-bad-1"),
            "deferred",
            Some("NOT-A-DATE"),
        )
        .expect("save malformed");

    let rows = storage
        .list_suggestions_by_state("deferred", 50)
        .expect("list deferred");
    assert_eq!(rows.len(), 2, "both rows in DB");

    // Simulate the fallback filter (parse resurface_at, skip on failure).
    let now = Utc::now();
    let valid: Vec<_> = rows
        .iter()
        .filter_map(|r| {
            r.resurface_at
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| {
                    let resurface = dt.with_timezone(&chrono::Utc);
                    let remaining = (resurface - now).num_minutes().max(0);
                    (r.suggestion_id.as_str(), remaining)
                })
        })
        .collect();

    assert_eq!(valid.len(), 1, "malformed row must be filtered out");
    assert_eq!(valid[0].0, "def-good-1");
    assert!(
        valid[0].1 > 0,
        "remaining_minutes must be positive for a future resurface_at"
    );

    // Priority normalisation: storage stores "Medium" (PascalCase via enum_to_sql_str)
    // which must normalise to "medium" on the wire (#5699).
    let good_row = rows
        .iter()
        .find(|r| r.suggestion_id == "def-good-1")
        .unwrap();
    assert_eq!(good_row.priority.to_lowercase(), "medium");
}

/// (6) Flatten wire guard: `serde_json::to_value` of a `SuggestionHistoryDto`
/// must emit `title` and `feedback` at the TOP level (not nested under
/// `suggestion`). Catches Rust-level flatten drift that FE mock fixtures can't. (#5699)
#[test]
fn suggestion_history_dto_flatten_emits_top_level_fields() {
    use super::types::{SuggestionHistoryDto, SuggestionViewDto};

    let dto = SuggestionHistoryDto {
        suggestion: SuggestionViewDto {
            id: "test-id".to_string(),
            title: "Work Guidance".to_string(),
            body: "body".to_string(),
            priority: "medium".to_string(),
            category: None,
            source: "local".to_string(),
            confidence_score: 0.9,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            is_read: false,
            reasoning: None,
            context_scope: None,
        },
        feedback: Some("accepted".to_string()),
    };

    let value = serde_json::to_value(&dto).expect("serialise");
    let obj = value.as_object().expect("top-level object");

    // title must be at root, NOT under a 'suggestion' key.
    assert!(
        obj.contains_key("title"),
        "title must be at top level — flatten missing?"
    );
    assert!(
        obj.contains_key("feedback"),
        "feedback must be at top level"
    );
    assert!(
        !obj.contains_key("suggestion"),
        "'suggestion' key must not appear — flatten not working"
    );
    // id also top-level.
    assert!(obj.contains_key("id"), "id must be at top level");
}
