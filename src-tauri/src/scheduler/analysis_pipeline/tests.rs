//! Tests for the analysis pipeline.

use super::*;
use chrono::{DateTime, Utc};
use maekon_core::config::{ClusteringAlgorithm, PiiFilterLevel, TieredMemoryConfig};
use maekon_core::error::CoreError;
use maekon_core::models::app_registry::KeystrokeProfile;
use maekon_core::models::event::{InputActivityEvent, KeyboardActivity, MouseActivity};
use maekon_core::models::tiered_memory::{
    CalibrationEntry, PresetProfile, ResolvedParams, WorkType,
};
use maekon_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use maekon_core::types::TimeWindow;
use std::sync::Arc;

// ── Mock CalibrationWriter ──────────────────────────────────────
struct NoopCalibrationWriter;

impl CalibrationWriter for NoopCalibrationWriter {
    fn log_batch(&self, _entries: &[CalibrationEntry]) -> Result<(), CoreError> {
        Ok(())
    }
    fn flag_noise_range(&self, _window: &TimeWindow) -> Result<u64, CoreError> {
        Ok(0)
    }
}

// ── Mock CalibrationReader ──────────────────────────────────────
struct NoopCalibrationReader;

#[async_trait::async_trait]
impl CalibrationReader for NoopCalibrationReader {
    async fn get_entries(
        &self,
        _window: &TimeWindow,
        _exclude_noise: bool,
    ) -> Result<Vec<CalibrationEntry>, CoreError> {
        Ok(vec![])
    }
    async fn enforce_retention(&self, _max_days: u32, _max_rows: u64) -> Result<u64, CoreError> {
        Ok(0)
    }
}

struct FewCalibrationReader {
    entries: Vec<CalibrationEntry>,
}

#[async_trait::async_trait]
impl CalibrationReader for FewCalibrationReader {
    async fn get_entries(
        &self,
        _window: &TimeWindow,
        _exclude_noise: bool,
    ) -> Result<Vec<CalibrationEntry>, CoreError> {
        Ok(self.entries.clone())
    }
    async fn enforce_retention(&self, _max_days: u32, _max_rows: u64) -> Result<u64, CoreError> {
        Ok(0)
    }
}

// ── Mock StorageService ─────────────────────────────────────────
struct NoopStorage;

#[async_trait::async_trait]
impl maekon_core::ports::storage::StorageService for NoopStorage {
    async fn save_event(
        &self,
        _event: &maekon_core::models::event::Event,
    ) -> Result<(), CoreError> {
        Ok(())
    }
    async fn get_events(
        &self,
        _from: DateTime<Utc>,
        _to: DateTime<Utc>,
        _limit: usize,
    ) -> Result<Vec<maekon_core::models::event::Event>, CoreError> {
        Ok(vec![])
    }
    async fn get_pending_events(
        &self,
        _limit: usize,
    ) -> Result<Vec<maekon_core::models::event::Event>, CoreError> {
        Ok(vec![])
    }
    async fn mark_as_sent(&self, _event_ids: &[String]) -> Result<(), CoreError> {
        Ok(())
    }
    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        Ok(0)
    }
    async fn save_suggestion(
        &self,
        _suggestion: &maekon_core::models::suggestion::Suggestion,
    ) -> Result<(), CoreError> {
        Ok(())
    }
    async fn save_activity_segment(
        &self,
        _summary: &maekon_core::models::tiered_memory::SegmentSummary,
    ) -> Result<(), CoreError> {
        Ok(())
    }
    async fn update_segment_llm_summary(
        &self,
        _segment_id: &str,
        _summary: &str,
    ) -> Result<(), CoreError> {
        Ok(())
    }
}

struct RecordingSummaryStorage {
    writes: std::sync::atomic::AtomicUsize,
    last: parking_lot::Mutex<Option<maekon_core::models::ai_summary::AiSummaryArtifact>>,
}

#[async_trait::async_trait]
impl maekon_core::ports::storage::StorageService for RecordingSummaryStorage {
    async fn save_event(
        &self,
        _event: &maekon_core::models::event::Event,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn get_events(
        &self,
        _from: DateTime<Utc>,
        _to: DateTime<Utc>,
        _limit: usize,
    ) -> Result<Vec<maekon_core::models::event::Event>, CoreError> {
        Ok(vec![])
    }

    async fn get_pending_events(
        &self,
        _limit: usize,
    ) -> Result<Vec<maekon_core::models::event::Event>, CoreError> {
        Ok(vec![])
    }

    async fn mark_as_sent(&self, _event_ids: &[String]) -> Result<(), CoreError> {
        Ok(())
    }

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        Ok(0)
    }

    async fn save_suggestion(
        &self,
        _suggestion: &maekon_core::models::suggestion::Suggestion,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn save_activity_segment(
        &self,
        _summary: &maekon_core::models::tiered_memory::SegmentSummary,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn update_segment_llm_summary(
        &self,
        _segment_id: &str,
        _summary: &str,
    ) -> Result<(), CoreError> {
        Ok(())
    }

    async fn update_segment_ai_summary(
        &self,
        _segment_id: &str,
        artifact: &maekon_core::models::ai_summary::AiSummaryArtifact,
    ) -> Result<(), CoreError> {
        self.writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        *self.last.lock() = Some(artifact.clone());
        Ok(())
    }
}

struct FixedSummaryProvider;

#[async_trait::async_trait]
impl maekon_core::ports::analysis_provider::AnalysisProvider for FixedSummaryProvider {
    async fn analyze(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<Vec<maekon_core::models::suggestion::Suggestion>, CoreError> {
        Ok(vec![])
    }

    async fn summarize_text(
        &self,
        _context_json: &str,
        _system_prompt: &str,
    ) -> Result<String, CoreError> {
        Ok("Provider-backed segment summary".to_string())
    }

    fn provider_name(&self) -> &str {
        "fixed-summary-provider"
    }
}

/// Helper: build a minimal AdaptiveTriggerState for testing.
fn make_trigger_state() -> AdaptiveTriggerState {
    let config = TieredMemoryConfig::default();
    AdaptiveTriggerState {
        trigger: maekon_analysis::AdaptiveTrigger::new(),
        segment_buffer: maekon_analysis::SegmentBuffer::new(200),
        calibration_buffer: maekon_analysis::CalibrationBuffer::new(50, 60),
        title_bar_parser: maekon_analysis::TitleBarParser::new(),
        work_type_classifier: maekon_analysis::WorkTypeClassifier::new(),
        content_tracker: maekon_analysis::ContentTracker::new(),
        segment_summarizer: maekon_analysis::SegmentSummarizer::new(),
        params: ResolvedParams::default(),
        calibration_writer: Arc::new(NoopCalibrationWriter),
        regime_classifier: Arc::new(parking_lot::Mutex::new(
            maekon_analysis::RegimeClassifier::new(1.5),
        )),
        regime_manager: Arc::new(parking_lot::Mutex::new(
            maekon_analysis::RegimeManager::new(&config),
        )),
        regime_detector: maekon_analysis::RegimeDetector::new(),
        param_resolver: maekon_analysis::ParamResolver::new(PresetProfile::Developer),
        calibration_reader: Arc::new(NoopCalibrationReader),
        current_regime_id: None,
        last_detection_time: None,
        ema_tracker: maekon_analysis::auto_tuner::EmaStatsTracker::new(0.05),
        drift_detector: maekon_analysis::auto_tuner::DriftDetector::new(0.05, 3.0),
        auto_tune_tick_count: 0,
        regime_analysis: None,
        override_store: None,
        recluster_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        regime_detection_interval_hours: 2,
        last_drift_detected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        llm_summarizer: None,
        llm_summary_provider_class: None,
        llm_summary_unavailable_reason: Some(
            maekon_core::models::ai_summary::AiSummaryFailureReason::PipelineDisabled,
        ),
        embedding_pipeline: None,
        text_search: None,
        gui_pipeline_state: None,
        gui_work_type_refiner: maekon_analysis::GuiWorkTypeRefiner,
        llm_work_type_refiner: None,
        app_registry: Arc::new(maekon_core::app_registry::AppRegistry::new()),
        heatmap_aggregator: crate::scheduler::heatmap::HeatmapAggregator::new(),
    }
}

fn make_input_snap() -> InputActivityEvent {
    InputActivityEvent {
        timestamp: Utc::now(),
        period_secs: 3,
        mouse: MouseActivity {
            click_count: 2,
            move_distance: 150.0,
            scroll_count: 0,
            last_position: Some((500.0, 300.0)),
            double_click_count: 0,
            right_click_count: 0,
        },
        keyboard: KeyboardActivity {
            keystrokes_per_min: 40,
            total_keystrokes: 10,
            typing_bursts: 1,
            shortcut_count: 0,
            correction_count: 0,
        },
        app_name: "VS Code".to_string(),
        keystroke_profile: None,
    }
}

fn make_calibration_entry(timestamp: DateTime<Utc>) -> CalibrationEntry {
    CalibrationEntry {
        timestamp,
        event_type: "window".to_string(),
        app_name: "VS Code".to_string(),
        app_category: maekon_core::models::work_session::AppCategory::Development,
        event_importance: 0.5,
        density_signal: 0.5,
        importance_signal: 0.5,
        context_signal: 0.5,
        buffer_signal: 0.5,
        trigger_score: 0.5,
        trigger_action: None,
        active_regime_id: None,
        params_version_id: "test".to_string(),
        params_json: "{}".to_string(),
        is_noise: false,
    }
}

#[tokio::test]
async fn app_switch_triggers_trigger_evaluation() {
    let mut ts = make_trigger_state();
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = Arc::new(NoopStorage);
    let input = make_input_snap();

    // Simulate app switch: VS Code → Chrome
    let prev_app = Some("Chrome".to_string());
    run_analysis_tick(
        &mut ts,
        "VS Code",
        "main.rs - maekon - Visual Studio Code",
        &prev_app,
        true, // app_changed
        &input,
        None,
        None,
        &storage,
        PiiFilterLevel::Off,
    )
    .await;

    // The trigger should have processed at least one event (density > 0)
    assert!(ts.trigger.current_density_signal() > 0.0);
    // Context signal should be boosted (AppSwitchNew is a context event)
    assert!(ts.trigger.current_context_signal() > 0.0);
}

#[test]
fn constrained_clustering_panic_restores_regime_analysis_facade() {
    let mut ts = make_trigger_state();
    ts.regime_analysis = Some(maekon_analysis::RegimeAnalysisFacade::new(
        ClusteringAlgorithm::Kmeans,
    ));

    let facade = ts.regime_analysis.take().expect("facade should be present");
    let (facade_back, result, algorithm): (_, Result<(), CoreError>, _) =
        super::regime::recluster_with_panic_capture(facade, |_facade| {
            panic!("synthetic clustering panic")
        });
    ts.regime_analysis = Some(facade_back);

    assert_eq!(algorithm, "kmeans");
    assert!(matches!(result, Err(CoreError::Internal { .. })));
    assert!(ts.regime_analysis.is_some());
}

#[tokio::test]
async fn content_tracker_accumulates_on_same_app() {
    let mut ts = make_trigger_state();
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = Arc::new(NoopStorage);
    let input = make_input_snap();

    // Two ticks on same app, no app change.
    // Use the standard VS Code title format: "{file} - {project} - Visual Studio Code"
    for _ in 0..2 {
        run_analysis_tick(
            &mut ts,
            "VS Code",
            "main.rs - maekon - Visual Studio Code",
            &None,
            false,
            &input,
            None,
            None,
            &storage,
            PiiFilterLevel::Off,
        )
        .await;
    }

    // Content tracker should have an active item (not yet drained)
    // Drain and verify
    let activities = ts.content_tracker.drain_all(Utc::now());
    // Title bar parser parses "main.rs" from the VS Code title format
    assert!(!activities.is_empty());
    assert_eq!(activities[0].content_label, "main.rs");
}

#[tokio::test]
async fn analysis_tick_uses_subcategory_classifier_for_terminal_commands() {
    let mut ts = make_trigger_state();
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = Arc::new(NoopStorage);
    let mut input = make_input_snap();
    input.app_name = "Terminal".to_string();
    input.keyboard = KeyboardActivity {
        keystrokes_per_min: 30,
        total_keystrokes: 100,
        typing_bursts: 1,
        shortcut_count: 0,
        correction_count: 0,
    };
    input.keystroke_profile = Some(KeystrokeProfile {
        enter_ratio: 0.20,
        total_keystrokes: 100,
        ..KeystrokeProfile::default()
    });

    run_analysis_tick(
        &mut ts,
        "Terminal",
        "dev@host: ~/projects/oneshim",
        &None,
        false,
        &input,
        None,
        None,
        &storage,
        PiiFilterLevel::Off,
    )
    .await;

    let activities = ts.content_tracker.drain_all(Utc::now());
    assert_eq!(activities.len(), 1);
    assert_eq!(activities[0].work_type, WorkType::TerminalCommands);
}

#[tokio::test]
async fn regime_classification_runs() {
    let mut ts = make_trigger_state();
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = Arc::new(NoopStorage);
    let input = make_input_snap();

    // Feed several events from a development app
    for i in 0..5 {
        let app_changed = i == 0;
        run_analysis_tick(
            &mut ts,
            "VS Code",
            "main.rs - maekon - Visual Studio Code",
            &None,
            app_changed,
            &input,
            None,
            None,
            &storage,
            PiiFilterLevel::Off,
        )
        .await;
    }

    // Auto-tune tick count should have incremented
    assert_eq!(ts.auto_tune_tick_count, 5);
}

#[tokio::test]
async fn multiple_app_switches_populate_content() {
    let mut ts = make_trigger_state();
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = Arc::new(NoopStorage);
    let input = make_input_snap();

    // VS Code → Chrome → Slack
    let apps = [
        ("VS Code", "main.rs - maekon - Visual Studio Code"),
        ("Chrome", "Google Search"),
        ("Slack", "#general — Slack"),
    ];

    let mut prev: Option<String> = None;
    for (name, title) in &apps {
        let changed = prev.as_deref() != Some(*name);
        run_analysis_tick(
            &mut ts,
            name,
            title,
            &prev,
            changed,
            &input,
            None,
            None,
            &storage,
            PiiFilterLevel::Off,
        )
        .await;
        prev = Some(name.to_string());
    }

    // Drain content activities — should have at least 2 (VS Code finalized
    // when Chrome started, Chrome finalized when Slack started)
    let activities = ts.content_tracker.drain_all(Utc::now());
    assert!(
        activities.len() >= 2,
        "expected >= 2 activities, got {}",
        activities.len()
    );
}

#[tokio::test]
async fn params_resolver_updates_on_tick() {
    let mut ts = make_trigger_state();
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = Arc::new(NoopStorage);
    let input = make_input_snap();

    // Initial params from developer preset
    let _initial_t_high = ts.params.t_high;

    run_analysis_tick(
        &mut ts,
        "VS Code",
        "main.rs - maekon - Visual Studio Code",
        &None,
        true,
        &input,
        None,
        None,
        &storage,
        PiiFilterLevel::Off,
    )
    .await;

    // After the tick, params should be resolved (may be same or different
    // depending on regime, but they should exist)
    assert!(ts.params.t_high > 0.0);
    assert!(ts.params.t_low >= 0.0);
    assert!(ts.params.t_low < ts.params.t_high);
}

/// Verifies that the `LLM_SUMMARY_SEMAPHORE` constant-new semaphore is
/// correctly initialised with a permit cap of 4 and that `try_acquire`
/// is non-blocking when permits are available and returns `Err` when
/// exhausted — without spawning any real LLM tasks.
#[tokio::test]
async fn llm_summary_semaphore_caps_at_four() {
    use super::segment::LLM_SUMMARY_SEMAPHORE;

    // Drain all 4 permits.
    let p1 = LLM_SUMMARY_SEMAPHORE.try_acquire().expect("permit 1");
    let p2 = LLM_SUMMARY_SEMAPHORE.try_acquire().expect("permit 2");
    let p3 = LLM_SUMMARY_SEMAPHORE.try_acquire().expect("permit 3");
    let p4 = LLM_SUMMARY_SEMAPHORE.try_acquire().expect("permit 4");

    // 5th acquisition must fail (semaphore exhausted).
    assert!(
        matches!(
            LLM_SUMMARY_SEMAPHORE.try_acquire(),
            Err(tokio::sync::TryAcquireError::NoPermits)
        ),
        "semaphore should be exhausted after 4 permits"
    );

    // Release permits — subsequent acquisition must succeed again.
    drop(p1);
    drop(p2);
    drop(p3);
    drop(p4);

    // Pin: after all 4 permits are released a fresh acquisition must succeed,
    // proving RAII release works correctly.
    let _reacquired = LLM_SUMMARY_SEMAPHORE
        .try_acquire()
        .expect("semaphore must have a free permit after all 4 are dropped");
}

#[tokio::test]
async fn eligible_segment_persists_exactly_one_generated_outcome() {
    use maekon_core::models::ai_summary::AiSummaryProviderClass;
    use maekon_core::models::tiered_memory::{SegmentSummary, TriggerReason};
    use std::collections::HashMap;

    let now = Utc::now();
    let summary = SegmentSummary {
        segment_id: "segment-summary-once".to_string(),
        start_time: now - chrono::Duration::minutes(30),
        end_time: now,
        duration_secs: 1800,
        regime_id: None,
        trigger_reason: TriggerReason::ForcedMaxDuration,
        event_count: 12,
        app_breakdown: HashMap::from([("Editor".to_string(), 1800)]),
        category_breakdown: HashMap::from([("Development".to_string(), 1800)]),
        context_switch_count: 1,
        dominant_category: "Development".to_string(),
        avg_importance: 0.8,
        patterns_detected: vec![],
        content_activities: vec![],
        container: None,
        llm_summary: None,
    };
    let summarizer = Arc::new(
        maekon_analysis::LlmSegmentSummarizer::new_with_provider_class(
            Arc::new(FixedSummaryProvider),
            Box::new(str::to_string),
            true,
            60,
            AiSummaryProviderClass::Loopback,
        ),
    );
    let recording = Arc::new(RecordingSummaryStorage {
        writes: std::sync::atomic::AtomicUsize::new(0),
        last: parking_lot::Mutex::new(None),
    });
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = recording.clone();

    let artifact =
        super::segment::generate_and_persist_summary_outcome(&summarizer, &storage, &summary).await;

    assert!(artifact.is_generated());
    assert_eq!(
        artifact.provider_class,
        Some(AiSummaryProviderClass::Loopback)
    );
    assert_eq!(
        recording.writes.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    assert_eq!(recording.last.lock().as_ref(), Some(&artifact));
}

/// #8051 regression: closing a segment must index its content into the FTS5
/// `search_fts` table so the dashboard "keyword" search mode returns data.
/// Before this wiring the only content writers were test-only (dead-writer),
/// so keyword search always returned empty in production.
#[tokio::test]
async fn segment_close_indexes_content_for_keyword_search() {
    use maekon_analysis::content_tracker::ContentUpdateInput;
    use maekon_analysis::TriggerDecision;
    use maekon_core::models::tiered_memory::{ContentType, EngagementMetrics, TriggerInput};
    use maekon_core::models::work_session::AppCategory;
    use maekon_core::ports::text_search::TextSearchProvider;

    // A single real in-memory SQLite instance is used as BOTH the segment
    // store (StorageService) and the FTS content indexer (TextSearchProvider),
    // mirroring the production Port Instance Sharing wiring.
    let sqlite = Arc::new(maekon_storage::sqlite::SqliteStorage::open_in_memory(30).unwrap());
    let storage: Arc<dyn maekon_core::ports::storage::StorageService> = sqlite.clone();

    let mut ts = make_trigger_state();
    ts.text_search = Some(sqlite.clone() as Arc<dyn TextSearchProvider>);

    let now = Utc::now();

    // 1. Open a segment.
    super::segment::handle_segment_lifecycle(
        &mut ts,
        TriggerDecision::OpenSegment,
        TriggerInput::AppSwitchNew {
            app_name: "VS Code".to_string(),
            prev_app: String::new(),
            category: AppCategory::Development,
        },
        now,
        &storage,
    )
    .await;

    // 2. Accumulate distinctive content the summary will carry.
    ts.content_tracker.update(ContentUpdateInput {
        content_label: "authentication refactor".to_string(),
        content_type: ContentType::File,
        work_type: WorkType::ActiveCoding,
        engagement: EngagementMetrics::default(),
        confidence: 1.0,
        timestamp: now,
        gui_summary: None,
    });

    // 3. Close the segment (later, so duration > 0).
    let later = now + chrono::Duration::seconds(120);
    super::segment::handle_segment_lifecycle(
        &mut ts,
        TriggerDecision::CloseSegment,
        TriggerInput::AppPoll {
            app_name: "VS Code".to_string(),
        },
        later,
        &storage,
    )
    .await;

    // 4. The closed segment content is now keyword-searchable.
    let hits = sqlite.search_fts("authentication", 10).await.unwrap();
    assert_eq!(
        hits.len(),
        1,
        "closed segment content must be indexed into FTS on close"
    );
    assert!(hits[0].matched_text.contains("authentication"));

    // Negative control: an unrelated term does not match.
    let miss = sqlite.search_fts("nonexistentterm", 10).await.unwrap();
    assert!(miss.is_empty());
}

#[tokio::test]
async fn drift_detection_sets_last_drift_flag() {
    let mut ts = make_trigger_state();
    // Feed stable data to initialize detector
    for _ in 0..200 {
        ts.drift_detector.observe(0.5);
    }
    // Force a drift observation with a shifted value
    let drifted = ts.drift_detector.observe(0.95);
    if drifted {
        ts.last_drift_detected
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    assert!(ts
        .last_drift_detected
        .load(std::sync::atomic::Ordering::Relaxed));
}

/// #8045 C1: the periodic detection tick forwards retention-purged regime ids
/// to the classifier, so a hard-deleted regime's per-regime reaction bucket is
/// evicted in the same sweep (no dead keys accumulate on a 24/7 agent).
#[tokio::test]
async fn retention_purge_cascades_to_classifier_stats() {
    let mut ts = make_trigger_state();
    let now = Utc::now();

    // Seed an Archived regime idle far past the retention horizon
    // (archive_days 30 + archived_retention_days 30 = 60d; use 100d). Its
    // centroid is comm-heavy — far from the Development-category calibration
    // features below — so update_from_detection cannot re-bind/reactivate it
    // before the maintenance sweep runs.
    let stale = maekon_core::models::tiered_memory::Regime {
        regime_id: "regime-stale".to_string(),
        name: None,
        auto_label: "stale".to_string(),
        centroid: maekon_core::models::tiered_memory::RegimeFeatures {
            category_communication: 1.0,
            avg_event_rate: 0.9,
            avg_importance: 0.1,
            context_activity_signal: 0.9,
            communication_ratio: 0.9,
            ..Default::default()
        },
        optimal_params: maekon_core::models::tiered_memory::TriggerParams::default(),
        sample_count: 200,
        first_seen: now - chrono::Duration::days(200),
        last_seen: now - chrono::Duration::days(100),
        status: maekon_core::models::tiered_memory::RegimeStatus::Archived,
    };
    ts.regime_manager.lock().hydrate_from(vec![stale]);

    // Seed a per-regime reaction bucket for that regime in the classifier.
    ts.regime_classifier.lock().record_user_reaction(
        &maekon_core::models::suggestion::SuggestionFeedback {
            suggestion_id: "s1".to_string(),
            feedback_type: maekon_core::models::suggestion::FeedbackType::Accepted,
            timestamp: now,
            comment: None,
            regime_id: Some("regime-stale".to_string()),
        },
    );
    assert!(
        ts.regime_classifier
            .lock()
            .per_regime_stats()
            .contains_key("regime-stale"),
        "precondition: classifier holds a bucket for the stale regime"
    );

    // Enough calibration entries (>= 50) so the detection branch reaches the
    // maintenance sweep.
    ts.calibration_reader = Arc::new(FewCalibrationReader {
        entries: (0..60)
            .map(|i| make_calibration_entry(now - chrono::Duration::minutes(i)))
            .collect(),
    });

    super::regime::run_periodic_regime_detection(&mut ts, now).await;

    assert!(
        !ts.regime_manager
            .lock()
            .all_regimes()
            .iter()
            .any(|r| r.regime_id == "regime-stale"),
        "expired archived regime must be hard-deleted by the maintenance sweep"
    );
    assert!(
        !ts.regime_classifier
            .lock()
            .per_regime_stats()
            .contains_key("regime-stale"),
        "the purge must cascade: classifier per-regime bucket evicted in the same tick"
    );
}

#[tokio::test]
async fn on_demand_recluster_clears_request_when_samples_are_insufficient() {
    let mut ts = make_trigger_state();
    let now = Utc::now();
    let previous_detection = now - chrono::Duration::minutes(10);
    ts.last_detection_time = Some(previous_detection);
    ts.calibration_reader = Arc::new(FewCalibrationReader {
        entries: vec![make_calibration_entry(now)],
    });
    ts.recluster_requested
        .store(true, std::sync::atomic::Ordering::Relaxed);

    super::regime::run_periodic_regime_detection(&mut ts, now).await;

    assert!(
        !ts.recluster_requested
            .load(std::sync::atomic::Ordering::Relaxed),
        "insufficient samples must consume the on-demand request to avoid a tick hot-loop"
    );
    assert_eq!(
        ts.last_detection_time,
        Some(previous_detection),
        "insufficient on-demand data must not make the scheduler look recently detected"
    );
}
