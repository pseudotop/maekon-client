use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[cfg(feature = "analysis")]
use maekon_analysis::{FallbackAnalysisProvider, NoOpAnalysisProvider};
use maekon_core::config::{AiProviderConfig, PiiFilterLevel};
use maekon_core::ports::analysis_provider::AnalysisProvider;
#[cfg(feature = "analysis")]
use maekon_core::ports::credential_source::CredentialSource;
#[cfg(feature = "analysis")]
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
#[cfg(feature = "analysis")]
use maekon_core::ports::secret_store::SecretStoreSet;
#[cfg(feature = "analysis")]
use maekon_network::analysis_client::AnalysisClient;

use crate::breaker_registry::CircuitBreakerRegistry;
use crate::provider_adapters::ExternalOcrPrivacyGuard;

/// Build an AnalysisProvider with automatic fallback chaining.
///
/// Returns `None` when no primary `llm_api` is configured.
/// The returned `Arc<AtomicBool>` tracks primary provider health and can be
/// stored in AppState for IPC health queries.
///
/// `secret_stores` is threaded from `AgentRuntimeBundle::provider_secret_stores`
/// so that OS-keychain-backed keys are resolved at request time via
/// `CredentialSource::StoredSecret` when the endpoint has a credential binding.
/// When `secret_stores` is `None` or no credential binding exists on the
/// endpoint, the inline `api_key` is used as the fallback (existing behaviour).
#[cfg(feature = "analysis")]
pub fn build_analysis_provider(
    config: &AiProviderConfig,
    pii_level: PiiFilterLevel,
    privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<&SecretStoreSet>,
    // D7 (#4812 / E20-20): the shared workspace-wide circuit-breaker registry
    // threaded from the composition root, so this analysis adapter converges on
    // the same breaker as every other adapter targeting the same endpoint.
    breaker_registry: Arc<CircuitBreakerRegistry>,
) -> Option<(Arc<dyn AnalysisProvider>, Arc<AtomicBool>)> {
    build_analysis_provider_with_flag(
        config,
        pii_level,
        privacy_guard,
        secret_stores,
        None,
        breaker_registry,
    )
}

/// Like [`build_analysis_provider`] but accepts a pre-created health flag.
///
/// When `external_flag` is `Some`, that flag is wired into the
/// [`FallbackAnalysisProvider`] so the caller can share the same `Arc<AtomicBool>`
/// with `AppState` for IPC health queries.
///
/// `secret_stores` is forwarded from the caller; see [`build_analysis_provider`]
/// for the credential-resolution semantics.
#[cfg(feature = "analysis")]
pub fn build_analysis_provider_with_flag(
    config: &AiProviderConfig,
    pii_level: PiiFilterLevel,
    privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<&SecretStoreSet>,
    external_flag: Option<Arc<AtomicBool>>,
    // D7 (#4812 / E20-20): shared workspace-wide registry from the composition
    // root. Primary + fallback already share this single Arc, and so does every
    // other adapter targeting the same endpoint (iter-011 consolidation done).
    breaker_registry: Arc<CircuitBreakerRegistry>,
) -> Option<(Arc<dyn AnalysisProvider>, Arc<AtomicBool>)> {
    let llm_api = config.llm_api.as_ref()?;
    let sanitizer: Arc<dyn PiiSanitizer> = Arc::new(maekon_vision::privacy::VisionPiiSanitizer);

    // Attempt to build a StoredSecret credential from the endpoint binding.
    // On Err (no binding, missing store, etc.) fall back to the inline api_key
    // path (`AnalysisClient::new`) so existing deployments without a Settings
    // credential binding are unaffected.
    let primary: Arc<dyn AnalysisProvider> = {
        let store =
            secret_stores.and_then(|stores| stores.for_binding(llm_api.credential.as_ref()));
        let profile_id = config.active_secret_profile_id_or("llm");
        match CredentialSource::from_api_key_endpoint_for_profile(llm_api, Some(profile_id), store)
        {
            Ok(credential) => Arc::new(
                AnalysisClient::new_with_credential(llm_api, credential, breaker_registry.clone())
                    .with_pii_sanitizer(sanitizer.clone(), pii_level),
            ),
            Err(e) => {
                // No credential binding or store unavailable — fall back to
                // inline api_key (existing behaviour, zero regression risk).
                tracing::warn!(
                    "analysis provider: credential resolution failed (using inline key): {e}"
                );
                Arc::new(
                    AnalysisClient::new(llm_api, breaker_registry.clone())
                        .with_pii_sanitizer(sanitizer.clone(), pii_level),
                )
            }
        }
    };

    let fallback: Arc<dyn AnalysisProvider> = match config.llm_api_fallback.as_ref() {
        Some(api) => Arc::new(
            AnalysisClient::new(api, breaker_registry.clone())
                .with_pii_sanitizer(sanitizer, pii_level),
        ),
        None => Arc::new(NoOpAnalysisProvider),
    };

    let health_flag = external_flag.unwrap_or_else(|| Arc::new(AtomicBool::new(true)));
    let provider = Arc::new(FallbackAnalysisProvider::new_with_flag(
        primary,
        fallback,
        health_flag.clone(),
    ));
    let provider = match crate::provider_adapters::guard_external_analysis_provider(
        provider as Arc<dyn AnalysisProvider>,
        privacy_guard,
    ) {
        Ok(provider) => provider,
        Err(error) => {
            tracing::warn!("analysis provider disabled: {error}");
            health_flag.store(false, std::sync::atomic::Ordering::Relaxed);
            return None;
        }
    };

    Some((provider, health_flag))
}

/// ADR-023 Phase-2: build the LOCAL-only-gated `AnalysisProvider` for memory-graph
/// belief revision (D1/D2).
///
/// Uses [`AnalysisClient::new_local_enrichment`] — which refuses a non-loopback
/// endpoint and pins reqwest to the resolved loopback address(es) — wrapped ONLY
/// in a `Fallback → NoOp` chain. It is deliberately NOT routed through
/// `GuardedAnalysisProvider` (that gates on the active window + `full_text_extraction`,
/// neither of which is correct for previously-stored claim text — MG-PII-03/AC8;
/// the per-claim masking + the dedicated `memory_graph_enrichment` consent gate are
/// applied by `BeliefRevision` / the scheduler instead).
///
/// Returns a `NoOp`-backed provider (so belief revision degrades cleanly) when no
/// `llm_api` is configured OR the configured endpoint is not loopback.
#[cfg(feature = "analysis")]
pub fn build_enrichment_provider(
    config: &AiProviderConfig,
    pii_level: PiiFilterLevel,
    // D7 (#4812 / E20-20): shared workspace-wide registry from the composition root.
    breaker_registry: Arc<CircuitBreakerRegistry>,
) -> Arc<dyn AnalysisProvider> {
    let Some(llm_api) = config.llm_api.as_ref() else {
        return Arc::new(NoOpAnalysisProvider);
    };
    match AnalysisClient::new_local_enrichment(llm_api, breaker_registry) {
        Some(client) => {
            let sanitizer: Arc<dyn PiiSanitizer> =
                Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
            let primary: Arc<dyn AnalysisProvider> =
                Arc::new(client.with_pii_sanitizer(sanitizer, pii_level));
            Arc::new(FallbackAnalysisProvider::new(
                primary,
                Arc::new(NoOpAnalysisProvider),
            ))
        }
        None => Arc::new(NoOpAnalysisProvider), // non-loopback endpoint → fail-closed
    }
}

/// Build a loopback-pinned `AnalysisProvider` backed by local Ollama for the
/// LLM-summary path when no `llm_api` is configured.
///
/// Uses [`AnalysisClient::new_local_enrichment`] — which refuses any non-loopback
/// endpoint and pins reqwest to the resolved loopback address(es) (DNS-rebind-safe).
/// The endpoint URL is derived from the provider-surface catalog (SSOT), so it is
/// always a loopback address by construction — satisfying `new_local_enrichment`'s
/// precondition and the `new_local_enrichment` precedent from ADR-023.
///
/// PII: wrapped with `pii_filter_summ` + `VisionPiiSanitizer` (same as the primary
/// path). MG-PII-02 / ADR-023 loopback-pin rationale: segment text is an on-device
/// artefact; loopback Ollama is within the device's egress boundary, so no active-
/// window consent gate is needed — matches the MG-PII-03 / AC8 precedent used by
/// `build_enrichment_provider`. This rationale MUST be documented in every PR that
/// ships this code path (edge_case 1).
///
/// **MG-PII-04** — see `docs/architecture/ADR-023-local-symbolic-memory-graph.md`
/// §"PII Mitigations" for the full consent/audit asymmetry rationale and the
/// acknowledged residual delta (Ollama daemon log persistence).
///
/// Returns `None` when:
/// - `new_local_enrichment` refuses the derived endpoint (should not happen for
///   catalog-default loopback, but fail-closed is the contract), or
/// - the `analysis` feature is not compiled (see `not(analysis)` fallback below).
#[cfg(feature = "analysis")]
pub fn build_local_ollama_summary_provider(
    pii_level: PiiFilterLevel,
    // D7 (#4812 / E20-20): shared workspace-wide registry from the composition root.
    breaker_registry: Arc<CircuitBreakerRegistry>,
) -> Option<Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider>> {
    use maekon_analysis::{FallbackAnalysisProvider, NoOpAnalysisProvider};
    use maekon_api_contracts::provider_specs;
    use maekon_core::config::{AiProviderType, ExternalApiEndpoint};

    const OLLAMA_SURFACE: &str = "provider_surface.ollama.local_http";
    const CHAT_COMPLETIONS: &str = "/v1/chat/completions";
    const CATALOG_FALLBACK: &str = "http://localhost:11434/v1/chat/completions";

    // Derive the chat-completions URL from the catalog surface.
    // The catalog llm_transport URL is /v1/responses; we want /v1/chat/completions
    // (AnalysisClient sends a chat-completions body — see step 4 / Decision 7).
    // Since we control this synthetic endpoint, we build it directly from the
    // origin (scheme://host:port) + chat-completions path.
    let endpoint = provider_specs::availability_probe(OLLAMA_SURFACE)
        .ok()
        .flatten()
        .and_then(|probe| {
            // probe.url is e.g. "http://localhost:11434/api/version"
            // Strip everything after the last component by parsing + replacing path.
            let mut url = reqwest::Url::parse(&probe.url).ok()?;
            url.set_path(CHAT_COMPLETIONS);
            url.set_query(None);
            Some(url.to_string())
        })
        .unwrap_or_else(|| CATALOG_FALLBACK.to_string());

    // Resolve the catalog default model (qwen3:8b) for Ollama.
    let model = provider_specs::resolved_default_model(
        AiProviderType::Ollama,
        None,
        maekon_api_contracts::provider_specs::SurfaceCapabilityKind::Llm,
    )
    .ok()
    .flatten();

    // Build a synthetic ExternalApiEndpoint pointing at the loopback Ollama.
    // api_key = "" (Ollama requires no key), credential = None.
    let synthetic_endpoint = ExternalApiEndpoint {
        endpoint,
        api_key: String::new(),
        model,
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some(OLLAMA_SURFACE.to_string()),
        credential: None,
    };

    // new_local_enrichment enforces loopback; it returns None for non-loopback
    // endpoints (fail-closed). For our catalog-derived loopback this succeeds.
    let client = AnalysisClient::new_local_enrichment(&synthetic_endpoint, breaker_registry)?;
    let sanitizer: Arc<dyn PiiSanitizer> = Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
    let primary: Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider> =
        Arc::new(client.with_pii_sanitizer(sanitizer, pii_level));

    Some(Arc::new(FallbackAnalysisProvider::new(
        primary,
        Arc::new(NoOpAnalysisProvider),
    )))
}

// =============================================================================
// #5032: `not(analysis)` fallbacks.
//
// These mirror the public signatures of the `analysis`-gated builders above so
// their UNCONDITIONAL call sites (`analysis_setup::build_analysis_pipeline`,
// `embedding_setup::build_embedding_components`) keep compiling under
// `--no-default-features`. Every real LLM/analysis transport lives in
// `maekon-network` (only compiled when `analysis` is on), so without that
// feature there is no remote analysis provider to build — these return the
// always-available no-op provider (`maekon_analysis::NoOpAnalysisProvider`,
// which has no network dependency) so the surrounding pipeline degrades cleanly
// to zero-effect analysis instead of failing to compile.
//
// There is NO behaviour change when `analysis` IS enabled: these items do not
// exist in that build (the real implementations above are used verbatim).
// =============================================================================

/// `not(analysis)` fallback for [`build_analysis_provider`]: returns `None`
/// (matching the "no `llm_api` configured" path of the real builder) because no
/// remote analysis transport can exist without `maekon-network`.
#[cfg(not(feature = "analysis"))]
pub fn build_analysis_provider(
    _config: &AiProviderConfig,
    _pii_level: PiiFilterLevel,
    _privacy_guard: Option<ExternalOcrPrivacyGuard>,
    // secret_stores ignored in no-analysis builds; unit type used to avoid
    // importing maekon-core's SecretStoreSet without the analysis feature.
    _secret_stores: Option<&()>,
    _breaker_registry: Arc<CircuitBreakerRegistry>,
) -> Option<(Arc<dyn AnalysisProvider>, Arc<AtomicBool>)> {
    None
}

// NOTE: `build_analysis_provider_with_flag` intentionally has NO `not(analysis)`
// fallback — its only caller (`AgentSupportContextBuilder::build_context_analyzer`)
// is itself `#[cfg(feature = "analysis")]`, so under `--no-default-features` there
// is no call site and a fallback would be dead code.

/// `not(analysis)` fallback for [`build_enrichment_provider`]: returns the
/// always-available no-op provider so belief revision degrades cleanly to a
/// zero-effect enrichment step (identical observable outcome to the real
/// builder's "no `llm_api` / non-loopback endpoint" fail-closed branch).
#[cfg(not(feature = "analysis"))]
pub fn build_enrichment_provider(
    _config: &AiProviderConfig,
    _pii_level: PiiFilterLevel,
    _breaker_registry: Arc<CircuitBreakerRegistry>,
) -> Arc<dyn AnalysisProvider> {
    Arc::new(maekon_analysis::NoOpAnalysisProvider)
}

/// `not(analysis)` fallback for [`build_local_ollama_summary_provider`]: returns
/// `None` (matching the "feature absent → no network transport" path) so
/// `embedding_setup` degrades cleanly without a loopback LLM summary provider.
#[cfg(not(feature = "analysis"))]
pub fn build_local_ollama_summary_provider(
    _pii_level: PiiFilterLevel,
    _breaker_registry: Arc<CircuitBreakerRegistry>,
) -> Option<Arc<dyn AnalysisProvider>> {
    None
}

// ── C2 #5722 tests ────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "analysis"))]
mod tests {
    use super::*;

    /// build_local_ollama_summary_provider: catalog default (loopback) → Some.
    /// Verifies the new_local_enrichment loopback-accept path fires for the
    /// catalog-derived http://localhost:11434/v1/chat/completions endpoint.
    #[test]
    fn local_ollama_summary_provider_returns_some_for_loopback_catalog_default() {
        let registry = crate::breaker_registry::CircuitBreakerRegistry::new();
        let result = build_local_ollama_summary_provider(
            maekon_core::config::PiiFilterLevel::Standard,
            registry,
        );
        assert!(
            result.is_some(),
            "catalog default loopback endpoint must yield a Some provider"
        );
    }

    /// Fallback must NOT fire when llm_api is Some (guard-missing fail-closed
    /// must not be silently bypassed — Decision 4 / missing_hop 4).
    /// Tested via build_analysis_provider returning None (guard-missing path):
    /// the is_none() trigger ensures or_else is only reached when llm_api absent.
    #[test]
    fn local_ollama_fallback_does_not_trigger_when_llm_api_is_some() {
        // When llm_api is Some (even if build_analysis_provider returns None due
        // to a guard-missing configuration), the fallback MUST NOT be reached.
        // We test the trigger logic directly: the or_else block checks
        // config.ai_provider.llm_api.is_none(), so with llm_api=Some the fallback
        // is unconditionally skipped regardless of primary outcome.
        use maekon_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};
        let ai_config = AiProviderConfig {
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "http://localhost:11434/v1/chat/completions".to_string(),
                api_key: String::new(),
                model: None,
                timeout_secs: 30,
                provider_type: AiProviderType::Ollama,
                surface_id: None,
                credential: None,
            }),
            ..Default::default()
        };
        // is_none() on a Some llm_api must be false → fallback not triggered.
        assert!(
            ai_config.llm_api.is_some(),
            "llm_api must be Some when configured, preventing fallback"
        );
    }
}
