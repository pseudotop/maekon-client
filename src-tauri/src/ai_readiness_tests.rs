use super::*;
use crate::feature_capabilities::FeatureMaturity;
use maekon_core::ai_readiness::{AiReadinessReasonCode, AiReadinessStatus};

#[path = "ai_readiness_tests/local_chat_preflight.rs"]
mod local_chat_preflight;

fn provider_snapshot(
    feature_id: &str,
    availability: FeatureAvailability,
    cli: Option<ProviderCliReadiness>,
) -> FeatureCapabilitySnapshot {
    FeatureCapabilitySnapshot {
        features: vec![FeatureCapability {
            feature_id: feature_id.to_string(),
            maturity: FeatureMaturity::Stable,
            availability,
            provider_cli_readiness: cli,
            provider_cli_discovery: None,
            preferred: true,
            requires: Vec::new(),
            status_reason: None,
            status_copy_key: None,
            setup_copy_key: None,
            setup_docs_url: None,
            configuration_env_vars: Vec::new(),
        }],
        ai_readiness: None,
        audio_compiled: false,
        ocr_available: false,
        power_status_available: false,
        active_window_available: false,
        automation_sandbox_available: false,
        linux_session_type: None,
    }
}

fn endpoint(surface_id: &str) -> ExternalApiEndpoint {
    ExternalApiEndpoint {
        endpoint: "https://provider.example/v1".to_string(),
        api_key: "configured".to_string(),
        model: Some("model-a".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: Some(surface_id.to_string()),
        credential: None,
    }
}

#[test]
fn selected_invocation_ready_cli_is_chat_ready() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.openai.subprocess_cli"));

    let readiness = build_ai_readiness_snapshot(
        &provider,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );

    let chat = readiness
        .find(AiCapabilityId::ChatSubprocess)
        .expect("subprocess chat readiness");
    assert_eq!(chat.status, AiReadinessStatus::Ready);
    assert_eq!(chat.reason_code, AiReadinessReasonCode::Ready);
    assert_eq!(readiness.capabilities.len(), 7);
}

#[test]
fn configured_http_is_unverified_until_a_model_invocation_succeeds() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.fixture.direct_http"));

    let readiness = build_ai_readiness_snapshot(
        &provider,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );

    let chat = readiness
        .find(AiCapabilityId::ChatHttpApi)
        .expect("HTTP chat readiness");
    assert_eq!(chat.status, AiReadinessStatus::Blocked);
    assert_eq!(
        chat.reason_code,
        AiReadinessReasonCode::ProviderInvocationUnverified
    );
    assert_eq!(
        chat.dimensions.model_availability,
        AiModelAvailability::Unverified
    );
}

#[test]
fn local_mode_reports_missing_runtime_instead_of_claiming_ready() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;

    let readiness = build_ai_readiness_snapshot(
        &provider,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );

    let chat = readiness
        .find(AiCapabilityId::ChatLocalLlm)
        .expect("local chat readiness");
    assert_eq!(chat.status, AiReadinessStatus::Blocked);
    assert_eq!(chat.reason_code, AiReadinessReasonCode::ProviderNotDetected);
}

#[test]
fn changing_provider_selection_requires_restart() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let boot = AppConfig::default_config();
    let mut current = AppConfig::default_config();
    current.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;

    let readiness = build_ai_readiness_snapshot(
        &provider,
        &current,
        &boot,
        &maekon_core::consent::ConsentPermissions::default(),
    );

    let chat = readiness
        .find(AiCapabilityId::ChatSubprocess)
        .expect("subprocess chat readiness");
    assert_eq!(chat.reason_code, AiReadinessReasonCode::RestartRequired);
    assert!(chat.dimensions.apply_pending);
}

#[test]
fn cli_probe_states_map_to_distinct_readiness_axes() {
    assert_eq!(
        cli_axes_from_readiness(&[]),
        ProviderReadinessAxes {
            detection: AiProviderDetection::NotDetected,
            auth: AiProviderAuthReadiness::Required,
            invocation: AiProviderInvocationReadiness::Unavailable,
        }
    );
    assert_eq!(
        cli_axes_from_readiness(&[ProviderCliReadiness::NotDetected]),
        ProviderReadinessAxes {
            detection: AiProviderDetection::NotDetected,
            auth: AiProviderAuthReadiness::Required,
            invocation: AiProviderInvocationReadiness::Unavailable,
        }
    );
    assert_eq!(
        cli_axes_from_readiness(&[ProviderCliReadiness::AuthUnverified]),
        ProviderReadinessAxes {
            detection: AiProviderDetection::Detected,
            auth: AiProviderAuthReadiness::Unverified,
            invocation: AiProviderInvocationReadiness::Unverified,
        }
    );
    assert_eq!(
        cli_axes_from_readiness(&[ProviderCliReadiness::AuthReady]),
        ProviderReadinessAxes {
            detection: AiProviderDetection::Detected,
            auth: AiProviderAuthReadiness::Ready,
            invocation: AiProviderInvocationReadiness::Unverified,
        }
    );
    assert_eq!(
        cli_axes_from_readiness(&[ProviderCliReadiness::InvocationReady]),
        ProviderReadinessAxes {
            detection: AiProviderDetection::Detected,
            auth: AiProviderAuthReadiness::Ready,
            invocation: AiProviderInvocationReadiness::Ready,
        }
    );
}

#[test]
fn selected_cli_surface_wins_over_other_detected_surfaces() {
    let mut provider = provider_snapshot(
        "provider_surface.selected.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::AuthRequired),
    );
    provider.features.push(FeatureCapability {
        feature_id: "provider_surface.other.subprocess_cli".to_string(),
        maturity: FeatureMaturity::Stable,
        availability: FeatureAvailability::Available,
        provider_cli_readiness: Some(ProviderCliReadiness::InvocationReady),
        provider_cli_discovery: None,
        preferred: false,
        requires: Vec::new(),
        status_reason: None,
        status_copy_key: None,
        setup_copy_key: None,
        setup_docs_url: None,
        configuration_env_vars: Vec::new(),
    });

    let axes = selected_cli_provider_axes(
        &provider,
        Some(&endpoint("provider_surface.selected.subprocess_cli")),
    );
    assert_eq!(axes.detection, AiProviderDetection::Detected);
    assert_eq!(axes.auth, AiProviderAuthReadiness::Required);
    assert_eq!(axes.invocation, AiProviderInvocationReadiness::Unavailable);
    assert!(selected_feature(&provider, &endpoint("provider_surface.unknown")).is_none());

    let unknown =
        selected_cli_provider_axes(&provider, Some(&endpoint("provider_surface.unknown")));
    assert_eq!(unknown.detection, AiProviderDetection::NotDetected);
    assert_eq!(unknown.auth, AiProviderAuthReadiness::Required);
    assert_eq!(
        unknown.invocation,
        AiProviderInvocationReadiness::Unavailable
    );

    let unselected = selected_cli_provider_axes(&provider, None);
    assert_eq!(unselected.detection, AiProviderDetection::Detected);
    assert_eq!(unselected.auth, AiProviderAuthReadiness::Ready);
    assert_eq!(unselected.invocation, AiProviderInvocationReadiness::Ready);
}

#[test]
fn http_axes_distinguish_missing_auth_and_probe_availability() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Unavailable,
        None,
    );
    let empty = AiProviderConfig::default();
    assert_eq!(
        http_provider_axes(&provider, &empty).detection,
        AiProviderDetection::NotDetected
    );

    let mut no_auth = AiProviderConfig {
        access_mode: AiAccessMode::ProviderApiKey,
        llm_api: Some(endpoint("provider_surface.fixture.direct_http")),
        ..AiProviderConfig::default()
    };
    no_auth.llm_api.as_mut().expect("endpoint").api_key.clear();
    let unavailable = http_provider_axes(&provider, &no_auth);
    assert_eq!(unavailable.auth, AiProviderAuthReadiness::Required);
    assert_eq!(
        unavailable.invocation,
        AiProviderInvocationReadiness::Unavailable
    );

    let partial_provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::PartiallyAvailable,
        None,
    );
    no_auth.access_mode = AiAccessMode::ProviderOAuth;
    let partial = http_provider_axes(&partial_provider, &no_auth);
    assert_eq!(partial.auth, AiProviderAuthReadiness::Unverified);
    assert_eq!(
        partial.invocation,
        AiProviderInvocationReadiness::Unverified
    );

    no_auth.llm_api.as_mut().expect("endpoint").endpoint.clear();
    assert_eq!(
        http_provider_axes(&partial_provider, &no_auth).detection,
        AiProviderDetection::NotDetected
    );
}

#[test]
fn local_runtime_probe_requires_an_available_ollama_surface() {
    let available = provider_snapshot(
        "provider_surface.ollama.local_http",
        FeatureAvailability::Available,
        None,
    );
    assert_eq!(
        local_provider_axes(&available),
        ProviderReadinessAxes {
            detection: AiProviderDetection::Detected,
            auth: AiProviderAuthReadiness::NotRequired,
            invocation: AiProviderInvocationReadiness::Unverified,
        }
    );

    let unavailable = provider_snapshot(
        "provider_surface.ollama.local_http",
        FeatureAvailability::Unavailable,
        None,
    );
    assert_eq!(
        local_provider_axes(&unavailable),
        ProviderReadinessAxes {
            detection: AiProviderDetection::NotDetected,
            auth: AiProviderAuthReadiness::NotRequired,
            invocation: AiProviderInvocationReadiness::Unavailable,
        }
    );

    let mut by_requirement = provider_snapshot(
        "provider_surface.local.fixture",
        FeatureAvailability::Available,
        None,
    );
    by_requirement.features[0]
        .requires
        .push("local_server:ollama".to_string());
    assert_eq!(
        local_provider_axes(&by_requirement).detection,
        AiProviderDetection::Detected
    );
}

#[test]
fn config_helpers_distinguish_empty_config_from_selected_provider() {
    let empty = AiProviderConfig::default();
    assert!(!endpoint_or_profile_configured(&empty));
    assert_eq!(
        configured_model_availability(None),
        AiModelAvailability::Unavailable
    );

    let profile = AiProviderConfig {
        active_profile_id: Some("profile-a".to_string()),
        ..AiProviderConfig::default()
    };
    assert!(endpoint_or_profile_configured(&profile));

    let mut configured = AiProviderConfig {
        llm_api: Some(endpoint("provider_surface.fixture.direct_http")),
        ..AiProviderConfig::default()
    };
    assert!(endpoint_or_profile_configured(&configured));
    assert!(endpoint_has_credential(
        configured.llm_api.as_ref().expect("endpoint")
    ));
    assert_eq!(
        configured_model_availability(configured.llm_api.as_ref()),
        AiModelAvailability::Unverified
    );

    configured.llm_api.as_mut().expect("endpoint").model = Some("  ".to_string());
    assert_eq!(
        configured_model_availability(configured.llm_api.as_ref()),
        AiModelAvailability::Unavailable
    );
    configured
        .llm_api
        .as_mut()
        .expect("endpoint")
        .endpoint
        .clear();
    configured.active_profile_id = None;
    assert!(!endpoint_or_profile_configured(&configured));
}

#[test]
fn provider_fingerprint_observes_every_invocation_relevant_field() {
    let base = AiProviderConfig::default();
    let base_fingerprint = provider_selection_fingerprint(&base);
    let mut variants = Vec::new();

    let mut access = base.clone();
    access.access_mode = AiAccessMode::LocalModel;
    variants.push(access);
    let mut profile = base.clone();
    profile.active_profile_id = Some("profile-a".to_string());
    variants.push(profile);
    let mut endpoint_config = base.clone();
    endpoint_config.llm_api = Some(endpoint("provider_surface.fixture.direct_http"));
    variants.push(endpoint_config.clone());
    endpoint_config.llm_api.as_mut().expect("endpoint").model = None;
    variants.push(endpoint_config.clone());
    endpoint_config
        .llm_api
        .as_mut()
        .expect("endpoint")
        .surface_id = None;
    variants.push(endpoint_config.clone());
    endpoint_config
        .llm_api
        .as_mut()
        .expect("endpoint")
        .provider_type = AiProviderType::OpenAi;
    variants.push(endpoint_config.clone());
    endpoint_config
        .llm_api
        .as_mut()
        .expect("endpoint")
        .api_key
        .clear();
    variants.push(endpoint_config);

    for variant in variants {
        assert_ne!(provider_selection_fingerprint(&variant), base_fingerprint);
    }
}

#[test]
fn local_ocr_and_cli_analysis_report_independent_ready_capabilities() {
    let mut provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    provider.ocr_available = true;
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.openai.subprocess_cli"));
    config.vision.ocr_enabled = true;
    config.analysis.enabled = true;
    let consent = maekon_core::consent::ConsentPermissions {
        ocr_processing: true,
        activity_pattern_learning: true,
        ..Default::default()
    };

    let readiness = build_ai_readiness_snapshot(&provider, &config, &config, &consent);

    assert_eq!(
        readiness
            .find(AiCapabilityId::OcrCapture)
            .expect("OCR readiness")
            .status,
        AiReadinessStatus::Ready
    );
    assert_eq!(
        readiness
            .find(AiCapabilityId::OcrSuggestionAnalysis)
            .expect("suggestion readiness")
            .status,
        AiReadinessStatus::Ready
    );
}

#[test]
fn remote_ocr_requires_a_non_empty_endpoint() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.ocr_provider = OcrProviderType::Remote;
    config.ai_provider.ocr_api = Some(endpoint("provider_surface.fixture.direct_http"));

    let configured = build_ai_readiness_snapshot(
        &provider,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );
    assert!(
        configured
            .find(AiCapabilityId::OcrCapture)
            .expect("remote OCR readiness")
            .dimensions
            .endpoint_or_profile_configured
    );

    config
        .ai_provider
        .ocr_api
        .as_mut()
        .expect("OCR endpoint")
        .endpoint
        .clear();
    let missing = build_ai_readiness_snapshot(
        &provider,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );
    assert!(
        !missing
            .find(AiCapabilityId::OcrCapture)
            .expect("remote OCR readiness")
            .dimensions
            .endpoint_or_profile_configured
    );
}

#[test]
fn suggestion_mode_mismatch_requires_every_cli_fallback_condition() {
    let ready_cli = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let no_cli = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );

    let mut config = AppConfig::default_config();
    let blocked = build_ai_readiness_snapshot(
        &ready_cli,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );
    assert!(
        !blocked
            .find(AiCapabilityId::OcrSuggestionAnalysis)
            .expect("suggestion readiness")
            .dimensions
            .access_mode_compatible
    );

    config.ai_provider.active_profile_id = Some("profile-a".to_string());
    let configured = build_ai_readiness_snapshot(
        &ready_cli,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );
    assert!(
        configured
            .find(AiCapabilityId::OcrSuggestionAnalysis)
            .expect("suggestion readiness")
            .dimensions
            .access_mode_compatible
    );

    config.ai_provider.active_profile_id = None;
    let unavailable_cli = build_ai_readiness_snapshot(
        &no_cli,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
    );
    assert!(
        unavailable_cli
            .find(AiCapabilityId::OcrSuggestionAnalysis)
            .expect("suggestion readiness")
            .dimensions
            .access_mode_compatible
    );
}

#[test]
fn local_analysis_uses_the_catalog_default_without_http_credentials() {
    let provider = provider_snapshot(
        "provider_surface.ollama.local_http",
        FeatureAvailability::Available,
        None,
    );
    let consent = maekon_core::consent::ConsentPermissions::default();
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;

    let catalog_default = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    let dimensions = &catalog_default
        .find(AiCapabilityId::OcrSuggestionAnalysis)
        .expect("suggestion readiness")
        .dimensions;
    assert!(dimensions.access_mode_compatible);
    assert!(dimensions.endpoint_or_profile_configured);
    assert_eq!(dimensions.provider_detection, AiProviderDetection::Detected);
    assert_eq!(
        dimensions.provider_auth,
        AiProviderAuthReadiness::NotRequired
    );
    assert_eq!(
        dimensions.provider_invocation,
        AiProviderInvocationReadiness::Unverified
    );
    assert_eq!(
        dimensions.model_availability,
        AiModelAvailability::Unverified
    );

    config.ai_provider.llm_api = Some(endpoint("provider_surface.ollama.local_http"));
    config
        .ai_provider
        .llm_api
        .as_mut()
        .expect("local endpoint")
        .api_key
        .clear();
    let configured = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    let configured_dimensions = &configured
        .find(AiCapabilityId::OcrSuggestionAnalysis)
        .expect("suggestion readiness")
        .dimensions;
    assert_eq!(
        configured_dimensions.provider_auth,
        AiProviderAuthReadiness::NotRequired
    );
    assert_eq!(
        configured_dimensions.provider_invocation,
        AiProviderInvocationReadiness::Unverified
    );

    config.ai_provider.access_mode = AiAccessMode::ProviderOAuth;
    config.ai_provider.llm_api = None;
    let non_local = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    assert!(
        non_local
            .find(AiCapabilityId::OcrSuggestionAnalysis)
            .expect("suggestion readiness")
            .dimensions
            .access_mode_compatible
    );
}

#[test]
fn local_analysis_rejects_a_non_ollama_endpoint_before_provider_checks() {
    let provider = provider_snapshot(
        "provider_surface.ollama.local_http",
        FeatureAvailability::Available,
        None,
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.ollama.local_http"));
    config.analysis.enabled = true;
    let consent = maekon_core::consent::ConsentPermissions {
        ocr_processing: true,
        activity_pattern_learning: true,
        ..Default::default()
    };

    let mismatch = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    let suggestion = mismatch
        .find(AiCapabilityId::OcrSuggestionAnalysis)
        .expect("suggestion readiness");
    assert!(!suggestion.dimensions.access_mode_compatible);
    assert_eq!(
        suggestion.reason_code,
        AiReadinessReasonCode::AccessModeMismatch
    );

    config
        .ai_provider
        .llm_api
        .as_mut()
        .expect("local endpoint")
        .provider_type = AiProviderType::Ollama;
    let ollama = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    assert!(
        ollama
            .find(AiCapabilityId::OcrSuggestionAnalysis)
            .expect("suggestion readiness")
            .dimensions
            .access_mode_compatible
    );
}

#[test]
fn ocr_provider_axes_require_mode_surface_and_oauth_independently() {
    let direct_http = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );
    let subscription_without_cli = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(endpoint("provider_surface.fixture.direct_http")),
        ..AiProviderConfig::default()
    };
    assert_eq!(
        ocr_provider_axes(&direct_http, &subscription_without_cli, false).invocation,
        AiProviderInvocationReadiness::Unverified
    );

    let ready_cli = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let api_key_with_cli_surface = AiProviderConfig {
        access_mode: AiAccessMode::ProviderApiKey,
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(endpoint("provider_surface.openai.subprocess_cli")),
        ..AiProviderConfig::default()
    };
    assert_eq!(
        ocr_provider_axes(&ready_cli, &api_key_with_cli_surface, false).invocation,
        AiProviderInvocationReadiness::Unverified
    );

    let mut oauth = AiProviderConfig {
        access_mode: AiAccessMode::ProviderOAuth,
        ocr_provider: OcrProviderType::Remote,
        ocr_api: Some(endpoint("provider_surface.fixture.direct_http")),
        ..AiProviderConfig::default()
    };
    oauth
        .ocr_api
        .as_mut()
        .expect("OCR endpoint")
        .api_key
        .clear();
    assert_eq!(
        ocr_provider_axes(&direct_http, &oauth, false).auth,
        AiProviderAuthReadiness::Ready
    );
    oauth.access_mode = AiAccessMode::ProviderApiKey;
    assert_eq!(
        ocr_provider_axes(&direct_http, &oauth, false).auth,
        AiProviderAuthReadiness::Required
    );
}

#[test]
fn cli_analysis_does_not_require_an_http_endpoint_or_profile() {
    let mut config = AiProviderConfig::default();
    assert!(!analysis_endpoint_or_profile_configured(&config));

    config.access_mode = AiAccessMode::ProviderSubscriptionCli;
    assert!(analysis_endpoint_or_profile_configured(&config));
}

#[test]
fn ready_cli_under_api_key_mode_is_an_explicit_suggestion_mode_mismatch() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let mut config = AppConfig::default_config();
    config.analysis.enabled = true;
    let consent = maekon_core::consent::ConsentPermissions {
        ocr_processing: true,
        activity_pattern_learning: true,
        ..Default::default()
    };

    let readiness = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    let suggestion = readiness
        .find(AiCapabilityId::OcrSuggestionAnalysis)
        .expect("suggestion readiness");

    assert_eq!(suggestion.status, AiReadinessStatus::Blocked);
    assert_eq!(
        suggestion.reason_code,
        AiReadinessReasonCode::AccessModeMismatch
    );
}

#[test]
fn ocr_consent_withdrawal_blocks_capture_and_analysis_independently() {
    let mut provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    provider.ocr_available = true;
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.openai.subprocess_cli"));
    config.vision.ocr_enabled = true;
    config.analysis.enabled = true;
    let consent = maekon_core::consent::ConsentPermissions {
        activity_pattern_learning: true,
        ..Default::default()
    };

    let readiness = build_ai_readiness_snapshot(&provider, &config, &config, &consent);

    for capability in [
        AiCapabilityId::OcrCapture,
        AiCapabilityId::OcrSuggestionAnalysis,
    ] {
        assert_eq!(
            readiness
                .find(capability)
                .expect("OCR capability readiness")
                .reason_code,
            AiReadinessReasonCode::ConsentRequired
        );
    }
}

#[test]
fn local_ocr_platform_absence_cannot_be_hidden_by_configuration() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;
    config.vision.ocr_enabled = true;
    let consent = maekon_core::consent::ConsentPermissions {
        ocr_processing: true,
        ..Default::default()
    };

    let readiness = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    assert_eq!(
        readiness
            .find(AiCapabilityId::OcrCapture)
            .expect("OCR readiness")
            .reason_code,
        AiReadinessReasonCode::CompiledCapabilityMissing
    );
}

#[test]
fn selected_cli_ocr_surface_does_not_fall_back_to_local_ocr() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let mut config = AiProviderConfig {
        access_mode: AiAccessMode::ProviderSubscriptionCli,
        ocr_provider: OcrProviderType::Local,
        ocr_api: Some(endpoint("provider_surface.openai.subprocess_cli")),
        ..AiProviderConfig::default()
    };

    assert!(!ocr_uses_local_runtime(&config));
    assert_eq!(
        ocr_provider_axes(&provider, &config, false).invocation,
        AiProviderInvocationReadiness::Ready
    );

    config.ocr_api = Some(endpoint("provider_surface.unknown"));
    assert!(!ocr_uses_local_runtime(&config));
    let unknown = ocr_provider_axes(&provider, &config, false);
    assert_eq!(unknown.detection, AiProviderDetection::NotDetected);
    assert_eq!(unknown.auth, AiProviderAuthReadiness::Required);
    assert_eq!(
        unknown.invocation,
        AiProviderInvocationReadiness::Unavailable
    );

    config.access_mode = AiAccessMode::LocalModel;
    assert!(ocr_uses_local_runtime(&config));
}
