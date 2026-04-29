use std::collections::HashSet;

use crate::ai_providers::{ProviderModelSupportStatus, ProviderParameterProfile};

use super::enums::{
    ProviderAuthScheme, SurfaceCapabilityKind, SurfaceExecutionKind, SurfacePlacementKind,
    SurfaceStability,
};
use super::models::{ProviderKnownModelSpec, ProviderSurfaceSpec};
use super::parsers::{
    default_model_support_status, parse_surface_execution_kind, parse_surface_placement_kind,
    parse_surface_stability,
};

pub(super) fn known_model_matches(
    model: &ProviderKnownModelSpec,
    normalized_model_id: &str,
) -> bool {
    if model.id.eq_ignore_ascii_case(normalized_model_id) {
        return true;
    }

    if model
        .aliases
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(normalized_model_id))
    {
        return true;
    }

    model.id_prefixes.iter().any(|prefix| {
        let normalized_prefix = prefix.trim().to_ascii_lowercase();
        !normalized_prefix.is_empty()
            && (normalized_model_id == normalized_prefix
                || normalized_model_id.starts_with(&normalized_prefix))
    })
}

pub(super) fn capability_status_from_profile(
    profile: &crate::ai_providers::ProviderModelCapabilityProfile,
    model_id: &str,
) -> Result<ProviderModelSupportStatus, String> {
    let normalized = model_id.trim().to_ascii_lowercase();
    if normalized.is_empty() {
        return Ok(ProviderModelSupportStatus::Unknown);
    }

    for pattern in &profile.deny_patterns {
        if model_pattern_matches(pattern, &normalized)? {
            return Ok(ProviderModelSupportStatus::Unsupported);
        }
    }

    for pattern in &profile.allow_patterns {
        if model_pattern_matches(pattern, &normalized)? {
            return Ok(ProviderModelSupportStatus::Supported);
        }
    }

    Ok(default_model_support_status(&profile.default_support))
}

pub(super) fn model_pattern_matches(
    pattern: &str,
    normalized_model_id: &str,
) -> Result<bool, String> {
    let normalized_pattern = pattern.trim().to_ascii_lowercase();
    if normalized_pattern.is_empty() {
        return Ok(false);
    }

    if normalized_pattern == "*" {
        return Ok(true);
    }

    if !normalized_pattern.contains('*') {
        return Ok(normalized_model_id == normalized_pattern);
    }

    let parts = normalized_pattern.split('*').collect::<Vec<_>>();
    let starts_with_wildcard = normalized_pattern.starts_with('*');
    let ends_with_wildcard = normalized_pattern.ends_with('*');
    let mut search_start = 0usize;

    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if index == 0 && !starts_with_wildcard {
            if !normalized_model_id[search_start..].starts_with(part) {
                return Ok(false);
            }
            search_start += part.len();
            continue;
        }

        let haystack = &normalized_model_id[search_start..];
        let Some(found_at) = haystack.find(part) else {
            return Ok(false);
        };
        search_start += found_at + part.len();
    }

    if !ends_with_wildcard {
        if let Some(last_part) = parts.iter().rev().find(|part| !part.is_empty()) {
            return Ok(normalized_model_id.ends_with(last_part));
        }
    }

    Ok(true)
}

pub(super) fn stability_sort_key(raw: &str) -> i32 {
    match parse_surface_stability(raw).unwrap_or(SurfaceStability::Experimental) {
        SurfaceStability::Ga => 3,
        SurfaceStability::Preview => 2,
        SurfaceStability::Experimental => 1,
        SurfaceStability::Deprecated => 0,
    }
}

pub(super) fn default_direct_surface_placement_sort_key(raw: &str) -> i32 {
    match parse_surface_placement_kind(raw).unwrap_or(SurfacePlacementKind::CustomHosted) {
        SurfacePlacementKind::ProviderHosted => 4,
        SurfacePlacementKind::CustomHosted => 3,
        SurfacePlacementKind::SelfHosted => 2,
        SurfacePlacementKind::InstalledCli => 1,
    }
}

pub(super) fn surface_declares_model_selection(
    surface: &ProviderSurfaceSpec,
    capability: SurfaceCapabilityKind,
) -> bool {
    let has_defaults = match capability {
        SurfaceCapabilityKind::Llm => !surface.default_models.llm_models.is_empty(),
        SurfaceCapabilityKind::Ocr => !surface.default_models.ocr_models.is_empty(),
    };
    if has_defaults {
        return true;
    }

    let model_catalog_support = surface
        .model_catalog_transport
        .as_ref()
        .map(|transport| match capability {
            SurfaceCapabilityKind::Llm => transport.llm_supported,
            SurfaceCapabilityKind::Ocr => transport.ocr_supported,
        })
        .unwrap_or(false);
    if model_catalog_support {
        return true;
    }

    if surface.known_models.iter().any(|model| match capability {
        SurfaceCapabilityKind::Llm => model.capabilities.llm,
        SurfaceCapabilityKind::Ocr => model.capabilities.ocr,
    }) {
        return true;
    }

    let rules = match capability {
        SurfaceCapabilityKind::Llm => &surface.capability_rules.llm,
        SurfaceCapabilityKind::Ocr => &surface.capability_rules.ocr,
    };
    !rules.allow_patterns.is_empty() || rules.default_support.eq_ignore_ascii_case("supported")
}

pub(super) fn validate_parameter_usage(
    profile: &ProviderParameterProfile,
    parameters: &[&str],
) -> Result<(), String> {
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

    for parameter in parameters {
        let normalized = parameter.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err("Parameter usage contains an empty field name.".to_string());
        }
        if unsupported.contains(&normalized) {
            return Err(format!(
                "Parameter '{parameter}' is explicitly unsupported by this provider surface."
            ));
        }
        if !supported.is_empty() && !supported.contains(&normalized) {
            return Err(format!(
                "Parameter '{parameter}' is not declared as supported by this provider surface."
            ));
        }
    }

    Ok(())
}

pub(super) fn transport_url_is_allowed(url: &str, auth_scheme: ProviderAuthScheme) -> bool {
    let trimmed = url.trim();
    if trimmed.starts_with("https://") {
        return true;
    }

    if auth_scheme != ProviderAuthScheme::None || !trimmed.starts_with("http://") {
        return false;
    }

    let Some(rest) = trimmed.strip_prefix("http://") else {
        return false;
    };
    let host_port = rest.split('/').next().unwrap_or_default();
    if host_port.starts_with("[::1]") {
        return true;
    }
    let host = host_port.split(':').next().unwrap_or_default();

    matches!(host, "localhost" | "127.0.0.1")
}

pub(super) fn preferred_direct_surface_from_vendor<'a>(
    catalog: &'a super::models::ProviderSurfaceCatalog,
    vendor_id: &str,
) -> Option<&'a ProviderSurfaceSpec> {
    catalog
        .surfaces
        .iter()
        .filter(|surface| {
            surface.vendor_id.eq_ignore_ascii_case(vendor_id)
                && matches!(
                    parse_surface_execution_kind(&surface.execution_kind),
                    Ok(SurfaceExecutionKind::DirectHttp)
                )
        })
        .max_by(|left, right| {
            left.preferred_for_product_auth
                .cmp(&right.preferred_for_product_auth)
                .then_with(|| {
                    stability_sort_key(&left.stability).cmp(&stability_sort_key(&right.stability))
                })
                .then_with(|| {
                    default_direct_surface_placement_sort_key(&left.placement_kind).cmp(
                        &default_direct_surface_placement_sort_key(&right.placement_kind),
                    )
                })
                .then_with(|| right.display_name.cmp(&left.display_name))
        })
}
