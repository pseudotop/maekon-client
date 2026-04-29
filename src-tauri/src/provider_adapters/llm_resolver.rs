use std::sync::Arc;

use oneshim_automation::local_llm::LocalLlmProvider;
use oneshim_core::config::{AiProviderConfig, AiProviderType, LlmProviderType};
use oneshim_core::error::CoreError;
#[cfg(feature = "server")]
use oneshim_core::ports::credential_source::CredentialSource;
use oneshim_core::ports::llm_provider::LlmProvider;
#[cfg(feature = "server")]
use oneshim_core::ports::oauth::OAuthPort;
use oneshim_core::ports::secret_store::SecretStoreSet;
use oneshim_core::provider_surface::provider_vendor_id_or_default;
#[cfg(feature = "server")]
use oneshim_network::ai_llm_client::RemoteLlmProvider;
#[cfg(feature = "server")]
use tracing::debug;
use tracing::warn;

#[cfg(feature = "server")]
use super::helpers::{
    oauth_llm_endpoint, require_endpoint_config, resolve_remote_with_optional_fallback,
};
use super::types::{LlmProviderResolution, ProviderSource};
#[cfg(feature = "server")]
use crate::oauth_provider_registry::{
    managed_oauth_provider_id_for_endpoint, managed_oauth_transport_url_for_endpoint,
};
use crate::subprocess_provider::{
    cli_id_for_surface_id, preferred_cli_surface_for_config, probe_for_surface_id,
    runtime_supported_for_surface, select_cli_surface_for_config, ProbedSubprocessCli,
    SubprocessCliAuthStatus, SubprocessLlmProvider,
};

pub(super) fn resolve_cli_subscription_llm_provider_with_detected(
    config: &AiProviderConfig,
    detected: &[ProbedSubprocessCli],
) -> LlmProviderResolution {
    if let Some(surface) = select_cli_surface_for_config(config, detected) {
        return Ok((
            Arc::new(SubprocessLlmProvider::new(surface, config)) as Arc<dyn LlmProvider>,
            ProviderSource::CliSubscription,
            None,
        ));
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
        code: oneshim_core::error_codes::ConfigCode::Invalid,
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
                    SubprocessCliAuthStatus::Unknown => format!(
                        "Installed {} CLI was detected, but authentication status could not be verified.",
                        cli_id_for_surface_id(&surface.detected.surface_id)
                            .unwrap_or_else(|_| surface.detected.surface_id.clone())
                    ),
                };
            }
        }

        let provider_label = provider_vendor_id_or_default(provider_type);

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

    "No supported provider CLI runtime was detected on PATH (checked: codex, claude, claude-code, gemini)."
        .to_string()
}

#[allow(unused_variables)]
pub(super) fn resolve_llm_provider(
    config: &AiProviderConfig,
    secret_stores: Option<SecretStoreSet>,
) -> LlmProviderResolution {
    match config.llm_provider {
        LlmProviderType::Local => Ok((
            Arc::new(LocalLlmProvider::new()),
            ProviderSource::Local,
            None,
        )),
        LlmProviderType::Remote => {
            #[cfg(feature = "server")]
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
                        Ok(Arc::new(RemoteLlmProvider::new_with_credential(
                            endpoint,
                            credential,
                            // D7: per-adapter registry; iter-011 will consolidate.
                            oneshim_network::CircuitBreakerRegistry::new(),
                        )?) as Arc<dyn LlmProvider>)
                    },
                    || Arc::new(LocalLlmProvider::new()) as Arc<dyn LlmProvider>,
                )
            }
            #[cfg(not(feature = "server"))]
            {
                // Iter-109: feature-gate disabled = service unavailable in
                // this build (iter-108 pattern). User's config isn't
                // "invalid" per se; it's fine in a build with the feature.
                Err(CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "Remote LLM provider requires the 'server' feature".to_string(),
                })
            }
        }
    }
}

/// Resolve LLM provider using OAuth-managed credentials.
///
/// Uses surface metadata to resolve the provider ID and authenticated request
/// URL. OpenAI keeps a default managed surface fallback when `llm_api` is omitted.
#[cfg(feature = "server")]
pub(super) fn resolve_llm_provider_oauth(
    config: &AiProviderConfig,
    oauth_port: Arc<dyn OAuthPort>,
) -> LlmProviderResolution {
    let endpoint = oauth_llm_endpoint(config);
    let provider_id = managed_oauth_provider_id_for_endpoint(
        &endpoint,
        oneshim_api_contracts::provider_specs::ProviderTransportKind::Llm,
    )?;
    let api_base_url = managed_oauth_transport_url_for_endpoint(
        &endpoint,
        oneshim_api_contracts::provider_specs::ProviderTransportKind::Llm,
    )?;
    let credential = CredentialSource::ManagedOAuth {
        provider_id,
        oauth_port,
        api_base_url,
    };

    let provider = RemoteLlmProvider::new_with_credential(
        &endpoint,
        credential,
        // D7: per-adapter registry; iter-011 will consolidate.
        oneshim_network::CircuitBreakerRegistry::new(),
    )?;
    debug!(
        model = %provider.provider_name(),
        "LLM provider resolved with OAuth credential"
    );
    Ok((
        Arc::new(provider) as Arc<dyn LlmProvider>,
        ProviderSource::OAuth,
        None,
    ))
}
