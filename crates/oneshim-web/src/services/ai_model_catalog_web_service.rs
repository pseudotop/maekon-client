use std::time::Duration;

use oneshim_api_contracts::ai_providers::{ProviderModelsRequest, ProviderModelsResponse};

use crate::error::ApiError;
use crate::services::ai_model_catalog_assembler::{build_model_details, parse_models};
use crate::services::ai_model_catalog_auth::resolve_model_discovery_api_key;
use crate::services::ai_model_catalog_endpoint::{
    normalize_optional_surface_id, resolve_models_endpoint, resolve_requested_provider_type,
};
use crate::services::ai_model_catalog_service::truncate_error;
use crate::services::ai_provider_spec_service::{self, ProviderAuthScheme};
use crate::services::web_contexts::AiModelCatalogWebContext;

const MODEL_DISCOVERY_TIMEOUT_SECS: u64 = 20;

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

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(MODEL_DISCOVERY_TIMEOUT_SECS))
            .build()
            .map_err(|error| {
                ApiError::Internal(format!("Failed to create model discovery client: {error}"))
            })?;

        let mut builder = client.get(&endpoint);
        match auth_scheme {
            ProviderAuthScheme::None => {}
            ProviderAuthScheme::Bearer => {
                let api_key = api_key.as_deref().unwrap_or_default();
                builder = builder.header("Authorization", format!("Bearer {api_key}"));
            }
            ProviderAuthScheme::XApiKey => {
                let api_key = api_key.as_deref().unwrap_or_default();
                builder = builder
                    .header("x-api-key", api_key)
                    .header("anthropic-version", "2023-06-01");
            }
            ProviderAuthScheme::XGoogApiKey => {
                let api_key = api_key.as_deref().unwrap_or_default();
                builder = builder.header("x-goog-api-key", api_key);
            }
            ProviderAuthScheme::AwsSignatureV4 => {
                unreachable!("AWS Signature V4 discovery exits early with an explicit notice.")
            }
        }

        let response = builder.send().await.map_err(|error| {
            ApiError::ServiceUnavailable(format!("Model discovery request failed: {error}"))
        })?;

        let status = response.status();
        let body = response.text().await.map_err(|error| {
            ApiError::ServiceUnavailable(format!(
                "Failed to read model discovery response: {error}"
            ))
        })?;
        if !status.is_success() {
            let message = format!(
                "Model discovery failed ({}): {}",
                status,
                truncate_error(&body)
            );
            // Semantic ApiError mapping per iter-54..59 pattern (ApiError
            // variants are web-layer HTTP status equivalents).
            return Err(match status.as_u16() {
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

        self.discover_provider_models(request).await
    }
}
