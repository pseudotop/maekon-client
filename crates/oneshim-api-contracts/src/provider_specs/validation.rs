use std::collections::HashSet;

use oneshim_core::provider_surface::canonical_provider_surface_id;

use crate::ai_providers::ProviderParameterProfile;

use super::enums::{
    ModelCatalogStrategy, SubprocessAuthProbeMode, SurfaceCapabilityKind, SurfaceExecutionKind,
};
use super::helpers::{surface_declares_model_selection, transport_url_is_allowed};
use super::models::ProviderSurfaceCatalog;
use super::parsers::{
    parse_auth_scheme, parse_model_catalog_strategy, parse_provider_type, parse_request_shape,
    parse_subprocess_auth_probe_mode, parse_subprocess_invocation_mode,
    parse_surface_execution_kind, parse_surface_placement_kind, parse_surface_stability,
};

pub(super) fn validate_surface_catalog(catalog: &ProviderSurfaceCatalog) -> Result<(), String> {
    if catalog.vendors.is_empty() {
        return Err("Provider surface catalog must contain at least one vendor.".to_string());
    }
    if catalog.surfaces.is_empty() {
        return Err("Provider surface catalog must contain at least one surface.".to_string());
    }

    let mut vendor_ids = HashSet::new();
    let mut surface_ids = HashSet::new();
    let mut aliases = HashSet::new();

    for vendor in &catalog.vendors {
        let vendor_id = vendor.vendor_id.trim().to_ascii_lowercase();
        if vendor_id.is_empty() {
            return Err("Provider surface catalog contains an empty vendor_id.".to_string());
        }
        if !vendor_ids.insert(vendor_id.clone()) {
            return Err(format!(
                "Duplicate vendor_id '{vendor_id}' in provider surface catalog."
            ));
        }
        if vendor.display_name.trim().is_empty() {
            return Err(format!(
                "Vendor '{}' is missing a display_name.",
                vendor.vendor_id
            ));
        }

        parse_provider_type(&vendor.provider_type)?;
        if let Some(projection) = vendor.projection.as_ref() {
            if projection
                .api_key_env_vars
                .iter()
                .any(|value| value.trim().is_empty())
            {
                return Err(format!(
                    "Vendor '{}' projection contains an empty api_key_env_vars entry.",
                    vendor.vendor_id
                ));
            }
            if projection
                .api_key_temp_file_prefix
                .as_deref()
                .map(str::trim)
                .is_some_and(str::is_empty)
            {
                return Err(format!(
                    "Vendor '{}' projection api_key_temp_file_prefix cannot be empty.",
                    vendor.vendor_id
                ));
            }
        }

        let provider_key = vendor.provider_type.trim().to_ascii_lowercase();
        for alias in &vendor.aliases {
            let normalized = alias.trim().to_ascii_lowercase();
            if normalized.is_empty() {
                return Err(format!(
                    "Vendor '{}' contains an empty alias.",
                    vendor.vendor_id
                ));
            }
            if normalized == provider_key {
                continue;
            }
            if !aliases.insert(normalized.clone()) {
                return Err(format!(
                    "Provider surface alias '{}' is defined more than once.",
                    alias
                ));
            }
        }
    }

    for surface in &catalog.surfaces {
        let surface_id = surface.surface_id.trim().to_ascii_lowercase();
        if surface_id.is_empty() {
            return Err("Provider surface catalog contains an empty surface_id.".to_string());
        }
        if !surface_ids.insert(surface_id.clone()) {
            return Err(format!(
                "Duplicate surface_id '{}' in provider surface catalog.",
                surface.surface_id
            ));
        }
        if canonical_provider_surface_id(&surface.surface_id).is_none() {
            return Err(format!(
                "Unknown provider surface id '{}' is not registered in oneshim-core.",
                surface.surface_id
            ));
        }
        if !vendor_ids.contains(&surface.vendor_id.trim().to_ascii_lowercase()) {
            return Err(format!(
                "Surface '{}' references unknown vendor_id '{}'.",
                surface.surface_id, surface.vendor_id
            ));
        }
        if surface.display_name.trim().is_empty() {
            return Err(format!(
                "Surface '{}' is missing a display_name.",
                surface.surface_id
            ));
        }
        parse_surface_placement_kind(&surface.placement_kind)?;
        parse_surface_stability(&surface.stability)?;
        if surface.references.is_empty() {
            return Err(format!(
                "Surface '{}' must include at least one reference URL.",
                surface.surface_id
            ));
        }
        for model in &surface.known_models {
            if model.id.trim().is_empty() {
                return Err(format!(
                    "Surface '{}' contains a known model with an empty id.",
                    surface.surface_id
                ));
            }
            if model
                .display_name
                .as_deref()
                .is_some_and(|value| value.trim().is_empty())
            {
                return Err(format!(
                    "Surface '{}' known model '{}' has an empty display_name.",
                    surface.surface_id, model.id
                ));
            }
            if model.aliases.iter().any(|alias| alias.trim().is_empty()) {
                return Err(format!(
                    "Surface '{}' known model '{}' contains an empty alias.",
                    surface.surface_id, model.id
                ));
            }
            if model
                .id_prefixes
                .iter()
                .any(|prefix| prefix.trim().is_empty())
            {
                return Err(format!(
                    "Surface '{}' known model '{}' contains an empty id_prefix.",
                    surface.surface_id, model.id
                ));
            }
        }
        if surface
            .related_surface_ids
            .iter()
            .any(|value| value.trim().is_empty())
        {
            return Err(format!(
                "Surface '{}' contains an empty related_surface_id.",
                surface.surface_id
            ));
        }
        if surface
            .related_surface_ids
            .iter()
            .any(|value| value.eq_ignore_ascii_case(&surface.surface_id))
        {
            return Err(format!(
                "Surface '{}' cannot reference itself in related_surface_ids.",
                surface.surface_id
            ));
        }

        let surface_provider_type = parse_provider_type(&surface.provider_type)?;
        let vendor_provider_type = catalog
            .vendors
            .iter()
            .find(|vendor| vendor.vendor_id.eq_ignore_ascii_case(&surface.vendor_id))
            .ok_or_else(|| {
                format!(
                    "Surface '{}' references unknown vendor_id '{}'.",
                    surface.surface_id, surface.vendor_id
                )
            })
            .and_then(|vendor| parse_provider_type(&vendor.provider_type))?;

        if surface_provider_type != vendor_provider_type {
            return Err(format!(
                "Surface '{}' provider_type '{}' does not match vendor '{}'.",
                surface.surface_id, surface.provider_type, surface.vendor_id
            ));
        }

        validate_parameter_profile(&surface.parameter_profiles.llm)?;
        validate_parameter_profile(&surface.parameter_profiles.ocr)?;
        validate_model_capability_profile(&surface.capability_rules.llm)?;
        validate_model_capability_profile(&surface.capability_rules.ocr)?;
        validate_model_capability_profile(&surface.capability_rules.image_input)?;
        validate_model_capability_profile(&surface.capability_rules.structured_output)?;
        let catalog_strategy = parse_model_catalog_strategy(&surface.catalog_strategy)?;

        if let Some(provisioning) = surface.provisioning.as_ref() {
            if provisioning
                .configuration_env_vars
                .iter()
                .any(|value| value.trim().is_empty())
            {
                return Err(format!(
                    "Surface '{}' provisioning contains an empty configuration_env_var entry.",
                    surface.surface_id
                ));
            }
            if provisioning
                .setup_copy_key
                .as_deref()
                .map(str::trim)
                .is_some_and(str::is_empty)
            {
                return Err(format!(
                    "Surface '{}' provisioning setup_copy_key cannot be empty.",
                    surface.surface_id
                ));
            }
            if provisioning
                .docs_url
                .as_deref()
                .map(str::trim)
                .is_some_and(str::is_empty)
            {
                return Err(format!(
                    "Surface '{}' provisioning docs_url cannot be empty.",
                    surface.surface_id
                ));
            }
        }

        if surface.supports.llm
            && surface.default_models.llm_models.is_empty()
            && !surface_declares_model_selection(surface, SurfaceCapabilityKind::Llm)
        {
            return Err(format!(
                "Surface '{}' supports LLM but defines neither default llm_models nor any LLM model-selection strategy.",
                surface.surface_id
            ));
        }

        match parse_surface_execution_kind(&surface.execution_kind)? {
            SurfaceExecutionKind::DirectHttp | SurfaceExecutionKind::ManagedHttp => {
                if surface.supports.llm {
                    let transport = surface.llm_transport.as_ref().ok_or_else(|| {
                        format!(
                            "Surface '{}' supports LLM but is missing llm_transport.",
                            surface.surface_id
                        )
                    })?;
                    validate_transport_spec(
                        &surface.surface_id,
                        "llm",
                        &transport.url,
                        &transport.auth_scheme,
                        Some(&transport.request_shape),
                    )?;
                }
                if surface.supports.ocr {
                    let transport = surface.ocr_transport.as_ref().ok_or_else(|| {
                        format!(
                            "Surface '{}' supports OCR but is missing ocr_transport.",
                            surface.surface_id
                        )
                    })?;
                    validate_transport_spec(
                        &surface.surface_id,
                        "ocr",
                        &transport.url,
                        &transport.auth_scheme,
                        Some(&transport.request_shape),
                    )?;
                }
                if surface.supports.model_catalog {
                    if catalog_strategy != ModelCatalogStrategy::HttpModelsEndpoint {
                        return Err(format!(
                            "Surface '{}' must use catalog_strategy='http_models_endpoint' for direct or managed HTTP model discovery.",
                            surface.surface_id
                        ));
                    }
                    let transport = surface.model_catalog_transport.as_ref().ok_or_else(|| {
                        format!(
                            "Surface '{}' supports model_catalog but is missing model_catalog_transport.",
                            surface.surface_id
                        )
                    })?;
                    validate_transport_spec(
                        &surface.surface_id,
                        "model_catalog",
                        &transport.url,
                        &transport.auth_scheme,
                        None,
                    )?;

                    let response_shape = transport.response_shape.trim().to_ascii_lowercase();
                    match response_shape.as_str() {
                        "standard_data_or_models" | "google_models" => {}
                        _ => {
                            return Err(format!(
                                "Surface '{}' has unsupported model catalog response shape '{}'.",
                                surface.surface_id, transport.response_shape
                            ))
                        }
                    }

                    if !transport.ocr_supported
                        && transport
                            .ocr_notice
                            .as_deref()
                            .map(str::trim)
                            .unwrap_or("")
                            .is_empty()
                    {
                        return Err(format!(
                            "Surface '{}' must include an OCR notice when model catalog OCR is unsupported.",
                            surface.surface_id
                        ));
                    }
                }
                if let Some(probe) = surface.availability_probe.as_ref() {
                    validate_transport_spec(
                        &surface.surface_id,
                        "availability_probe",
                        &probe.url,
                        &probe.auth_scheme,
                        None,
                    )?;
                    let method = probe.method.trim().to_ascii_uppercase();
                    if method != "GET" && method != "HEAD" {
                        return Err(format!(
                            "Surface '{}' availability_probe method '{}' is unsupported.",
                            surface.surface_id, probe.method
                        ));
                    }
                }
            }
            SurfaceExecutionKind::SubprocessCli => {
                if surface.supports.model_catalog
                    && catalog_strategy != ModelCatalogStrategy::SubprocessProbe
                {
                    return Err(format!(
                        "Subprocess surface '{}' must use catalog_strategy='subprocess_probe' when model_catalog is enabled.",
                        surface.surface_id
                    ));
                }
                let subprocess = surface.subprocess_transport.as_ref().ok_or_else(|| {
                    format!(
                        "Surface '{}' uses subprocess_cli but is missing subprocess_transport.",
                        surface.surface_id
                    )
                })?;
                if subprocess.tool_id.trim().is_empty() {
                    return Err(format!(
                        "Subprocess surface '{}' must declare a non-empty tool_id.",
                        surface.surface_id
                    ));
                }
                if subprocess.executable_candidates.is_empty() {
                    return Err(format!(
                        "Subprocess surface '{}' must declare executable_candidates.",
                        surface.surface_id
                    ));
                }
                let auth_probe_mode =
                    parse_subprocess_auth_probe_mode(&subprocess.auth_probe_mode)?;
                parse_subprocess_invocation_mode(&subprocess.invocation_mode)?;
                if auth_probe_mode != SubprocessAuthProbeMode::None
                    && subprocess.auth_probe_command.is_empty()
                {
                    return Err(format!(
                        "Subprocess surface '{}' must declare auth_probe_command when auth_probe_mode is enabled.",
                        surface.surface_id
                    ));
                }
            }
        }
    }

    for surface in &catalog.surfaces {
        for related_surface_id in &surface.related_surface_ids {
            let related_surface = catalog
                .surfaces
                .iter()
                .find(|candidate| {
                    candidate
                        .surface_id
                        .eq_ignore_ascii_case(related_surface_id)
                })
                .ok_or_else(|| {
                    format!(
                        "Surface '{}' references unknown related_surface_id '{}'.",
                        surface.surface_id, related_surface_id
                    )
                })?;
            if !related_surface
                .vendor_id
                .eq_ignore_ascii_case(&surface.vendor_id)
            {
                return Err(format!(
                    "Surface '{}' related_surface_id '{}' must share the same vendor.",
                    surface.surface_id, related_surface_id
                ));
            }
        }
    }

    Ok(())
}

fn validate_transport_spec(
    transport_owner: &str,
    transport_name: &str,
    url: &str,
    auth_scheme: &str,
    request_shape: Option<&str>,
) -> Result<(), String> {
    let auth_scheme = parse_auth_scheme(auth_scheme)?;
    if url.trim().is_empty() {
        return Err(format!(
            "Transport owner '{}' transport '{}' is missing a URL.",
            transport_owner, transport_name
        ));
    }
    if !transport_url_is_allowed(url, auth_scheme) {
        return Err(format!(
            "Transport owner '{}' transport '{}' must use an https URL or an allowed local no-auth URL.",
            transport_owner, transport_name
        ));
    }
    if let Some(shape) = request_shape {
        parse_request_shape(shape)?;
    }
    Ok(())
}

pub(super) fn validate_parameter_profile(profile: &ProviderParameterProfile) -> Result<(), String> {
    let supported = profile
        .supported
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let unsupported = profile
        .unsupported
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .collect::<HashSet<_>>();

    if supported.iter().any(|value| value.is_empty())
        || unsupported.iter().any(|value| value.is_empty())
    {
        return Err("Parameter profile contains an empty parameter entry.".to_string());
    }

    if let Some(overlap) = supported.intersection(&unsupported).next() {
        return Err(format!(
            "Parameter profile contains overlapping supported/unsupported field '{}'.",
            overlap
        ));
    }

    Ok(())
}

fn validate_model_capability_profile(
    profile: &crate::ai_providers::ProviderModelCapabilityProfile,
) -> Result<(), String> {
    let default_support = profile.default_support.trim();
    if !default_support.is_empty() {
        match default_support.to_ascii_lowercase().as_str() {
            "supported" | "unsupported" | "unknown" => {}
            other => {
                return Err(format!(
                    "Model capability profile has unsupported default_support '{}'.",
                    other
                ));
            }
        }
    }

    if profile
        .allow_patterns
        .iter()
        .chain(profile.deny_patterns.iter())
        .any(|value| value.trim().is_empty())
    {
        return Err("Model capability profile contains an empty pattern.".to_string());
    }

    Ok(())
}
