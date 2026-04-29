use oneshim_api_contracts::ai_providers::ProviderModelsRequest;
use oneshim_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};
use oneshim_core::ports::credential_source::CredentialSource;

use crate::error::ApiError;
use crate::services::ai_model_catalog_endpoint::{
    normalize_optional_endpoint, normalize_optional_surface_id, saved_endpoint_surface_id,
};
use crate::services::settings_service::is_masked_key;
use crate::services::web_contexts::AiModelCatalogWebContext;

pub(crate) async fn resolve_model_discovery_api_key(
    request: &ProviderModelsRequest,
    context: &AiModelCatalogWebContext,
    provider_type: AiProviderType,
) -> Result<String, ApiError> {
    let api_key = request.api_key.trim();
    if !api_key.is_empty() && !is_masked_key(api_key) {
        return Ok(api_key.to_string());
    }

    if request.use_saved_secret {
        if let Some(saved) =
            resolve_saved_model_discovery_api_key(request, context, provider_type).await?
        {
            return Ok(saved);
        }
    }

    Err(ApiError::BadRequest(
        "A full API key is required to fetch model catalog.".to_string(),
    ))
}

pub(crate) async fn resolve_saved_model_discovery_api_key(
    request: &ProviderModelsRequest,
    context: &AiModelCatalogWebContext,
    provider_type: AiProviderType,
) -> Result<Option<String>, ApiError> {
    let Some(config_manager) = context.config_manager.as_ref() else {
        return Ok(None);
    };
    let Some(surface) = parse_model_surface(request.surface.as_deref()) else {
        return Ok(None);
    };

    let saved_config = config_manager.get();
    let Some(saved_endpoint) = endpoint_for_surface(&saved_config.ai_provider, surface) else {
        return Ok(None);
    };

    if saved_endpoint.provider_type != provider_type {
        return Ok(None);
    }

    if let Some(request_surface_id) = normalize_optional_surface_id(request.surface_id.as_deref()) {
        let saved_surface_id = saved_endpoint_surface_id(
            &saved_config.ai_provider,
            saved_endpoint,
            request.surface.as_deref(),
        );
        if saved_surface_id.as_deref() != Some(request_surface_id.as_str()) {
            return Ok(None);
        }
    }

    if let Some(request_endpoint) = request.endpoint.as_deref() {
        let request_endpoint = normalize_optional_endpoint(request_endpoint);
        let saved_endpoint_normalized = normalize_optional_endpoint(&saved_endpoint.endpoint);
        if request_endpoint != saved_endpoint_normalized {
            return Ok(None);
        }
    }

    if let Ok(source) = CredentialSource::from_api_key_endpoint_for_profile(
        saved_endpoint,
        Some(
            saved_config
                .ai_provider
                .active_secret_profile_id_or(surface.profile_id()),
        ),
        context
            .secret_stores
            .as_ref()
            .and_then(|stores| stores.for_binding(saved_endpoint.credential.as_ref()))
            .or_else(|| context.secret_store.clone()),
    ) {
        if let Ok(secret) = source.resolve_bearer_token().await {
            if !secret.trim().is_empty() {
                return Ok(Some(secret));
            }
        }
    }

    Ok(None)
}

#[derive(Clone, Copy)]
enum ModelSurface {
    Ocr,
    Llm,
}

impl ModelSurface {
    fn profile_id(self) -> &'static str {
        match self {
            Self::Ocr => "ocr",
            Self::Llm => "llm",
        }
    }
}

fn parse_model_surface(value: Option<&str>) -> Option<ModelSurface> {
    match value?.trim() {
        "ocr_api" => Some(ModelSurface::Ocr),
        "llm_api" => Some(ModelSurface::Llm),
        _ => None,
    }
}

fn endpoint_for_surface(
    config: &AiProviderConfig,
    surface: ModelSurface,
) -> Option<&ExternalApiEndpoint> {
    match surface {
        ModelSurface::Ocr => config.ocr_api.as_ref(),
        ModelSurface::Llm => config.llm_api.as_ref(),
    }
}
