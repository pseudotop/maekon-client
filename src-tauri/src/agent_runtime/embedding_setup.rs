use std::sync::Arc;
use tracing::{info, warn};

use crate::provider_adapters::ExternalOcrPrivacyGuard;
use maekon_core::config::AppConfig;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
#[cfg(feature = "analysis")]
use maekon_core::ports::secret_store::SecretStoreSet;

// ---------------------------------------------------------------------------
// Loopback Ollama defaults (D′ — #5755)
// ---------------------------------------------------------------------------

/// Default Ollama OpenAI-compatible embedding endpoint on the loopback interface.
///
/// Ollama exposes an OpenAI-compatible `/v1/embeddings` path at this address.
/// Used when `EmbeddingConfig::remote_endpoint` is `None` **and** the resolved
/// endpoint is loopback.
#[cfg(feature = "analysis")]
const OLLAMA_LOOPBACK_ENDPOINT: &str = "http://localhost:11434/v1/embeddings";

/// Default embedding model served by Ollama on the loopback interface.
#[cfg(feature = "analysis")]
const OLLAMA_LOOPBACK_DEFAULT_MODEL: &str = "embeddinggemma";

/// Output dimensionality of `embeddinggemma`.
#[cfg(feature = "analysis")]
const OLLAMA_LOOPBACK_DEFAULT_DIMS: usize = 768;

/// Default model used for external (non-loopback) remote endpoints.
#[cfg(feature = "analysis")]
const EXTERNAL_DEFAULT_MODEL: &str = "text-embedding-3-small";

/// Default output dimensionality for `text-embedding-3-small`.
#[cfg(feature = "analysis")]
const EXTERNAL_DEFAULT_DIMS: usize = 384;

const QUANTIZED_BACKFILL_BATCH_SIZE: usize = 512;

// ---------------------------------------------------------------------------
// Target resolution helper
// ---------------------------------------------------------------------------

/// Credential kind produced by `resolve_remote_embedding_target`.
#[cfg(feature = "analysis")]
pub(super) enum RemoteCredentialKind {
    /// Loopback endpoint — no auth required.
    NoAuth,
    /// External endpoint — use the configured LLM API key.
    ApiKey(String),
    /// External endpoint with a dedicated keychain/secret-backend binding.
    ///
    /// The actual bearer token is resolved at request time by the
    /// `RemoteEmbeddingProvider` via `CredentialSource::StoredSecret`.
    Stored(maekon_core::ports::credential_source::CredentialSource),
}

/// Resolved parameters for a `RemoteEmbeddingProvider` instance.
#[cfg(feature = "analysis")]
pub(super) struct RemoteEmbeddingTarget {
    pub endpoint: String,
    pub model: String,
    pub dims: usize,
    pub credential: RemoteCredentialKind,
}

/// Derive the remote embedding target from `EmbeddingConfig`.
///
/// Resolution rules:
/// 1. `endpoint` = `remote_endpoint` if present, otherwise `OLLAMA_LOOPBACK_ENDPOINT`.
/// 2. Loopback test on the resolved endpoint:
///    - loopback → `model` default `"embeddinggemma"`, `dims` default 768,
///      `NoAuth`. `remote_credential` is ignored for loopback (security
///      invariant: credentials must not be sent to an unauthenticated local
///      socket).
///    - external → `model` default `"text-embedding-3-small"`, `dims` default
///      384. Credential priority (highest first): (a)
///      `embedding.remote_credential` Some + `secret_stores` Some →
///      `Stored(CredentialSource::StoredSecret)`; (b) fallback → `ApiKey`
///      from `llm_api.api_key` (pre-D′ coupling).
/// 3. `remote_model` / `remote_dimensions` override their respective defaults when
///    `Some` (regardless of loopback/external).
///
/// This is a pure function — no I/O, no side effects — so it is unit-testable
/// without network access.
///
/// Requires the `analysis` feature because it calls `maekon_http_core::outbound::host_is_loopback`.
#[cfg(feature = "analysis")]
pub(super) fn resolve_remote_embedding_target(
    embedding_cfg: &maekon_core::config::EmbeddingConfig,
    llm_api_key: Option<&str>,
    secret_stores: Option<&maekon_core::ports::secret_store::SecretStoreSet>,
) -> RemoteEmbeddingTarget {
    let endpoint = embedding_cfg
        .remote_endpoint
        .clone()
        .unwrap_or_else(|| OLLAMA_LOOPBACK_ENDPOINT.to_string());

    let is_loopback = maekon_http_core::outbound::host_is_loopback(&endpoint);

    let (default_model, default_dims, credential) = if is_loopback {
        // Loopback invariant: remote_credential is intentionally ignored here.
        // Credentials must never be forwarded to an unauthenticated local socket.
        (
            OLLAMA_LOOPBACK_DEFAULT_MODEL.to_string(),
            OLLAMA_LOOPBACK_DEFAULT_DIMS,
            RemoteCredentialKind::NoAuth,
        )
    } else {
        // External endpoint: try dedicated remote_credential binding first, then
        // fall back to the pre-D′ llm_api inline key coupling.
        let credential =
            try_build_stored_credential(embedding_cfg.remote_credential.as_ref(), secret_stores)
                .unwrap_or_else(|| {
                    RemoteCredentialKind::ApiKey(llm_api_key.unwrap_or_default().to_string())
                });
        (
            EXTERNAL_DEFAULT_MODEL.to_string(),
            EXTERNAL_DEFAULT_DIMS,
            credential,
        )
    };

    let model = embedding_cfg.remote_model.clone().unwrap_or(default_model);
    let dims = embedding_cfg.remote_dimensions.unwrap_or(default_dims);

    RemoteEmbeddingTarget {
        endpoint,
        model,
        dims,
        credential,
    }
}

/// #6914: PII level applied to off-device (remote) embedding uploads, after passing the egress PII floor.
///
/// Remote embedding sends OCR-derived content labels / segment summaries to a third-party API, so —
/// just like external LLM / OCR / window-title egress — it must pass through the
/// `ExternalDataPolicy::effective_egress_pii_level` SSOT (AllowFiltered floors to at least Basic).
/// Using the raw `privacy.pii_filter_level` directly would leak verbatim under the `AllowFiltered + Off`
/// combination. Local on-device embedding masking is not egress, so this floor is not applied there
/// (avoiding over-masking).
///
/// Only called (in production) from the `feature = "analysis"`-gated
/// `egress_pii_level` binding above, but this pure function is also exercised
/// directly by unit tests below that are gated on `not(feature = "embedding")`
/// (independent of `analysis`) — kept unconditional with a matching allow
/// rather than hard-gated, so `cargo test --no-default-features` (a
/// hypothetical future cell) would not lose that coverage (#7743 ctd-W3 A2b
/// follow-up).
#[cfg_attr(not(feature = "analysis"), allow(dead_code))]
fn embedding_egress_pii_level(config: &AppConfig) -> PiiFilterLevel {
    config
        .ai_provider
        .external_data_policy
        .effective_egress_pii_level(config.privacy.pii_filter_level)
}

/// Attempt to construct a `Stored` credential from a `CredentialBinding` + `SecretStoreSet`.
///
/// Returns `None` when:
/// - `binding` is `None` (no dedicated embedding credential configured), or
/// - `secret_stores` is `None` (stores not wired in this build path), or
/// - the binding has no `secret_ref` and the backend is not `Env` (ambiguous), or
/// - `provider_api_key_secret_ref` returns an error (invalid segment characters).
///
/// On `None` the caller falls back to the inline `llm_api.api_key` path — zero
/// regression risk for existing deployments.
#[cfg(feature = "analysis")]
fn try_build_stored_credential(
    binding: Option<&maekon_core::config::CredentialBinding>,
    secret_stores: Option<&maekon_core::ports::secret_store::SecretStoreSet>,
) -> Option<RemoteCredentialKind> {
    use maekon_core::config::CredentialBackendKind;
    use maekon_core::ports::credential_source::CredentialSource;
    use maekon_core::ports::secret_store::{provider_api_key_secret_ref, SecretStoreSet};

    let binding = binding?;
    let stores: &SecretStoreSet = secret_stores?;
    let store = stores.for_binding(Some(binding))?;

    let (namespace, key_str) = if let Some(secret_ref) = binding.secret_ref.as_ref() {
        // Explicit secret_ref: use namespace + key directly.
        (secret_ref.namespace.clone(), secret_ref.key.clone())
    } else if binding.backend_kind == CredentialBackendKind::Env {
        // Env backend without secret_ref: derive from canonical profile "embedding".
        match provider_api_key_secret_ref("embedding", "embedding") {
            Ok((ns, k)) => (ns, k.to_string()),
            Err(e) => {
                warn!(
                    "embedding credential: could not derive env secret ref (falling back to inline key): {e}"
                );
                return None;
            }
        }
    } else {
        // No secret_ref and not an Env backend — cannot derive a namespace.
        // Fall back to inline key.
        return None;
    };

    Some(RemoteCredentialKind::Stored(
        CredentialSource::StoredSecret {
            namespace,
            key: key_str,
            secret_store: store,
        },
    ))
}

/// Loopback-FORCED variant for the Local-arm demotion path.
///
/// A user who selected `provider = Local` expressed an on-device intent; the
/// demotion (non-`embedding` build) must preserve that boundary.  Unlike
/// [`resolve_remote_embedding_target`], this NEVER reads
/// `remote_endpoint` — a stale external URL left in config must not turn a
/// Local selection into off-device egress.  `remote_model`/`remote_dimensions`
/// overrides still apply (harmless on loopback).
#[cfg(all(not(feature = "embedding"), feature = "analysis"))]
pub(super) fn loopback_embedding_target(
    embedding_cfg: &maekon_core::config::EmbeddingConfig,
) -> RemoteEmbeddingTarget {
    RemoteEmbeddingTarget {
        endpoint: OLLAMA_LOOPBACK_ENDPOINT.to_string(),
        model: embedding_cfg
            .remote_model
            .clone()
            .unwrap_or_else(|| OLLAMA_LOOPBACK_DEFAULT_MODEL.to_string()),
        dims: embedding_cfg
            .remote_dimensions
            .unwrap_or(OLLAMA_LOOPBACK_DEFAULT_DIMS),
        credential: RemoteCredentialKind::NoAuth,
    }
}

/// Components produced by the embedding pipeline setup.
pub(super) struct EmbeddingComponents {
    pub embedding_pipeline: Option<Arc<maekon_analysis::EmbeddingPipeline>>,
    pub llm_summarizer: Option<Arc<maekon_analysis::LlmSegmentSummarizer>>,
    /// EmbeddingProvider to wire into scheduler.
    pub embedding_provider:
        Option<Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>>,
    /// VectorStore to wire into scheduler.
    pub vector_store: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>>,
    /// #6266: the same provider's `ReloadableModel` facet (only the local
    /// embedding provider supports hot-reload), surfaced so `reload_embedding_model`
    /// IPC can reach it. `None` for remote/no-op providers.
    pub reloadable_model: Option<Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>>,
}

/// Build the embedding pipeline + LLM segment summarizer from config.
///
/// Returns `None` components when embedding is disabled or prerequisites are missing.
///
/// `secret_stores` is threaded from `AgentRuntimeBundle::provider_secret_stores`
/// so that the LLM summary path can resolve OS-keychain-backed keys.  The
/// remote embedding adapter sources its key from `embedding.remote_credential`
/// (keychain/file binding, #5734) or falls back to `llm_api` inline (see FLAG in
/// analysis_helpers); if a dedicated embedding endpoint with its own credential
/// binding is added in the future, `secret_stores` is already available here.
pub(super) fn build_embedding_components(
    config: &AppConfig,
    vector_store_opt: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>>,
    external_llm_privacy_guard: Option<ExternalOcrPrivacyGuard>,
    #[cfg(feature = "analysis")] secret_stores: Option<&SecretStoreSet>,
    // #6830: egress-audit sink threaded to the remote embedding provider so each
    // external upload records a ledger row (analysis-only, like `secret_stores`).
    #[cfg(feature = "analysis")] egress_ledger: Option<
        Arc<dyn maekon_core::ports::egress_ledger::EgressLedgerSink>,
    >,
    // D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    // registry threaded from the composition root. Both the remote embedding
    // adapter and the LLM-summary analysis provider built below converge on this
    // one Arc, so co-located endpoints share a breaker.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
) -> EmbeddingComponents {
    let mut embedding_pipeline_arc: Option<Arc<maekon_analysis::EmbeddingPipeline>> = None;
    let mut llm_summarizer_arc: Option<Arc<maekon_analysis::LlmSegmentSummarizer>> = None;
    let mut embedding_provider_out: Option<
        Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
    > = None;
    let mut vector_store_out: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>> = None;
    // #6266: only the `embedding`-feature Local provider supports hot-reload; the
    // demoted/remote/no-op arms leave this `None` (so `mut` is unused there).
    #[cfg_attr(not(feature = "embedding"), allow(unused_mut))]
    let mut reloadable_model_out: Option<
        Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>,
    > = None;

    if config.analysis.embedding.enabled {
        let embedding_config = &config.analysis.embedding;
        let pii_level = config.privacy.pii_filter_level;
        // #6914: off-device (remote) embedding egress passes through the egress PII floor SSOT
        // (RemoteEmbeddingProvider sanitizer = final gate just before POST). Local pipeline masking
        // is not egress, so it keeps the raw pii_level. See embedding_egress_pii_level for details.
        // Only the two `feature = "analysis"`-gated match arms below (Local-demoted-to-loopback
        // and Remote) read this — under `--no-default-features` neither arm compiles, so the
        // binding itself is gated to match its sole consumers.
        #[cfg(feature = "analysis")]
        let egress_pii_level = embedding_egress_pii_level(config);

        // Create EmbeddingProvider based on config
        let embedding_provider: Option<
            Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
        > = match embedding_config.provider {
            #[cfg(feature = "embedding")]
            maekon_core::config::EmbeddingProviderType::Local => {
                match maekon_embedding::LocalEmbeddingProvider::new(Some(
                    embedding_config.local_model.as_str(),
                )) {
                    Ok(provider) => {
                        info!("Local embedding provider initialized");
                        // #6266: keep one concrete Arc and expose BOTH facets from
                        // it — the EmbeddingProvider (scheduler) and the
                        // ReloadableModel (reload_embedding_model IPC). They share
                        // the same instance, so a reload mutates the live model.
                        let concrete = Arc::new(provider);
                        reloadable_model_out = Some(concrete.clone()
                            as Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>);
                        Some(
                            concrete
                                as Arc<
                                    dyn maekon_core::ports::embedding_provider::EmbeddingProvider,
                                >,
                        )
                    }
                    Err(e) => {
                        warn!("Local embedding provider init failed: {e}");
                        None
                    }
                }
            }
            // D′ (#5755): when Local is selected but the `embedding` feature is
            // off (default release build), degrade gracefully to loopback Ollama
            // rather than silently producing no vectors.
            //
            // Demotion only fires when the `analysis` feature is also on
            // (maekon-network available).  Without `analysis`, there is no remote
            // transport at all — keep the existing warn+None.
            #[cfg(all(not(feature = "embedding"), feature = "analysis"))]
            maekon_core::config::EmbeddingProviderType::Local => {
                // Privacy invariant: Local = on-device intent. The demotion
                // target is FORCED to loopback — a stale `remote_endpoint`
                // (external URL) in config must never cause egress for a user
                // who did not select provider = Remote.
                let target = loopback_embedding_target(embedding_config);
                info!(
                    "Local embedding unavailable in this build — delegating to \
                     loopback Ollama ({}@{}, dims={})",
                    target.model, target.endpoint, target.dims
                );
                let credential = match target.credential {
                    RemoteCredentialKind::NoAuth => {
                        maekon_core::ports::credential_source::CredentialSource::NoAuth
                    }
                    RemoteCredentialKind::ApiKey(key) => {
                        maekon_core::ports::credential_source::CredentialSource::ApiKey(key)
                    }
                    // loopback_embedding_target always returns NoAuth — this arm
                    // is unreachable at runtime but required for exhaustiveness.
                    RemoteCredentialKind::Stored(cs) => cs,
                };
                let mut provider =
                    maekon_network::remote_embedding_client::RemoteEmbeddingProvider::new_with_credential(
                        target.endpoint,
                        credential,
                        target.model,
                        target.dims,
                        30,
                        breaker_registry.clone(),
                    )
                    .with_pii_sanitizer(
                        Arc::new(maekon_vision::privacy::VisionPiiSanitizer),
                        // #6914: egress gate — apply the egress floor, not the raw pii_level.
                        egress_pii_level,
                    );
                // #6830: audit external embedding egress (loopback is gated out inside
                // the provider). Harmless on this demoted arm — a loopback target never records.
                if let Some(ledger) = egress_ledger.clone() {
                    provider = provider.with_egress_ledger(ledger);
                }
                Some(Arc::new(provider))
            }
            #[cfg(all(not(feature = "embedding"), not(feature = "analysis")))]
            maekon_core::config::EmbeddingProviderType::Local => {
                warn!("Local embedding requested but 'embedding' feature not enabled");
                None
            }
            // #5032: the remote embedding adapter lives in `maekon-network`, which
            // is only compiled when the `analysis` feature is on. Under
            // `--no-default-features` there is no remote transport to build, so
            // this arm falls back to `None` (same outcome as "no endpoint
            // configured"); the downstream NoOp fallback (below) keeps the
            // pipeline functional with zero vectors. No behaviour change when
            // `analysis` is enabled.
            //
            // D′ (#5755): the Remote arm now uses `resolve_remote_embedding_target`
            // so that a loopback endpoint (e.g. Ollama at localhost:11434) gets
            // NoAuth + correct model/dims defaults automatically.  External
            // endpoints continue to use `llm_api.api_key` (pre-D′ coupling).
            #[cfg(feature = "analysis")]
            maekon_core::config::EmbeddingProviderType::Remote => {
                let llm_api_key = config
                    .ai_provider
                    .llm_api
                    .as_ref()
                    .map(|api| api.api_key.as_str());
                let target =
                    resolve_remote_embedding_target(embedding_config, llm_api_key, secret_stores);
                let credential = match target.credential {
                    RemoteCredentialKind::NoAuth => {
                        maekon_core::ports::credential_source::CredentialSource::NoAuth
                    }
                    RemoteCredentialKind::ApiKey(key) => {
                        maekon_core::ports::credential_source::CredentialSource::ApiKey(key)
                    }
                    // Dedicated embedding keychain binding resolved by
                    // try_build_stored_credential. The CredentialSource is already
                    // fully built — pass it through unchanged.
                    RemoteCredentialKind::Stored(cs) => cs,
                };
                let mut provider =
                    maekon_network::remote_embedding_client::RemoteEmbeddingProvider::new_with_credential(
                        target.endpoint,
                        credential,
                        target.model,
                        target.dims,
                        30,
                        breaker_registry.clone(),
                    )
                    .with_pii_sanitizer(
                        Arc::new(maekon_vision::privacy::VisionPiiSanitizer),
                        // #6914: egress gate — apply the egress floor, not the raw pii_level.
                        egress_pii_level,
                    );
                // #6830: record one egress-ledger row per successful external embedding
                // upload (loopback endpoints are gated out inside the provider).
                if let Some(ledger) = egress_ledger.clone() {
                    provider = provider.with_egress_ledger(ledger);
                }
                Some(Arc::new(provider))
            }
            #[cfg(not(feature = "analysis"))]
            maekon_core::config::EmbeddingProviderType::Remote => {
                warn!(
                    "Remote embedding requested but the 'analysis' feature \
                     (maekon-network) is not compiled in — using no-op fallback"
                );
                None
            }
        };

        // Wrap the successfully created provider with a fallback → NoOp so that a
        // transient runtime failure does not propagate through the whole pipeline
        // but instead degrades to zero vectors.
        //
        // #4813: this fallback wrap must also work in the default/OSS build where
        // the `embedding` feature is off. Previously the entire wrap was hidden
        // behind the `embedding` feature, so the default build (with only the
        // remote provider enabled) ran without a runtime fallback. The remote
        // provider itself has no (maekon-network) feature gate, so the missing
        // fallback was the only problem. NoOpEmbeddingProvider always exists in
        // maekon-core, so we apply the wrap regardless of feature flags.
        //
        // When the `embedding` feature is enabled, we keep using maekon-embedding's
        // richer implementation, which provides health tracking
        // (`is_primary_healthy`).
        let embedding_provider: Option<
            Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
        > = embedding_provider.map(|p| {
            let noop = Arc::new(
                maekon_core::ports::embedding_provider::NoOpEmbeddingProvider::new(p.dimensions()),
            );
            #[cfg(feature = "embedding")]
            {
                Arc::new(maekon_embedding::FallbackEmbeddingProvider::new(p, noop))
                    as Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>
            }
            #[cfg(not(feature = "embedding"))]
            {
                // Lightweight wrapper that provides the same degrade semantics
                // using only maekon-core, without the (optional) maekon-embedding
                // dependency.
                Arc::new(RemoteFallbackEmbeddingProvider::new(p, noop))
                    as Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>
            }
        });

        if let (Some(ref provider), Some(ref vector_store)) =
            (&embedding_provider, &vector_store_opt)
        {
            let pii_filter_embed: maekon_analysis::PiiFilter = Box::new(move |text: &str| {
                maekon_vision::privacy::sanitize_title_with_level(text, pii_level)
            });
            let skip_float32 = embedding_config.quantization_enabled
                && !embedding_config.quantization_float32_retention;
            let pipeline = Arc::new(maekon_analysis::EmbeddingPipeline::with_float32_retention(
                provider.clone(),
                pii_filter_embed,
                vector_store.clone(),
                embedding_config.quantization_enabled,
                skip_float32,
            ));
            embedding_pipeline_arc = Some(pipeline);
            schedule_quantized_backfill(
                vector_store.clone(),
                embedding_config.quantization_enabled,
            );

            // Build LlmSegmentSummarizer if LLM summary is enabled.
            //
            // Two-path resolution (Decision 4, C2 #5722):
            //
            // Primary: llm_api Some → build_analysis_provider (existing path,
            //   supports all configured providers + GuardedAnalysisProvider).
            //   Triggered only when llm_api is NOT None.
            //
            // Fallback: llm_api None AND primary did NOT fire (i.e. no guard-missing
            //   fail-closed was involved) → build_local_ollama_summary_provider:
            //   loopback-pinned, catalog-default Ollama, no active-window gate.
            //   The fallback is explicitly triggered by `config.ai_provider.llm_api
            //   .is_none()` to avoid silently routing guard-missing or misconfigured
            //   provider failures to loopback Ollama (regression risk documented in
            //   verify.missing_hops[3]).
            //
            // Note: llm_summary is guarded by `llm_summary_enabled` (default false,
            //   explicit opt-in) + `embedding.enabled` (also default false). On
            //   standard shipped builds (`--features grpc`) neither the `embedding`
            //   feature nor a Local embedding provider is active, so this block is
            //   unreachable — see Decision 3.
            if embedding_config.llm_summary_enabled {
                let pii_level_summ = config.privacy.pii_filter_level;

                // Normalise the secret_stores reference so it has the right type
                // in both `analysis` and `not(analysis)` builds.  In `analysis`
                // builds the parameter is `Option<&SecretStoreSet>`; in other
                // builds the `not(analysis)` fallback of `build_analysis_provider`
                // expects `Option<&()>`, so we supply `None::<&()>`.
                #[cfg(feature = "analysis")]
                let secret_stores_for_summary = secret_stores;
                #[cfg(not(feature = "analysis"))]
                let secret_stores_for_summary: Option<&()> = None;

                let analysis_provider: Option<
                    Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider>,
                > = if config.ai_provider.llm_api.is_some() {
                    // Primary path: configured llm_api → build_analysis_provider.
                    crate::agent_runtime::analysis_helpers::build_analysis_provider(
                        &config.ai_provider,
                        pii_level_summ,
                        external_llm_privacy_guard.clone(),
                        secret_stores_for_summary,
                        breaker_registry.clone(),
                    )
                    .map(|(p, _)| p)
                } else {
                    None
                }
                .or_else(|| {
                    // Fallback path: llm_api None → attempt local Ollama (loopback only).
                    // MG-PII-02 / ADR-023: catalog-derived loopback endpoint; PII is
                    // filtered via pii_filter_summ + VisionPiiSanitizer inside the
                    // provider. Not routed through GuardedAnalysisProvider (no active-
                    // window gate needed for loopback device-local egress — MG-PII-03/AC8).
                    if config.ai_provider.llm_api.is_none() {
                        crate::agent_runtime::analysis_helpers::build_local_ollama_summary_provider(
                            pii_level_summ,
                            breaker_registry.clone(),
                        )
                    } else {
                        None
                    }
                });

                if let Some(provider) = analysis_provider {
                    let pii_filter_summ: maekon_analysis::PiiFilter =
                        Box::new(move |text: &str| {
                            maekon_vision::privacy::sanitize_title_with_level(text, pii_level_summ)
                        });
                    let min_duration = embedding_config.min_segment_for_summary_secs;
                    llm_summarizer_arc =
                        Some(Arc::new(maekon_analysis::LlmSegmentSummarizer::new(
                            provider,
                            pii_filter_summ,
                            true,
                            min_duration,
                        )));
                    info!("LLM segment summarizer enabled");
                } else {
                    // No provider yielded — neither llm_api configured nor local
                    // Ollama fallback available (embedding feature absent, or
                    // new_local_enrichment refused the derived endpoint).
                    warn!("LLM summary enabled but no LLM provider available (configure llm_api or ensure Ollama is accessible)");
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
            maekon_core::ports::embedding_provider::NoOpEmbeddingProvider::new(384),
        ));
    }

    EmbeddingComponents {
        embedding_pipeline: embedding_pipeline_arc,
        llm_summarizer: llm_summarizer_arc,
        embedding_provider: embedding_provider_out,
        vector_store: vector_store_out,
        reloadable_model: reloadable_model_out,
    }
}

/// Build the adaptive `SearchConfig` from the embedding config section.
///
/// Shared by the scheduler ingestion wiring (`agent_runtime::run`) and the web
/// semantic-search wiring ([`build_web_search_components`]) so both coordinators
/// apply the SAME strategy thresholds / forced-strategy / quantization gate.
pub(crate) fn search_config_from(
    embedding_config: &maekon_core::config::EmbeddingConfig,
) -> maekon_analysis::SearchConfig {
    maekon_analysis::SearchConfig {
        brute_force_threshold: 10_000,
        ivf_threshold: 100_000,
        hnsw_threshold: 5_000,
        oversample_factor: embedding_config.binary_oversample_factor,
        default_nprobe: embedding_config.ivf_nprobe,
        forced_strategy: match embedding_config.index_strategy.as_str() {
            "auto" => None,
            s @ ("brute_force" | "ivf" | "ivf_binary") => Some(s.to_string()),
            // "hnsw" is meaningful only when compiled with the feature AND an
            // AnnIndex is wired (not the case in production).
            #[cfg(feature = "hnsw")]
            "hnsw" => Some("hnsw".to_string()),
            other => {
                // review4 F9: an unrecognized index_strategy previously degraded
                // silently to a full brute-force scan. Warn once and fall back to
                // auto strategy selection.
                warn!(
                    index_strategy = %other,
                    "unrecognized embedding.index_strategy; using auto strategy selection"
                );
                None
            }
        },
        // #7479: when quantization is disabled the coordinator routes to the f32
        // search path — the INT8 tiers read `vector_int8`, NULL for every row.
        quantization_enabled: embedding_config.quantization_enabled,
    }
}

/// Query-side semantic-search components for the web dashboard (#8059).
///
/// All `None` = honest degrade (embedding disabled, or no real provider could
/// be built), so `/api/semantic-search/capabilities` reports unavailable rather
/// than letting `mode=semantic` 501 or `mode=hybrid` silently keyword-degrade.
#[derive(Default)]
pub(crate) struct WebSearchComponents {
    pub(crate) embedding_provider:
        Option<Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>>,
    pub(crate) vector_store: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>>,
    pub(crate) adaptive_search:
        Option<Arc<dyn maekon_core::ports::adaptive_search::AdaptiveSearchPort>>,
}

/// Build the web dashboard's semantic-search query-side components, tapping the
/// SAME [`build_embedding_components`] source the scheduler ingestion pipeline
/// uses. This guarantees query embeddings match document embeddings (identical
/// model/dims + credential resolution + egress PII floor) and that search reads
/// the SAME `embedding_vectors` table the scheduler writes into (both build a
/// `SqliteVectorStore` over the same SQLite connection).
///
/// Returns all-`None` unless `analysis.embedding.enabled` AND a real embedding
/// provider was constructed. The honest-availability signal is
/// [`EmbeddingComponents::vector_store`]`.is_some()`, which is `Some` only in
/// that case; the always-present NoOp fallback provider is deliberately NOT
/// surfaced here (it would falsely flip `semantic_available` to `true` while
/// returning zero vectors).
///
/// Built once at web-server startup — a later `analysis.embedding.enabled` flip
/// requires an app restart to appear (see the PR's restart-requirement note).
///
/// The freshly-built `AdaptiveSearchCoordinator` starts with a cold
/// `cached_vector_count` (0), so it selects the brute-force/f32 tier. With
/// quantization disabled (the default) the coordinator routes to the f32
/// `search_filtered` path regardless of count, so results are correct — the
/// cold count affects only strategy SELECTION (performance at very large
/// collections), never correctness. The web search path intentionally does not
/// run the scheduler's periodic `refresh_count` maintenance.
///
/// `external_llm_privacy_guard` is deliberately `None`: the web path never runs
/// the LLM segment summarizer (it consumes only the embedding provider + vector
/// store), so the summarizer built inside `build_embedding_components` is
/// discarded and makes no network calls.
pub(crate) fn build_web_search_components(
    config: &AppConfig,
    sqlite_storage: &Arc<maekon_storage::sqlite::SqliteStorage>,
    #[cfg(feature = "analysis")] secret_stores: Option<&SecretStoreSet>,
    #[cfg(feature = "analysis")] egress_ledger: Option<
        Arc<dyn maekon_core::ports::egress_ledger::EgressLedgerSink>,
    >,
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
) -> WebSearchComponents {
    let vector_store: Arc<dyn maekon_core::ports::vector_store::VectorStore> = Arc::new(
        maekon_storage::sqlite::vector_store_impl::SqliteVectorStore::new(
            sqlite_storage.connection_arc(),
        ),
    );

    let components = build_embedding_components(
        config,
        Some(vector_store.clone()),
        None, // external_llm_privacy_guard: the web path discards the summarizer.
        #[cfg(feature = "analysis")]
        secret_stores,
        #[cfg(feature = "analysis")]
        egress_ledger,
        breaker_registry,
    );

    // `vector_store` is `Some` only when embedding is enabled AND a real provider
    // was wired; in that case `embedding_provider` is that real (fallback-wrapped)
    // provider — the NoOp fallback is applied only when `vector_store` stays
    // `None`. Gating on both keeps `semantic_available` honest.
    match (components.vector_store, components.embedding_provider) {
        (Some(vs), Some(ep)) => {
            let vector_index: Arc<dyn maekon_core::ports::vector_index::VectorIndex> = Arc::new(
                maekon_storage::sqlite::vector_index_impl::SqliteVectorIndex::new(
                    sqlite_storage.connection_arc(),
                ),
            );
            let coordinator: Arc<dyn maekon_core::ports::adaptive_search::AdaptiveSearchPort> =
                Arc::new(maekon_analysis::AdaptiveSearchCoordinator::new(
                    vs.clone(),
                    vector_index,
                    search_config_from(&config.analysis.embedding),
                ));
            WebSearchComponents {
                embedding_provider: Some(ep),
                vector_store: Some(vs),
                adaptive_search: Some(coordinator),
            }
        }
        _ => WebSearchComponents::default(),
    }
}

/// Lightweight fallback wrapper for the default/OSS build (`embedding` feature
/// off) (#4813).
///
/// When the `embedding` feature is enabled, maekon-embedding's
/// `FallbackEmbeddingProvider` is used instead, so this type is not compiled.
/// It provides the same semantics — degrading to the fallback (NoOp) on primary
/// failure — using only maekon-core.
#[cfg(not(feature = "embedding"))]
struct RemoteFallbackEmbeddingProvider {
    primary: Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
    fallback: Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
}

#[cfg(not(feature = "embedding"))]
impl RemoteFallbackEmbeddingProvider {
    fn new(
        primary: Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
        fallback: Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
    ) -> Self {
        Self { primary, fallback }
    }
}

#[cfg(not(feature = "embedding"))]
#[async_trait::async_trait]
impl maekon_core::ports::embedding_provider::EmbeddingProvider for RemoteFallbackEmbeddingProvider {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, maekon_core::error::CoreError> {
        match self.primary.embed(text).await {
            Ok(v) => Ok(v),
            Err(e) => {
                warn!("primary embedding failed, falling back to no-op: {e}");
                self.fallback.embed(text).await
            }
        }
    }

    async fn embed_batch(
        &self,
        texts: &[String],
    ) -> Result<Vec<Vec<f32>>, maekon_core::error::CoreError> {
        match self.primary.embed_batch(texts).await {
            Ok(v) => Ok(v),
            Err(e) => {
                warn!("primary batch embedding failed, falling back to no-op: {e}");
                self.fallback.embed_batch(texts).await
            }
        }
    }

    fn dimensions(&self) -> usize {
        self.primary.dimensions()
    }

    fn model_id(&self) -> &str {
        self.primary.model_id()
    }

    /// #6477 F18 (sibling of `FallbackEmbeddingProvider`): forward idle-eviction
    /// to both wrapped providers instead of using the trait's no-op default, so a
    /// future local/resident primary under this `not(feature = "embedding")` arm is
    /// still evicted when idle.
    fn evict_if_idle(&self, idle_after: std::time::Duration) -> bool {
        let primary = self.primary.evict_if_idle(idle_after);
        let fallback = self.fallback.evict_if_idle(idle_after);
        primary || fallback
    }
}

fn schedule_quantized_backfill(
    vector_store: Arc<dyn maekon_core::ports::vector_store::VectorStore>,
    quantization_enabled: bool,
) {
    if !quantization_enabled {
        return;
    }

    tauri::async_runtime::spawn(async move {
        match backfill_quantized_vectors_once(vector_store, QUANTIZED_BACKFILL_BATCH_SIZE).await {
            Ok(0) => {}
            Ok(rows) => info!(
                rows = rows,
                "Backfilled INT8 quantization for existing embedding vectors"
            ),
            Err(error) => warn!("Failed to backfill INT8 quantization: {error}"),
        }
    });
}

async fn backfill_quantized_vectors_once(
    vector_store: Arc<dyn maekon_core::ports::vector_store::VectorStore>,
    batch_size: usize,
) -> Result<u64, CoreError> {
    let pending = vector_store.count_unquantized().await?;
    if pending == 0 {
        return Ok(0);
    }
    vector_store.backfill_quantized(batch_size).await
}

/// Pure-function tests for `resolve_remote_embedding_target`.
/// Gated on `analysis` because the helper itself requires `maekon_network`.
#[cfg(all(test, feature = "analysis"))]
mod target_resolver_tests {
    use super::*;
    use maekon_core::config::EmbeddingConfig;

    fn cfg_with(
        endpoint: Option<&str>,
        model: Option<&str>,
        dims: Option<usize>,
    ) -> EmbeddingConfig {
        EmbeddingConfig {
            remote_endpoint: endpoint.map(|s| s.to_string()),
            remote_model: model.map(|s| s.to_string()),
            remote_dimensions: dims,
            ..EmbeddingConfig::default()
        }
    }

    #[test]
    fn loopback_default_uses_ollama_model_and_noauth() {
        let cfg = cfg_with(None, None, None);
        let target = resolve_remote_embedding_target(&cfg, None, None);
        assert_eq!(target.endpoint, OLLAMA_LOOPBACK_ENDPOINT);
        assert_eq!(target.model, OLLAMA_LOOPBACK_DEFAULT_MODEL);
        assert_eq!(target.dims, OLLAMA_LOOPBACK_DEFAULT_DIMS);
        assert!(matches!(target.credential, RemoteCredentialKind::NoAuth));
    }

    #[test]
    fn external_endpoint_uses_openai_defaults_and_api_key() {
        let cfg = cfg_with(Some("https://api.openai.com/v1/embeddings"), None, None);
        let target = resolve_remote_embedding_target(&cfg, Some("sk-test"), None);
        assert_eq!(target.model, EXTERNAL_DEFAULT_MODEL);
        assert_eq!(target.dims, EXTERNAL_DEFAULT_DIMS);
        assert!(matches!(target.credential, RemoteCredentialKind::ApiKey(_)));
        if let RemoteCredentialKind::ApiKey(key) = &target.credential {
            assert_eq!(key, "sk-test");
        }
    }

    #[test]
    fn explicit_loopback_endpoint_resolves_as_loopback() {
        let cfg = cfg_with(Some("http://127.0.0.1:11434/v1/embeddings"), None, None);
        let target = resolve_remote_embedding_target(&cfg, Some("sk-key"), None);
        assert_eq!(target.model, OLLAMA_LOOPBACK_DEFAULT_MODEL);
        assert!(matches!(target.credential, RemoteCredentialKind::NoAuth));
    }

    #[test]
    fn remote_model_override_wins_for_loopback() {
        let cfg = cfg_with(None, Some("nomic-embed-text"), Some(512));
        let target = resolve_remote_embedding_target(&cfg, None, None);
        assert_eq!(target.model, "nomic-embed-text");
        assert_eq!(target.dims, 512);
        assert!(matches!(target.credential, RemoteCredentialKind::NoAuth));
    }

    #[test]
    fn remote_model_override_wins_for_external() {
        let cfg = cfg_with(
            Some("https://my-provider.com/embeddings"),
            Some("custom-model"),
            Some(1024),
        );
        let target = resolve_remote_embedding_target(&cfg, Some("sk-custom"), None);
        assert_eq!(target.model, "custom-model");
        assert_eq!(target.dims, 1024);
    }

    #[test]
    fn no_api_key_external_falls_back_to_empty_string() {
        let cfg = cfg_with(Some("https://api.openai.com/v1/embeddings"), None, None);
        let target = resolve_remote_embedding_target(&cfg, None, None);
        if let RemoteCredentialKind::ApiKey(key) = &target.credential {
            assert_eq!(key, "");
        } else {
            panic!("expected ApiKey for external endpoint");
        }
    }

    // ── remote_credential binding tests ──────────────────────────────────────

    /// Minimal in-memory SecretStore double for resolver tests.
    struct FixedStore(String);

    #[async_trait::async_trait]
    impl maekon_core::ports::secret_store::SecretStore for FixedStore {
        async fn store(
            &self,
            _ns: &str,
            _k: &str,
            _v: &str,
        ) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
        async fn retrieve(
            &self,
            _ns: &str,
            _k: &str,
        ) -> Result<Option<String>, maekon_core::error::CoreError> {
            Ok(Some(self.0.clone()))
        }
        async fn delete(&self, _ns: &str, _k: &str) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
        async fn delete_namespace(&self, _ns: &str) -> Result<(), maekon_core::error::CoreError> {
            Ok(())
        }
    }

    fn make_stores_with_os(key: &str) -> maekon_core::ports::secret_store::SecretStoreSet {
        use maekon_core::config::CredentialBackendKind;
        use maekon_core::ports::secret_store::SecretStoreSet;
        use std::sync::Arc;
        SecretStoreSet {
            os_secret_store: Some(Arc::new(FixedStore(key.to_string()))),
            file_secret_store: None,
            env_secret_store: None,
            default_backend_kind: CredentialBackendKind::OsSecretStore,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        }
    }

    /// external + remote_credential Some + stores Some → Stored variant.
    #[test]
    fn external_with_binding_and_stores_uses_stored_credential() {
        use maekon_core::config::{
            CredentialAuthMode, CredentialBackendKind, CredentialBinding, SecretRef,
        };
        let mut cfg = cfg_with(Some("https://api.openai.com/v1/embeddings"), None, None);
        cfg.remote_credential = Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/embedding".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: false,
        });
        let stores = make_stores_with_os("sk-stored-embed");
        let target = resolve_remote_embedding_target(&cfg, Some("sk-inline"), Some(&stores));
        assert!(
            matches!(target.credential, RemoteCredentialKind::Stored(_)),
            "expected Stored variant when binding + stores are present"
        );
    }

    /// binding Some but stores None → falls back to ApiKey (inline key).
    #[test]
    fn external_with_binding_but_no_stores_falls_back_to_api_key() {
        use maekon_core::config::{
            CredentialAuthMode, CredentialBackendKind, CredentialBinding, SecretRef,
        };
        let mut cfg = cfg_with(Some("https://api.openai.com/v1/embeddings"), None, None);
        cfg.remote_credential = Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/embedding".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: false,
        });
        let target = resolve_remote_embedding_target(&cfg, Some("sk-fallback"), None);
        if let RemoteCredentialKind::ApiKey(key) = &target.credential {
            assert_eq!(key, "sk-fallback");
        } else {
            panic!("expected ApiKey fallback when stores are absent, got Stored/NoAuth");
        }
    }

    /// loopback + binding Some + stores Some → NoAuth (loopback security invariant).
    #[test]
    fn loopback_with_binding_still_uses_no_auth() {
        use maekon_core::config::{
            CredentialAuthMode, CredentialBackendKind, CredentialBinding, SecretRef,
        };
        let mut cfg = cfg_with(Some("http://localhost:11434/v1/embeddings"), None, None);
        cfg.remote_credential = Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/embedding".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: false,
        });
        let stores = make_stores_with_os("sk-should-not-be-used");
        let target = resolve_remote_embedding_target(&cfg, Some("sk-also-not-used"), Some(&stores));
        assert!(
            matches!(target.credential, RemoteCredentialKind::NoAuth),
            "loopback must always be NoAuth regardless of remote_credential binding"
        );
    }
}

#[cfg(all(test, not(feature = "embedding")))]
mod tests {
    use super::*;
    use maekon_core::error::CoreError;
    use maekon_core::models::embedding::{EmbeddingMetadata, SearchFilters, SearchResult};
    use maekon_core::ports::embedding_provider::{EmbeddingProvider, NoOpEmbeddingProvider};
    use maekon_core::ports::vector_store::VectorStore;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    /// A primary provider that always fails — used to verify the fallback path.
    struct FailingProvider;

    struct BackfillVectorStore {
        pending: AtomicU64,
        count_calls: AtomicUsize,
        backfill_calls: AtomicUsize,
        last_batch_size: AtomicUsize,
    }

    impl BackfillVectorStore {
        fn new(pending: u64) -> Self {
            Self {
                pending: AtomicU64::new(pending),
                count_calls: AtomicUsize::new(0),
                backfill_calls: AtomicUsize::new(0),
                last_batch_size: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl VectorStore for BackfillVectorStore {
        async fn store(
            &self,
            _vector: Vec<f32>,
            _metadata: EmbeddingMetadata,
        ) -> Result<(), CoreError> {
            Ok(())
        }

        async fn search(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
        }

        async fn search_filtered(
            &self,
            _query_vector: &[f32],
            _limit: usize,
            _time_decay_hours: f32,
            _filters: &SearchFilters,
        ) -> Result<Vec<SearchResult>, CoreError> {
            Ok(vec![])
        }

        async fn enforce_retention(&self, _max_days: u32) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn mark_stale(&self, _old_model_id: &str) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn update_vector(
            &self,
            _id: i64,
            _vector: Vec<f32>,
            _model_id: &str,
        ) -> Result<u64, CoreError> {
            Ok(0)
        }

        async fn count_unquantized(&self) -> Result<u64, CoreError> {
            self.count_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.pending.load(Ordering::SeqCst))
        }

        async fn backfill_quantized(&self, batch_size: usize) -> Result<u64, CoreError> {
            self.backfill_calls.fetch_add(1, Ordering::SeqCst);
            self.last_batch_size.store(batch_size, Ordering::SeqCst);
            Ok(self.pending.swap(0, Ordering::SeqCst))
        }

        async fn get_current_model_id(&self) -> Result<Option<String>, CoreError> {
            Ok(None)
        }

        async fn get_stale_vectors(&self, _limit: usize) -> Result<Vec<(i64, String)>, CoreError> {
            Ok(vec![])
        }

        async fn get_metadata_by_ids(
            &self,
            _ids: &[u64],
        ) -> Result<HashMap<u64, EmbeddingMetadata>, CoreError> {
            Ok(HashMap::new())
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingProvider for FailingProvider {
        async fn embed(&self, _text: &str) -> Result<Vec<f32>, CoreError> {
            Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "remote embedding down".into(),
            })
        }

        fn dimensions(&self) -> usize {
            384
        }

        fn model_id(&self) -> &str {
            "failing"
        }
    }

    /// Even in the default build, a remote failure must degrade to NoOp (zero
    /// vectors) (#4813).
    #[tokio::test]
    async fn default_build_falls_back_to_noop_on_primary_failure() {
        let primary = Arc::new(FailingProvider);
        let noop = Arc::new(NoOpEmbeddingProvider::new(primary.dimensions()));
        let provider = RemoteFallbackEmbeddingProvider::new(primary, noop);

        let vec = provider
            .embed("hello")
            .await
            .expect("fallback should yield a zero vector, not an error");
        assert_eq!(vec.len(), 384);
        assert!(vec.iter().all(|&x| x == 0.0));

        let batch = provider
            .embed_batch(&["a".to_string(), "b".to_string()])
            .await
            .expect("batch fallback should succeed");
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().all(|v| v.len() == 384));
    }

    #[tokio::test]
    async fn quantized_backfill_runs_when_pending_rows_exist() {
        let store = Arc::new(BackfillVectorStore::new(3));
        let rows = backfill_quantized_vectors_once(store.clone(), 17)
            .await
            .unwrap();

        assert_eq!(rows, 3);
        assert_eq!(store.count_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.backfill_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.last_batch_size.load(Ordering::SeqCst), 17);
    }

    #[tokio::test]
    async fn quantized_backfill_skips_when_no_pending_rows() {
        let store = Arc::new(BackfillVectorStore::new(0));
        let rows = backfill_quantized_vectors_once(store.clone(), 17)
            .await
            .unwrap();

        assert_eq!(rows, 0);
        assert_eq!(store.count_calls.load(Ordering::SeqCst), 1);
        assert_eq!(store.backfill_calls.load(Ordering::SeqCst), 0);
    }

    /// Privacy regression (D′ lead review): the Local-arm demotion target must
    /// be loopback-FORCED. A stale external `remote_endpoint` in config must
    /// never route a provider=Local user's text off-device — Local expresses an
    /// on-device intent and only an explicit Remote selection may use the
    /// configured endpoint.
    #[test]
    #[cfg(all(not(feature = "embedding"), feature = "analysis"))]
    fn local_demotion_target_ignores_external_remote_endpoint() {
        let cfg = maekon_core::config::EmbeddingConfig {
            remote_endpoint: Some("https://api.openai.com/v1/embeddings".to_string()),
            ..Default::default()
        };

        let target = super::loopback_embedding_target(&cfg);

        assert_eq!(target.endpoint, super::OLLAMA_LOOPBACK_ENDPOINT);
        assert!(matches!(
            target.credential,
            super::RemoteCredentialKind::NoAuth
        ));
        assert_eq!(target.model, super::OLLAMA_LOOPBACK_DEFAULT_MODEL);
        assert_eq!(target.dims, super::OLLAMA_LOOPBACK_DEFAULT_DIMS);
    }

    /// remote_model / remote_dimensions overrides still apply on the demotion
    /// path (harmless on loopback — model name + MRL dims only).
    #[test]
    #[cfg(all(not(feature = "embedding"), feature = "analysis"))]
    fn local_demotion_target_honors_model_and_dims_overrides() {
        let cfg = maekon_core::config::EmbeddingConfig {
            remote_model: Some("qwen3-embedding:0.6b".to_string()),
            remote_dimensions: Some(256),
            ..Default::default()
        };

        let target = super::loopback_embedding_target(&cfg);

        assert_eq!(target.endpoint, super::OLLAMA_LOOPBACK_ENDPOINT);
        assert_eq!(target.model, "qwen3-embedding:0.6b");
        assert_eq!(target.dims, 256);
    }

    /// #6914 regression guard: the PII level for remote embedding egress must pass through the
    /// egress floor SSOT. Under the `AllowFiltered + Off` combination it was raw `Off` (verbatim
    /// leak) before the fix, but it must now floor up to `Basic` via `effective_egress_pii_level`
    /// — and must **differ** from the raw pii_filter_level(Off) (if unchanged, the floor was not
    /// applied = bug regressed).
    #[test]
    fn embedding_egress_pii_level_floors_allow_filtered_off_to_basic() {
        let mut config = maekon_core::config::AppConfig::default_config();
        config.ai_provider.external_data_policy =
            maekon_core::config::ExternalDataPolicy::AllowFiltered;
        config.privacy.pii_filter_level = PiiFilterLevel::Off;

        let egress = super::embedding_egress_pii_level(&config);
        assert_eq!(
            egress,
            PiiFilterLevel::Basic,
            "AllowFiltered + Off 는 egress 에서 Basic 으로 floor 되어야 한다"
        );
        assert_ne!(
            egress, config.privacy.pii_filter_level,
            "egress 레벨이 raw pii_filter_level(Off)과 같으면 floor 미적용 — 버그 재발"
        );
    }

    /// The PiiFilterStrict policy pins to Strict regardless of the configured level (strongest egress masking).
    #[test]
    fn embedding_egress_pii_level_strict_policy_pins_strict() {
        let mut config = maekon_core::config::AppConfig::default_config();
        config.ai_provider.external_data_policy =
            maekon_core::config::ExternalDataPolicy::PiiFilterStrict;
        config.privacy.pii_filter_level = PiiFilterLevel::Off;

        assert_eq!(
            super::embedding_egress_pii_level(&config),
            PiiFilterLevel::Strict
        );
    }
}
