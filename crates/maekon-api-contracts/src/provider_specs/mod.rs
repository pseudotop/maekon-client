mod enums;
mod helpers;
mod models;
mod parsers;
mod queries;
mod resolvers;
mod validation;

#[cfg(test)]
mod tests;

use std::sync::OnceLock;

#[cfg(test)]
use validation::{validate_parameter_profile, validate_surface_catalog};

// --- Re-exports: models ---
pub use models::{
    ProviderAvailabilityProbeSpec, ProviderKnownModelCapabilities, ProviderKnownModelSpec,
    ProviderLlmCapabilities, ProviderOcrCapabilities, ProviderSurfaceCatalog,
    ProviderSurfaceProvisioningSpec, ProviderSurfaceSpec, ProviderSurfaceSupports,
    ProviderUnknownModelPolicySet, ProviderVendorProjectionSpec, ProviderVendorSpec,
    SubprocessTransportSpec, SurfaceDefaultModels,
};

// --- Re-exports: enums ---
pub use enums::{
    ModelCatalogResponseShape, ModelCatalogStrategy, ProviderAuthScheme, ProviderRequestShape,
    ProviderTransportKind, ProviderUnknownModelPolicy, SubprocessAuthProbeMode,
    SubprocessInvocationMode, SurfaceCapabilityKind, SurfaceExecutionKind,
    SurfaceModelCapabilityKind, SurfacePlacementKind, SurfaceStability,
};

// --- Re-exports: parsers ---
pub use parsers::{
    parse_model_catalog_strategy, parse_subprocess_auth_probe_mode,
    parse_subprocess_invocation_mode, parse_surface_execution_kind, parse_surface_placement_kind,
    parse_surface_stability,
};

// --- Re-exports: queries ---
pub use queries::{
    availability_probe, default_surface_model, known_model_spec_for_surface,
    list_provider_surface_specs, list_subprocess_surface_specs,
    model_capability_status_for_surface, model_catalog_strategy,
    ocr_requires_structured_output_model, parameter_profile_for_surface, provider_surface_catalog,
    provider_surface_spec, subprocess_auth_probe_mode, subprocess_invocation_mode,
    subprocess_supports_json_output, subprocess_transport,
    surface_requires_explicit_model_selection, surface_stability, surface_supports_capability,
    surface_supports_model_selection, surface_supports_parameter, unknown_model_policy_for_surface,
    validate_supported_surface_parameters,
};

// --- Re-exports: resolvers ---
pub use resolvers::{
    auth_scheme, default_direct_surface_spec, default_llm_model, default_ocr_model,
    default_surface_id_for_access_mode, known_model_capability_warning,
    model_catalog_response_shape, request_shape, resolve_provider_type, resolved_auth_scheme,
    resolved_default_model, resolved_model_capability_status,
    resolved_model_catalog_response_shape, resolved_model_catalog_strategy,
    resolved_model_catalog_transport, resolved_ocr_requires_structured_output_model,
    resolved_parameter_profile, resolved_request_shape,
    resolved_surface_requires_explicit_model_selection, resolved_surface_spec,
    resolved_surface_supports_model_selection, resolved_surface_supports_parameter,
    resolved_transport_spec, transport_spec, validate_known_model_capability,
    validate_supported_parameters,
};

// ---------------------------------------------------------------------------
// CRITICAL: These items MUST stay in mod.rs (include_str! + OnceLock catalog)
// ---------------------------------------------------------------------------

const PROVIDER_SURFACE_SPECS_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../specs/providers/provider-surface-catalog.json"
));

static SURFACE_CATALOG: OnceLock<Result<ProviderSurfaceCatalog, String>> = OnceLock::new();

fn load_surface_catalog() -> Result<ProviderSurfaceCatalog, String> {
    let catalog = serde_json::from_str::<ProviderSurfaceCatalog>(PROVIDER_SURFACE_SPECS_JSON)
        .map_err(|e| format!("Failed to parse provider surface catalog: {e}"))?;
    validation::validate_surface_catalog(&catalog)?;
    Ok(catalog)
}

fn surface_catalog() -> Result<&'static ProviderSurfaceCatalog, String> {
    match SURFACE_CATALOG.get_or_init(load_surface_catalog) {
        Ok(catalog) => Ok(catalog),
        Err(message) => Err(message.clone()),
    }
}
