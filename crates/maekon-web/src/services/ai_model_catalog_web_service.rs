use std::time::Duration;

use maekon_api_contracts::ai_providers::{ProviderModelsRequest, ProviderModelsResponse};
use maekon_core::ports::provider_model_catalog::{
    ProviderModelCatalogError, ProviderModelCatalogHeader, ProviderModelCatalogRequest,
};

use crate::error::ApiError;
use crate::services::ai_model_catalog_assembler::{build_model_details, parse_models};
use crate::services::ai_model_catalog_auth::resolve_model_discovery_api_key;
use crate::services::ai_model_catalog_endpoint::{
    normalize_optional_surface_id, reject_internal_discovery_endpoint, resolve_models_endpoint,
    resolve_requested_provider_type,
};
use crate::services::ai_model_catalog_service::truncate_error;
use crate::services::ai_provider_spec_service::{self, ProviderAuthScheme};
use crate::services::web_contexts::AiModelCatalogWebContext;

const MODEL_DISCOVERY_TIMEOUT_SECS: u64 = 20;
const ANTHROPIC_API_VERSION: &str = "2023-06-01";

#[derive(Clone)]
pub struct AiModelCatalogQueryService {
    ctx: AiModelCatalogWebContext,
}

impl AiModelCatalogQueryService {
    pub fn new(ctx: AiModelCatalogWebContext) -> Self {
        Self { ctx }
    }

    pub async fn discover_provider_models(
        &self,
        request: &ProviderModelsRequest,
    ) -> Result<ProviderModelsResponse, ApiError> {
        // The local (loopback) path proceeds without a host pin (empty vector) — it must allow
        // legitimate internal endpoints such as localhost Ollama, so it is not subject to the
        // SSRF guard/pinning (#6894/#6902).
        self.discover_with_pinned_addrs(request, Vec::new()).await
    }

    /// Shared discovery implementation. When `pinned_addrs` is non-empty, the transport pins the
    /// endpoint host to those addresses to prevent re-resolution (#6902 — addresses validated by
    /// the integration SSRF guard).
    async fn discover_with_pinned_addrs(
        &self,
        request: &ProviderModelsRequest,
        pinned_addrs: Vec<std::net::SocketAddr>,
    ) -> Result<ProviderModelsResponse, ApiError> {
        let requested_surface_id = normalize_optional_surface_id(request.surface_id.as_deref());
        let provider_type = resolve_requested_provider_type(
            request.provider_type.as_str(),
            requested_surface_id.as_deref(),
        )?;
        let endpoint = resolve_models_endpoint(
            provider_type,
            requested_surface_id.as_deref(),
            request.endpoint.as_deref(),
        )?;
        let auth_scheme = ai_provider_spec_service::model_catalog_auth_scheme_for_surface(
            provider_type,
            requested_surface_id.as_deref(),
        )?;
        // AWS Bedrock intentionally unsupported per ADR-019 §3. Return early
        // BEFORE resolving AWS credentials so users without keys see the graceful
        // "unsupported" notice instead of a generic "no API key" error.
        if matches!(auth_scheme, ProviderAuthScheme::AwsSignatureV4) {
            return Ok(ProviderModelsResponse {
                models: Vec::new(),
                model_details: Vec::new(),
                notice: Some("AWS Bedrock is intentionally unsupported in this build.".to_string()),
            });
        }
        let api_key = if matches!(auth_scheme, ProviderAuthScheme::None) {
            None
        } else {
            Some(resolve_model_discovery_api_key(request, &self.ctx, provider_type).await?)
        };
        if let Some(notice) = ai_provider_spec_service::ocr_model_catalog_notice_for_surface(
            provider_type,
            requested_surface_id.as_deref(),
            &endpoint,
        )? {
            return Ok(ProviderModelsResponse {
                models: Vec::new(),
                model_details: Vec::new(),
                notice: Some(notice),
            });
        }

        let transport = self.ctx.model_catalog_client.as_ref().ok_or_else(|| {
            ApiError::ServiceUnavailable("Model discovery transport is not configured.".to_string())
        })?;
        let response = transport
            .fetch_models(ProviderModelCatalogRequest {
                endpoint: endpoint.clone(),
                headers: model_catalog_headers(auth_scheme, api_key.as_deref()),
                timeout: Duration::from_secs(MODEL_DISCOVERY_TIMEOUT_SECS),
                // #6902: on the integration path, pass the addresses resolved and validated by
                // the SSRF guard as pins (empty means no pin — preserves the local path's
                // existing behavior).
                resolved_addrs: pinned_addrs,
            })
            .await
            .map_err(model_catalog_error_to_api)?;

        let status = response.status;
        let body = response.body;
        if !(200..300).contains(&status) {
            let message = format!(
                "Model discovery failed ({}): {}",
                status,
                truncate_error(&body)
            );
            // Semantic ApiError mapping per iter-54..59 pattern (ApiError
            // variants are web-layer HTTP status equivalents).
            return Err(match status {
                400 => ApiError::BadRequest(message),
                401 => ApiError::Unauthorized(message),
                403 => ApiError::Forbidden(message),
                404 => ApiError::NotFound(message),
                // 408/429/502/503/504 all represent transient or retry-worthy
                // upstream failures — map to ServiceUnavailable (ApiError has
                // no dedicated TooManyRequests/Timeout variants).
                408 | 429 | 502 | 503 | 504 => ApiError::ServiceUnavailable(message),
                _ => ApiError::Internal(message),
            });
        }

        let mut discovered_models = parse_models(
            ai_provider_spec_service::model_catalog_response_shape_for_surface(
                provider_type,
                requested_surface_id.as_deref(),
            )?,
            &body,
        )?;
        discovered_models.sort_by(|left, right| left.id.cmp(&right.id));
        discovered_models.dedup_by(|left, right| left.id == right.id);
        let model_details = build_model_details(
            provider_type,
            requested_surface_id.as_deref(),
            &discovered_models,
        )?;
        let models = discovered_models
            .iter()
            .map(|model| model.id.clone())
            .collect::<Vec<_>>();

        Ok(ProviderModelsResponse {
            model_details,
            notice: if models.is_empty() {
                Some("Provider returned no models for this configuration.".to_string())
            } else {
                None
            },
            models,
        })
    }

    pub async fn discover_provider_models_for_integration(
        &self,
        request: &ProviderModelsRequest,
    ) -> Result<ProviderModelsResponse, ApiError> {
        if request.use_saved_secret {
            return Err(ApiError::BadRequest(
                "Integration model discovery requires caller-supplied credentials and does not permit use_saved_secret."
                    .to_string(),
            ));
        }

        // #6894: SSRF guard — when `web.allow_external` is set this path is exposed via an
        // external (`0.0.0.0`) bind and the endpoint is caller-controlled, so outbound traffic
        // toward internal hosts is blocked. The local (loopback) `discover_provider_models` path
        // targets the user's own machine, so it is not applied. The endpoint is resolved and
        // checked the same way the delegate re-resolves it (idempotent).
        let requested_surface_id = normalize_optional_surface_id(request.surface_id.as_deref());
        let provider_type = resolve_requested_provider_type(
            request.provider_type.as_str(),
            requested_surface_id.as_deref(),
        )?;
        let endpoint = resolve_models_endpoint(
            provider_type,
            requested_surface_id.as_deref(),
            request.endpoint.as_deref(),
        )?;
        // #6902: take the addresses the guard resolved and validated and forward them as pins all
        // the way to the transport. This stops the transport from re-resolving the host, closing
        // the DNS rebinding (TOCTOU) window where the host could flip to an internal IP after
        // passing the guard. (The guard only returns validated external addresses; internal ones
        // are already rejected.)
        let pinned_addrs = reject_internal_discovery_endpoint(&endpoint).await?;

        self.discover_with_pinned_addrs(request, pinned_addrs).await
    }
}

fn model_catalog_headers(
    auth_scheme: ProviderAuthScheme,
    api_key: Option<&str>,
) -> Vec<ProviderModelCatalogHeader> {
    let api_key = api_key.unwrap_or_default();
    match auth_scheme {
        ProviderAuthScheme::None => Vec::new(),
        ProviderAuthScheme::Bearer => {
            vec![ProviderModelCatalogHeader::new(
                "Authorization",
                format!("Bearer {api_key}"),
            )]
        }
        ProviderAuthScheme::XApiKey => vec![
            ProviderModelCatalogHeader::new("x-api-key", api_key),
            ProviderModelCatalogHeader::new("anthropic-version", ANTHROPIC_API_VERSION),
        ],
        ProviderAuthScheme::XGoogApiKey => {
            vec![ProviderModelCatalogHeader::new("x-goog-api-key", api_key)]
        }
        ProviderAuthScheme::AwsSignatureV4 => {
            unreachable!("AWS Signature V4 discovery exits early with an explicit notice.")
        }
    }
}

fn model_catalog_error_to_api(error: ProviderModelCatalogError) -> ApiError {
    match error {
        ProviderModelCatalogError::ClientBuild(message) => ApiError::Internal(format!(
            "Failed to create model discovery client: {message}"
        )),
        ProviderModelCatalogError::InvalidHeader(message) => ApiError::Internal(format!(
            "Failed to prepare model discovery request: {message}"
        )),
        ProviderModelCatalogError::Request(message) => {
            ApiError::ServiceUnavailable(format!("Model discovery request failed: {message}"))
        }
        ProviderModelCatalogError::ResponseBody(message) => ApiError::ServiceUnavailable(format!(
            "Failed to read model discovery response: {message}"
        )),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn model_discovery_service_does_not_own_reqwest_transport() {
        let source = include_str!("ai_model_catalog_web_service.rs");

        let client_builder_pattern = ["reqwest::", "Client::builder"].concat();
        let send_pattern = [".send", "().await"].concat();

        assert!(
            !source.contains(&client_builder_pattern),
            "model discovery HTTP transport belongs in maekon-network behind a core port"
        );
        assert!(
            !source.contains(&send_pattern),
            "model discovery service should call an injected transport port, not send HTTP directly"
        );
    }
}
