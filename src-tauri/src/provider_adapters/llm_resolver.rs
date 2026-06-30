use std::sync::Arc;

use maekon_api_contracts::provider_specs::provider_surface_spec;
use maekon_automation::local_llm::LocalLlmProvider;
use maekon_core::config::{AiProviderConfig, AiProviderType, LlmProviderType};
use maekon_core::error::CoreError;
#[cfg(feature = "analysis")]
use maekon_core::ports::credential_source::CredentialSource;
use maekon_core::ports::llm_provider::LlmProvider;
#[cfg(feature = "analysis")]
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::secret_store::SecretStoreSet;
use maekon_core::provider_surface::{provider_type_from_vendor_id, provider_vendor_id_or_default};
// Migrated from `server` to `analysis`: `analysis = [dep:maekon-network]` is
// default-on; `server` implies `analysis`, so `RemoteLlmProvider` is
// available in all four CI cells (default, server, grpc, --no-default-features).
// Previously gated on `server`, which silently excluded the standalone build.
#[cfg(feature = "analysis")]
use maekon_network::ai_llm_client::RemoteLlmProvider;
#[cfg(feature = "analysis")]
use tracing::debug;
use tracing::warn;

use super::guarded_llm::GuardedLlmProvider;
#[cfg(feature = "analysis")]
use super::helpers::{
    oauth_llm_endpoint, require_endpoint_config, resolve_remote_with_optional_fallback,
};
use super::types::{
    ensure_non_external_endpoints_are_loopback, ExternalOcrPrivacyGuard, LlmProviderResolution,
    ProviderSource,
};
#[cfg(feature = "analysis")]
use crate::oauth_provider_registry::{
    managed_oauth_provider_id_for_endpoint, managed_oauth_transport_url_for_endpoint,
};
use crate::subprocess_provider::{
    cli_id_for_surface_id, preferred_cli_surface_for_config, probe_for_surface_id,
    runtime_supported_for_surface, select_cli_surface_for_config, ProbedSubprocessCli,
    SubprocessCliAuthStatus, SubprocessLlmProvider,
};

fn guard_external_llm_provider(
    provider: Arc<dyn LlmProvider>,
    privacy_guard: Option<ExternalOcrPrivacyGuard>,
) -> Result<Arc<dyn LlmProvider>, CoreError> {
    if !provider.is_external() {
        ensure_non_external_endpoints_are_loopback(
            provider.provider_name(),
            provider.egress_endpoint_urls(),
        )?;
        return Ok(provider);
    }

    let guard = privacy_guard.ok_or_else(|| CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Missing,
        message: "External LLM provider requires a runtime privacy guard.".to_string(),
    })?;
    Ok(Arc::new(GuardedLlmProvider::new(provider, guard)) as Arc<dyn LlmProvider>)
}

pub(super) fn resolve_cli_subscription_llm_provider_with_detected(
    config: &AiProviderConfig,
    privacy_guard: Option<ExternalOcrPrivacyGuard>,
    detected: &[ProbedSubprocessCli],
) -> LlmProviderResolution {
    if let Some(surface) = select_cli_surface_for_config(config, detected) {
        let provider =
            Arc::new(SubprocessLlmProvider::new(surface, config)) as Arc<dyn LlmProvider>;
        let provider = match guard_external_llm_provider(provider, privacy_guard) {
            Ok(provider) => provider,
            Err(error) if config.fallback_to_local => {
                let reason = error.to_string();
                warn!(
                    fallback_reason = %reason,
                    "CLI subscription LLM privacy guard unavailable, falling back to local rule-based LLM"
                );
                return Ok((
                    Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
                    ProviderSource::LocalFallback,
                    Some(reason),
                ));
            }
            Err(error) => return Err(error),
        };
        return Ok((provider, ProviderSource::CliSubscription, None));
    }

    let reason = cli_subscription_unavailable_reason(config, detected);
    if config.fallback_to_local {
        warn!(
            fallback_reason = %reason,
            "CLI subscription runtime unavailable, falling back to local rule-based LLM"
        );
        return Ok((
            Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
            ProviderSource::LocalFallback,
            Some(reason),
        ));
    }

    Err(CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Invalid,
        message: reason,
    })
}

fn cli_subscription_unavailable_reason(
    config: &AiProviderConfig,
    detected: &[ProbedSubprocessCli],
) -> String {
    if let Some(provider_type) = config
        .llm_api
        .as_ref()
        .map(|endpoint| endpoint.provider_type)
        .filter(|provider_type| *provider_type != AiProviderType::Generic)
    {
        if let Some(surface_id) = preferred_cli_surface_for_config(config) {
            if let Some(surface) = probe_for_surface_id(detected, &surface_id) {
                return match surface.auth_status {
                    SubprocessCliAuthStatus::Authenticated => format!(
                        "Installed {} CLI was detected but the runtime adapter could not be selected.",
                        cli_id_for_surface_id(&surface.detected.surface_id)
                            .unwrap_or_else(|_| surface.detected.surface_id.clone())
                    ),
                    SubprocessCliAuthStatus::Unauthenticated => format!(
                        "Installed {} CLI is not authenticated. Sign in through the provider-owned CLI first.",
                        cli_id_for_surface_id(&surface.detected.surface_id)
                            .unwrap_or_else(|_| surface.detected.surface_id.clone())
                    ),
                    SubprocessCliAuthStatus::Unsupported => format!(
                        "Installed {} CLI is authenticated but the active plan or provider mode does not support headless CLI use.",
                        cli_id_for_surface_id(&surface.detected.surface_id)
                            .unwrap_or_else(|_| surface.detected.surface_id.clone())
                    ),
                    SubprocessCliAuthStatus::StaleSession => format!(
                        "Installed {} CLI session appears expired or stale. Refresh the provider-owned CLI sign-in first.",
                        cli_id_for_surface_id(&surface.detected.surface_id)
                            .unwrap_or_else(|_| surface.detected.surface_id.clone())
                    ),
                    SubprocessCliAuthStatus::InteractiveRequired => format!(
                        "Installed {} CLI requires an interactive sign-in step before Maekon can use it headlessly.",
                        cli_id_for_surface_id(&surface.detected.surface_id)
                            .unwrap_or_else(|_| surface.detected.surface_id.clone())
                    ),
                    SubprocessCliAuthStatus::Unknown => format!(
                        "Installed {} CLI was detected, but authentication status could not be verified.",
                        cli_id_for_surface_id(&surface.detected.surface_id)
                            .unwrap_or_else(|_| surface.detected.surface_id.clone())
                    ),
                };
            }
        }

        let provider_label = provider_vendor_id_or_default(provider_type);
        if let Some(reason) =
            unsupported_detected_cli_reason(provider_type, detected, provider_label)
        {
            return reason;
        }

        return format!(
            "No supported installed CLI runtime was detected for provider '{provider_label}'."
        );
    }

    if detected
        .iter()
        .any(|surface| !runtime_supported_for_surface(&surface.detected.surface_id))
    {
        return "Detected provider CLI executables do not yet have a supported runtime adapter."
            .to_string();
    }

    "No supported provider CLI runtime was detected on PATH (checked: codex, claude, claude-code, gemini, antigravity)."
        .to_string()
}

fn unsupported_detected_cli_reason(
    provider_type: AiProviderType,
    detected: &[ProbedSubprocessCli],
    provider_label: &str,
) -> Option<String> {
    detected.iter().find_map(|surface| {
        let spec = provider_surface_spec(&surface.detected.surface_id).ok()?;
        let surface_provider_type = provider_type_from_vendor_id(&spec.vendor_id)?;
        if surface_provider_type != provider_type
            || runtime_supported_for_surface(&surface.detected.surface_id)
        {
            return None;
        }

        let cli_id = cli_id_for_surface_id(&surface.detected.surface_id)
            .unwrap_or_else(|_| surface.detected.surface_id.clone());
        Some(format!(
            "Installed {cli_id} CLI was detected for provider '{provider_label}', but '{}' is manual/GUI-only or has no stable headless runtime adapter.",
            spec.display_name
        ))
    })
}

#[allow(unused_variables)]
pub(super) fn resolve_llm_provider(
    config: &AiProviderConfig,
    privacy_guard: Option<ExternalOcrPrivacyGuard>,
    secret_stores: Option<SecretStoreSet>,
    // D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    // registry threaded from the composition root.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    // Per-call LLM health handle forwarded to the Local arm (FU-3). `None` is safe.
    llm_call_health: Option<Arc<maekon_core::ports::llm_provider::LlmCallHealth>>,
) -> LlmProviderResolution {
    match config.llm_provider {
        // FU-3 (#5741): the explicit "Local" LLM choice in ProviderApiKey mode
        // now routes through the same Ollama-backed resolver as LocalModel mode
        // (C3, #5728) — real local LLM with the rule matcher as runtime
        // fallback. Ok-degrade invariant holds: this arm never returns Err.
        LlmProviderType::Local => resolve_local_model_llm_provider(
            config,
            privacy_guard.clone(),
            breaker_registry.clone(),
            // #5734: forward the per-call health handle to FU-3 Local arm
            // (same Ollama-backed path as LocalModel/C3).
            llm_call_health,
        ),
        LlmProviderType::Remote => {
            #[cfg(feature = "analysis")]
            {
                resolve_remote_with_optional_fallback(
                    "llm",
                    config.fallback_to_local,
                    || {
                        let endpoint = require_endpoint_config(config.llm_api.as_ref(), "llm_api")?;
                        let secret_store = secret_stores
                            .as_ref()
                            .and_then(|stores| stores.for_binding(endpoint.credential.as_ref()));
                        let profile_id = config.active_secret_profile_id_or("llm");
                        let credential = CredentialSource::from_api_key_endpoint_for_profile(
                            endpoint,
                            Some(profile_id),
                            secret_store,
                        )?;
                        let provider = Arc::new(RemoteLlmProvider::new_with_credential(
                            endpoint,
                            credential,
                            // D7 (#4812 / E20-20): shared workspace-wide registry
                            // from the composition root (iter-011 done).
                            breaker_registry.clone(),
                        )?) as Arc<dyn LlmProvider>;
                        guard_external_llm_provider(provider, privacy_guard.clone())
                    },
                    || Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
                )
            }
            #[cfg(not(feature = "analysis"))]
            {
                // Iter-109: feature-gate disabled = service unavailable in
                // this build (iter-108 pattern). User's config isn't
                // "invalid" per se; it's fine in a build with the feature.
                Err(CoreError::ServiceUnavailable {
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
                    message: "Remote LLM provider requires the 'analysis' feature".to_string(),
                })
            }
        }
    }
}

/// Resolve LLM provider using OAuth-managed credentials.
///
/// Uses surface metadata to resolve the provider ID and authenticated request
/// URL. OpenAI keeps a default managed surface fallback when `llm_api` is omitted.
#[cfg(feature = "analysis")]
pub(super) fn resolve_llm_provider_oauth(
    config: &AiProviderConfig,
    privacy_guard: Option<ExternalOcrPrivacyGuard>,
    oauth_port: Arc<dyn OAuthPort>,
    // D7 (#4812 / E20-20): shared workspace-wide registry from the composition root.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
) -> LlmProviderResolution {
    let endpoint = oauth_llm_endpoint(config);
    let provider_id = managed_oauth_provider_id_for_endpoint(
        &endpoint,
        maekon_api_contracts::provider_specs::ProviderTransportKind::Llm,
    )?;
    let api_base_url = managed_oauth_transport_url_for_endpoint(
        &endpoint,
        maekon_api_contracts::provider_specs::ProviderTransportKind::Llm,
    )?;
    let credential = CredentialSource::ManagedOAuth {
        provider_id,
        oauth_port,
        api_base_url,
    };

    let provider = RemoteLlmProvider::new_with_credential(
        &endpoint,
        credential,
        // D7 (#4812 / E20-20): shared workspace-wide registry (iter-011 done).
        breaker_registry,
    )?;
    let provider =
        guard_external_llm_provider(Arc::new(provider) as Arc<dyn LlmProvider>, privacy_guard)?;
    debug!(
        model = %provider.provider_name(),
        "LLM provider resolved with OAuth credential"
    );
    Ok((provider, ProviderSource::OAuth, None))
}

// ── LocalModel arm: Ollama-backed resolver ────────────────────────────────────

/// Resolve an LLM provider for `AiAccessMode::LocalModel`.
///
/// # Decision 1 — Ok-degrade invariant
/// This function NEVER returns `Err`.  Any construction failure (config,
/// lifecycle policy, unsupported surface) is caught here and degrades to
/// `(LocalLlmProvider, ProviderSource::Local, Some(reason))`.  Returning `Err`
/// from the LocalModel arm would trigger `should_fallback_to_noop` in
/// `automation_controller_builder`, replacing the *entire* controller (OCR +
/// presets + everything) with a noop — unacceptable for a user who simply has
/// no Ollama running.
///
/// # Decision 2 — health flag
/// When the caller supplies a shared `AtomicBool`, it is attached via
/// `RemoteLlmProvider::with_health_flag` (per-call success/failure observable).
/// The production call site currently passes `None`: exposing LLM health in
/// `AiRuntimeStatus` needs a new field on the contract-frozen status DTO
/// (openapi + http-interface-manifest), which is a separate slice. Until then
/// the per-call honesty signal is the structured `warn!(err.code, ...)` emitted
/// by `FallbackLlmProvider` on every fallback.
///
/// # Decision 3 — 30s timeout
/// Connection-refused is instant, so daemon-down UX is already snappy.  The
/// generous timeout covers daemon-UP + cold model load (10-60s) + qwen3 CPU
/// inference.  A 5s timeout would open the circuit breaker on the first request
/// and permanently fall back to the rule matcher on slow hardware.
///
/// # Decision 4 — token budget
/// `build_responses_api_body` defaults to `max_output_tokens=512`; qwen3 emits
/// thinking tokens that can consume that entire budget before the JSON output,
/// leaving an empty parse and a permanent silent fallback.  This resolver raises
/// the budget via `RemoteLlmProvider::with_token_budget(2048)` for the Ollama
/// intent path only — other providers keep the 512 default.
///
/// # Privacy semantics
/// Loopback Ollama is NOT external egress (precedent: feature_capabilities.rs:
/// 689-716, `host_is_loopback`).  The guard chain (sensitive-app, consent,
/// PII filter) is deliberately skipped for loopback, matching the in-process
/// rule-matcher posture.  Non-loopback Ollama (LAN) routes through
/// `guard_external_llm_provider` for full consent + PII filtering.
///
/// The process boundary delta (raw visible_texts cross into the Ollama daemon
/// log on loopback) is documented here and emitted as an `info!` at runtime on
/// first use.
#[cfg(feature = "analysis")]
pub(super) fn resolve_local_model_llm_provider(
    config: &AiProviderConfig,
    privacy_guard: Option<super::types::ExternalOcrPrivacyGuard>,
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    // Decision 2 (#5734): optional tri-state per-call health handle wired into
    // AutomationStatusDto.llm_healthy.  Pass Some(handle) from the builder to
    // reflect real Ollama call outcomes; None is safe (no tracking).
    llm_call_health: Option<Arc<maekon_core::ports::llm_provider::LlmCallHealth>>,
) -> LlmProviderResolution {
    use super::fallback_llm::{FallbackLlmProvider, LoopbackLlmProvider};
    use super::types::{endpoint_is_loopback, ProviderSource};
    use maekon_api_contracts::provider_specs::{
        resolved_default_model, resolved_transport_spec, ProviderTransportKind,
        SurfaceCapabilityKind,
    };
    use maekon_core::config::{AiProviderType, ExternalApiEndpoint};

    const OLLAMA_SURFACE: &str = "provider_surface.ollama.local_http";
    // Decision 3: generous per-call timeout — covers cold model load + CPU inference.
    const INTENT_TIMEOUT_SECS: u64 = 30;

    // ── Build the Ollama intent endpoint ──────────────────────────────────────
    //
    // Derive base_url + model from C2's config accessor.  Falls back to the
    // catalog (http://localhost:11434/v1/responses + qwen3:8b) when no Ollama
    // llm_api is configured.
    let target = crate::session_manager::factory::resolve_local_llm_target(config);

    // Resolve catalog transport path for the Ollama surface.
    let transport_path = match resolved_transport_spec(
        AiProviderType::Ollama,
        Some(OLLAMA_SURFACE),
        ProviderTransportKind::Llm,
    ) {
        Ok(spec) => {
            // Strip the base from the catalog URL to get the path only.
            // Catalog URL: http://localhost:11434/v1/responses
            // We want: /v1/responses
            // Then re-join with the C2-resolved base_url.
            let catalog_url = &spec.url;
            // Find the path component by stripping scheme://host:port prefix.
            let path = if let Ok(parsed) = reqwest::Url::parse(catalog_url) {
                parsed.path().to_string()
            } else {
                "/v1/responses".to_string()
            };
            path
        }
        Err(_) => "/v1/responses".to_string(),
    };

    let intent_endpoint_url = format!(
        "{base}{path}",
        base = target.base_url.trim_end_matches('/'),
        path = transport_path
    );
    let is_loopback = endpoint_is_loopback(&intent_endpoint_url);

    if intent_endpoint_url.starts_with("http://") && !is_loopback {
        let reason = format!(
            "LocalModel Ollama endpoint uses cleartext http:// to a non-loopback host; \
             refusing remote cleartext egress: {intent_endpoint_url}"
        );
        warn!(
            endpoint = %intent_endpoint_url,
            "LocalModel remote cleartext Ollama endpoint rejected, falling back to local rule-based LLM"
        );
        return Ok((
            Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
            ProviderSource::Local,
            Some(reason),
        ));
    }

    // Resolve default model from catalog when not set in config.
    let model = target.default_model.clone().or_else(|| {
        resolved_default_model(
            AiProviderType::Ollama,
            Some(OLLAMA_SURFACE),
            SurfaceCapabilityKind::Llm,
        )
        .ok()
        .flatten()
    });

    // ── Construct RemoteLlmProvider (Ok-degrade on any error) ─────────────────
    let endpoint = ExternalApiEndpoint {
        endpoint: intent_endpoint_url.clone(),
        api_key: String::new(), // NoAuth: Ollama credential_kind=none
        model,
        timeout_secs: INTENT_TIMEOUT_SECS,
        provider_type: AiProviderType::Ollama,
        surface_id: Some(OLLAMA_SURFACE.to_string()),
        credential: None,
    };

    let remote_provider = match RemoteLlmProvider::new(&endpoint, breaker_registry) {
        Ok(p) => {
            // Decision 4: raise token budget for Ollama intent path.
            // qwen3 emits thinking tokens that can exhaust the default 512-token
            // cap before the JSON output is emitted, producing an empty parse
            // and a permanent silent fallback to the rule matcher.
            let p = p.with_token_budget(2048);
            // Decision 2 (#5734): attach tri-state health handle when provided.
            if let Some(handle) = llm_call_health {
                p.with_call_health(handle)
            } else {
                p
            }
        }
        Err(e) => {
            let reason = format!("Ollama LLM provider construction failed: {e}");
            warn!(
                err = %e,
                "LocalModel Ollama provider construction failed, falling back to local rule-based LLM"
            );
            return Ok((
                Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
                ProviderSource::Local,
                Some(reason),
            ));
        }
    };

    // ── Loopback check + optional egress guard ────────────────────────────────
    //
    // Decision 6: fail-closed classifier (maekon_network::http_client::
    // host_is_loopback).  Unparseable host → treated as external.
    //
    // Loopback: no guard required (same posture as in-process rule matcher).
    // Non-loopback (LAN Ollama): route through guard_external_llm_provider.
    if is_loopback {
        // First-use log: document the process-boundary privacy delta.
        tracing::info!(
            endpoint = %intent_endpoint_url,
            "LocalModel: routing intent planning to loopback Ollama — screen context \
             will cross the process boundary into the Ollama daemon (which may \
             persist prompts in its logs). This is intentional for privacy-first \
             local-LLM use. No external egress occurs."
        );
    }

    let primary: Arc<dyn LlmProvider> = if is_loopback {
        // Wrap in LoopbackLlmProvider to override is_external()=false.
        // RemoteLlmProvider::is_external() is hard-true; loopback is not egress.
        Arc::new(LoopbackLlmProvider::new(Arc::new(remote_provider)))
    } else {
        // Non-loopback Ollama: wrap in guard (consent + PII filter).
        match guard_external_llm_provider(
            Arc::new(remote_provider) as Arc<dyn LlmProvider>,
            privacy_guard,
        ) {
            Ok(guarded) => guarded,
            Err(e) => {
                // Missing privacy guard for non-loopback host — Ok-degrade.
                let reason = format!("non-loopback Ollama requires privacy guard: {e}");
                warn!(
                    err.code = %e.code(),
                    endpoint = %intent_endpoint_url,
                    "LocalModel non-loopback Ollama guard unavailable, falling back to local rule-based LLM"
                );
                return Ok((
                    Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
                    ProviderSource::Local,
                    Some(reason),
                ));
            }
        }
    };

    // ── Compose FallbackLlmProvider ───────────────────────────────────────────
    let fallback: Arc<dyn LlmProvider> = Arc::new(LocalLlmProvider::new());
    let composite = FallbackLlmProvider::new(primary, fallback);

    Ok((
        Arc::new(composite) as Arc<dyn LlmProvider>,
        ProviderSource::LocalOllama,
        None,
    ))
}

/// `not(analysis)` twin: when the analysis feature is off, fall back to the
/// rule matcher with a reason string.  Maintains infallibility (Ok-degrade).
#[cfg(not(feature = "analysis"))]
pub(super) fn resolve_local_model_llm_provider(
    _config: &AiProviderConfig,
    _privacy_guard: Option<super::types::ExternalOcrPrivacyGuard>,
    _breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    // #5734: health handle unused when analysis feature is off; parameter kept for
    // ABI parity with the cfg(analysis) twin.
    _llm_call_health: Option<Arc<maekon_core::ports::llm_provider::LlmCallHealth>>,
) -> LlmProviderResolution {
    Ok((
        Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
        ProviderSource::Local,
        Some("Ollama intent path requires the 'analysis' feature (default-on)".to_string()),
    ))
}
