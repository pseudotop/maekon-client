use super::feedback::{find_suggestion_explain_payload, submit_suggestion_feedback_to_runtime};
use super::queries::{pending_suggestions_snapshot, suggestion_history_snapshot};
use super::replay::{suggestion_replay_log_context, validate_suggestion_replay_payload};
use super::types::SuggestionReplayEventPayload;
use chrono::Utc;
use maekon_core::config::AutomationConfig;
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

    let suggestions =
        pending_suggestions_snapshot(&suggestion_state, &storage, &AutomationConfig::default())
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
    let suggestions = pending_suggestions_snapshot(&state, &storage, &AutomationConfig::default())
        .await
        .expect("pending suggestions");

    assert_eq!(
        suggestions.len(),
        1,
        "the locally-generated suggestion must surface from the live queue (no server, no SQLite fallback)"
    );
    assert_eq!(suggestions[0].id, "local-gen-1");
}

/// #7600 fails-before: before this change `SuggestionRuntimeState` had no
/// `shared_regime` field and `submit_suggestion_feedback_to_runtime` never
/// read a regime_id at all — `FeedbackSender::accept` did not even accept
/// one. This is the end-to-end regression guard for the emission site: the
/// live regime snapshot -> `SuggestionRuntimeState::current_regime_id` ->
/// `submit_suggestion_feedback_to_runtime` -> `FeedbackSender::accept` ->
/// `SuggestionFeedback.regime_id` -> the `FeedbackSignalSink`.
#[cfg(feature = "local-suggestions")]
#[tokio::test]
async fn accept_feedback_attaches_live_regime_id_from_shared_state() {
    use crate::scheduler::shared_regime_state::SharedRegimeState;
    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::models::suggestion::SuggestionFeedback;
    use maekon_core::models::tiered_memory::{Regime, RegimeFeatures, RegimeStatus, TriggerParams};
    use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
    use maekon_suggestion::deferred::DeferredManager;
    use maekon_suggestion::feedback::FeedbackSender;
    use maekon_suggestion::feedback_retry::FeedbackRetryQueue;
    use maekon_suggestion::history::SuggestionHistory;
    use maekon_suggestion::queue::SuggestionQueue;
    use maekon_suggestion::scorer::FeedbackScorer;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Mutex;

    struct CapturingSink(Arc<StdMutex<Option<Option<String>>>>);
    #[async_trait]
    impl FeedbackSignalSink for CapturingSink {
        async fn record_user_reaction(
            &self,
            feedback: &SuggestionFeedback,
        ) -> Result<(), CoreError> {
            *self.0.lock().unwrap() = Some(feedback.regime_id.clone());
            Ok(())
        }
    }

    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
    storage
        .save_rule_suggestion_sync(&sample_suggestion("regime-feedback-1"))
        .expect("save suggestion");

    let captured = Arc::new(StdMutex::new(None));
    let sink: Arc<dyn FeedbackSignalSink> = Arc::new(CapturingSink(captured.clone()));
    let api: Arc<dyn maekon_core::ports::api_client::ApiClient> =
        Arc::new(crate::local_api_client::LocalApiClient);
    let feedback = Arc::new(FeedbackSender::new_with_sink(api, Some(sink)));
    let manager = Arc::new(crate::suggestion_manager::SuggestionManager::new(
        Arc::new(Mutex::new(SuggestionQueue::new(50))),
        Arc::new(Mutex::new(SuggestionHistory::new(100))),
        feedback,
        Arc::new(Mutex::new(FeedbackScorer::new())),
        Arc::new(Mutex::new(DeferredManager::new(50))),
        Arc::new(Mutex::new(FeedbackRetryQueue::new(100, 5))),
        storage.clone(),
    ));

    // Live regime snapshot, mirroring what the monitor loop writes each tick.
    let shared_regime = Arc::new(SharedRegimeState::new());
    let regime = Regime {
        regime_id: "regime-under-test".to_string(),
        name: None,
        auto_label: "Deep Focus (VSCode)".to_string(),
        centroid: RegimeFeatures::default(),
        optimal_params: TriggerParams::default(),
        sample_count: 10,
        first_seen: Utc::now(),
        last_seen: Utc::now(),
        status: RegimeStatus::Active,
    };
    shared_regime.update(Some(&regime), "VSCode");

    let state = SuggestionRuntimeState::new(Some(manager), None).with_shared_regime(shared_regime);

    submit_suggestion_feedback_to_runtime(&state, &storage, "regime-feedback-1", "accept", None)
        .await
        .expect("accept succeeds");

    assert_eq!(
        captured.lock().unwrap().clone(),
        Some(Some("regime-under-test".to_string())),
        "the sink must observe the live regime_id read from SharedRegimeState"
    );
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

    let suggestions =
        pending_suggestions_snapshot(&suggestion_state, &storage, &AutomationConfig::default())
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

    let suggestions =
        pending_suggestions_snapshot(&suggestion_state, &storage, &AutomationConfig::default())
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
            reasoning: None,
            context_scope: None,
            action: None,
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

// ---------------------------------------------------------------------------
// #7917 T4.1 — run_suggestion_action (suggestion → automation bridge)
// ---------------------------------------------------------------------------

mod run_action_tests {
    use super::*;
    use crate::commands::suggestions::action::run_suggestion_action_inner;
    use async_trait::async_trait;
    use maekon_core::error::{CoreError, GuiInteractionError};
    use maekon_core::error_codes::PolicyCode;
    use maekon_core::models::automation::{
        AutomationCommand, CommandResult, ExecutionPolicyDto, GuiExecutionResult,
        PendingConfirmation, PlannedIntentResult, WorkflowResult,
    };
    use maekon_core::models::gui::{
        GuiConfirmRequest, GuiCreateSessionRequest, GuiCreateSessionResponse, GuiExecutionRequest,
        GuiExecutionTicket, GuiHighlightRequest, GuiInteractionSession, GuiSessionEvent,
    };
    use maekon_core::models::intent::{IntentCommand, IntentResult, WorkflowPreset};
    use maekon_core::models::ui_scene::UiScene;
    use maekon_core::ports::automation::AutomationPort;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::broadcast;

    /// A locally-minted, rule-based `NeedFocusTime` nudge — the one (type, source)
    /// pair the MVP binds to `deep-work-start`.
    fn bound_suggestion(id: &str) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::NeedFocusTime,
            content: "You've been context-switching — a focus block might help.".to_string(),
            priority: Priority::High,
            confidence_score: 0.9,
            relevance_score: 0.9,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: SuggestionSource::RuleBased,
            reasoning: None,
            context_scope: None,
        }
    }

    fn enabled_automation() -> AutomationConfig {
        AutomationConfig {
            enabled: true,
            ..Default::default()
        }
    }

    #[derive(Clone, Copy)]
    enum RunOutcome {
        Success,
        // #7947: `Denied` is constructed ONLY by the `local-suggestions`-gated
        // `manager_path` tests (`denied_run_emits_nothing`). In the
        // `--no-default-features` cell that module is compiled out, so allow the
        // variant to be unused there — otherwise the workspace `dead_code = "deny"`
        // lint fails the no-default test build. The default/server/grpc cells keep
        // constructing it, so coverage there is unchanged.
        #[cfg_attr(not(feature = "local-suggestions"), allow(dead_code))]
        Denied,
    }

    /// Coordination handles so a test can hold `run_workflow` open across a second
    /// concurrent call (the in-flight-guard test).
    struct RunGate {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    /// Minimal `AutomationPort` test double: only `run_workflow` is exercised;
    /// every other method is unreachable in these tests.
    struct FakeAutomation {
        outcome: RunOutcome,
        calls: Arc<AtomicUsize>,
        gate: Option<Arc<RunGate>>,
    }

    #[async_trait]
    impl AutomationPort for FakeAutomation {
        async fn run_workflow(&self, preset: &WorkflowPreset) -> Result<WorkflowResult, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                gate.entered.notify_one();
                gate.release.notified().await;
            }
            match self.outcome {
                RunOutcome::Success => Ok(WorkflowResult {
                    preset_id: preset.id.clone(),
                    success: true,
                    steps_executed: preset.steps.len(),
                    total_steps: preset.steps.len(),
                    total_elapsed_ms: 0,
                    step_results: Vec::new(),
                    message: "ok".to_string(),
                }),
                // Models the Block-policy / UserDenied arm: run_workflow returns a
                // CoreError, never Ok — so the caller emits nothing.
                RunOutcome::Denied => Err(CoreError::PolicyDenied {
                    code: PolicyCode::Denied,
                    message: "automation blocked by policy".to_string(),
                }),
            }
        }

        async fn execute_command(
            &self,
            _cmd: &AutomationCommand,
        ) -> Result<CommandResult, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn execute_intent(&self, _cmd: &IntentCommand) -> Result<IntentResult, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn execute_intent_hint(
            &self,
            _command_id: &str,
            _session_id: &str,
            _intent_hint: &str,
        ) -> Result<PlannedIntentResult, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn analyze_scene(
            &self,
            _app_name: Option<&str>,
            _screen_id: Option<&str>,
        ) -> Result<UiScene, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn analyze_scene_from_image(
            &self,
            _image_data: Vec<u8>,
            _image_format: String,
            _app_name: Option<&str>,
            _screen_id: Option<&str>,
        ) -> Result<UiScene, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn gui_create_session(
            &self,
            _req: GuiCreateSessionRequest,
        ) -> Result<GuiCreateSessionResponse, GuiInteractionError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn gui_get_session(
            &self,
            _session_id: &str,
            _capability_token: &str,
        ) -> Result<GuiInteractionSession, GuiInteractionError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn gui_highlight_session(
            &self,
            _session_id: &str,
            _capability_token: &str,
            _req: GuiHighlightRequest,
        ) -> Result<GuiInteractionSession, GuiInteractionError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn gui_confirm_candidate(
            &self,
            _session_id: &str,
            _capability_token: &str,
            _req: GuiConfirmRequest,
        ) -> Result<GuiExecutionTicket, GuiInteractionError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn gui_execute(
            &self,
            _session_id: &str,
            _capability_token: &str,
            _req: GuiExecutionRequest,
        ) -> Result<GuiExecutionResult, GuiInteractionError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn gui_cancel_session(
            &self,
            _session_id: &str,
            _capability_token: &str,
        ) -> Result<GuiInteractionSession, GuiInteractionError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn gui_subscribe_events(
            &self,
            _session_id: &str,
            _capability_token: &str,
        ) -> Result<broadcast::Receiver<GuiSessionEvent>, GuiInteractionError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn list_pending_confirmations(&self) -> Result<Vec<PendingConfirmation>, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn submit_confirmation(
            &self,
            _command_id: &str,
            _nonce: &str,
            _approved: bool,
        ) -> Result<(), CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn list_execution_policies(&self) -> Result<Vec<ExecutionPolicyDto>, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn add_execution_policy(
            &self,
            _policy: ExecutionPolicyDto,
        ) -> Result<ExecutionPolicyDto, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
        async fn remove_execution_policy(&self, _policy_id: &str) -> Result<bool, CoreError> {
            unimplemented!("unused in run_suggestion_action tests")
        }
    }

    fn fake(outcome: RunOutcome, calls: Arc<AtomicUsize>) -> Arc<dyn AutomationPort> {
        Arc::new(FakeAutomation {
            outcome,
            calls,
            gate: None,
        })
    }

    fn acted_in_storage(storage: &SqliteStorage, id: &str) -> bool {
        storage
            .list_recent_suggestions(50)
            .expect("list")
            .iter()
            .find(|r| r.suggestion_id == id)
            .and_then(super::super::mapping::storage_feedback_label)
            == Some("accepted".to_string())
    }

    // ── Storage-fallback (manager-less) path ────────────────────────────────

    /// DA B: with NO manager the command still resolves the binding from storage
    /// and runs — and on success marks acted.
    #[tokio::test]
    async fn managerless_run_resolves_and_executes_then_marks_acted() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
        storage
            .save_rule_suggestion_sync(&bound_suggestion("run-1"))
            .expect("save");
        let state = SuggestionRuntimeState::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let controller = fake(RunOutcome::Success, calls.clone());

        run_suggestion_action_inner(
            &state,
            &storage,
            &enabled_automation(),
            &controller,
            "run-1",
        )
        .await
        .expect("run succeeds");

        assert_eq!(calls.load(Ordering::SeqCst), 1, "run_workflow called once");
        assert!(acted_in_storage(&storage, "run-1"), "acted_at must be set");
    }

    /// A network-sourced suggestion of the bound type is REFUSED before any run
    /// (frozen invariant). Nothing executes, nothing is marked.
    #[tokio::test]
    async fn non_rule_based_suggestion_is_refused() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
        let mut network = bound_suggestion("net-1");
        network.source = SuggestionSource::LlmServer;
        storage.save_rule_suggestion_sync(&network).expect("save");
        let state = SuggestionRuntimeState::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let controller = fake(RunOutcome::Success, calls.clone());

        let err = run_suggestion_action_inner(
            &state,
            &storage,
            &enabled_automation(),
            &controller,
            "net-1",
        )
        .await
        .expect_err("network-sourced suggestion must be refused");
        assert_eq!(err.code, "validation.invalid_arguments");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "run_workflow must not run");
        assert!(!acted_in_storage(&storage, "net-1"), "must not be acted");
    }

    /// Automation disabled ⇒ clean error, no run, no acted_at.
    #[tokio::test]
    async fn disabled_automation_is_a_clean_error_with_no_side_effects() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
        storage
            .save_rule_suggestion_sync(&bound_suggestion("dis-1"))
            .expect("save");
        let state = SuggestionRuntimeState::default();
        let calls = Arc::new(AtomicUsize::new(0));
        let controller = fake(RunOutcome::Success, calls.clone());

        let err = run_suggestion_action_inner(
            &state,
            &storage,
            &AutomationConfig::default(), // enabled = false
            &controller,
            "dis-1",
        )
        .await
        .expect_err("disabled automation must error");
        assert_eq!(err.code, "validation.invalid_arguments");
        assert_eq!(calls.load(Ordering::SeqCst), 0, "run_workflow must not run");
        assert!(!acted_in_storage(&storage, "dis-1"), "must not be acted");
    }

    /// DA D: a second concurrent run for the same id is refused while the first is
    /// in flight — the preset executes exactly once.
    #[tokio::test]
    async fn concurrent_double_call_executes_once() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
        storage
            .save_rule_suggestion_sync(&bound_suggestion("cc-1"))
            .expect("save");
        let state = Arc::new(SuggestionRuntimeState::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(RunGate {
            entered: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let controller: Arc<dyn AutomationPort> = Arc::new(FakeAutomation {
            outcome: RunOutcome::Success,
            calls: calls.clone(),
            gate: Some(gate.clone()),
        });

        // Call A: reserves, enters run_workflow, then parks on the release gate.
        let state_a = state.clone();
        let storage_a = storage.clone();
        let controller_a = controller.clone();
        let handle = tokio::spawn(async move {
            run_suggestion_action_inner(
                &state_a,
                &storage_a,
                &enabled_automation(),
                &controller_a,
                "cc-1",
            )
            .await
        });

        // Wait until A holds the reservation (it is inside run_workflow).
        gate.entered.notified().await;

        // Call B for the same id: must be refused immediately (reservation held).
        let b = run_suggestion_action_inner(
            &state,
            &storage,
            &enabled_automation(),
            &controller,
            "cc-1",
        )
        .await
        .expect_err("second concurrent call must be refused");
        assert_eq!(b.code, "validation.invalid_arguments");

        // Let A finish.
        gate.release.notify_one();
        handle.await.expect("join A").expect("A succeeds");

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "the preset must execute exactly once despite the double-fire"
        );
    }

    /// History views stay unbound even for a bound-type suggestion (ADR-027): a
    /// suggestion the user already acted on must never re-offer a one-click run.
    #[tokio::test]
    async fn history_snapshot_stays_unbound_for_bound_type() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
        storage
            .save_rule_suggestion_sync(&bound_suggestion("hist-bound-1"))
            .expect("save");
        storage
            .mark_unified_suggestion_acted("hist-bound-1")
            .expect("act");
        let state = SuggestionRuntimeState::default();

        let history = suggestion_history_snapshot(&state, &storage, Some(50))
            .await
            .expect("history");
        let entry = history
            .iter()
            .find(|h| h.suggestion.id == "hist-bound-1")
            .expect("row present");
        assert!(
            entry.suggestion.action.is_none(),
            "history must never carry an action, even for a bound type"
        );
    }

    // ── Manager (live-queue) path ───────────────────────────────────────────

    #[cfg(feature = "local-suggestions")]
    mod manager_path {
        use super::*;
        use maekon_core::models::suggestion::{FeedbackType, SuggestionFeedback};
        use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
        use maekon_suggestion::deferred::DeferredManager;
        use maekon_suggestion::feedback::FeedbackSender;
        use maekon_suggestion::feedback_retry::FeedbackRetryQueue;
        use maekon_suggestion::history::SuggestionHistory;
        use maekon_suggestion::queue::SuggestionQueue;
        use maekon_suggestion::scorer::FeedbackScorer;
        use std::sync::Mutex as StdMutex;
        use tokio::sync::Mutex;

        /// Records every feedback signal the sink observes (proves "emitted once").
        struct CountingSink(Arc<StdMutex<Vec<FeedbackType>>>);
        #[async_trait]
        impl FeedbackSignalSink for CountingSink {
            async fn record_user_reaction(
                &self,
                feedback: &SuggestionFeedback,
            ) -> Result<(), CoreError> {
                self.0.lock().unwrap().push(feedback.feedback_type.clone());
                Ok(())
            }
        }

        fn build_manager(
            storage: Arc<SqliteStorage>,
            sink: Option<Arc<dyn FeedbackSignalSink>>,
        ) -> (
            Arc<crate::suggestion_manager::SuggestionManager>,
            Arc<Mutex<SuggestionQueue>>,
        ) {
            let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
            let api: Arc<dyn maekon_core::ports::api_client::ApiClient> =
                Arc::new(crate::local_api_client::LocalApiClient);
            let feedback = Arc::new(FeedbackSender::new_with_sink(api, sink));
            let manager = Arc::new(crate::suggestion_manager::SuggestionManager::new(
                queue.clone(),
                Arc::new(Mutex::new(SuggestionHistory::new(100))),
                feedback,
                Arc::new(Mutex::new(FeedbackScorer::new())),
                Arc::new(Mutex::new(DeferredManager::new(50))),
                Arc::new(Mutex::new(FeedbackRetryQueue::new(100, 5))),
                storage,
            ));
            (manager, queue)
        }

        /// TL C-2: the MANAGER-path DTO snapshot enriches a bound suggestion and
        /// leaves a network-sourced one of the SAME type unbound.
        #[tokio::test]
        async fn manager_snapshot_enriches_only_the_bound_rule_based_item() {
            let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
            let (manager, queue) = build_manager(storage.clone(), None);
            {
                let mut q = queue.lock().await;
                assert!(q.push(bound_suggestion("bound-1")));
                let mut network = bound_suggestion("network-1");
                network.source = SuggestionSource::LlmServer;
                assert!(q.push(network));
            }
            let state = SuggestionRuntimeState::new(Some(manager), None);

            let views = pending_suggestions_snapshot(&state, &storage, &enabled_automation())
                .await
                .expect("snapshot");

            let bound = views.iter().find(|v| v.id == "bound-1").expect("bound");
            assert_eq!(
                bound.action.as_ref().map(|a| a.label.as_str()),
                Some("Clear Distractions"),
                "bound rule-based item must carry the derived action"
            );
            let network = views.iter().find(|v| v.id == "network-1").expect("network");
            assert!(
                network.action.is_none(),
                "network-sourced item of the same type must stay unbound"
            );
        }

        /// A successful run emits exactly one Accepted signal, moves the item
        /// queue→history, and marks acted_at.
        #[tokio::test]
        async fn successful_run_emits_accepted_once_and_moves_to_history() {
            let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
            storage
                .save_rule_suggestion_sync(&bound_suggestion("ok-1"))
                .expect("save");
            let signals = Arc::new(StdMutex::new(Vec::new()));
            let sink: Arc<dyn FeedbackSignalSink> = Arc::new(CountingSink(signals.clone()));
            let (manager, queue) = build_manager(storage.clone(), Some(sink));
            assert!(queue.lock().await.push(bound_suggestion("ok-1")));
            let state = SuggestionRuntimeState::new(Some(manager), None);
            let calls = Arc::new(AtomicUsize::new(0));
            let controller = fake(RunOutcome::Success, calls.clone());

            run_suggestion_action_inner(
                &state,
                &storage,
                &enabled_automation(),
                &controller,
                "ok-1",
            )
            .await
            .expect("run succeeds");

            assert_eq!(calls.load(Ordering::SeqCst), 1);
            assert_eq!(
                signals.lock().unwrap().clone(),
                vec![FeedbackType::Accepted],
                "exactly one Accepted signal must be emitted"
            );
            assert_eq!(queue.lock().await.len(), 0, "item moved out of the queue");
            assert!(acted_in_storage(&storage, "ok-1"), "acted_at must be set");
        }

        /// A denied (Block-policy) run emits NOTHING — the learning signal and
        /// acted_at are untouched, and the item stays in the queue.
        #[tokio::test]
        async fn denied_run_emits_nothing() {
            let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("storage"));
            storage
                .save_rule_suggestion_sync(&bound_suggestion("blocked-1"))
                .expect("save");
            let signals = Arc::new(StdMutex::new(Vec::new()));
            let sink: Arc<dyn FeedbackSignalSink> = Arc::new(CountingSink(signals.clone()));
            let (manager, queue) = build_manager(storage.clone(), Some(sink));
            assert!(queue.lock().await.push(bound_suggestion("blocked-1")));
            let state = SuggestionRuntimeState::new(Some(manager), None);
            let calls = Arc::new(AtomicUsize::new(0));
            let controller = fake(RunOutcome::Denied, calls.clone());

            let err = run_suggestion_action_inner(
                &state,
                &storage,
                &enabled_automation(),
                &controller,
                "blocked-1",
            )
            .await
            .expect_err("denied run must error");
            assert_eq!(err.code, "policy.denied");

            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "run_workflow was attempted"
            );
            assert!(
                signals.lock().unwrap().is_empty(),
                "a denied run must emit no feedback signal"
            );
            assert_eq!(queue.lock().await.len(), 1, "item stays in the queue");
            assert!(
                !acted_in_storage(&storage, "blocked-1"),
                "acted_at must not be set on a denied run"
            );
        }
    }
}
