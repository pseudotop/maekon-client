use super::{gui_pipeline, heatmap};
use maekon_core::app_registry::AppRegistry;
use maekon_core::models::tiered_memory::ResolvedParams;
use maekon_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use std::sync::Arc;

/// Wraps all components needed for the adaptive tiered-memory pipeline.
/// Kept as owned (non-Arc) so the monitor loop can mutate the components
/// without interior-mutability overhead.
pub(crate) struct AdaptiveTriggerState {
    pub trigger: maekon_analysis::AdaptiveTrigger,
    pub segment_buffer: maekon_analysis::SegmentBuffer,
    pub calibration_buffer: maekon_analysis::CalibrationBuffer,
    pub title_bar_parser: maekon_analysis::TitleBarParser,
    pub work_type_classifier: maekon_analysis::WorkTypeClassifier,
    pub content_tracker: maekon_analysis::ContentTracker,
    pub segment_summarizer: maekon_analysis::SegmentSummarizer,
    pub params: ResolvedParams,
    pub calibration_writer: Arc<dyn CalibrationWriter>,

    pub regime_classifier: Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>,
    pub regime_manager: Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>,
    pub regime_detector: maekon_analysis::RegimeDetector,
    pub param_resolver: maekon_analysis::ParamResolver,
    pub calibration_reader: Arc<dyn CalibrationReader>,
    pub current_regime_id: Option<String>,
    pub last_detection_time: Option<chrono::DateTime<chrono::Utc>>,

    pub ema_tracker: maekon_analysis::auto_tuner::EmaStatsTracker,
    pub drift_detector: maekon_analysis::auto_tuner::DriftDetector,
    pub auto_tune_tick_count: u64,
    pub regime_analysis: Option<maekon_analysis::RegimeAnalysisFacade>,
    pub override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    pub recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    pub regime_detection_interval_hours: i64,
    pub last_drift_detected: Arc<std::sync::atomic::AtomicBool>,

    pub(crate) llm_summarizer: Option<Arc<maekon_analysis::LlmSegmentSummarizer>>,
    pub(crate) embedding_pipeline: Option<Arc<maekon_analysis::EmbeddingPipeline>>,
    pub(crate) gui_pipeline_state: Option<gui_pipeline::GuiPipelineState>,
    pub(crate) gui_work_type_refiner: maekon_analysis::GuiWorkTypeRefiner,
    pub(crate) llm_work_type_refiner: Option<Arc<maekon_analysis::LlmWorkTypeRefiner>>,
    pub(crate) app_registry: Arc<AppRegistry>,
    pub(crate) heatmap_aggregator: heatmap::HeatmapAggregator,
}
