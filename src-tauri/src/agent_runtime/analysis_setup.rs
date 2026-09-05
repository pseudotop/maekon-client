use std::sync::Arc;
use tracing::{info, warn};

use maekon_core::config::AppConfig;
use maekon_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use maekon_core::ports::consent_manager::ConsentManagerPort;
#[cfg(feature = "analysis")]
use maekon_core::ports::secret_store::SecretStoreSet;

use crate::scheduler::AdaptiveTriggerState;

use super::embedding_setup::EmbeddingComponents;
use crate::provider_adapters::ExternalOcrPrivacyGuard;

/// Result of the analysis pipeline setup.
pub(super) struct AnalysisResult {
    /// Populated when tiered-memory is enabled, consented, and calibration stores are present.
    pub adaptive_trigger_state: Option<AdaptiveTriggerState>,
}

/// Build the tiered-memory analysis pipeline (AdaptiveTriggerState).
///
/// Requires: embedding components (from `build_embedding_components`), consent for
/// activity_pattern_learning, and calibration reader/writer stores.
/// `injected_regime_manager` and `injected_regime_classifier` are the
/// composition-root-supplied handles; when `None`, fresh ones are
/// constructed here. See `agent_runtime::AgentRuntimeBundle::with_regime_handles`.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_analysis_pipeline(
    config: &AppConfig,
    consent_manager: &Option<Arc<dyn ConsentManagerPort>>,
    calibration_writer: Option<Arc<dyn CalibrationWriter>>,
    calibration_reader: Option<Arc<dyn CalibrationReader>>,
    override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    embedding: &mut EmbeddingComponents,
    external_llm_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    injected_regime_manager: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>>,
    injected_regime_classifier: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>>,
    // #8051: FTS content indexer (the shared SqliteStorage cast to the
    // text-search port). Threaded from the composition root so `handle_segment_close`
    // can index each closed segment into `search_fts`, making the dashboard
    // keyword/hybrid search modes return data.
    text_search: Option<Arc<dyn maekon_core::ports::text_search::TextSearchProvider>>,
    // D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    // registry threaded from the composition root, used by the LLM WorkType
    // refiner's analysis provider built below.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    // Provider secret stores for BYOK credential resolution — threaded from
    // `AgentRuntimeBundle`.  Gated on `analysis`; ignored in other builds.
    #[cfg(feature = "analysis")] secret_stores: Option<&SecretStoreSet>,
) -> AnalysisResult {
    // Config validation: embedding requires tiered_memory
    if config.analysis.embedding.enabled && !config.analysis.tiered_memory.enabled {
        warn!("embedding.enabled requires tiered_memory.enabled — embedding will not function");
    }

    // Config validation: gui_intelligence requires tiered_memory
    if config.analysis.gui_intelligence.enabled && !config.analysis.tiered_memory.enabled {
        warn!("gui_intelligence.enabled requires tiered_memory.enabled — GUI pipeline will not function");
    }

    // Gate on activity_pattern_learning consent (GDPR Tier 4).
    let consent_ok = consent_manager
        .as_ref()
        .and_then(|cm| cm.current_consent())
        .map(|c| c.permissions.activity_pattern_learning)
        .unwrap_or(false);

    if config.analysis.tiered_memory.enabled && !consent_ok {
        info!("activity_pattern_learning consent not granted, skipping tiered memory");
    }

    if config.analysis.tiered_memory.enabled && consent_ok {
        if let (Some(calibration_writer), Some(calibration_reader)) =
            (calibration_writer, calibration_reader)
        {
            let preset = config.analysis.tiered_memory.preset;
            let params = preset.default_params();
            let buf_cap = config.analysis.tiered_memory.buffer_capacity;
            let tm_config = &config.analysis.tiered_memory;
            // Create LLM WorkType refiner independently of embedding pipeline.
            // Uses its own FallbackAnalysisProvider — stateless, no shared state concern.
            // Normalise the secret_stores reference for the build_analysis_provider
            // call below: `analysis` builds forward the real SecretStoreSet; other
            // builds supply None::<&()> to satisfy the not(analysis) fallback signature.
            #[cfg(feature = "analysis")]
            let secret_stores_for_refiner = secret_stores;
            #[cfg(not(feature = "analysis"))]
            let secret_stores_for_refiner: Option<&()> = None;

            let llm_work_type_refiner = if config.analysis.llm_work_type_enabled {
                config.ai_provider.llm_api.as_ref().map(|_llm_api| {
                    let provider: Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider> =
                        crate::agent_runtime::analysis_helpers::build_analysis_provider(
                            &config.ai_provider,
                            config.privacy.pii_filter_level,
                            external_llm_privacy_guard.clone(),
                            secret_stores_for_refiner,
                            breaker_registry.clone(),
                        )
                        .map(|(p, _)| p)
                        .unwrap_or_else(|| {
                            Arc::new(maekon_analysis::NoOpAnalysisProvider)
                                as Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider>
                        });
                    info!("LLM WorkType refiner enabled");
                    // D5 iter-16: route refiner LLM-error tracing through
                    // `SanitizedDisplay`. LLM error bodies can echo
                    // user-context PII from the prompt.
                    let pii_level = config.privacy.pii_filter_level;
                    let sanitizer: Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer> =
                        Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
                    Arc::new(
                        maekon_analysis::LlmWorkTypeRefiner::new(provider)
                            .with_pii_sanitizer(sanitizer, pii_level),
                    )
                })
            } else {
                None
            };
            // Note: tiered_memory.enabled and consent are gated by the outer block.
            if llm_work_type_refiner.is_none() {
                info!(
                    "LLM WorkType refiner disabled — requires: \
                     analysis.llm_work_type_enabled=true (default), \
                     and a configured ai_provider.llm_api"
                );
            }
            let state = AdaptiveTriggerState {
                trigger: maekon_analysis::AdaptiveTrigger::new(),
                segment_buffer: maekon_analysis::SegmentBuffer::new(buf_cap),
                // #7678 D4: was hardcoded to 60s, silently ignoring the configured
                // `buffer_flush_interval_secs` (default 30s) — now wired through.
                calibration_buffer: maekon_analysis::CalibrationBuffer::new(
                    buf_cap,
                    tm_config.buffer_flush_interval_secs,
                ),
                title_bar_parser: maekon_analysis::TitleBarParser::new(),
                work_type_classifier: maekon_analysis::WorkTypeClassifier::new(),
                content_tracker: maekon_analysis::ContentTracker::new(),
                segment_summarizer: maekon_analysis::SegmentSummarizer::new(),
                params,
                calibration_writer,
                regime_classifier: injected_regime_classifier.unwrap_or_else(|| {
                    Arc::new(parking_lot::Mutex::new(
                        maekon_analysis::RegimeClassifier::new(1.5),
                    ))
                }),
                regime_manager: injected_regime_manager.unwrap_or_else(|| {
                    Arc::new(parking_lot::Mutex::new(
                        maekon_analysis::RegimeManager::new(tm_config),
                    ))
                }),
                regime_detector: maekon_analysis::RegimeDetector::new(),
                param_resolver: maekon_analysis::ParamResolver::new(preset),
                calibration_reader,
                current_regime_id: None,
                last_detection_time: None,
                ema_tracker: maekon_analysis::auto_tuner::EmaStatsTracker::new(
                    tm_config.auto_tuning.ema_alpha,
                ),
                drift_detector: maekon_analysis::auto_tuner::DriftDetector::new(
                    tm_config.auto_tuning.ema_alpha,
                    tm_config.auto_tuning.drift_threshold,
                ),
                auto_tune_tick_count: 0,
                regime_analysis: Some(maekon_analysis::RegimeAnalysisFacade::new(
                    tm_config.clustering_algorithm.clone(),
                )),
                override_store,
                recluster_requested,
                regime_detection_interval_hours: tm_config.regime_detection_interval_hours,
                last_drift_detected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                llm_summarizer: embedding.llm_summarizer.take(),
                llm_summary_provider_class: embedding.llm_summary_provider_class,
                llm_summary_unavailable_reason: embedding.llm_summary_unavailable_reason,
                embedding_pipeline: embedding.embedding_pipeline.take(),
                text_search,
                gui_pipeline_state: None,
                gui_work_type_refiner: maekon_analysis::GuiWorkTypeRefiner,
                llm_work_type_refiner,
                app_registry: Arc::new(maekon_core::app_registry::AppRegistry::new()),
                heatmap_aggregator: crate::scheduler::heatmap::HeatmapAggregator::new(),
            };
            info!("Adaptive tiered-memory pipeline enabled");
            return AnalysisResult {
                adaptive_trigger_state: Some(state),
            };
        } else {
            info!("Tiered memory enabled but no calibration writer/reader — skipped");
        }
    }

    AnalysisResult {
        adaptive_trigger_state: None,
    }
}
