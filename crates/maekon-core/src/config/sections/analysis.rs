// OOS-TBD: ADR-013 file split (cycle 37+) — LOC: 542
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
    /// ADR-023 Phase-2: enable LLM belief revision (D1 relation extraction + D2
    /// contradiction → supersede). Gated additionally by a LOCAL-only egress
    /// check and the `memory_graph_enrichment` consent. Default false (opt-in).
    #[serde(default)]
    pub belief_revision_enabled: bool,
    /// Minimum LLM confidence to supersede an Active claim (D2). Deliberately
    /// HIGH — a wrong supersede destroys a durable user belief. Default 0.9.
    #[serde(default = "default_supersede_confidence_threshold")]
    pub supersede_confidence_threshold: f64,
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
            belief_revision_enabled: false,
            supersede_confidence_threshold: default_supersede_confidence_threshold(),
        }
    }
}

/// Floor for `analysis.interval_secs` (#6177). Mirrors the IPC validator
/// (`commands::analysis::validate_analysis_config`, `interval_secs >= 10`) so a
/// hand-edited / downgrade config cannot drive a sub-10s analysis loop.
pub(crate) const ANALYSIS_INTERVAL_SECS_FLOOR: u64 = 10;

/// Floor for `analysis.throttle_secs` (#6177). Mirrors the IPC validator
/// (`throttle_secs >= 1`).
pub(crate) const ANALYSIS_THROTTLE_SECS_FLOOR: u64 = 1;

impl AnalysisConfig {
    /// Validate that analysis interval values are within acceptable bounds (#6177).
    ///
    /// The analysis scheduler loop builds a `tokio::time::interval` from
    /// `interval_secs` (and gates a full pass on `full_interval_secs`); a zero
    /// period panics tokio's interval (`period > 0` assertion), so enforce the
    /// same floors the IPC validator does. The defaults (interval 300s / full
    /// 1800s / throttle 120s) satisfy these bounds.
    pub fn validate_bounds(&self) -> Result<(), String> {
        if self.interval_secs < ANALYSIS_INTERVAL_SECS_FLOOR {
            return Err(format!(
                "analysis.interval_secs must be >= {ANALYSIS_INTERVAL_SECS_FLOOR}"
            ));
        }
        if self.full_interval_secs < self.interval_secs {
            return Err(
                "analysis.full_interval_secs must be >= analysis.interval_secs".to_string(),
            );
        }
        if self.throttle_secs < ANALYSIS_THROTTLE_SECS_FLOOR {
            return Err(format!(
                "analysis.throttle_secs must be >= {ANALYSIS_THROTTLE_SECS_FLOOR}"
            ));
        }
        Ok(())
    }

    /// Raise any sub-floor analysis interval to its floor in place, returning the
    /// dotted-path identities of the fields that were clamped (#6169/#6177).
    ///
    /// Unlike [`Self::validate_bounds`] this never errors — it is used by the
    /// fail-open INITIAL-LOAD path so a hand-edited / downgrade config.json with
    /// sub-floor intervals is corrected to a safe value rather than aborting
    /// startup or reaching the scheduler unvalidated.
    pub(crate) fn clamp_bounds(&mut self) -> Vec<&'static str> {
        let mut clamped = Vec::new();
        if self.interval_secs < ANALYSIS_INTERVAL_SECS_FLOOR {
            self.interval_secs = ANALYSIS_INTERVAL_SECS_FLOOR;
            clamped.push("analysis.interval_secs");
        }
        // Re-establish the full >= incremental invariant after the interval clamp.
        if self.full_interval_secs < self.interval_secs {
            self.full_interval_secs = self.interval_secs;
            clamped.push("analysis.full_interval_secs");
        }
        if self.throttle_secs < ANALYSIS_THROTTLE_SECS_FLOOR {
            self.throttle_secs = ANALYSIS_THROTTLE_SECS_FLOOR;
            clamped.push("analysis.throttle_secs");
        }
        clamped
    }
}

/// Default D2 supersede confidence threshold — conservatively HIGH (0.9): a wrong
/// supersede destroys a durable user belief, so only very confident contradictions
/// auto-supersede.
fn default_supersede_confidence_threshold() -> f64 {
    0.9
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

impl std::fmt::Display for ClusteringAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hdbscan => f.write_str("HDBSCAN"),
            Self::Kmeans => f.write_str("KMEANS"),
            Self::Gmm => f.write_str("GMM"),
        }
    }
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

impl std::fmt::Display for EmbeddingProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local => f.write_str("LOCAL"),
            Self::Remote => f.write_str("REMOTE"),
        }
    }
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

    /// Override model name for the remote embedding provider.
    ///
    /// When `None`, the model is derived from the endpoint:
    /// - loopback endpoint (localhost / 127.x / ::1): `"embeddinggemma"` (Ollama default)
    /// - external endpoint: `"text-embedding-3-small"` (OpenAI default)
    ///
    /// Set explicitly when using a non-default model with the configured endpoint.
    #[serde(default)]
    pub remote_model: Option<String>,

    /// Override output dimension for the remote embedding provider.
    ///
    /// When `None`, the dimension is derived from the endpoint:
    /// - loopback endpoint: 768 (embeddinggemma)
    /// - external endpoint: 384 (text-embedding-3-small)
    ///
    /// Matryoshka models (e.g. nomic-embed-text) support truncation: setting a
    /// lower value here enables MRL truncation + L2 re-normalisation inside
    /// `RemoteEmbeddingProvider`. Setting a *higher* value than the model output
    /// is not supported — the provider will warn and use the actual response length.
    #[serde(default)]
    pub remote_dimensions: Option<usize>,

    /// Keychain / secret-backend credential binding for the remote embedding endpoint.
    ///
    /// When `Some`, an external (non-loopback) remote endpoint resolves its API
    /// key from the declared secret store backend instead of the inline
    /// `llm_api.api_key` field.  On loopback endpoints this field is ignored
    /// (loopback always uses `NoAuth` regardless of this binding — secrets must
    /// not be sent to a local unauthenticated socket).
    ///
    /// When `None` (default), the existing coupling to `llm_api.api_key` is
    /// preserved for backward compatibility.
    #[serde(default)]
    pub remote_credential: Option<crate::config::CredentialBinding>,
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
            remote_model: None,
            remote_dimensions: None,
            remote_credential: None,
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

    // ── #6177 analysis interval bounds ────────────────────────────────────

    #[test]
    fn analysis_validate_bounds_default_passes() {
        // Contract: the default analysis config (interval 300s / full 1800s /
        // throttle 120s) must satisfy its own interval floors.
        AnalysisConfig::default()
            .validate_bounds()
            .expect("default AnalysisConfig must satisfy validate_bounds");
    }

    #[test]
    fn analysis_validate_bounds_rejects_zero_interval() {
        // #6177: interval_secs == 0 would panic the analysis loop's
        // tokio::time::interval (period > 0 assertion).
        let mut cfg = AnalysisConfig::default();
        cfg.interval_secs = 0;
        let err = cfg.validate_bounds().unwrap_err();
        assert!(
            err.contains("interval_secs"),
            "zero interval error must name the field, got: {err}"
        );
    }

    #[test]
    fn analysis_validate_bounds_rejects_full_below_interval() {
        let mut cfg = AnalysisConfig::default();
        cfg.interval_secs = 60;
        cfg.full_interval_secs = 30;
        let err = cfg.validate_bounds().unwrap_err();
        assert!(err.contains("full_interval_secs"), "got: {err}");
    }

    #[test]
    fn analysis_clamp_bounds_raises_zero_interval_to_floor() {
        // #6177: clamp_bounds is the fail-open INITIAL-LOAD counterpart — it
        // raises a sub-floor interval to the floor instead of erroring, and the
        // result must satisfy validate_bounds.
        let mut cfg = AnalysisConfig::default();
        cfg.interval_secs = 0;
        cfg.full_interval_secs = 0;
        cfg.throttle_secs = 0;
        let clamped = cfg.clamp_bounds();
        assert_eq!(cfg.interval_secs, ANALYSIS_INTERVAL_SECS_FLOOR);
        assert!(cfg.full_interval_secs >= cfg.interval_secs);
        assert_eq!(cfg.throttle_secs, ANALYSIS_THROTTLE_SECS_FLOOR);
        assert!(clamped.contains(&"analysis.interval_secs"));
        cfg.validate_bounds()
            .expect("clamped analysis config must satisfy validate_bounds");
    }

    #[test]
    fn analysis_clamp_bounds_is_noop_for_valid_config() {
        let mut cfg = AnalysisConfig::default();
        let clamped = cfg.clamp_bounds();
        assert!(
            clamped.is_empty(),
            "a default (in-bounds) config must not be clamped, got: {clamped:?}"
        );
    }

    #[test]
    fn clustering_algorithm_display_matches_serde_token() {
        assert_eq!(ClusteringAlgorithm::Hdbscan.to_string(), "HDBSCAN");
        assert_eq!(ClusteringAlgorithm::Kmeans.to_string(), "KMEANS");
        assert_eq!(ClusteringAlgorithm::Gmm.to_string(), "GMM");
    }

    #[test]
    fn embedding_provider_type_display_matches_serde_token() {
        assert_eq!(EmbeddingProviderType::Local.to_string(), "LOCAL");
        assert_eq!(EmbeddingProviderType::Remote.to_string(), "REMOTE");
    }

    #[test]
    fn embedding_config_remote_fields_default_to_none() {
        let config = EmbeddingConfig::default();
        assert!(config.remote_model.is_none());
        assert!(config.remote_dimensions.is_none());
    }

    #[test]
    fn embedding_config_remote_fields_deserialize_from_json() {
        let json = r#"{
            "enabled": true,
            "provider": "REMOTE",
            "remote_endpoint": "http://localhost:11434/v1/embeddings",
            "remote_model": "nomic-embed-text",
            "remote_dimensions": 512
        }"#;
        let config: EmbeddingConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.remote_model.as_deref(), Some("nomic-embed-text"));
        assert_eq!(config.remote_dimensions, Some(512));
    }

    #[test]
    fn embedding_config_remote_fields_absent_deserializes_to_none() {
        // Fields with #[serde(default)] must not fail when absent from JSON.
        let json = r#"{"enabled": true, "provider": "REMOTE"}"#;
        let config: EmbeddingConfig = serde_json::from_str(json).unwrap();
        assert!(config.remote_model.is_none());
        assert!(config.remote_dimensions.is_none());
    }

    // ── remote_credential field tests ────────────────────────────────────────

    /// Backward compatibility: remote_credential defaults to None when absent.
    #[test]
    fn remote_credential_defaults_to_none() {
        let config = EmbeddingConfig::default();
        assert!(config.remote_credential.is_none());
    }

    /// Field absent from JSON must deserialize to None (serde default).
    #[test]
    fn remote_credential_absent_in_json_is_none() {
        let json = r#"{"enabled": true, "provider": "REMOTE"}"#;
        let config: EmbeddingConfig = serde_json::from_str(json).unwrap();
        assert!(config.remote_credential.is_none());
    }

    /// Round-trip: Some(CredentialBinding) serialises and re-deserialises intact.
    #[test]
    fn remote_credential_some_roundtrips() {
        use crate::config::{CredentialAuthMode, CredentialBackendKind, CredentialBinding};
        let binding = CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(crate::config::SecretRef {
                namespace: "provider/openai/embedding".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: false,
        };
        let cfg = EmbeddingConfig {
            remote_credential: Some(binding.clone()),
            ..Default::default()
        };

        let serialised = serde_json::to_string(&cfg).expect("serialise");
        let restored: EmbeddingConfig = serde_json::from_str(&serialised).expect("deserialise");
        let restored_binding = restored.remote_credential.expect("binding round-tripped");
        assert_eq!(restored_binding.auth_mode, binding.auth_mode);
        assert_eq!(restored_binding.backend_kind, binding.backend_kind);
        assert_eq!(
            restored_binding
                .secret_ref
                .as_ref()
                .map(|r| r.namespace.as_str()),
            Some("provider/openai/embedding")
        );
    }
}
