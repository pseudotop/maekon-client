use crate::ai_providers::{ProviderModelSupportStatus, ProviderParameterProfile};

use super::enums::{
    ModelCatalogStrategy, ProviderUnknownModelPolicy, SubprocessAuthProbeMode,
    SubprocessInvocationMode, SurfaceCapabilityKind, SurfaceExecutionKind,
    SurfaceModelCapabilityKind, SurfaceStability,
};
use super::helpers::{
    capability_status_from_profile, known_model_matches, surface_declares_model_selection,
    validate_parameter_usage,
};
use super::models::{
    ProviderAvailabilityProbeSpec, ProviderKnownModelSpec, ProviderSurfaceSpec,
    SubprocessTransportSpec,
};
use super::parsers::{
    parse_model_catalog_strategy, parse_subprocess_auth_probe_mode,
    parse_subprocess_invocation_mode, parse_surface_execution_kind, parse_surface_stability,
};

pub fn list_provider_surface_specs() -> Result<super::models::ProviderSurfaceCatalog, String> {
    Ok(super::surface_catalog()?.clone())
}

pub fn provider_surface_catalog() -> Result<&'static super::models::ProviderSurfaceCatalog, String>
{
    super::surface_catalog()
}

pub fn provider_surface_spec(surface_id: &str) -> Result<&'static ProviderSurfaceSpec, String> {
    let normalized = surface_id.trim().to_ascii_lowercase();
    super::surface_catalog()?
        .surfaces
        .iter()
        .find(|surface| surface.surface_id.eq_ignore_ascii_case(&normalized))
        .ok_or_else(|| format!("Provider surface spec for {surface_id} is missing."))
}

pub fn default_surface_model(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
) -> Result<Option<String>, String> {
    let surface = provider_surface_spec(surface_id)?;
    Ok(match capability {
        SurfaceCapabilityKind::Llm => surface.default_models.llm_models.first().cloned(),
        SurfaceCapabilityKind::Ocr => surface.default_models.ocr_models.first().cloned(),
    })
}

pub fn model_catalog_strategy(surface_id: &str) -> Result<ModelCatalogStrategy, String> {
    let surface = provider_surface_spec(surface_id)?;
    parse_model_catalog_strategy(&surface.catalog_strategy)
}

pub fn surface_stability(surface_id: &str) -> Result<SurfaceStability, String> {
    let surface = provider_surface_spec(surface_id)?;
    parse_surface_stability(&surface.stability)
}

pub fn subprocess_transport(surface_id: &str) -> Result<&'static SubprocessTransportSpec, String> {
    let surface = provider_surface_spec(surface_id)?;
    match parse_surface_execution_kind(&surface.execution_kind)? {
        SurfaceExecutionKind::SubprocessCli => {
            surface.subprocess_transport.as_ref().ok_or_else(|| {
                format!(
                    "Surface '{}' uses subprocess_cli but is missing subprocess_transport.",
                    surface.surface_id
                )
            })
        }
        _ => Err(format!(
            "Surface '{}' is not a subprocess_cli surface.",
            surface.surface_id
        )),
    }
}

pub fn subprocess_supports_json_output(surface_id: &str) -> Result<bool, String> {
    Ok(subprocess_transport(surface_id)?.json_output_supported)
}

pub fn subprocess_invocation_mode(surface_id: &str) -> Result<SubprocessInvocationMode, String> {
    parse_subprocess_invocation_mode(&subprocess_transport(surface_id)?.invocation_mode)
}

pub fn subprocess_auth_probe_mode(surface_id: &str) -> Result<SubprocessAuthProbeMode, String> {
    parse_subprocess_auth_probe_mode(&subprocess_transport(surface_id)?.auth_probe_mode)
}

pub fn surface_supports_capability(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
) -> Result<bool, String> {
    let surface = provider_surface_spec(surface_id)?;
    Ok(match capability {
        SurfaceCapabilityKind::Llm => surface.supports.llm,
        SurfaceCapabilityKind::Ocr => surface.supports.ocr,
    })
}

pub fn list_subprocess_surface_specs() -> Result<Vec<&'static ProviderSurfaceSpec>, String> {
    Ok(super::surface_catalog()?
        .surfaces
        .iter()
        .filter(|surface| {
            matches!(
                parse_surface_execution_kind(&surface.execution_kind),
                Ok(SurfaceExecutionKind::SubprocessCli)
            )
        })
        .collect())
}

pub fn parameter_profile_for_surface(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
) -> Result<&'static ProviderParameterProfile, String> {
    let surface = provider_surface_spec(surface_id)?;
    Ok(match capability {
        SurfaceCapabilityKind::Llm => &surface.parameter_profiles.llm,
        SurfaceCapabilityKind::Ocr => &surface.parameter_profiles.ocr,
    })
}

pub fn validate_supported_surface_parameters(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
    parameters: &[&str],
) -> Result<(), String> {
    let profile = parameter_profile_for_surface(surface_id, capability)?;
    validate_parameter_usage(profile, parameters)
}

pub fn surface_supports_parameter(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
    parameter: &str,
) -> Result<bool, String> {
    let normalized = parameter.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(false);
    }

    let profile = parameter_profile_for_surface(surface_id, capability)?;
    Ok(profile
        .supported
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(&normalized)))
}

pub fn surface_supports_model_selection(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
) -> Result<bool, String> {
    let surface = provider_surface_spec(surface_id)?;
    Ok(surface_declares_model_selection(surface, capability))
}

pub fn availability_probe(
    surface_id: &str,
) -> Result<Option<&'static ProviderAvailabilityProbeSpec>, String> {
    Ok(provider_surface_spec(surface_id)?
        .availability_probe
        .as_ref())
}

pub fn known_model_spec_for_surface(
    surface_id: &str,
    model_id: &str,
) -> Result<Option<&'static ProviderKnownModelSpec>, String> {
    let normalized = model_id.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(None);
    }

    let surface = provider_surface_spec(surface_id)?;
    Ok(surface
        .known_models
        .iter()
        .find(|model| known_model_matches(model, &normalized)))
}

pub fn model_capability_status_for_surface(
    surface_id: &str,
    capability: SurfaceModelCapabilityKind,
    model_id: &str,
) -> Result<ProviderModelSupportStatus, String> {
    let normalized = model_id.trim();
    if normalized.is_empty() {
        return Ok(ProviderModelSupportStatus::Unknown);
    }

    let surface = provider_surface_spec(surface_id)?;
    if let Some(model) = known_model_spec_for_surface(&surface.surface_id, normalized)? {
        let explicit = match capability {
            SurfaceModelCapabilityKind::Llm => Some(model.capabilities.llm),
            SurfaceModelCapabilityKind::Ocr => Some(model.capabilities.ocr),
            SurfaceModelCapabilityKind::ImageInput => Some(model.capabilities.image_input),
            SurfaceModelCapabilityKind::StructuredOutput => None,
        };
        if let Some(supported) = explicit {
            return Ok(if supported {
                ProviderModelSupportStatus::Supported
            } else {
                ProviderModelSupportStatus::Unsupported
            });
        }
    }

    let profile = match capability {
        SurfaceModelCapabilityKind::Llm => &surface.capability_rules.llm,
        SurfaceModelCapabilityKind::Ocr => &surface.capability_rules.ocr,
        SurfaceModelCapabilityKind::ImageInput => &surface.capability_rules.image_input,
        SurfaceModelCapabilityKind::StructuredOutput => &surface.capability_rules.structured_output,
    };

    capability_status_from_profile(profile, normalized)
}

pub fn surface_requires_explicit_model_selection(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
) -> Result<bool, String> {
    Ok(surface_supports_model_selection(surface_id, capability)?
        && default_surface_model(surface_id, capability)?
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .is_none())
}

pub fn ocr_requires_structured_output_model(surface_id: &str) -> Result<bool, String> {
    Ok(provider_surface_spec(surface_id)?
        .ocr_capabilities
        .requires_structured_output_model)
}

pub fn unknown_model_policy_for_surface(
    surface_id: &str,
    capability: SurfaceCapabilityKind,
) -> Result<ProviderUnknownModelPolicy, String> {
    let surface = provider_surface_spec(surface_id)?;
    Ok(match capability {
        SurfaceCapabilityKind::Llm => surface.unknown_model_policy.llm,
        SurfaceCapabilityKind::Ocr => surface.unknown_model_policy.ocr,
    })
}
