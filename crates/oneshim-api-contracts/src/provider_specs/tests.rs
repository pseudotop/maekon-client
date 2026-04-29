use oneshim_core::config::{AiAccessMode, AiProviderType};

use crate::ai_providers::{ProviderModelSupportStatus, ProviderParameterProfile};

use super::*;

#[test]
fn loads_provider_surface_catalog() {
    let catalog = list_provider_surface_specs().expect("surface catalog should load");
    assert!(catalog.vendors.len() >= 4);
    assert!(catalog.surfaces.len() >= 6);
}

#[test]
fn resolves_aliases() {
    assert_eq!(
        resolve_provider_type("openai-compatible"),
        Some(AiProviderType::Generic)
    );
    assert_eq!(
        resolve_provider_type("gemini"),
        Some(AiProviderType::Google)
    );
    assert_eq!(
        resolve_provider_type("ollama"),
        Some(AiProviderType::Ollama)
    );
}

#[test]
fn returns_openai_llm_shape() {
    let shape = request_shape(AiProviderType::OpenAi, ProviderTransportKind::Llm)
        .expect("llm shape should resolve");
    assert_eq!(shape, ProviderRequestShape::OpenAiResponses);
}

#[test]
fn returns_google_catalog_shape() {
    let shape =
        model_catalog_response_shape(AiProviderType::Google).expect("catalog shape should resolve");
    assert_eq!(shape, ModelCatalogResponseShape::GoogleModels);
}

#[test]
fn returns_default_model_for_surface() {
    let model = default_surface_model(
        "provider_surface.openai.subprocess_cli",
        SurfaceCapabilityKind::Llm,
    )
    .expect("model should resolve");
    assert_eq!(model.as_deref(), Some("gpt-5.4"));
}

#[test]
fn derives_catalog_default_surface_by_access_mode_and_capability() {
    assert_eq!(
        default_surface_id_for_access_mode(
            AiProviderType::OpenAi,
            AiAccessMode::ProviderOAuth,
            SurfaceCapabilityKind::Llm,
        )
        .expect("managed oauth default should resolve"),
        Some("provider_surface.openai.managed_oauth")
    );
    assert_eq!(
        default_surface_id_for_access_mode(
            AiProviderType::Google,
            AiAccessMode::ProviderSubscriptionCli,
            SurfaceCapabilityKind::Llm,
        )
        .expect("google subprocess default should resolve"),
        Some("provider_surface.google.subprocess_cli")
    );
    assert_eq!(
        default_surface_id_for_access_mode(
            AiProviderType::OpenAi,
            AiAccessMode::ProviderSubscriptionCli,
            SurfaceCapabilityKind::Ocr,
        )
        .expect("ocr subprocess default should resolve"),
        Some("provider_surface.openai.subprocess_cli")
    );
}

#[test]
fn resolves_openai_direct_transport_without_surface_projection() {
    let transport = transport_spec(AiProviderType::OpenAi, ProviderTransportKind::Llm)
        .expect("transport should resolve");
    assert_eq!(transport.url, "https://api.openai.com/v1/responses");
}

#[test]
fn rejects_duplicate_aliases() {
    let mut catalog = list_provider_surface_specs().expect("surface catalog should load");
    catalog.vendors[1].aliases = vec!["shared".to_string()];
    catalog.vendors[2].aliases = vec!["shared".to_string()];

    let err = validate_surface_catalog(&catalog).expect_err("duplicate aliases should fail");
    assert!(err.contains("defined more than once"));
}

#[test]
fn rejects_overlapping_supported_and_unsupported_parameters() {
    let err = validate_parameter_profile(&ProviderParameterProfile {
        supported: vec!["temperature".to_string()],
        unsupported: vec!["temperature".to_string()],
        notes: Vec::new(),
    })
    .expect_err("overlapping parameters should fail");
    assert!(err.contains("overlapping supported/unsupported"));
}

#[test]
fn resolves_openai_managed_surface_shape() {
    let shape = resolved_request_shape(
        AiProviderType::OpenAi,
        Some("provider_surface.openai.managed_oauth"),
        ProviderTransportKind::Llm,
    )
    .expect("managed surface should resolve");
    assert_eq!(shape, ProviderRequestShape::OpenAiResponses);
}

#[test]
fn resolves_subprocess_surface_modes() {
    assert_eq!(
        subprocess_invocation_mode("provider_surface.openai.subprocess_cli")
            .expect("invocation mode should resolve"),
        SubprocessInvocationMode::CodexExecJson
    );
    assert_eq!(
        model_catalog_strategy("provider_surface.openai.direct_api")
            .expect("catalog strategy should resolve"),
        ModelCatalogStrategy::HttpModelsEndpoint
    );
    assert_eq!(
        subprocess_auth_probe_mode("provider_surface.anthropic.subprocess_cli")
            .expect("probe mode should resolve"),
        SubprocessAuthProbeMode::ClaudeAuthStatusJson
    );
}

#[test]
fn loads_managed_oauth_provisioning_from_catalog() {
    let surface = provider_surface_spec("provider_surface.google.managed_oauth")
        .expect("google managed surface should load");
    let provisioning = surface
        .provisioning
        .as_ref()
        .expect("google managed surface should declare provisioning");
    assert_eq!(
        provisioning.configuration_env_vars,
        vec!["ONESHIM_GOOGLE_OAUTH_CLIENT_ID".to_string()]
    );
    assert_eq!(
        provisioning.setup_copy_key.as_deref(),
        Some("featureCapability.surface.provider_surface.google.managed_oauth.setup")
    );
    assert_eq!(
        provisioning.docs_url.as_deref(),
        Some("https://developers.google.com/identity/protocols/oauth2/native-app")
    );
}

#[test]
fn loads_vendor_projection_metadata_from_catalog() {
    let catalog = surface_catalog().expect("catalog should load");
    let openai = catalog
        .vendors
        .iter()
        .find(|vendor| vendor.vendor_id == "openai")
        .expect("openai vendor should exist");
    let projection = openai
        .projection
        .as_ref()
        .expect("openai vendor should declare projection metadata");
    assert_eq!(
        projection.api_key_env_vars,
        vec!["OPENAI_API_KEY".to_string()]
    );
    assert_eq!(
        projection.api_key_temp_file_prefix.as_deref(),
        Some("openai")
    );
}

#[test]
fn lists_subprocess_surface_specs_from_catalog() {
    let surfaces = list_subprocess_surface_specs().expect("subprocess surfaces should load");
    let ids: Vec<&str> = surfaces
        .iter()
        .map(|surface| surface.surface_id.as_str())
        .collect();
    assert!(ids.contains(&"provider_surface.openai.subprocess_cli"));
    assert!(ids.contains(&"provider_surface.anthropic.subprocess_cli"));
    assert!(ids.contains(&"provider_surface.google.subprocess_cli"));
}

#[test]
fn reports_json_output_support_for_gemini_subprocess() {
    assert!(
        subprocess_supports_json_output("provider_surface.google.subprocess_cli")
            .expect("json output support should resolve")
    );
}

#[test]
fn validates_supported_parameters_for_openai_managed_surface() {
    validate_supported_parameters(
        AiProviderType::OpenAi,
        Some("provider_surface.openai.managed_oauth"),
        SurfaceCapabilityKind::Llm,
        &["model", "instructions", "input", "max_output_tokens"],
    )
    .expect("managed OpenAI parameters should be allowed");
}

#[test]
fn rejects_undeclared_parameter_usage_for_surface() {
    let err = validate_supported_surface_parameters(
        "provider_surface.anthropic.subprocess_cli",
        SurfaceCapabilityKind::Llm,
        &["response_format"],
    )
    .expect_err("unsupported subprocess parameter should fail");
    assert!(err.contains("not declared as supported") || err.contains("explicitly unsupported"));
}

#[test]
fn loads_related_surface_ids_from_catalog() {
    let surface = provider_surface_spec("provider_surface.openai.managed_oauth")
        .expect("managed oauth surface should exist");
    assert_eq!(
        surface.related_surface_ids,
        vec!["provider_surface.openai.subprocess_cli".to_string()]
    );
}

#[test]
fn rejects_unknown_related_surface_id() {
    let mut catalog = list_provider_surface_specs().expect("surface catalog should load");
    catalog.surfaces[0].related_surface_ids = vec!["provider_surface.missing".to_string()];

    let err = validate_surface_catalog(&catalog).expect_err("unknown related surface should fail");
    assert!(err.contains("unknown related_surface_id"));
}

#[test]
fn rejects_cross_vendor_related_surface_id() {
    let mut catalog = list_provider_surface_specs().expect("surface catalog should load");
    let managed = catalog
        .surfaces
        .iter_mut()
        .find(|surface| surface.surface_id == "provider_surface.openai.managed_oauth")
        .expect("managed oauth surface should exist");
    managed.related_surface_ids = vec!["provider_surface.anthropic.subprocess_cli".to_string()];

    let err =
        validate_surface_catalog(&catalog).expect_err("cross-vendor related surface should fail");
    assert!(err.contains("must share the same vendor"));
}

#[test]
fn resolves_ollama_no_auth_surface() {
    let auth_scheme = resolved_auth_scheme(
        AiProviderType::Ollama,
        Some("provider_surface.ollama.local_http"),
        ProviderTransportKind::Llm,
    )
    .expect("ollama auth scheme should resolve");
    assert_eq!(auth_scheme, ProviderAuthScheme::None);
}

#[test]
fn resolves_ollama_self_hosted_placement() {
    let surface = provider_surface_spec("provider_surface.ollama.local_http")
        .expect("ollama surface should exist");
    let placement =
        parse_surface_placement_kind(&surface.placement_kind).expect("placement should parse");
    assert_eq!(placement, SurfacePlacementKind::SelfHosted);
}

#[test]
fn matches_known_ollama_vision_model_by_prefix() {
    let known = known_model_spec_for_surface(
        "provider_surface.ollama.local_http",
        "qwen3-vl:8b-instruct-q4_K_M",
    )
    .expect("known model lookup should succeed")
    .expect("vision model should match by prefix");
    assert!(known.capabilities.ocr);
}

#[test]
fn rejects_known_non_vision_ocr_model() {
    let err = validate_known_model_capability(
        AiProviderType::Ollama,
        Some("provider_surface.ollama.local_http"),
        SurfaceCapabilityKind::Ocr,
        "qwen3:8b",
    )
    .expect_err("text-only model should be rejected for OCR");
    assert!(err.contains("not marked as OCR-capable"));
}

#[test]
fn resolves_ollama_availability_probe() {
    let probe = availability_probe("provider_surface.ollama.local_http")
        .expect("availability probe should resolve")
        .expect("ollama probe should exist");
    assert_eq!(probe.url, "http://localhost:11434/api/version");
    assert_eq!(probe.auth_scheme, "none");
}

#[test]
fn resolves_unknown_model_policy_from_catalog() {
    assert_eq!(
        unknown_model_policy_for_surface(
            "provider_surface.openai.direct_api",
            SurfaceCapabilityKind::Llm
        )
        .expect("llm policy should resolve"),
        ProviderUnknownModelPolicy::Allow
    );
    assert_eq!(
        unknown_model_policy_for_surface(
            "provider_surface.openai.direct_api",
            SurfaceCapabilityKind::Ocr
        )
        .expect("ocr policy should resolve"),
        ProviderUnknownModelPolicy::Reject
    );
}

#[test]
fn rejects_unknown_ocr_models_when_surface_policy_requires_known_models() {
    let error = validate_known_model_capability(
        AiProviderType::OpenAi,
        Some("provider_surface.openai.direct_api"),
        SurfaceCapabilityKind::Ocr,
        "custom-text-model",
    )
    .expect_err("unknown OCR model should be rejected");
    assert!(error.contains("not catalogued") || error.contains("not marked"));
}

#[test]
fn surfaces_with_warn_policy_emit_unknown_model_warning() {
    let warning = known_model_capability_warning(
        AiProviderType::Generic,
        Some("provider_surface.generic.direct_api"),
        SurfaceCapabilityKind::Ocr,
        "totally-new-model",
    )
    .expect("warning lookup should succeed");
    assert!(warning.is_some());
}

#[test]
fn resolves_capability_rules_for_local_openai_compatible_surface() {
    assert_eq!(
        model_capability_status_for_surface(
            "provider_surface.generic.local_openai_compatible",
            SurfaceModelCapabilityKind::Ocr,
            "qwen2.5-vl-7b-instruct"
        )
        .expect("ocr support should resolve"),
        ProviderModelSupportStatus::Supported
    );
    assert_eq!(
        model_capability_status_for_surface(
            "provider_surface.generic.local_openai_compatible",
            SurfaceModelCapabilityKind::StructuredOutput,
            "text-embedding-3-small"
        )
        .expect("structured output support should resolve"),
        ProviderModelSupportStatus::Unsupported
    );
}

#[test]
fn explicit_model_selection_is_required_for_local_openai_compatible_surface() {
    assert!(surface_requires_explicit_model_selection(
        "provider_surface.generic.local_openai_compatible",
        SurfaceCapabilityKind::Llm
    )
    .expect("selection requirement should resolve"));
}

#[test]
fn loads_surface_execution_capabilities_from_catalog() {
    let catalog = provider_surface_catalog().expect("catalog should load");
    let openai = catalog
        .surfaces
        .iter()
        .find(|surface| surface.surface_id == "provider_surface.openai.direct_api")
        .expect("openai direct surface should exist");
    assert!(openai.llm_capabilities.structured_output);

    let google = catalog
        .surfaces
        .iter()
        .find(|surface| surface.surface_id == "provider_surface.google.direct_api")
        .expect("google direct surface should exist");
    assert_eq!(google.ocr_capabilities.strategy, "vision_api");
    assert!(google.ocr_capabilities.supports_geometry);
    assert!(google.ocr_capabilities.supports_confidence);
    assert!(openai.ocr_capabilities.requires_structured_output_model);
}
