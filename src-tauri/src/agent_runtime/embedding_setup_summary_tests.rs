#![cfg(test)]

use super::*;
use maekon_core::config::AiAccessMode;
use maekon_core::ports::vector_store::VectorStore;

fn build_summary_components(config: &AppConfig) -> EmbeddingComponents {
    let storage = Arc::new(
        maekon_storage::sqlite::SqliteStorage::open_in_memory(30)
            .expect("summary readiness test storage"),
    );
    let vector_store: Arc<dyn VectorStore> = Arc::new(
        maekon_storage::sqlite::vector_store_impl::SqliteVectorStore::new(storage.connection_arc()),
    );
    build_embedding_components(
        config,
        Some(vector_store),
        None,
        None,
        None,
        crate::breaker_registry::CircuitBreakerRegistry::new(),
    )
}

#[test]
fn summary_readiness_requires_both_embedding_and_summary_toggles() {
    let mut embedding_disabled = AppConfig::default_config();
    embedding_disabled.analysis.embedding.enabled = false;
    embedding_disabled.analysis.embedding.llm_summary_enabled = true;
    let components = build_summary_components(&embedding_disabled);
    assert!(components.llm_summarizer.is_none());
    assert_eq!(
        components.llm_summary_unavailable_reason,
        Some(AiSummaryFailureReason::PipelineDisabled)
    );

    let mut summary_disabled = AppConfig::default_config();
    summary_disabled.analysis.embedding.enabled = true;
    summary_disabled.analysis.embedding.llm_summary_enabled = false;
    let components = build_summary_components(&summary_disabled);
    assert!(components.llm_summarizer.is_none());
    assert_eq!(
        components.llm_summary_unavailable_reason,
        Some(AiSummaryFailureReason::PipelineDisabled)
    );
}

#[test]
fn local_summary_fallback_requires_absent_api_and_local_mode() {
    let mut local = AppConfig::default_config();
    local.analysis.embedding.enabled = true;
    local.analysis.embedding.llm_summary_enabled = true;
    local.ai_provider.llm_api = None;
    local.ai_provider.access_mode = AiAccessMode::LocalModel;
    let components = build_summary_components(&local);
    assert!(components.llm_summarizer.is_some());
    assert_eq!(components.llm_summary_unavailable_reason, None);
    assert_eq!(
        components.llm_summary_provider_class,
        Some(AiSummaryProviderClass::Loopback)
    );

    let mut api_mode_without_api = local;
    api_mode_without_api.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    let components = build_summary_components(&api_mode_without_api);
    assert!(components.llm_summarizer.is_none());
    assert_eq!(
        components.llm_summary_unavailable_reason,
        Some(AiSummaryFailureReason::ProviderUnavailable)
    );
}

#[test]
fn cli_summary_construction_runs_without_http_and_has_no_implicit_fallback() {
    use crate::subprocess_provider::{DetectedSubprocessCli, SubprocessAnalysisProvider};
    use maekon_core::ports::analysis_provider::AnalysisProvider;
    let mut config = AppConfig::default_config();
    config.analysis.embedding.enabled = true;
    config.analysis.embedding.llm_summary_enabled = true;
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    for endpoint_present in [false, true] {
        config.ai_provider.llm_api =
            endpoint_present.then(|| maekon_core::config::ExternalApiEndpoint {
                endpoint: "http://127.0.0.1:11434/v1/chat/completions".into(),
                api_key: String::new(),
                model: Some("legacy".into()),
                timeout_secs: 30,
                provider_type: maekon_core::config::AiProviderType::Generic,
                surface_id: None,
                credential: None,
            });
        for cli_available in [false, true] {
            let storage =
                Arc::new(maekon_storage::sqlite::SqliteStorage::open_in_memory(30).unwrap());
            let store: Arc<dyn VectorStore> = Arc::new(
                maekon_storage::sqlite::vector_store_impl::SqliteVectorStore::new(
                    storage.connection_arc(),
                ),
            );
            let called = std::cell::Cell::new(false);
            let components = build_embedding_components_with_summary_resolver(
                &config,
                Some(store),
                None,
                None,
                crate::breaker_registry::CircuitBreakerRegistry::new(),
                || {
                    called.set(true);
                    cli_available.then(|| {
                        Arc::new(SubprocessAnalysisProvider::new(
                            DetectedSubprocessCli {
                                surface_id: "provider_surface.openai.subprocess_cli".into(),
                                executable_path: "unused-summary-construction-fixture".into(),
                            },
                            &config.ai_provider,
                        )) as Arc<dyn AnalysisProvider>
                    })
                },
            );
            assert!(
                called.get(),
                "CLI resolution must run even when llm_api is None"
            );
            assert_eq!(
                components.llm_summary_provider_class,
                Some(AiSummaryProviderClass::Subprocess)
            );
            if cli_available {
                let summarizer = components
                    .llm_summarizer
                    .expect("CLI segment summarizer constructed");
                assert_eq!(
                    summarizer.provider_class(),
                    AiSummaryProviderClass::Subprocess
                );
                assert_eq!(
                    summarizer.analysis_provider().provider_name(),
                    "subprocess-codex-analysis"
                );
                assert_eq!(components.llm_summary_unavailable_reason, None);
            } else {
                assert!(
                    components.llm_summarizer.is_none(),
                    "no HTTP/Ollama summary fallback"
                );
                assert_eq!(
                    components.llm_summary_unavailable_reason,
                    Some(AiSummaryFailureReason::ProviderUnavailable)
                );
            }
        }
    }
}
