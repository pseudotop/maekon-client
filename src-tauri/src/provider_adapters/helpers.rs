#[cfg(feature = "analysis")]
use std::sync::Arc;

#[cfg(feature = "analysis")]
use maekon_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};
#[cfg(feature = "analysis")]
use maekon_core::error::CoreError;
#[cfg(feature = "analysis")]
use maekon_network::oauth::provider_config::OAuthProviderConfig;
#[cfg(feature = "analysis")]
use tracing::warn;

#[cfg(feature = "analysis")]
use super::types::ProviderSource;

#[cfg(feature = "analysis")]
pub(super) const DEFAULT_OPENAI_OAUTH_MODEL: &str = "gpt-5.4";

#[cfg(feature = "analysis")]
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

#[cfg(feature = "analysis")]
pub(super) fn require_endpoint_config<'a>(
    endpoint: Option<&'a ExternalApiEndpoint>,
    field_name: &str,
) -> Result<&'a ExternalApiEndpoint, CoreError> {
    let endpoint = endpoint.ok_or_else(|| CoreError::Config {
        code: maekon_core::error_codes::ConfigCode::Missing,
        message: format!("Remote AI provider usage requires `{field_name}` to be configured."),
    })?;

    if endpoint.endpoint.trim().is_empty() {
        return Err(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}.endpoint` must not be empty."),
        });
    }
    if !(endpoint.endpoint.starts_with("http://") || endpoint.endpoint.starts_with("https://")) {
        return Err(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: format!("`{field_name}.endpoint` must be an http:// or https:// URL."),
        });
    }
    // #6259: fail-closed cleartext gate. This is THE chokepoint for the BYOK
    // direct-API resolver (resolve_llm_provider / resolve_ocr_provider Remote
    // arms + resolve_ocr_provider_oauth). A remote `http://` endpoint would send
    // the Bearer API key + screen-context/image payload in cleartext to a
    // non-loopback host — the same exposure that `build_reqwest_client_for_url`
    // (REST) and `SseStreamClient::validated_base_url` (SSE) already reject, and
    // that the sibling Ollama LocalModel arm gates via loopback. `http://` is
    // permitted only for loopback (local self-hosted models); HTTPS is always
    // allowed. `endpoint_is_loopback` is fail-closed: an unparseable/missing host
    // is treated as external.
    if endpoint.endpoint.starts_with("http://")
        && !super::types::endpoint_is_loopback(&endpoint.endpoint)
    {
        return Err(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::Invalid,
            message: format!(
                "`{field_name}.endpoint` uses cleartext http:// to a non-loopback host; \
                 a remote AI endpoint must use https:// (cleartext is allowed only for \
                 loopback/local providers) to avoid leaking the API key and screen context."
            ),
        });
    }
    if endpoint.timeout_secs == 0 {
        return Err(CoreError::Config {
            code: maekon_core::error_codes::ConfigCode::OutOfRange,
            message: format!("`{field_name}.timeout_secs` must be greater than 0."),
        });
    }

    Ok(endpoint)
}

#[cfg(feature = "analysis")]
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

#[cfg(feature = "analysis")]
const MAX_FALLBACK_REASON_CHARS: usize = 240;

#[cfg(feature = "analysis")]
fn format_fallback_reason(err: &CoreError) -> String {
    let raw = err.to_string().replace(['\n', '\r'], " ");
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= MAX_FALLBACK_REASON_CHARS {
        return normalized;
    }

    let truncated: String = normalized.chars().take(MAX_FALLBACK_REASON_CHARS).collect();
    format!("{truncated}...")
}
