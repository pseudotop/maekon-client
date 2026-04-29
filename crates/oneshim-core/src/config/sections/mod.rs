// config/sections/ — 도메인별 설정 섹션 모음 (ADR-003 디렉터리 모듈)
//
// 분할 기준:
//   ai_validation — OCR/장면 검증 설정 (OcrValidationConfig, SceneActionOverrideConfig,
//                   SceneIntelligenceConfig, ExternalApiEndpoint)
//   ai            — AI 프로바이더 오케스트레이터 (AiProviderConfig)
//   monitoring    — 시스템 감시/캡처/스케줄/파일 접근 설정
//   network       — 서버/gRPC/TLS/Web 연결 설정
//   privacy       — 개인정보/샌드박스/자동화 설정
//   storage       — 스토리지/무결성/알림/업데이트/텔레메트리 설정

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

// ── pub(super) 재노출 — config/mod.rs 의 AppConfig::default_config() 에서 직접 사용 ──
// sub-file 에서 pub(crate) 로 선언된 함수들을 config/ 레벨로 re-export

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
