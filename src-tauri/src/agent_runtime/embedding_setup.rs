use std::sync::Arc;
use tracing::{info, warn};

use oneshim_core::config::AppConfig;

/// Components produced by the embedding pipeline setup.
pub(super) struct EmbeddingComponents {
    pub embedding_pipeline: Option<Arc<oneshim_analysis::EmbeddingPipeline>>,
    pub llm_summarizer: Option<Arc<oneshim_analysis::LlmSegmentSummarizer>>,
    /// EmbeddingProvider to wire into scheduler.
    pub embedding_provider:
        Option<Arc<dyn oneshim_core::ports::embedding_provider::EmbeddingProvider>>,
    /// VectorStore to wire into scheduler.
    pub vector_store: Option<Arc<dyn oneshim_core::ports::vector_store::VectorStore>>,
}

/// Build the embedding pipeline + LLM segment summarizer from config.
///
/// Returns `None` components when embedding is disabled or prerequisites are missing.
pub(super) fn build_embedding_components(
    config: &AppConfig,
    vector_store_opt: Option<Arc<dyn oneshim_core::ports::vector_store::VectorStore>>,
) -> EmbeddingComponents {
    let mut embedding_pipeline_arc: Option<Arc<oneshim_analysis::EmbeddingPipeline>> = None;
    let mut llm_summarizer_arc: Option<Arc<oneshim_analysis::LlmSegmentSummarizer>> = None;
    let mut embedding_provider_out: Option<
        Arc<dyn oneshim_core::ports::embedding_provider::EmbeddingProvider>,
    > = None;
    let mut vector_store_out: Option<Arc<dyn oneshim_core::ports::vector_store::VectorStore>> =
        None;

    if config.analysis.embedding.enabled {
        let embedding_config = &config.analysis.embedding;
        let pii_level = config.privacy.pii_filter_level;

        // Create EmbeddingProvider based on config
        let embedding_provider: Option<
            Arc<dyn oneshim_core::ports::embedding_provider::EmbeddingProvider>,
        > = match embedding_config.provider {
            #[cfg(feature = "embedding")]
            oneshim_core::config::EmbeddingProviderType::Local => {
                match oneshim_embedding::LocalEmbeddingProvider::new() {
                    Ok(provider) => {
                        info!("Local embedding provider initialized");
                        Some(Arc::new(provider))
                    }
                    Err(e) => {
                        warn!("Local embedding provider init failed: {e}");
                        None
                    }
                }
            }
            #[cfg(not(feature = "embedding"))]
            oneshim_core::config::EmbeddingProviderType::Local => {
                warn!("Local embedding requested but 'embedding' feature not enabled");
                None
            }
            oneshim_core::config::EmbeddingProviderType::Remote => {
                if let Some(ref endpoint) = embedding_config.remote_endpoint {
                    let api_key = config
                        .ai_provider
                        .llm_api
                        .as_ref()
                        .map(|api| api.api_key.clone())
                        .unwrap_or_default();
                    Some(Arc::new(
                        oneshim_network::remote_embedding_client::RemoteEmbeddingProvider::new(
                            endpoint.clone(),
                            api_key,
                            "text-embedding-3-small".to_string(),
                            384,
                            30,
                            // D7: per-adapter registry for now; a workspace-wide
                            // registry threaded from the composition root will
                            // come in iter-011 when all 5 adapters are wired.
                            oneshim_network::CircuitBreakerRegistry::new(),
                        ),
                    ))
                } else {
                    warn!("Remote embedding requested but no endpoint configured");
                    None
                }
            }
        };

        // Wrap successful provider with FallbackEmbeddingProvider → NoOp,
        // so transient failures at runtime degrade to zero vectors instead of
        // propagating errors through the pipeline.
        #[cfg(feature = "embedding")]
        let embedding_provider: Option<
            Arc<dyn oneshim_core::ports::embedding_provider::EmbeddingProvider>,
        > = embedding_provider.map(|p| {
            let noop = Arc::new(
                oneshim_core::ports::embedding_provider::NoOpEmbeddingProvider::new(p.dimensions()),
            );
            Arc::new(oneshim_embedding::FallbackEmbeddingProvider::new(p, noop))
                as Arc<dyn oneshim_core::ports::embedding_provider::EmbeddingProvider>
        });

        if let (Some(ref provider), Some(ref vector_store)) =
            (&embedding_provider, &vector_store_opt)
        {
            let pii_filter_embed: oneshim_analysis::PiiFilter = Box::new(move |text: &str| {
                oneshim_vision::privacy::sanitize_title_with_level(text, pii_level)
            });
            let skip_float32 = embedding_config.quantization_enabled
                && !embedding_config.quantization_float32_retention;
            let pipeline = Arc::new(oneshim_analysis::EmbeddingPipeline::with_float32_retention(
                provider.clone(),
                pii_filter_embed,
                vector_store.clone(),
                embedding_config.quantization_enabled,
                skip_float32,
            ));
            embedding_pipeline_arc = Some(pipeline);

            // Build LlmSegmentSummarizer if LLM summary is enabled
            if embedding_config.llm_summary_enabled {
                if let Some(ref _llm_api) = config.ai_provider.llm_api {
                    let analysis_provider: Arc<
                        dyn oneshim_core::ports::analysis_provider::AnalysisProvider,
                    > = crate::agent_runtime::analysis_helpers::build_analysis_provider(
                        &config.ai_provider,
                    )
                    .map(|(p, _)| p)
                    .unwrap_or_else(|| {
                        Arc::new(oneshim_analysis::NoOpAnalysisProvider)
                            as Arc<dyn oneshim_core::ports::analysis_provider::AnalysisProvider>
                    });
                    let pii_level_summ = config.privacy.pii_filter_level;
                    let pii_filter_summ: oneshim_analysis::PiiFilter =
                        Box::new(move |text: &str| {
                            oneshim_vision::privacy::sanitize_title_with_level(text, pii_level_summ)
                        });
                    let min_duration = embedding_config.min_segment_for_summary_secs;
                    llm_summarizer_arc =
                        Some(Arc::new(oneshim_analysis::LlmSegmentSummarizer::new(
                            analysis_provider,
                            pii_filter_summ,
                            true,
                            min_duration,
                        )));
                    info!("LLM segment summarizer enabled");
                } else {
                    warn!("LLM summary enabled but no LLM provider configured");
                }
            }

            // Stash for scheduler wiring
            vector_store_out = Some(vector_store.clone());
            embedding_provider_out = Some(provider.clone());

            info!(
                provider = provider.model_id(),
                "Layer 2 embedding pipeline wired"
            );
        }
    }

    // If both local and remote fail, use NoOp fallback so the pipeline stays
    // functional with degraded accuracy (zero vectors).
    if embedding_provider_out.is_none() {
        warn!("both local and remote embedding unavailable — using no-op fallback (vector features degraded)");
        embedding_provider_out = Some(Arc::new(
            oneshim_core::ports::embedding_provider::NoOpEmbeddingProvider::new(384),
        ));
    }

    EmbeddingComponents {
        embedding_pipeline: embedding_pipeline_arc,
        llm_summarizer: llm_summarizer_arc,
        embedding_provider: embedding_provider_out,
        vector_store: vector_store_out,
    }
}
