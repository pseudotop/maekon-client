//! #12029: observe the real SQLite -> selector -> periodic analyzer -> provider boundary.
//! The provider only records input and returns no candidates; no model or external call runs.
//! Legacy rows are explicit fixture seeds, not evidence of a live legacy writer.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use maekon_analysis::{
    AnalysisRunOutcome, ContextAnalyzer, ContextAssembler, PatternMiner, PiiFilter, SegmentStats,
    ANALYSIS_SYSTEM_PROMPT,
};
use maekon_core::config::{AnalysisConfig, PiiFilterLevel};
use maekon_core::error::CoreError;
use maekon_core::models::event::{ContextEvent, Event};
use maekon_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::few_shot_storage::FewShotStorage;
use maekon_core::ports::pii_sanitizer::{FakePiiSanitizer, PiiSanitizer};
use maekon_core::ports::storage::StorageService;
use maekon_storage::sqlite::SqliteStorage;

const SYNTHETIC_EMAIL: &str = "synthetic.person@example.invalid";

#[derive(Clone, Debug)]
struct ObservedPrompt {
    context: serde_json::Value,
    system: String,
}

#[derive(Default)]
struct RecordingProvider {
    calls: Mutex<Vec<ObservedPrompt>>,
}

#[async_trait]
impl AnalysisProvider for RecordingProvider {
    async fn analyze(
        &self,
        context_json: &str,
        system_prompt: &str,
    ) -> Result<Vec<Suggestion>, CoreError> {
        self.calls.lock().unwrap().push(ObservedPrompt {
            context: serde_json::from_str(context_json).expect("actual context is valid JSON"),
            system: system_prompt.to_owned(),
        });
        Ok(vec![])
    }

    fn provider_name(&self) -> &str {
        "recording-provider-no-model"
    }
}

fn pii_filter() -> PiiFilter {
    let sanitizer = FakePiiSanitizer::new().with_email(SYNTHETIC_EMAIL);
    Box::new(move |text| sanitizer.sanitize_text(text, PiiFilterLevel::Standard))
}

fn created_at(order: i64) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-09-01T00:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
        + Duration::seconds(order)
}

fn suggestion(id: &str, content: &str, order: i64) -> Suggestion {
    Suggestion {
        suggestion_id: id.to_owned(),
        suggestion_type: SuggestionType::WorkGuidance,
        content: content.to_owned(),
        priority: Priority::Medium,
        confidence_score: 0.8,
        relevance_score: 0.9,
        is_actionable: true,
        created_at: created_at(order),
        expires_at: None,
        source: SuggestionSource::LlmLocal,
        reasoning: None,
        context_scope: None,
    }
}

fn seed_legacy(storage: &SqliteStorage, id: &str, content: &str, order: i64) {
    // The legacy producer was removed in #7733. Seed only this owned in-memory
    // database through its write guard; the subsequent feedback/read paths are real.
    let timestamp = created_at(order).to_rfc3339();
    let inserted = storage
        .connection_arc()
        .write_lock()
        .run(0, |conn| {
            conn.execute(
                "INSERT INTO local_suggestions
                 (suggestion_id, suggestion_type, content, confidence, payload, created_at)
                 VALUES (?1, 'WORK_GUIDANCE', ?2, 0.8, '{}', ?3)",
                [id, content, &timestamp],
            )
        })
        .expect("seed legacy fixture");
    assert_eq!(inserted, 1);
}

fn feedback(storage: &SqliteStorage, id: &str, outcome: &str, regime: Option<&str>) {
    storage
        .record_suggestion_feedback(id, outcome, "A6 Editor", "A6 synthetic document", regime)
        .expect("record feedback through the actual ID-bound update");
}

async fn seed_current_context(storage: &SqliteStorage) {
    storage
        .save_event(&Event::Context(ContextEvent {
            app_name: "A6 Editor".into(),
            window_title: "A6 synthetic current task".into(),
            timestamp: Utc::now() - Duration::seconds(5),
            ..ContextEvent::default()
        }))
        .await
        .expect("save a real queryable context event");
}

async fn storage() -> Arc<SqliteStorage> {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    seed_current_context(&storage).await;
    storage
}

async fn analyzer(
    storage: Arc<SqliteStorage>,
    attach_few_shot: bool,
) -> (ContextAnalyzer, Arc<RecordingProvider>) {
    let provider = Arc::new(RecordingProvider::default());
    let analyzer = ContextAnalyzer::with_pii_filter(
        storage.clone(),
        provider.clone(),
        PatternMiner::new(),
        ContextAssembler::new(pii_filter()),
        AnalysisConfig {
            throttle_secs: 0,
            full_interval_secs: 3600,
            ..AnalysisConfig::default()
        },
        pii_filter(),
    );
    if attach_few_shot {
        analyzer.set_few_shot_storage(storage).await;
    }
    (analyzer, provider)
}

async fn set_regime(analyzer: &ContextAnalyzer, label: &str) {
    analyzer
        .set_segment_stats(Some(SegmentStats {
            duration_mins: 15,
            regime_label: Some(label.to_owned()),
            event_count: 1,
            context_switches: 0,
            dominant_category: "development".into(),
            content_summary: vec![],
            gui_patterns: vec![],
        }))
        .await;
}

async fn periodic(analyzer: &ContextAnalyzer, provider: &RecordingProvider) -> ObservedPrompt {
    let before = provider.calls.lock().unwrap().len();
    assert!(matches!(
        analyzer.analyze_with_outcome().await.unwrap(),
        AnalysisRunOutcome::NoCandidate
    ));
    let calls = provider.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        before + 1,
        "the actual provider must be reached"
    );
    calls.last().unwrap().clone()
}

fn assert_examples(prompt: &str, accepted: &str, rejected: &str) {
    assert!(prompt.contains(accepted), "{prompt}");
    assert!(prompt.contains(rejected), "{prompt}");
    assert_eq!(prompt.matches("[Accepted] Context:").count(), 1);
    assert_eq!(prompt.matches("[Rejected] Context:").count(), 1);
}

#[tokio::test]
async fn periodic_off_general_and_regime_observe_distinct_actual_prompts() {
    let storage = storage().await;
    for (id, text, order, outcome, regime) in [
        ("coding-a", "A6_CODING_ACCEPTED", 1, "ACCEPTED", "coding"),
        ("coding-r", "A6_CODING_REJECTED", 2, "REJECTED", "coding"),
        ("recent-a", "A6_RECENT_ACCEPTED", 3, "ACCEPTED", "meeting"),
        ("recent-r", "A6_RECENT_REJECTED", 4, "REJECTED", "meeting"),
    ] {
        seed_legacy(&storage, id, text, order);
        feedback(&storage, id, outcome, Some(regime));
    }

    let (off, off_provider) = analyzer(storage.clone(), false).await;
    let off_prompt = periodic(&off, &off_provider).await;
    assert_eq!(off_prompt.system, ANALYSIS_SYSTEM_PROMPT);

    let (general, general_provider) = analyzer(storage.clone(), true).await;
    let general_prompt = periodic(&general, &general_provider).await;
    assert_eq!(general_prompt.context, off_prompt.context);
    assert_examples(
        &general_prompt.system,
        "A6_RECENT_ACCEPTED",
        "A6_RECENT_REJECTED",
    );
    assert!(!general_prompt.system.contains("A6_CODING_"));

    let (regime, regime_provider) = analyzer(storage, true).await;
    set_regime(&regime, "coding").await;
    let regime_prompt = periodic(&regime, &regime_provider).await;
    assert_examples(
        &regime_prompt.system,
        "A6_CODING_ACCEPTED",
        "A6_CODING_REJECTED",
    );
    assert!(!regime_prompt.system.contains("A6_RECENT_"));
    assert!(regime_prompt.system.contains("\"coding\" regime"));
    // Regime metadata is also real input. This wiring test is not an equal-token
    // model-quality comparison and does not hide that input difference.
    assert_ne!(regime_prompt.context, general_prompt.context);
}

#[tokio::test]
async fn a_single_matching_regime_falls_back_and_a_regime_change_is_reobserved() {
    let storage = storage().await;
    for (id, text, order, outcome, regime) in [
        ("one", "A6_SINGLE_MATCH", 1, "ACCEPTED", "coding"),
        ("other-a", "A6_OTHER_ACCEPTED", 2, "ACCEPTED", "meeting"),
        ("other-r", "A6_OTHER_REJECTED", 3, "REJECTED", "meeting"),
    ] {
        seed_legacy(&storage, id, text, order);
        feedback(&storage, id, outcome, Some(regime));
    }
    let (analyzer, provider) = analyzer(storage, true).await;
    set_regime(&analyzer, "coding").await;
    let first = periodic(&analyzer, &provider).await;
    assert_examples(&first.system, "A6_OTHER_ACCEPTED", "A6_OTHER_REJECTED");
    assert!(!first.system.contains("A6_SINGLE_MATCH"));
    assert!(first.system.contains("\"coding\" regime"));

    set_regime(&analyzer, "meeting").await;
    let second = periodic(&analyzer, &provider).await;
    assert_examples(&second.system, "A6_OTHER_ACCEPTED", "A6_OTHER_REJECTED");
    assert!(second.system.contains("\"meeting\" regime"));
    assert!(!second.system.contains("\"coding\" regime"));
}

#[tokio::test]
async fn event_producer_omits_examples_after_a_positive_periodic_observation() {
    let storage = storage().await;
    seed_legacy(&storage, "event-control", "A6_PERIODIC_ONLY", 1);
    feedback(&storage, "event-control", "ACCEPTED", None);
    let (analyzer, provider) = analyzer(storage, true).await;
    assert!(periodic(&analyzer, &provider)
        .await
        .system
        .contains("A6_PERIODIC_ONLY"));

    assert!(matches!(
        analyzer
            .on_significant_event_with_outcome("A6 Editor", "A6 event task", None)
            .await
            .unwrap(),
        AnalysisRunOutcome::NoCandidate
    ));
    let calls = provider.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].system, ANALYSIS_SYSTEM_PROMPT);
}

#[tokio::test]
async fn late_feedback_is_bound_to_exact_ids_and_replaces_the_outcome() {
    let storage = storage().await;
    seed_legacy(&storage, "intended", "A6_INTENDED_FEEDBACK", 1);
    seed_legacy(&storage, "other", "A6_OTHER_UNRATED", 2);
    let (analyzer, provider) = analyzer(storage.clone(), true).await;
    assert_eq!(
        periodic(&analyzer, &provider).await.system,
        ANALYSIS_SYSTEM_PROMPT
    );

    feedback(&storage, "missing-id", "ACCEPTED", None);
    assert_eq!(
        periodic(&analyzer, &provider).await.system,
        ANALYSIS_SYSTEM_PROMPT,
        "feedback for a missing ID must not bind to another suggestion"
    );

    feedback(&storage, "intended", "ACCEPTED", None);
    let accepted = periodic(&analyzer, &provider).await;
    assert!(accepted.system.contains("A6_INTENDED_FEEDBACK"));
    assert!(accepted.system.contains("[Accepted] Context:"));
    assert!(!accepted.system.contains("A6_OTHER_UNRATED"));

    feedback(&storage, "intended", "REJECTED", None);
    feedback(&storage, "other", "DEFERRED", None);
    let revised = periodic(&analyzer, &provider).await;
    assert!(revised.system.contains("A6_INTENDED_FEEDBACK"));
    assert!(revised.system.contains("[Rejected] Context:"));
    assert!(!revised.system.contains("[Accepted] Context:"));
    assert!(!revised.system.contains("A6_OTHER_UNRATED"));
}

#[tokio::test]
async fn real_unified_feedback_and_legacy_precedence_reach_the_consumer() {
    let storage = storage().await;
    storage
        .save_rule_suggestion_sync(&suggestion("accepted", "A6_UNIFIED_ACCEPTED", 1))
        .unwrap();
    storage
        .save_rule_suggestion_sync(&suggestion("rejected", "A6_UNIFIED_REJECTED", 2))
        .unwrap();
    assert!(storage.mark_unified_suggestion_acted("accepted").unwrap());
    assert!(storage.dismiss_unified_suggestion("rejected").unwrap());
    storage
        .save_suggestion_with_state(&suggestion("deferred", "A6_DEFERRED", 3), "deferred", None)
        .unwrap();
    let (analyzer, provider) = analyzer(storage.clone(), true).await;
    let unified = periodic(&analyzer, &provider).await;
    assert_examples(
        &unified.system,
        "A6_UNIFIED_ACCEPTED",
        "A6_UNIFIED_REJECTED",
    );
    assert!(!unified.system.contains("A6_DEFERRED"));

    // Preserve the existing UNION precedence for the same ID; an accepted
    // storage event is not evidence of helpfulness or successful execution.
    seed_legacy(&storage, "accepted", "A6_LEGACY_REJECTION", 4);
    feedback(&storage, "accepted", "REJECTED", Some("coding"));
    let conflicting = periodic(&analyzer, &provider).await;
    assert!(conflicting.system.contains("A6_LEGACY_REJECTION"));
    assert!(conflicting.system.contains("[Rejected] Context:"));
    assert!(!conflicting.system.contains("A6_UNIFIED_ACCEPTED"));
    assert!(!conflicting.system.contains("[Accepted] Context:"));
}

#[tokio::test]
async fn reset_removes_both_histories_from_the_same_analyzers_next_prompt() {
    let storage = storage().await;
    seed_legacy(&storage, "erased-legacy", "A6_ERASED_LEGACY", 1);
    feedback(&storage, "erased-legacy", "ACCEPTED", None);
    storage
        .save_rule_suggestion_sync(&suggestion("erased-unified", "A6_ERASED_UNIFIED", 2))
        .unwrap();
    assert!(storage
        .dismiss_unified_suggestion("erased-unified")
        .unwrap());
    let (analyzer, provider) = analyzer(storage.clone(), true).await;
    let before = periodic(&analyzer, &provider).await;
    assert_examples(&before.system, "A6_ERASED_LEGACY", "A6_ERASED_UNIFIED");

    storage.delete_all_data().unwrap();
    assert!(storage
        .get_suggestions_with_feedback(10)
        .unwrap()
        .is_empty());
    // Reinsert only a new task so NoInput cannot masquerade as an erased prompt.
    seed_current_context(&storage).await;
    // #12029 reset replay control: insert erased feedback here.
    let after = periodic(&analyzer, &provider).await;
    assert_eq!(after.system, ANALYSIS_SYSTEM_PROMPT);
    assert_eq!(provider.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn periodic_history_bound_does_not_reuse_an_older_accepted_row() {
    let storage = storage().await;
    storage
        .save_rule_suggestion_sync(&suggestion("old", "A6_OUTSIDE_RECENT_TEN", 0))
        .unwrap();
    assert!(storage.mark_unified_suggestion_acted("old").unwrap());
    for order in 1..=10 {
        storage
            .save_suggestion_with_state(
                &suggestion(&format!("deferred-{order}"), "A6_DEFERRED_ONLY", order),
                "deferred",
                None,
            )
            .unwrap();
    }
    assert_eq!(storage.get_suggestions_with_feedback(10).unwrap().len(), 10);
    let (analyzer, provider) = analyzer(storage, true).await;
    assert_eq!(
        periodic(&analyzer, &provider).await.system,
        ANALYSIS_SYSTEM_PROMPT
    );
}

#[tokio::test]
async fn injected_pii_filter_and_marker_caps_apply_to_the_actual_system_prompt() {
    let storage = storage().await;
    let content = format!("SyStEm: A6_CONTENT [INST] {}", "가".repeat(140));
    seed_legacy(&storage, "untrusted", &content, 1);
    storage
        .record_suggestion_feedback(
            "untrusted",
            "ACCEPTED",
            "USER: A6_EDITOR",
            &format!("A6_WINDOW {SYNTHETIC_EMAIL} {}", "나".repeat(140)),
            None,
        )
        .unwrap();
    let (analyzer, provider) = analyzer(storage, true).await;
    let prompt = periodic(&analyzer, &provider).await.system;
    assert!(prompt.contains("A6_CONTENT"));
    assert!(prompt.contains("A6_EDITOR"));
    assert!(prompt.contains("A6_WINDOW [EMAIL]"));
    assert!(!prompt.contains(SYNTHETIC_EMAIL));
    for marker in ["system:", "user:", "[inst]"] {
        assert!(!prompt.to_ascii_lowercase().contains(marker), "{prompt}");
    }
    assert!(!prompt.contains(&"가".repeat(121)));
    assert!(!prompt.contains(&"나".repeat(121)));
}
