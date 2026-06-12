//! AdaptiveTriggerState — owned components for the adaptive tiered-memory
//! pipeline. Extracted from scheduler/mod.rs (ADR-013 split).
//!
//! Kept as an owned (non-Arc) struct so the monitor loop can mutate its
//! components without interior-mutability overhead.

use maekon_core::app_registry::AppRegistry;
use maekon_core::models::tiered_memory::ResolvedParams;
use maekon_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use std::sync::Arc;

use super::gui_pipeline::GuiPipelineState;
use super::heatmap::HeatmapAggregator;

pub(crate) struct AdaptiveTriggerState {
    // --- Base analysis pipeline ---
    pub trigger: maekon_analysis::AdaptiveTrigger,
    pub segment_buffer: maekon_analysis::SegmentBuffer,
    pub calibration_buffer: maekon_analysis::CalibrationBuffer,
    pub title_bar_parser: maekon_analysis::TitleBarParser,
    pub work_type_classifier: maekon_analysis::WorkTypeClassifier,
    pub content_tracker: maekon_analysis::ContentTracker,
    pub segment_summarizer: maekon_analysis::SegmentSummarizer,
    pub params: ResolvedParams,
    pub calibration_writer: Arc<dyn CalibrationWriter>,

    // --- Regime-aware pipeline ---
    //
    // Wrapped in `Arc<parking_lot::Mutex<_>>` so the composition root
    // can share handles with `AppState` for (a) startup hydration from
    // `RegimeStoragePort::load_all`, (b) shutdown save via the guard in
    // `main.rs::RunEvent::Exit`, and (c) `CompositeFeedbackSink` fan-out
    // (feedback_sink.rs). At runtime the scheduler has de-facto
    // exclusive access — the shutdown save guard fires only after
    // `shutdown_tx → shutdown_blocking()` drains the scheduler loops —
    // so scheduler-vs-save contention is absent.
    pub regime_classifier: Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>,
    pub regime_manager: Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>,
    pub regime_detector: maekon_analysis::RegimeDetector,
    pub param_resolver: maekon_analysis::ParamResolver,
    pub calibration_reader: Arc<dyn CalibrationReader>,
    /// ID of the current active regime (for transition detection).
    pub current_regime_id: Option<String>,
    /// Last time regime detection (k-means) was run.
    pub last_detection_time: Option<chrono::DateTime<chrono::Utc>>,

    // --- Auto-tuning ---
    pub ema_tracker: maekon_analysis::auto_tuner::EmaStatsTracker,
    pub drift_detector: maekon_analysis::auto_tuner::DriftDetector,
    pub auto_tune_tick_count: u64,
    pub regime_analysis: Option<maekon_analysis::RegimeAnalysisFacade>,
    pub override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    pub recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    pub regime_detection_interval_hours: i64,
    pub last_drift_detected: Arc<std::sync::atomic::AtomicBool>,

    // --- LLM/embedding pipeline ---
    pub(crate) llm_summarizer: Option<Arc<maekon_analysis::LlmSegmentSummarizer>>,
    pub(crate) embedding_pipeline: Option<Arc<maekon_analysis::EmbeddingPipeline>>,

    // --- GUI Activity Intelligence ---
    pub(crate) gui_pipeline_state: Option<GuiPipelineState>,
    pub(crate) gui_work_type_refiner: maekon_analysis::GuiWorkTypeRefiner,
    pub(crate) llm_work_type_refiner: Option<Arc<maekon_analysis::LlmWorkTypeRefiner>>,

    // --- Application classification ---
    pub(crate) app_registry: Arc<AppRegistry>,

    // --- Heatmap ---
    pub(crate) heatmap_aggregator: HeatmapAggregator,
}
