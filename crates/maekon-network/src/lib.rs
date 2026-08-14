// Cast safety: network metrics and buffer sizes — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A (B2): `significant_drop_tightening` is accepted crate-wide.
// Rationale: 18 flagged sites across 12 files — TokenManager,
// OAuth, integration, sync, circuit_breaker. All either (a) tokio::sync
// RwLock/Mutex held across token-exchange/network-roundtrip (intentional
// atomicity), or (b) held across in-memory state transitions that clippy's
// "tighten via single-usage" heuristic cannot rewrite (produces invalid
// Rust — confirmed on similar sites in PR #468). The nursery lint's false-
// positive rate here outweighs its diagnostic value.
// Rationale is embedded here so public source remains self-contained.
#![allow(clippy::significant_drop_tightening)]
// P2 nursery-hardening (PR-B): derive Eq alongside PartialEq when possible.
// (Enforced workspace-wide via `[workspace.lints.clippy]`, #7719.)

//! # maekon-network
//! ## Feature Flags
//! ```rust,ignore
//! use maekon_network::http_client::HttpApiClient;
//! use maekon_network::sse_client::SseStreamClient;
//! #[cfg(feature = "grpc")]
//! use maekon_network::grpc::{GrpcAuthClient, GrpcConfig};
//! ```

/// Anthropic API version header value — shared across analysis_client,
/// ai_llm_client, and ai_ocr_client.
pub const ANTHROPIC_API_VERSION: &str = "2023-06-01";

/// Default model name per AI provider type.
///
/// Used as a last-resort fallback when neither the user config nor the
/// provider-spec registry supply a model name.
pub fn default_model_for_provider(provider: &maekon_core::config::AiProviderType) -> &'static str {
    use maekon_core::config::AiProviderType;
    match provider {
        AiProviderType::Anthropic => "claude-sonnet-5",
        AiProviderType::OpenAi => "gpt-5.4",
        AiProviderType::Google => "gemini-2.5-flash",
        AiProviderType::Ollama => "qwen3:8b",
        AiProviderType::Bedrock => "anthropic.claude-3-5-sonnet-20241022-v2:0",
        AiProviderType::Copilot => "gpt-5.4",
        AiProviderType::Generic => "gpt-5-mini",
    }
}

pub mod error;
pub use error::NetworkError;

pub mod ai_llm_client;
// Re-export from maekon-core so callers using `maekon_network::LlmCallHealth`
// continue to compile without change.  The canonical definition lives in
// `maekon_core::ports::llm_provider::LlmCallHealth` (adapter crates must not
// depend on each other — this re-export preserves backward compatibility for
// src-tauri which imports via the network crate path).
pub use maekon_core::ports::llm_provider::LlmCallHealth;
pub mod ai_ocr_client;
pub mod analysis_client;
pub mod assignment_email_draft;
pub mod auth;
pub mod batch_uploader;
// ADR-034: the outbound-HTTP substrate (circuit_breaker / outbound /
// resilience) lives in `maekon-http-core` so a future integration crate can
// share it without an adapter→adapter dependency. P1 kept transitional
// re-exports here; P2 repointed every caller to `maekon_http_core::…` and
// removed them — reach the substrate directly, not through this crate.
pub mod codex_app_server;
pub mod codex_app_server_session;
pub mod compression;
pub mod connectivity;
pub mod context_home;
pub mod feature_perf_uploader;
pub mod http_api_session;
pub mod http_client;
pub mod local_llm_session;
pub(crate) mod mutex_ext;
pub mod oauth;
pub mod ollama_discovery;
pub mod provider_model_catalog_client;
pub mod remote_embedding_client;
pub mod sse_client;

pub mod sync;

#[cfg(feature = "grpc")]
pub mod grpc;
#[cfg(feature = "grpc")]
pub mod proto;
