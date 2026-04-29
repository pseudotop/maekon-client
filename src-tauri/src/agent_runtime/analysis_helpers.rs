use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use oneshim_analysis::{FallbackAnalysisProvider, NoOpAnalysisProvider};
use oneshim_core::config::AiProviderConfig;
use oneshim_core::ports::analysis_provider::AnalysisProvider;
use oneshim_network::analysis_client::AnalysisClient;

/// Build an AnalysisProvider with automatic fallback chaining.
///
/// Returns `None` when no primary `llm_api` is configured.
/// The returned `Arc<AtomicBool>` tracks primary provider health and can be
/// stored in AppState for IPC health queries.
pub fn build_analysis_provider(
    config: &AiProviderConfig,
) -> Option<(Arc<dyn AnalysisProvider>, Arc<AtomicBool>)> {
    build_analysis_provider_with_flag(config, None)
}

/// Like [`build_analysis_provider`] but accepts a pre-created health flag.
///
/// When `external_flag` is `Some`, that flag is wired into the
/// [`FallbackAnalysisProvider`] so the caller can share the same `Arc<AtomicBool>`
/// with `AppState` for IPC health queries.
pub fn build_analysis_provider_with_flag(
    config: &AiProviderConfig,
    external_flag: Option<Arc<AtomicBool>>,
) -> Option<(Arc<dyn AnalysisProvider>, Arc<AtomicBool>)> {
    let llm_api = config.llm_api.as_ref()?;
    // D7: share one registry between primary + fallback so both converge on
    // the same breaker if they target the same endpoint. iter-011 will
    // consolidate this to a workspace-wide registry.
    let breaker_registry = oneshim_network::CircuitBreakerRegistry::new();
    let primary: Arc<dyn AnalysisProvider> =
        Arc::new(AnalysisClient::new(llm_api, breaker_registry.clone()));

    let fallback: Arc<dyn AnalysisProvider> = match config.llm_api_fallback.as_ref() {
        Some(api) => Arc::new(AnalysisClient::new(api, breaker_registry.clone())),
        None => Arc::new(NoOpAnalysisProvider),
    };

    let health_flag = external_flag.unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
    let provider = Arc::new(FallbackAnalysisProvider::new_with_flag(
        primary,
        fallback,
        health_flag.clone(),
    ));

    Some((provider as Arc<dyn AnalysisProvider>, health_flag))
}
