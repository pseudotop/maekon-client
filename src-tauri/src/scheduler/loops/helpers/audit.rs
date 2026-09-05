//! Consent & PII level change audit helpers, and segment record conversion.

use tracing::{info, warn};

use maekon_core::models::storage_records::SegmentSummaryRecord;
use maekon_core::models::tiered_memory::{
    ContentActivity, SegmentSummary, TriggerReason, WorkType,
};

/// Log audit events when full_text_extraction consent or PII extraction
/// level changes between ticks. Returns updated `(prev_consent, prev_pii_level)`.
pub(crate) fn audit_consent_and_pii_changes(
    full_text_consent: bool,
    prev_full_text_consent: bool,
    pii_level: maekon_core::config::PiiFilterLevel,
    prev_pii_level: maekon_core::config::PiiFilterLevel,
) -> (bool, maekon_core::config::PiiFilterLevel) {
    let mut new_consent = prev_full_text_consent;
    let mut new_pii = prev_pii_level;

    if full_text_consent != prev_full_text_consent {
        if full_text_consent {
            info!(
                event = "full_text_extraction_consent_granted",
                "User granted full_text_extraction consent — Off PII level now effective"
            );
        } else {
            warn!(
                event = "full_text_extraction_consent_revoked",
                "User revoked full_text_extraction consent — falling back to Standard PII level"
            );
        }
        new_consent = full_text_consent;
    }

    if pii_level != prev_pii_level {
        info!(
            event = "pii_extraction_level_changed",
            old = ?prev_pii_level,
            new = ?pii_level,
            "PII extraction level changed"
        );
        new_pii = pii_level;
    }

    (new_consent, new_pii)
}

/// Convert a SegmentSummaryRecord (storage row) to SegmentSummary (domain model)
/// for use with DailyDigestGenerator.
pub(crate) fn record_to_segment_summary(r: &SegmentSummaryRecord) -> Option<SegmentSummary> {
    let start_time = r.start_time.parse().ok()?;
    let end_time = r.end_time.parse().ok()?;

    let app_breakdown: std::collections::HashMap<String, u64> =
        serde_json::from_str(&r.app_breakdown).unwrap_or_default();

    let content_activities: Vec<ContentActivity> =
        serde_json::from_str(&r.content_activities_json).unwrap_or_default();

    Some(SegmentSummary {
        segment_id: r.segment_id.clone(),
        start_time,
        end_time,
        duration_secs: r.duration_secs,
        regime_id: r.regime_id.clone(),
        trigger_reason: TriggerReason::RegimeChange,
        event_count: 0,
        app_breakdown,
        category_breakdown: std::collections::HashMap::new(),
        context_switch_count: r.context_switch_count,
        dominant_category: r.dominant_category.clone(),
        avg_importance: 0.5,
        patterns_detected: vec![],
        content_activities,
        container: None,
        llm_summary: r.llm_summary.clone(),
    })
}

/// Resolve the human-readable label for a regime id against a regime list.
///
/// Prefers the `Active` entry for the id (defense-in-depth against a stale
/// duplicate-id `Inactive` entry, mirroring the monitor loop's regime
/// resolution), then falls back to any entry with the id. Returns the `name`
/// when set, otherwise the auto-generated `auto_label`. `None` when the id is
/// absent — callers pass the absence through rather than leaking the opaque
/// positional id ("regime-N") downstream into the LLM prompt (#7480).
fn resolve_regime_label(
    regimes: &[maekon_core::models::tiered_memory::Regime],
    regime_id: &str,
) -> Option<String> {
    use maekon_core::models::tiered_memory::RegimeStatus;
    regimes
        .iter()
        .find(|r| r.regime_id == regime_id && r.status == RegimeStatus::Active)
        .or_else(|| regimes.iter().find(|r| r.regime_id == regime_id))
        .map(|r| r.name.clone().unwrap_or_else(|| r.auto_label.clone()))
}

fn category_for_work_type(work_type: WorkType) -> &'static str {
    match work_type {
        WorkType::ActiveCoding
        | WorkType::CodeReview
        | WorkType::TerminalCommands
        | WorkType::LogReading => "Development",
        WorkType::Writing
        | WorkType::Reading
        | WorkType::DocumentWriting
        | WorkType::DocumentReading => "Documentation",
        WorkType::ChatComposing => "Communication",
        WorkType::PassiveMeeting | WorkType::ActiveMeeting => "Meeting",
        WorkType::Browsing => "Browser",
        WorkType::Designing => "Design",
        WorkType::FormFilling | WorkType::Navigation | WorkType::Unknown => "Other",
    }
}

fn dominant_category_for_activities(activities: &[ContentActivity]) -> String {
    let mut durations = std::collections::HashMap::<&'static str, u64>::new();
    for activity in activities {
        let category = category_for_work_type(activity.work_type);
        *durations.entry(category).or_insert(0) += activity.duration_secs.max(1);
    }
    durations
        .into_iter()
        .max_by_key(|(_, duration)| *duration)
        .map(|(category, _)| category.to_string())
        .unwrap_or_else(|| "Other".to_string())
}

/// Build a `SegmentStats` snapshot from the current `AdaptiveTriggerState`.
/// Returns `None` if the content tracker has no active content.
pub(crate) fn build_segment_stats_snapshot(
    ts: &crate::scheduler::AdaptiveTriggerState,
) -> Option<maekon_analysis::SegmentStats> {
    let activities = ts.content_tracker.peek();
    let entries = maekon_analysis::to_content_summary_entries(&activities);
    if entries.is_empty() {
        return None;
    }

    let duration_mins = ts
        .trigger
        .current_segment_start()
        .map(|start| {
            let elapsed = (chrono::Utc::now() - start).num_seconds().max(0) as u32;
            elapsed / 60
        })
        .unwrap_or(0);

    let gui_patterns: Vec<String> = entries
        .iter()
        .flat_map(|e| e.gui_patterns.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    Some(maekon_analysis::SegmentStats {
        duration_mins,
        // #7480: emit the HUMAN regime label (name > auto_label) into the LLM
        // prompt, not the opaque positional id ("regime-N") which carries zero
        // semantic signal. Resolve it from the regime manager by the current id;
        // pass `None` through when the id is unresolved rather than leaking it.
        regime_label: ts.current_regime_id.as_deref().and_then(|id| {
            let mgr = ts.regime_manager.lock();
            resolve_regime_label(mgr.all_regimes(), id)
        }),
        event_count: activities.len() as u32,
        context_switches: activities.len().saturating_sub(1) as u32,
        dominant_category: dominant_category_for_activities(&activities),
        content_summary: entries,
        gui_patterns,
    })
}

#[cfg(test)]
mod tests {
    use super::{build_segment_stats_snapshot, resolve_regime_label};
    use crate::scheduler::AdaptiveTriggerState;
    use chrono::{DateTime, Utc};
    use maekon_core::config::TieredMemoryConfig;
    use maekon_core::error::CoreError;
    use maekon_core::models::tiered_memory::{
        CalibrationEntry, PresetProfile, Regime, RegimeFeatures, RegimeStatus, ResolvedParams,
        TriggerParams,
    };
    use maekon_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
    use maekon_core::types::TimeWindow;
    use std::sync::Arc;

    // ── Minimal calibration mocks (mirror analysis_pipeline test doubles) ──
    struct NoopCalibrationWriter;
    impl CalibrationWriter for NoopCalibrationWriter {
        fn log_batch(&self, _entries: &[CalibrationEntry]) -> Result<(), CoreError> {
            Ok(())
        }
        fn flag_noise_range(&self, _window: &TimeWindow) -> Result<u64, CoreError> {
            Ok(0)
        }
    }

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
        async fn enforce_retention(
            &self,
            _max_days: u32,
            _max_rows: u64,
        ) -> Result<u64, CoreError> {
            Ok(0)
        }
    }

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

    fn make_regime(id: &str, name: Option<&str>, auto_label: &str, status: RegimeStatus) -> Regime {
        Regime {
            regime_id: id.to_string(),
            name: name.map(str::to_string),
            auto_label: auto_label.to_string(),
            centroid: RegimeFeatures::default(),
            optimal_params: TriggerParams::default(),
            sample_count: 100,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status,
        }
    }

    fn push_active_content(ts: &mut AdaptiveTriggerState, label: &str, ts_at: DateTime<Utc>) {
        use maekon_analysis::content_tracker::ContentUpdateInput;
        use maekon_core::models::tiered_memory::{ContentType, EngagementMetrics, WorkType};
        // A single update makes the tracker's `peek()` non-empty (the active
        // in-progress content), which is the gate for producing a snapshot.
        ts.content_tracker.update(ContentUpdateInput {
            content_label: label.to_string(),
            content_type: ContentType::default(),
            work_type: WorkType::default(),
            engagement: EngagementMetrics::default(),
            confidence: 1.0,
            timestamp: ts_at,
            gui_summary: None,
        });
    }

    fn push_content(
        ts: &mut AdaptiveTriggerState,
        label: &str,
        content_type: maekon_core::models::tiered_memory::ContentType,
        work_type: maekon_core::models::tiered_memory::WorkType,
        ts_at: DateTime<Utc>,
    ) {
        use maekon_analysis::content_tracker::ContentUpdateInput;
        use maekon_core::models::tiered_memory::EngagementMetrics;

        ts.content_tracker.update(ContentUpdateInput {
            content_label: label.to_string(),
            content_type,
            work_type,
            engagement: EngagementMetrics::default(),
            confidence: 1.0,
            timestamp: ts_at,
            gui_summary: None,
        });
    }

    /// Pure rule: prefer `name`, else `auto_label`; prefer the `Active` entry
    /// under a duplicate id; `None` when the id is absent.
    #[test]
    fn resolve_regime_label_prefers_name_then_auto_label_and_active() {
        let regimes = vec![
            make_regime("regime-0", None, "stale label", RegimeStatus::Inactive),
            make_regime(
                "regime-0",
                None,
                "Deep Focus (VSCode)",
                RegimeStatus::Active,
            ),
            make_regime(
                "regime-1",
                Some("Custom Name"),
                "auto",
                RegimeStatus::Active,
            ),
        ];
        // Active entry wins over the stale inactive duplicate.
        assert_eq!(
            resolve_regime_label(&regimes, "regime-0").as_deref(),
            Some("Deep Focus (VSCode)")
        );
        // `name` overrides `auto_label`.
        assert_eq!(
            resolve_regime_label(&regimes, "regime-1").as_deref(),
            Some("Custom Name")
        );
        // Unknown id resolves to nothing (id is not leaked downstream).
        assert_eq!(resolve_regime_label(&regimes, "regime-9"), None);
    }

    /// #7480 regression: the LLM-prompt segment snapshot must carry the HUMAN
    /// regime label, not the opaque positional id. Pre-fix
    /// `build_segment_stats_snapshot` set `regime_label = current_regime_id`,
    /// so this asserted `Some("regime-0")` — the assertion below fails on
    /// pre-fix code.
    #[test]
    fn segment_stats_uses_human_regime_label_not_opaque_id() {
        let mut ts = make_trigger_state();
        ts.regime_manager.lock().hydrate_from(vec![make_regime(
            "regime-0",
            None,
            "Deep Focus (VSCode)",
            RegimeStatus::Active,
        )]);
        ts.current_regime_id = Some("regime-0".to_string());
        push_active_content(&mut ts, "main.rs", Utc::now());

        let stats = build_segment_stats_snapshot(&ts)
            .expect("snapshot must be produced when content is active");
        assert_eq!(
            stats.regime_label.as_deref(),
            Some("Deep Focus (VSCode)"),
            "the LLM prompt must carry the human label, not the opaque regime id"
        );
    }

    #[test]
    fn segment_stats_reports_real_content_counts_and_time_dominant_category() {
        use maekon_core::models::tiered_memory::{ContentType, WorkType};

        let mut ts = make_trigger_state();
        let base = Utc::now() - chrono::Duration::minutes(40);
        push_content(
            &mut ts,
            "team-chat",
            ContentType::Channel,
            WorkType::ChatComposing,
            base,
        );
        push_content(
            &mut ts,
            "main.rs",
            ContentType::File,
            WorkType::ActiveCoding,
            base + chrono::Duration::minutes(1),
        );

        let stats = build_segment_stats_snapshot(&ts)
            .expect("snapshot must be produced when content is active");

        assert_eq!(stats.event_count, 2);
        assert_eq!(stats.context_switches, 1);
        assert_eq!(stats.dominant_category, "Development");
    }
}
