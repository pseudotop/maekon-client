use oneshim_core::config::{AiAccessMode, AiProviderType};
use oneshim_core::provider_surface::{provider_type_from_vendor_id, provider_vendor_id_or_default};

use crate::ai_providers::{
    ProviderModelCatalogTransportSpec, ProviderModelSupportStatus, ProviderParameterProfile,
    ProviderTransportSpec,
};

use super::enums::{
    ModelCatalogResponseShape, ModelCatalogStrategy, ProviderAuthScheme, ProviderRequestShape,
    ProviderTransportKind, ProviderUnknownModelPolicy, SurfaceCapabilityKind, SurfaceExecutionKind,
    SurfaceModelCapabilityKind,
};
use super::helpers::{
    preferred_direct_surface_from_vendor, stability_sort_key, validate_parameter_usage,
};
use super::models::ProviderSurfaceSpec;
use super::parsers::{
    parse_auth_scheme, parse_model_catalog_strategy, parse_provider_type_name, parse_request_shape,
    parse_surface_execution_kind,
};
use super::queries::provider_surface_spec;

pub fn resolve_provider_type(raw: &str) -> Option<AiProviderType> {
    let normalized = raw.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return None;
    }

    let catalog = super::surface_catalog().ok()?;
    for vendor in &catalog.vendors {
        let canonical = vendor.provider_type.to_ascii_lowercase();
        if canonical == normalized
            || vendor
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&normalized))
        {
            if let Some(parsed) = parse_provider_type_name(&vendor.provider_type) {
                return Some(parsed);
            }
        }
    }

    parse_provider_type_name(&normalized)
}

pub fn resolved_surface_spec(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
) -> Result<&'static ProviderSurfaceSpec, String> {
    if let Some(surface_id) = surface_id.map(str::trim).filter(|value| !value.is_empty()) {
        let surface = provider_surface_spec(surface_id)?;
        let expected = provider_vendor_id_or_default(provider_type);
        if provider_type_from_vendor_id(&surface.provider_type) != Some(provider_type) {
            return Err(format!(
                "Surface '{}' does not match provider_type '{}'.",
                surface_id, expected
            ));
        }
        return Ok(surface);
    }

    default_direct_surface_spec(provider_type)
}

pub fn default_surface_id_for_access_mode(
    provider_type: AiProviderType,
    access_mode: AiAccessMode,
    capability: SurfaceCapabilityKind,
) -> Result<Option<&'static str>, String> {
    let execution_kind = match access_mode.normalized_for_ai_surfaces() {
        AiAccessMode::ProviderOAuth => SurfaceExecutionKind::ManagedHttp,
        AiAccessMode::ProviderSubscriptionCli => SurfaceExecutionKind::SubprocessCli,
        AiAccessMode::ProviderApiKey | AiAccessMode::LocalModel => SurfaceExecutionKind::DirectHttp,
    };

    let mut candidates = super::surface_catalog()?
        .surfaces
        .iter()
        .filter(|surface| {
            provider_type_from_vendor_id(&surface.provider_type) == Some(provider_type)
        })
        .filter(|surface| {
            parse_surface_execution_kind(&surface.execution_kind).ok() == Some(execution_kind)
        })
        .filter(|surface| match capability {
            SurfaceCapabilityKind::Llm => surface.supports.llm,
            SurfaceCapabilityKind::Ocr => surface.supports.ocr,
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        right
            .preferred_for_product_auth
            .cmp(&left.preferred_for_product_auth)
            .then_with(|| {
                stability_sort_key(&right.stability).cmp(&stability_sort_key(&left.stability))
            })
            .then_with(|| left.display_name.cmp(&right.display_name))
    });

    Ok(candidates
        .first()
        .map(|surface| surface.surface_id.as_str()))
}

pub fn transport_spec(
    provider_type: AiProviderType,
    kind: ProviderTransportKind,
) -> Result<&'static ProviderTransportSpec, String> {
    resolved_transport_spec(provider_type, None, kind)
}

pub fn resolved_transport_spec(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    kind: ProviderTransportKind,
) -> Result<&'static ProviderTransportSpec, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    match kind {
        ProviderTransportKind::Llm => surface.llm_transport.as_ref().ok_or_else(|| {
            format!(
                "Surface '{}' does not provide an llm_transport.",
                surface.surface_id
            )
        }),
        ProviderTransportKind::Ocr => surface.ocr_transport.as_ref().ok_or_else(|| {
            format!(
                "Surface '{}' does not provide an ocr_transport.",
                surface.surface_id
            )
        }),
        ProviderTransportKind::ModelCatalog => Err(
            "Model catalog transport uses a dedicated shape and must be resolved separately."
                .to_string(),
        ),
    }
}

pub fn auth_scheme(
    provider_type: AiProviderType,
    kind: ProviderTransportKind,
) -> Result<ProviderAuthScheme, String> {
    resolved_auth_scheme(provider_type, None, kind)
}

pub fn resolved_auth_scheme(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    kind: ProviderTransportKind,
) -> Result<ProviderAuthScheme, String> {
    let raw = match kind {
        ProviderTransportKind::Llm | ProviderTransportKind::Ocr => {
            resolved_transport_spec(provider_type, surface_id, kind)?
                .auth_scheme
                .as_str()
        }
        ProviderTransportKind::ModelCatalog => {
            resolved_model_catalog_transport(provider_type, surface_id)?
                .auth_scheme
                .as_str()
        }
    };
    parse_auth_scheme(raw)
}

pub fn request_shape(
    provider_type: AiProviderType,
    kind: ProviderTransportKind,
) -> Result<ProviderRequestShape, String> {
    resolved_request_shape(provider_type, None, kind)
}

pub fn resolved_request_shape(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    kind: ProviderTransportKind,
) -> Result<ProviderRequestShape, String> {
    parse_request_shape(&resolved_transport_spec(provider_type, surface_id, kind)?.request_shape)
}

pub fn model_catalog_response_shape(
    provider_type: AiProviderType,
) -> Result<ModelCatalogResponseShape, String> {
    resolved_model_catalog_response_shape(provider_type, None)
}

pub fn resolved_model_catalog_transport(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
) -> Result<&'static ProviderModelCatalogTransportSpec, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    surface.model_catalog_transport.as_ref().ok_or_else(|| {
        format!(
            "Surface '{}' does not provide a model_catalog_transport.",
            surface.surface_id
        )
    })
}

pub fn resolved_model_catalog_response_shape(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
) -> Result<ModelCatalogResponseShape, String> {
    let raw = resolved_model_catalog_transport(provider_type, surface_id)?
        .response_shape
        .trim()
        .to_ascii_lowercase();
    match raw.as_str() {
        "standard_data_or_models" => Ok(ModelCatalogResponseShape::StandardDataOrModels),
        "google_models" => Ok(ModelCatalogResponseShape::GoogleModels),
        _ => Err(format!(
            "Unsupported model catalog response shape '{raw}' for {}",
            provider_vendor_id_or_default(provider_type)
        )),
    }
}

pub fn default_llm_model(provider_type: AiProviderType) -> Result<Option<String>, String> {
    resolved_default_model(provider_type, None, SurfaceCapabilityKind::Llm)
}

pub fn default_ocr_model(provider_type: AiProviderType) -> Result<Option<String>, String> {
    resolved_default_model(provider_type, None, SurfaceCapabilityKind::Ocr)
}

pub fn resolved_default_model(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
) -> Result<Option<String>, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    Ok(match capability {
        SurfaceCapabilityKind::Llm => surface.default_models.llm_models.first().cloned(),
        SurfaceCapabilityKind::Ocr => surface.default_models.ocr_models.first().cloned(),
    })
}

pub fn resolved_model_catalog_strategy(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
) -> Result<ModelCatalogStrategy, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    parse_model_catalog_strategy(&surface.catalog_strategy)
}

pub fn resolved_parameter_profile(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
) -> Result<&'static ProviderParameterProfile, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    Ok(match capability {
        SurfaceCapabilityKind::Llm => &surface.parameter_profiles.llm,
        SurfaceCapabilityKind::Ocr => &surface.parameter_profiles.ocr,
    })
}

pub fn validate_supported_parameters(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
    parameters: &[&str],
) -> Result<(), String> {
    let profile = resolved_parameter_profile(provider_type, surface_id, capability)?;
    validate_parameter_usage(profile, parameters)
}

pub fn resolved_surface_supports_model_selection(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
) -> Result<bool, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    super::queries::surface_supports_model_selection(&surface.surface_id, capability)
}

pub fn resolved_surface_supports_parameter(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
    parameter: &str,
) -> Result<bool, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    super::queries::surface_supports_parameter(&surface.surface_id, capability, parameter)
}

pub fn resolved_model_capability_status(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceModelCapabilityKind,
    model_id: &str,
) -> Result<ProviderModelSupportStatus, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    super::queries::model_capability_status_for_surface(&surface.surface_id, capability, model_id)
}

pub fn resolved_surface_requires_explicit_model_selection(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
) -> Result<bool, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    super::queries::surface_requires_explicit_model_selection(&surface.surface_id, capability)
}

pub fn resolved_ocr_requires_structured_output_model(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
) -> Result<bool, String> {
    let surface = resolved_surface_spec(provider_type, surface_id)?;
    super::queries::ocr_requires_structured_output_model(&surface.surface_id)
}

pub fn validate_known_model_capability(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
    model_id: &str,
) -> Result<(), String> {
    let normalized = model_id.trim();
    if normalized.is_empty() {
        return Ok(());
    }

    let surface = resolved_surface_spec(provider_type, surface_id)?;
    let support = super::queries::model_capability_status_for_surface(
        &surface.surface_id,
        capability.into(),
        normalized,
    )?;
    if support == ProviderModelSupportStatus::Supported {
        return Ok(());
    }
    if support == ProviderModelSupportStatus::Unknown {
        let policy =
            super::queries::unknown_model_policy_for_surface(&surface.surface_id, capability)?;
        return match policy {
            ProviderUnknownModelPolicy::Allow | ProviderUnknownModelPolicy::Warn => Ok(()),
            ProviderUnknownModelPolicy::Reject => {
                let capability_label = match capability {
                    SurfaceCapabilityKind::Llm => "LLM",
                    SurfaceCapabilityKind::Ocr => "OCR",
                };
                let replacement =
                    super::queries::default_surface_model(&surface.surface_id, capability)?
                        .unwrap_or_else(|| "a compatible default model".to_string());
                Err(format!(
                    "Model '{}' is not catalogued for {} surface '{}'. Choose a known compatible model such as '{}'.",
                    normalized, capability_label, surface.surface_id, replacement
                ))
            }
        };
    }

    let capability_label = match capability {
        SurfaceCapabilityKind::Llm => "LLM",
        SurfaceCapabilityKind::Ocr => "OCR",
    };
    let replacement = super::queries::default_surface_model(&surface.surface_id, capability)?
        .unwrap_or_else(|| "a compatible default model".to_string());

    Err(format!(
        "Model '{}' is not marked as {}-capable for surface '{}'. Choose a compatible model such as '{}'.",
        normalized, capability_label, surface.surface_id, replacement
    ))
}

pub fn known_model_capability_warning(
    provider_type: AiProviderType,
    surface_id: Option<&str>,
    capability: SurfaceCapabilityKind,
    model_id: &str,
) -> Result<Option<String>, String> {
    let normalized = model_id.trim();
    if normalized.is_empty() {
        return Ok(None);
    }

    let surface = resolved_surface_spec(provider_type, surface_id)?;
    if super::queries::model_capability_status_for_surface(
        &surface.surface_id,
        capability.into(),
        normalized,
    )? != ProviderModelSupportStatus::Unknown
    {
        return Ok(None);
    }

    if super::queries::unknown_model_policy_for_surface(&surface.surface_id, capability)?
        != ProviderUnknownModelPolicy::Warn
    {
        return Ok(None);
    }

    let capability_label = match capability {
        SurfaceCapabilityKind::Llm => "LLM",
        SurfaceCapabilityKind::Ocr => "OCR",
    };

    Ok(Some(format!(
        "Model '{}' is not catalogued for {} surface '{}'. Continuing because this surface allows unknown models with a warning.",
        normalized, capability_label, surface.surface_id
    )))
}

pub fn default_direct_surface_spec(
    provider_type: AiProviderType,
) -> Result<&'static ProviderSurfaceSpec, String> {
    let vendor_id = provider_vendor_id_or_default(provider_type);
    let catalog = super::surface_catalog()?;
    let vendor = catalog
        .vendors
        .iter()
        .find(|vendor| vendor.vendor_id.eq_ignore_ascii_case(vendor_id))
        .ok_or_else(|| {
            format!("Provider vendor for {vendor_id} is missing from the surface catalog.")
        })?;
    preferred_direct_surface_from_vendor(catalog, &vendor.vendor_id).ok_or_else(|| {
        format!(
            "Provider type '{}' does not define a default direct_http surface.",
            vendor_id
        )
    })
}
