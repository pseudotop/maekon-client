use oneshim_api_contracts::provider_specs::{
    default_surface_id_for_access_mode as default_surface_id_from_catalog,
    resolved_model_catalog_strategy, resolved_surface_spec, ModelCatalogStrategy,
    ProviderSurfaceSpec, SurfaceCapabilityKind,
};
use oneshim_core::config::{AiProviderConfig, AiProviderType, ExternalApiEndpoint};

use crate::error::ApiError;
use crate::services::ai_provider_spec_service;

pub(crate) fn resolve_models_endpoint(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    endpoint: Option<&str>,
) -> Result<String, ApiError> {
    let endpoint = endpoint.and_then(normalize_optional_endpoint);
    let surface = resolved_surface_spec(provider_type, surface_id).map_err(ApiError::Internal)?;
    if !surface.supports.model_catalog || surface.model_catalog_transport.is_none() {
        return Err(ApiError::BadRequest(format!(
            "Selected provider surface '{}' does not support model discovery.",
            surface.surface_id
        )));
    }
    let catalog_strategy =
        resolved_model_catalog_strategy(provider_type, Some(surface.surface_id.as_str()))
            .map_err(ApiError::Internal)?;

    let default_endpoint = ai_provider_spec_service::default_model_catalog_endpoint_for_surface(
        provider_type,
        Some(surface.surface_id.as_str()),
    )?;

    if let Some(endpoint) = endpoint {
        return match catalog_strategy {
            ModelCatalogStrategy::HttpModelsEndpoint => {
                if let Some(derived) =
                    derive_model_catalog_endpoint_from_surface(surface, &endpoint)
                {
                    Ok(derived)
                } else {
                    Err(ApiError::BadRequest(format!(
                        "Could not derive a model catalog endpoint from '{}' for surface '{}'.",
                        endpoint, surface.surface_id
                    )))
                }
            }
            ModelCatalogStrategy::None | ModelCatalogStrategy::SubprocessProbe => {
                Err(ApiError::BadRequest(format!(
                    "Surface '{}' does not support HTTP model discovery from a custom endpoint.",
                    surface.surface_id
                )))
            }
        };
    }

    Ok(default_endpoint)
}

pub(crate) fn resolve_requested_provider_type(
    raw_provider_type: &str,
    surface_id: Option<&str>,
) -> Result<AiProviderType, ApiError> {
    if let Some(surface_id) = surface_id {
        let surface = oneshim_api_contracts::provider_specs::provider_surface_spec(surface_id)
            .map_err(ApiError::BadRequest)?;
        return ai_provider_spec_service::resolve_provider_type(&surface.provider_type);
    }

    ai_provider_spec_service::resolve_provider_type(raw_provider_type)
}

pub(crate) fn saved_endpoint_surface_id(
    config: &AiProviderConfig,
    endpoint: &ExternalApiEndpoint,
    requested_surface_kind: Option<&str>,
) -> Option<String> {
    endpoint
        .surface_id
        .as_deref()
        .and_then(|value| normalize_optional_surface_id(Some(value)))
        .or_else(|| {
            default_surface_id_from_catalog(
                endpoint.provider_type,
                config.access_mode,
                match requested_surface_kind
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "ocr" | "ocr_api" => SurfaceCapabilityKind::Ocr,
                    _ => SurfaceCapabilityKind::Llm,
                },
            )
            .ok()
            .flatten()
            .map(|value| value.to_ascii_lowercase())
        })
}

pub(crate) fn normalize_optional_surface_id(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

pub(crate) fn normalize_optional_endpoint(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.trim_end_matches('/').to_string())
}

pub(crate) fn derive_model_catalog_endpoint_from_surface(
    surface: &ProviderSurfaceSpec,
    endpoint: &str,
) -> Option<String> {
    let normalized_endpoint = normalize_optional_endpoint(endpoint)?;
    let configured = reqwest::Url::parse(&normalized_endpoint).ok()?;
    let catalog_transport = surface.model_catalog_transport.as_ref()?;
    let catalog_url = reqwest::Url::parse(&catalog_transport.url).ok()?;

    if configured.path() == catalog_url.path() {
        return Some(normalized_endpoint);
    }

    let candidate_transports = [
        surface
            .llm_transport
            .as_ref()
            .map(|transport| transport.url.as_str()),
        surface
            .ocr_transport
            .as_ref()
            .map(|transport| transport.url.as_str()),
    ];

    for candidate in candidate_transports.into_iter().flatten() {
        let default_transport = reqwest::Url::parse(candidate).ok()?;
        if let Some(derived) = derive_model_catalog_endpoint_from_transport(
            &configured,
            &default_transport,
            &catalog_url,
        ) {
            return Some(derived);
        }
    }

    if configured.path().is_empty() || configured.path() == "/" {
        return Some(rebased_url(&configured, &catalog_url));
    }

    if same_origin(&configured, &catalog_url) {
        return Some(rebased_url(&configured, &catalog_url));
    }

    None
}

fn derive_model_catalog_endpoint_from_transport(
    configured: &reqwest::Url,
    default_transport: &reqwest::Url,
    catalog_url: &reqwest::Url,
) -> Option<String> {
    let configured_path = configured.path();
    let default_transport_path = default_transport.path();

    if configured_path.ends_with(default_transport_path) {
        let prefix_len = configured_path
            .len()
            .saturating_sub(default_transport_path.len());
        let derived_path = format!("{}{}", &configured_path[..prefix_len], catalog_url.path());
        return Some(rebased_url_with_path(
            configured,
            &derived_path,
            catalog_url,
        ));
    }

    if path_is_prefix_of(configured_path, default_transport_path) {
        return Some(rebased_url(configured, catalog_url));
    }

    None
}

fn rebased_url(base: &reqwest::Url, catalog_url: &reqwest::Url) -> String {
    rebased_url_with_path(base, catalog_url.path(), catalog_url)
}

fn rebased_url_with_path(base: &reqwest::Url, path: &str, catalog_url: &reqwest::Url) -> String {
    let mut resolved = base.clone();
    resolved.set_path(path);
    resolved.set_query(catalog_url.query());
    resolved.set_fragment(None);
    resolved.to_string()
}

fn path_is_prefix_of(prefix_path: &str, full_path: &str) -> bool {
    let prefix = prefix_path.trim_end_matches('/');
    let full = full_path.trim_end_matches('/');
    if prefix.is_empty() || prefix == "/" {
        return true;
    }
    full == prefix || full.starts_with(&format!("{prefix}/"))
}

fn same_origin(left: &reqwest::Url, right: &reqwest::Url) -> bool {
    left.scheme().eq_ignore_ascii_case(right.scheme())
        && left
            .host_str()
            .zip(right.host_str())
            .is_some_and(|(l, r)| l.eq_ignore_ascii_case(r))
        && left.port_or_known_default() == right.port_or_known_default()
}
