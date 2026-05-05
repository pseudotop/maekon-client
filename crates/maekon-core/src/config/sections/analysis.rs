use serde::{Deserialize, Serialize};

use crate::config::enums::Weekday;
use crate::models::tiered_memory::PresetProfile;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_throttle_secs")]
    pub throttle_secs: u64,
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_full_interval_secs")]
    pub full_interval_secs: u64,
    #[serde(default = "default_min_confidence")]
    pub min_confidence: f64,
    #[serde(default = "default_max_suggestions")]
    pub max_suggestions: usize,
    #[serde(default = "default_server_coexistence_lookback_secs")]
    pub server_coexistence_lookback_secs: u64,
    /// Enable LLM-based work type refinement.
    /// When true AND ai_provider.llm_api is configured, the LLM refiner
    /// enhances rule-based WorkType classification with contextual analysis.
    /// Independent of the embedding pipeline — does not require embedding.enabled.
    /// Default: true (opt-out).
    #[serde(default = "default_true")]
    pub llm_work_type_enabled: bool,
    #[serde(default)]
    pub tiered_memory: TieredMemoryConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub gui_intelligence: GuiIntelligenceConfig,
    #[serde(default)]
    pub text_intelligence: TextIntelligenceConfig,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            throttle_secs: default_throttle_secs(),
            interval_secs: default_interval_secs(),
            full_interval_secs: default_full_interval_secs(),
            min_confidence: default_min_confidence(),
            max_suggestions: default_max_suggestions(),
            server_coexistence_lookback_secs: default_server_coexistence_lookback_secs(),
            llm_work_type_enabled: true,
            tiered_memory: TieredMemoryConfig::default(),
            embedding: EmbeddingConfig::default(),
            gui_intelligence: GuiIntelligenceConfig::default(),
            text_intelligence: TextIntelligenceConfig::default(),
        }
    }
}

/// Clustering algorithm selection for regime detection.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ClusteringAlgorithm {
    /// HDBSCAN: density-based clustering with automatic k and noise detection.
    #[default]
    Hdbscan,
    /// K-means: centroid-based clustering (legacy fallback).
    Kmeans,
    /// Gaussian Mixture Model: probabilistic soft clustering with BIC-based K selection.
    Gmm,
}

/// Configuration for the auto-tuning subsystem (EMA stats + drift detection).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoTuningConfig {
    /// Whether auto-tuning is enabled.
    #[serde(default = "default_auto_tuning_enabled")]
    pub enabled: bool,

    /// Exponential moving average smoothing factor (0 < alpha < 1).
    #[serde(default = "default_ema_alpha")]
    pub ema_alpha: f32,

    /// Drift detection threshold in sigma units.
    #[serde(default = "default_drift_threshold")]
    pub drift_threshold: f32,

    /// Adjusted Rand Index threshold below which re-clustering is triggered.
    #[serde(default = "default_reclustering_ari_threshold")]
    pub reclustering_ari_threshold: f32,
}

impl Default for AutoTuningConfig {
    fn default() -> Self {
        Self {
            enabled: default_auto_tuning_enabled(),
            ema_alpha: default_ema_alpha(),
            drift_threshold: default_drift_threshold(),
            reclustering_ari_threshold: default_reclustering_ari_threshold(),
        }
    }
}

fn default_auto_tuning_enabled() -> bool {
    true
}
fn default_ema_alpha() -> f32 {
    0.05
}
fn default_drift_threshold() -> f32 {
    2.0
}
fn default_reclustering_ari_threshold() -> f32 {
    0.7
}

/// Configuration for the Adaptive Tiered Memory subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredMemoryConfig {
    /// Master switch for the entire tiered memory pipeline.
    #[serde(default)]
    pub enabled: bool,

    /// Role-based preset profile used as the base parameter set.
    #[serde(default)]
    pub preset: PresetProfile,

    /// Calibration log retention in days.
    #[serde(default = "default_calibration_retention_days")]
    pub calibration_retention_days: u32,

    /// Maximum rows kept in the calibration_log table.
    #[serde(default = "default_calibration_max_rows")]
    pub calibration_max_rows: u64,

    /// In-memory buffer capacity before flushing to SQLite.
    #[serde(default = "default_buffer_capacity")]
    pub buffer_capacity: usize,

    /// Buffer flush interval in seconds.
    #[serde(default = "default_buffer_flush_interval_secs")]
    pub buffer_flush_interval_secs: u64,

    /// Maximum segment duration in seconds before force-close.
    #[serde(default = "default_max_segment_secs")]
    pub max_segment_secs: u64,

    /// Minimum segment duration in seconds before close is allowed.
    #[serde(default = "default_min_segment_secs")]
    pub min_segment_secs: u64,

    /// Clustering algorithm for regime detection.
    #[serde(default)]
    pub clustering_algorithm: ClusteringAlgorithm,

    /// Hours between automatic regime re-detection (0 = detection on every tick when data ready).
    #[serde(default = "default_regime_detection_interval_hours")]
    pub regime_detection_interval_hours: i64,

    /// Auto-tuning configuration (EMA stats + drift detection).
    #[serde(default)]
    pub auto_tuning: AutoTuningConfig,
}

impl Default for TieredMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: PresetProfile::default(),
            calibration_retention_days: default_calibration_retention_days(),
            calibration_max_rows: default_calibration_max_rows(),
            buffer_capacity: default_buffer_capacity(),
            buffer_flush_interval_secs: default_buffer_flush_interval_secs(),
            max_segment_secs: default_max_segment_secs(),
            min_segment_secs: default_min_segment_secs(),
            clustering_algorithm: ClusteringAlgorithm::default(),
            regime_detection_interval_hours: default_regime_detection_interval_hours(),
            auto_tuning: AutoTuningConfig::default(),
        }
    }
}

fn default_throttle_secs() -> u64 {
    120
}
fn default_interval_secs() -> u64 {
    300
}
fn default_full_interval_secs() -> u64 {
    1800
}
fn default_min_confidence() -> f64 {
    0.6
}
fn default_max_suggestions() -> usize {
    3
}
fn default_server_coexistence_lookback_secs() -> u64 {
    300
}

fn default_calibration_retention_days() -> u32 {
    14
}
fn default_calibration_max_rows() -> u64 {
    500_000
}
fn default_buffer_capacity() -> usize {
    100
}
fn default_buffer_flush_interval_secs() -> u64 {
    30
}
fn default_max_segment_secs() -> u64 {
    600
}
fn default_min_segment_secs() -> u64 {
    120
}
fn default_regime_detection_interval_hours() -> i64 {
    2
}

// ---------------------------------------------------------------------------
// Embedding configuration
// ---------------------------------------------------------------------------

/// Embedding provider type for vector generation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EmbeddingProviderType {
    #[default]
    Local,
    Remote,
}

/// Configuration for the embedding and vector RAG subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Master switch for the embedding pipeline.
    #[serde(default)]
    pub enabled: bool,

    /// Embedding provider type (local fastembed-rs or remote API).
    #[serde(default = "default_embedding_provider")]
    pub provider: EmbeddingProviderType,

    /// Local embedding model identifier (fastembed-rs compatible).
    ///
    /// Default: `"all-MiniLM-L6-v2-Q"` — quantized INT8 variant (~3x faster
    /// CPU inference, less than 1% accuracy loss vs FP32).
    /// Set to `"AllMiniLML6V2"` or `"all-MiniLM-L6-v2"` to use the
    /// full-precision model instead.
    #[serde(default = "default_local_model")]
    pub local_model: String,

    /// Remote embedding API endpoint (used when provider = Remote).
    #[serde(default)]
    pub remote_endpoint: Option<String>,

    /// Maximum number of search results returned by vector queries.
    #[serde(default = "default_max_search_results")]
    pub max_search_results: usize,

    /// Time decay half-life in hours for vector similarity weighting.
    #[serde(default = "default_time_decay_hours")]
    pub time_decay_hours: f32,

    /// Number of days to retain embedding vectors before cleanup.
    #[serde(default = "default_embedding_retention_days")]
    pub retention_days: u32,

    /// Enable LLM-based segment summarization before embedding.
    #[serde(default)]
    pub llm_summary_enabled: bool,

    /// Minimum segment duration in seconds before generating an LLM summary.
    #[serde(default = "default_min_segment_for_summary")]
    pub min_segment_for_summary_secs: u64,

    /// Day of week to generate the weekly digest.
    #[serde(default = "default_digest_day")]
    pub digest_day: Weekday,

    /// Enable INT8 scalar quantization for 4x storage reduction.
    /// When true, new vectors are stored in both f32 and INT8 formats,
    /// and search uses INT8 cosine similarity.
    #[serde(default)]
    pub quantization_enabled: bool,

    /// Whether to retain the original float32 vector when quantization is enabled.
    /// Default `true` (keep f32 alongside INT8 for rollback safety).
    /// When `false` AND `quantization_enabled` is `true`, new vectors are stored
    /// as INT8-only — the f32 BLOB column is set to NULL, saving ~4x storage.
    /// The column itself remains in the schema until a future migration drops it.
    #[serde(default = "default_quantization_float32_retention")]
    pub quantization_float32_retention: bool,

    /// Index strategy for vector search.
    /// "auto" (default): select based on collection size.
    /// "brute_force": always use brute-force INT8 scan.
    /// "ivf": always use IVF partitioning.
    /// "ivf_binary": always use IVF + 2-bit binary filter + INT8 re-rank.
    #[serde(default = "default_index_strategy")]
    pub index_strategy: String,

    /// Number of IVF partitions to probe at query time.
    /// Default 0 = auto-select (N / 10 where N = number of clusters).
    #[serde(default)]
    pub ivf_nprobe: usize,

    /// Oversample factor for 2-bit binary filter stage.
    /// Candidates = limit * oversample_factor, then re-ranked with INT8.
    /// Default 10.
    #[serde(default = "default_oversample_factor")]
    pub binary_oversample_factor: usize,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_embedding_provider(),
            local_model: default_local_model(),
            remote_endpoint: None,
            max_search_results: default_max_search_results(),
            time_decay_hours: default_time_decay_hours(),
            retention_days: default_embedding_retention_days(),
            llm_summary_enabled: false,
            min_segment_for_summary_secs: default_min_segment_for_summary(),
            digest_day: default_digest_day(),
            quantization_enabled: false,
            quantization_float32_retention: default_quantization_float32_retention(),
            index_strategy: default_index_strategy(),
            ivf_nprobe: 0,
            binary_oversample_factor: default_oversample_factor(),
        }
    }
}

fn default_quantization_float32_retention() -> bool {
    true
}

fn default_index_strategy() -> String {
    "auto".to_string()
}

fn default_oversample_factor() -> usize {
    10
}

fn default_embedding_provider() -> EmbeddingProviderType {
    EmbeddingProviderType::Local
}
fn default_local_model() -> String {
    "all-MiniLM-L6-v2-Q".to_string()
}
fn default_max_search_results() -> usize {
    5
}
fn default_time_decay_hours() -> f32 {
    168.0 // 1 week
}
fn default_embedding_retention_days() -> u32 {
    90
}
fn default_min_segment_for_summary() -> u64 {
    300 // 5 minutes
}
fn default_digest_day() -> Weekday {
    Weekday::Sun
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// GUI Intelligence configuration
// ---------------------------------------------------------------------------

/// Configuration for the GUI Activity Intelligence subsystem (Phase 2).
///
/// **Privacy**: The GUI pipeline MUST be gated on `activity_pattern_learning`
/// consent (GDPR Tier 4) in addition to this config flag. The scheduler
/// must check both `gui_intelligence.enabled` AND consent before constructing
/// `GuiPipelineState`. See `agent_runtime.rs` consent check pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiIntelligenceConfig {
    /// Master switch for GUI intelligence pipeline.
    /// Requires `activity_pattern_learning` consent to actually activate.
    #[serde(default = "default_gui_enabled")]
    pub enabled: bool,

    /// Aggregation time window in seconds. Events within a window are
    /// grouped into a single `GuiActivitySummary`.
    #[serde(default = "default_aggregation_window_secs")]
    pub aggregation_window_secs: u64,

    /// Maximum number of events buffered per segment before force-flush.
    #[serde(default = "default_max_events_per_segment")]
    pub max_events_per_segment: usize,

    /// Pixel distance threshold for proximity fallback in click-to-element
    /// matching. When no direct hit is found, the nearest region within
    /// this distance is used.
    #[serde(default = "default_proximity_threshold_px")]
    pub proximity_threshold_px: u32,

    /// Path to the ONNX model file for ML-based GUI element classification.
    /// Empty string (default) auto-detects from `{data_dir}/models/gui-classifier.onnx`.
    #[serde(default)]
    pub ml_model_path: String,
}

impl Default for GuiIntelligenceConfig {
    fn default() -> Self {
        Self {
            enabled: default_gui_enabled(),
            aggregation_window_secs: default_aggregation_window_secs(),
            max_events_per_segment: default_max_events_per_segment(),
            proximity_threshold_px: default_proximity_threshold_px(),
            ml_model_path: String::new(),
        }
    }
}

fn default_gui_enabled() -> bool {
    false
}
fn default_aggregation_window_secs() -> u64 {
    300
}
fn default_max_events_per_segment() -> usize {
    500
}
fn default_proximity_threshold_px() -> u32 {
    40
}

// ---------------------------------------------------------------------------
// Text Intelligence configuration
// ---------------------------------------------------------------------------

/// Configuration for the Text-Heavy App Intelligence subsystem (Phase 1).
///
/// **Privacy**: input_pattern_detail requires `activity_pattern_learning`
/// consent (GDPR Tier 4). accessibility_extraction (Phase 2) requires the
/// same consent tier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextIntelligenceConfig {
    /// Master switch for text-heavy app intelligence.
    /// When false, the system uses existing coarse classification only.
    #[serde(default)]
    pub enabled: bool,

    /// Enable key-category counters (Enter, Tab, Arrow, Backspace, Special).
    /// When false, only aggregate keystroke counts are tracked.
    #[serde(default = "default_input_pattern_detail")]
    pub input_pattern_detail: bool,

    /// Enable OS accessibility API extraction (Phase 2).
    /// Requires Accessibility permission on macOS.
    /// Requires `activity_pattern_learning` consent.
    #[serde(default)]
    pub accessibility_extraction: bool,

    /// PII filter level for accessibility-extracted text (Phase 2).
    #[serde(default = "default_pii_extraction_level")]
    pub pii_extraction_level: crate::config::enums::PiiFilterLevel,
}

impl Default for TextIntelligenceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            input_pattern_detail: default_input_pattern_detail(),
            accessibility_extraction: false,
            pii_extraction_level: default_pii_extraction_level(),
        }
    }
}

fn default_input_pattern_detail() -> bool {
    true
}

fn default_pii_extraction_level() -> crate::config::enums::PiiFilterLevel {
    crate::config::enums::PiiFilterLevel::Standard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analysis_config_without_text_intelligence_deserializes() {
        let json = r#"{"enabled": true}"#;
        let config: AnalysisConfig = serde_json::from_str(json).unwrap();
        assert!(!config.text_intelligence.enabled);
        assert!(config.text_intelligence.input_pattern_detail);
    }

    #[test]
    fn text_intelligence_config_defaults() {
        let config = TextIntelligenceConfig::default();
        assert!(!config.enabled);
        assert!(config.input_pattern_detail);
        assert!(!config.accessibility_extraction);
    }
}
