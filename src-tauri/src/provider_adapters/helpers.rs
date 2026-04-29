#[cfg(feature = "server")]
use std::sync::Arc;

#[cfg(feature = "server")]
use oneshim_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};
#[cfg(feature = "server")]
use oneshim_core::error::CoreError;
#[cfg(feature = "server")]
use oneshim_network::oauth::provider_config::OAuthProviderConfig;
#[cfg(feature = "server")]
use tracing::warn;

#[cfg(feature = "server")]
use super::types::ProviderSource;

#[cfg(feature = "server")]
pub(super) const DEFAULT_OPENAI_OAUTH_MODEL: &str = "gpt-5.4";

#[cfg(feature = "server")]
pub(super) fn oauth_llm_endpoint(config: &AiProviderConfig) -> ExternalApiEndpoint {
    let mut endpoint = config.llm_api.clone().unwrap_or(ExternalApiEndpoint {
        endpoint: OAuthProviderConfig::OPENAI_API_BASE_URL.to_string(),
        api_key: String::new(),
        model: Some(DEFAULT_OPENAI_OAUTH_MODEL.to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: Some("provider_surface.openai.managed_oauth".to_string()),
        credential: None,
    });

    if endpoint.endpoint.trim().is_empty() {
        endpoint.endpoint = OAuthProviderConfig::OPENAI_API_BASE_URL.to_string();
    }
    if endpoint.timeout_secs == 0 {
        endpoint.timeout_secs = 30;
    }
    if endpoint
        .model
        .as_deref()
        .map(|model| model.trim().is_empty())
        .unwrap_or(true)
    {
        endpoint.model = Some(DEFAULT_OPENAI_OAUTH_MODEL.to_string());
    }
    endpoint.provider_type = AiProviderType::OpenAi;
    endpoint.api_key.clear();
    endpoint
}

#[cfg(feature = "server")]
pub(super) fn require_endpoint_config<'a>(
    endpoint: Option<&'a ExternalApiEndpoint>,
    field_name: &str,
) -> Result<&'a ExternalApiEndpoint, CoreError> {
    let endpoint = endpoint.ok_or_else(|| CoreError::Config {
        code: oneshim_core::error_codes::ConfigCode::Missing,
        message: format!("Remote AI provider usage requires `{field_name}` to be configured."),
    })?;

    if endpoint.endpoint.trim().is_empty() {
        return Err(CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}.endpoint` must not be empty."),
        });
    }
    if !(endpoint.endpoint.starts_with("http://") || endpoint.endpoint.starts_with("https://")) {
        return Err(CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::Invalid,
            message: format!("`{field_name}.endpoint` must be an http:// or https:// URL."),
        });
    }
    if endpoint.timeout_secs == 0 {
        return Err(CoreError::Config {
            code: oneshim_core::error_codes::ConfigCode::OutOfRange,
            message: format!("`{field_name}.timeout_secs` must be greater than 0."),
        });
    }

    Ok(endpoint)
}

#[cfg(feature = "server")]
pub(super) fn resolve_remote_with_optional_fallback<T: ?Sized>(
    provider_kind: &str,
    fallback_to_local: bool,
    remote_builder: impl FnOnce() -> Result<Arc<T>, CoreError>,
    local_builder: impl FnOnce() -> Arc<T>,
) -> Result<(Arc<T>, ProviderSource, Option<String>), CoreError> {
    match remote_builder() {
        Ok(provider) => Ok((provider, ProviderSource::Remote, None)),
        Err(err) if fallback_to_local => {
            let fallback_reason = format_fallback_reason(&err);
            warn!(
                provider = provider_kind,
                error = %err,
                fallback_reason = %fallback_reason,
                "Remote provider initialization failed, falling back to the local provider"
            );
            Ok((
                local_builder(),
                ProviderSource::LocalFallback,
                Some(fallback_reason),
            ))
        }
        Err(err) => Err(err),
    }
}

#[cfg(feature = "server")]
const MAX_FALLBACK_REASON_CHARS: usize = 240;

#[cfg(feature = "server")]
fn format_fallback_reason(err: &CoreError) -> String {
    let raw = err.to_string().replace(['\n', '\r'], " ");
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_FALLBACK_REASON_CHARS {
        return normalized;
    }

    let truncated: String = normalized.chars().take(MAX_FALLBACK_REASON_CHARS).collect();
    format!("{truncated}...")
}
