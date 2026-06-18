// config/sections/ — per-domain config section collection (ADR-003 directory module)
//
// Split criteria:
//   ai_validation — OCR/scene validation config (OcrValidationConfig, SceneActionOverrideConfig,
//                   SceneIntelligenceConfig, ExternalApiEndpoint)
//   ai            — AI provider orchestrator (AiProviderConfig)
//   monitoring    — system monitoring/capture/schedule/file-access config
//   network       — server/gRPC/TLS/Web connection config
//   privacy       — privacy/sandbox/automation config
//   storage       — storage/integrity/notification/update/telemetry config

pub mod autostart;
pub use autostart::{should_prompt, AutostartConfig, AutostartPromptState};

pub mod external_grpc;
pub use external_grpc::{AuthMode, ExternalGrpcConfig, ExternalGrpcConfigError, JwtAlgorithm};

mod ai;
mod ai_session;
mod ai_validation;
mod analysis;
mod audio;
mod coaching;
mod focus_auto;
mod indicator;
mod integration;
mod monitoring;
mod network;
mod privacy;
mod storage;
mod suggestion;
mod sync;
mod tracking_schedule;
pub use tracking_schedule::*;

pub use ai::*;
pub use ai_session::*;
pub use ai_validation::*;
pub use analysis::*;
pub use audio::*;
pub use coaching::*;
pub use focus_auto::*;
pub use indicator::*;
pub use integration::*;
pub use monitoring::*;
pub use network::*;
pub use privacy::*;
pub use storage::*;
pub use suggestion::*;
pub use sync::*;

// ── pub(super) re-exports — used directly by AppConfig::default_config() in config/mod.rs ──
// Re-export functions declared pub(crate) in sub-files up to the config/ level

pub(super) use monitoring::default_capture_enabled;
pub(super) use monitoring::default_capture_throttle_ms;
pub(super) use monitoring::default_heartbeat_interval_ms;
pub(super) use monitoring::default_idle_threshold_secs;
pub(super) use monitoring::default_poll_interval_ms;
pub(super) use monitoring::default_process_interval_secs;
pub(super) use monitoring::default_sync_interval_ms;
pub(super) use monitoring::default_thumbnail_height;
pub(super) use monitoring::default_thumbnail_width;

pub(super) use network::default_request_timeout_ms;
pub(super) use network::default_sse_max_retry_secs;

pub(super) use storage::default_max_storage_mb;
pub(super) use storage::default_retention_days;
