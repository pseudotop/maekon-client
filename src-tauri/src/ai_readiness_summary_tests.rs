use super::*;
use crate::feature_capabilities::FeatureMaturity;
use maekon_core::ai_readiness::{AiReadinessReasonCode, AiReadinessStatus};

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
        wbs_assignee_available: false,
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

fn ready_summary_config() -> AppConfig {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_api = Some(endpoint("provider_surface.openai.subprocess_cli"));
    config.analysis.enabled = true;
    config.analysis.tiered_memory.enabled = true;
    config.analysis.embedding.enabled = true;
    config.analysis.embedding.llm_summary_enabled = true;
    config
}

#[test]
fn cli_summary_readiness_uses_cli_axes_with_or_without_a_legacy_http_endpoint() {
    let ready = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let missing = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Unavailable,
        None,
    );
    for endpoint_present in [false, true] {
        let mut config = ready_summary_config();
        if !endpoint_present {
            config.ai_provider.llm_api = None;
        }
        for (snapshot, available) in [(&ready, true), (&missing, false)] {
            let result =
                build_ai_readiness_snapshot(snapshot, &config, &config, &summary_consent(true));
            for id in [
                AiCapabilityId::SegmentSummary,
                AiCapabilityId::DailyNarrative,
            ] {
                let summary = result.find(id).expect("summary capability");
                assert!(summary.dimensions.access_mode_compatible);
                assert_eq!(
                    summary.dimensions.model_availability,
                    AiModelAvailability::NotRequired
                );
                if cfg!(feature = "analysis") && available {
                    assert_eq!(summary.reason_code, AiReadinessReasonCode::Ready);
                } else {
                    assert_ne!(summary.status, AiReadinessStatus::Ready);
                    assert_eq!(
                        summary.dimensions.compiled_capability,
                        cfg!(feature = "analysis")
                    );
                }
            }
        }
    }
}

fn summary_consent(granted: bool) -> maekon_core::consent::ConsentPermissions {
    maekon_core::consent::ConsentPermissions {
        activity_pattern_learning: granted,
        full_text_extraction: true,
        ..Default::default()
    }
}

#[test]
fn configured_http_analysis_tracks_full_text_grant_and_withdrawal() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Available,
        None,
    );
    for (mode, kind) in [
        (AiAccessMode::ProviderApiKey, AiProviderType::Generic),
        (AiAccessMode::ProviderOAuth, AiProviderType::Generic),
        (AiAccessMode::LocalModel, AiProviderType::Generic),
        (AiAccessMode::LocalModel, AiProviderType::Ollama),
    ] {
        let mut config = ready_summary_config();
        config.ai_provider.access_mode = mode;
        let mut api = endpoint("provider_surface.fixture.direct_http");
        api.provider_type = kind;
        api.endpoint = "http://127.0.0.1:11434/v1/chat/completions".to_string();
        config.ai_provider.llm_api = Some(api);
        for granted in [false, true, false] {
            let permissions = maekon_core::consent::ConsentPermissions {
                activity_pattern_learning: true,
                ocr_processing: true,
                full_text_extraction: granted,
                ..Default::default()
            };
            let snapshot = build_ai_readiness_snapshot(&provider, &config, &config, &permissions);
            for id in [
                AiCapabilityId::OcrSuggestionAnalysis,
                AiCapabilityId::SegmentSummary,
                AiCapabilityId::DailyNarrative,
            ] {
                let item = snapshot.find(id).expect("analysis readiness");
                let gate = item
                    .dimensions
                    .consent
                    .iter()
                    .find(|gate| gate.field == AiConsentField::FullTextExtraction)
                    .expect("configured HTTP invocation requires full-text consent");
                assert_eq!(gate.granted, granted);
                if !granted {
                    assert_eq!(item.reason_code, AiReadinessReasonCode::ConsentRequired);
                } else {
                    assert_ne!(item.reason_code, AiReadinessReasonCode::ConsentRequired);
                }
            }
        }
    }
}

#[test]
fn cli_and_catalog_local_analysis_do_not_add_http_full_text_gate() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let cli = ready_summary_config();
    let mut local = cli.clone();
    local.ai_provider.access_mode = AiAccessMode::LocalModel;
    local.ai_provider.llm_api = None;
    for config in [cli, local] {
        let permissions = maekon_core::consent::ConsentPermissions {
            activity_pattern_learning: true,
            ocr_processing: true,
            ..Default::default()
        };
        let snapshot = build_ai_readiness_snapshot(&provider, &config, &config, &permissions);
        for id in [
            AiCapabilityId::OcrSuggestionAnalysis,
            AiCapabilityId::SegmentSummary,
            AiCapabilityId::DailyNarrative,
        ] {
            let item = snapshot.find(id).expect("analysis readiness");
            assert!(item
                .dimensions
                .consent
                .iter()
                .all(|gate| { gate.field != AiConsentField::FullTextExtraction }));
            assert_ne!(item.reason_code, AiReadinessReasonCode::ConsentRequired);
        }
    }
}

#[test]
fn analysis_release_profile_can_report_both_summary_capabilities_ready() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let config = ready_summary_config();
    let readiness =
        build_ai_readiness_snapshot(&provider, &config, &config, &summary_consent(true));

    for capability in [
        AiCapabilityId::SegmentSummary,
        AiCapabilityId::DailyNarrative,
    ] {
        let summary = readiness.find(capability).expect("summary readiness");
        assert_eq!(summary.status, AiReadinessStatus::Ready);
        assert_eq!(summary.reason_code, AiReadinessReasonCode::Ready);
        assert!(summary.dimensions.compiled_capability);
        assert_eq!(
            summary.dimensions.apply_requirement,
            AiRuntimeApplyRequirement::Restart
        );
    }
    assert_eq!(readiness.capabilities.len(), 7);
}

#[test]
fn summary_runtime_requires_every_pipeline_switch() {
    let base = ready_summary_config();
    assert!(summary_runtime_enabled(&base));

    let mut variants = Vec::new();
    let mut analysis = base.clone();
    analysis.analysis.enabled = false;
    variants.push(analysis);
    let mut memory = base.clone();
    memory.analysis.tiered_memory.enabled = false;
    variants.push(memory);
    let mut embedding = base.clone();
    embedding.analysis.embedding.enabled = false;
    variants.push(embedding);
    let mut summarizer = base;
    summarizer.analysis.embedding.llm_summary_enabled = false;
    variants.push(summarizer);

    for variant in variants {
        assert!(!summary_runtime_enabled(&variant));
    }
}

#[test]
fn summary_skips_model_availability_only_for_subscription_cli() {
    let mut config = ready_summary_config().ai_provider;
    assert_eq!(
        summary_model_availability(&config),
        AiModelAvailability::NotRequired
    );

    config.access_mode = AiAccessMode::ProviderApiKey;
    assert_eq!(
        summary_model_availability(&config),
        AiModelAvailability::Unverified
    );

    config.llm_api.as_mut().expect("summary endpoint").model = None;
    assert_eq!(
        summary_model_availability(&config),
        AiModelAvailability::Unavailable
    );
}

#[test]
fn summary_uses_its_own_activity_pattern_consent() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let config = ready_summary_config();
    let readiness =
        build_ai_readiness_snapshot(&provider, &config, &config, &summary_consent(false));

    for capability in [
        AiCapabilityId::SegmentSummary,
        AiCapabilityId::DailyNarrative,
    ] {
        assert_eq!(
            readiness
                .find(capability)
                .expect("summary readiness")
                .reason_code,
            AiReadinessReasonCode::ConsentRequired
        );
    }
}

#[test]
fn summary_startup_change_requires_restart() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let boot = ready_summary_config();
    let mut current = boot.clone();
    current.analysis.embedding.min_segment_for_summary_secs += 1;
    let readiness = build_ai_readiness_snapshot(&provider, &current, &boot, &summary_consent(true));
    let summary = readiness
        .find(AiCapabilityId::SegmentSummary)
        .expect("summary readiness");

    assert_eq!(summary.reason_code, AiReadinessReasonCode::RestartRequired);
    assert!(summary.dimensions.apply_pending);
}

#[test]
fn local_summary_fallback_requires_the_bounded_ollama_probe() {
    let unavailable = provider_snapshot(
        "provider_surface.ollama.local_http",
        FeatureAvailability::Unavailable,
        None,
    );
    let mut config = ready_summary_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;
    config.ai_provider.llm_api = None;

    let blocked =
        build_ai_readiness_snapshot(&unavailable, &config, &config, &summary_consent(true));
    assert_eq!(
        blocked
            .find(AiCapabilityId::SegmentSummary)
            .expect("summary readiness")
            .reason_code,
        AiReadinessReasonCode::ProviderNotDetected
    );

    let available = provider_snapshot(
        "provider_surface.ollama.local_http",
        FeatureAvailability::Available,
        None,
    );
    let unverified =
        build_ai_readiness_snapshot(&available, &config, &config, &summary_consent(true));
    assert_eq!(
        unverified
            .find(AiCapabilityId::SegmentSummary)
            .expect("summary readiness")
            .reason_code,
        AiReadinessReasonCode::ProviderInvocationUnverified
    );

    config.ai_provider.llm_api = Some(endpoint("provider_surface.ollama.local_http"));
    let local_endpoint = config
        .ai_provider
        .llm_api
        .as_mut()
        .expect("local summary endpoint");
    local_endpoint.provider_type = AiProviderType::Ollama;
    local_endpoint.endpoint = "http://127.0.0.1:11434/v1/chat/completions".to_string();
    local_endpoint.api_key.clear();
    let explicit =
        build_ai_readiness_snapshot(&available, &config, &config, &summary_consent(true));
    let explicit_summary = explicit
        .find(AiCapabilityId::SegmentSummary)
        .expect("summary readiness");
    assert!(explicit_summary.dimensions.access_mode_compatible);
    assert_eq!(
        explicit_summary.reason_code,
        AiReadinessReasonCode::ProviderInvocationUnverified
    );

    config
        .ai_provider
        .llm_api
        .as_mut()
        .expect("local summary endpoint")
        .provider_type = AiProviderType::Generic;
    let denied = build_ai_readiness_snapshot(&available, &config, &config, &summary_consent(false));
    for capability in [
        AiCapabilityId::SegmentSummary,
        AiCapabilityId::DailyNarrative,
    ] {
        let summary = denied.find(capability).expect("local summary consent");
        assert!(summary.dimensions.access_mode_compatible);
        assert_eq!(summary.reason_code, AiReadinessReasonCode::ConsentRequired);
    }
    config
        .ai_provider
        .llm_api
        .as_mut()
        .expect("local summary endpoint")
        .endpoint = "https://remote.example.test/v1/chat/completions".to_string();
    let mismatch =
        build_ai_readiness_snapshot(&available, &config, &config, &summary_consent(true));
    assert_eq!(
        mismatch
            .find(AiCapabilityId::SegmentSummary)
            .expect("summary readiness")
            .reason_code,
        AiReadinessReasonCode::AccessModeMismatch
    );
}
#[test]
fn local_analysis_and_summaries_share_generic_loopback_axes() {
    let provider = provider_snapshot(
        "provider_surface.fixture.direct_http",
        FeatureAvailability::Unavailable,
        None,
    );
    let mut config = ready_summary_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "http://127.0.0.1:8080/v1/chat/completions".to_string(),
        api_key: String::new(),
        model: Some("local-model".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: None,
        credential: None,
    });

    let axes = analysis_provider_axes(&provider, &config.ai_provider);
    assert!(analysis_access_mode_compatible(&config.ai_provider, false));
    assert_eq!(axes.detection, AiProviderDetection::NotRequired);
    assert_eq!(axes.auth, AiProviderAuthReadiness::NotRequired);
    assert_eq!(axes.invocation, AiProviderInvocationReadiness::Unverified);

    let consent = maekon_core::consent::ConsentPermissions {
        ocr_processing: true,
        ..summary_consent(true)
    };
    let readiness = build_ai_readiness_snapshot(&provider, &config, &config, &consent);
    for capability in [
        AiCapabilityId::OcrSuggestionAnalysis,
        AiCapabilityId::SegmentSummary,
        AiCapabilityId::DailyNarrative,
    ] {
        let dimensions = &readiness
            .find(capability)
            .expect("generic loopback analysis readiness")
            .dimensions;
        assert!(dimensions.access_mode_compatible);
        assert_eq!(
            dimensions.provider_detection,
            AiProviderDetection::NotRequired
        );
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
    }

    let assert_incompatible = |config: &AppConfig, expected_detection, expected_invocation| {
        let readiness = build_ai_readiness_snapshot(&provider, config, config, &consent);
        for capability in [
            AiCapabilityId::OcrSuggestionAnalysis,
            AiCapabilityId::SegmentSummary,
            AiCapabilityId::DailyNarrative,
        ] {
            let capability = readiness
                .find(capability)
                .expect("remote endpoint readiness");
            assert!(!capability.dimensions.access_mode_compatible);
            assert_eq!(
                capability.reason_code,
                AiReadinessReasonCode::AccessModeMismatch
            );
            assert_eq!(capability.dimensions.provider_detection, expected_detection);
            assert_eq!(
                capability.dimensions.provider_invocation,
                expected_invocation
            );
        }
    };
    config.ai_provider.llm_api_fallback = Some(ExternalApiEndpoint {
        endpoint: "https://fallback.example.test/v1/chat/completions".to_string(),
        api_key: String::new(),
        model: Some("remote-fallback".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Generic,
        surface_id: None,
        credential: None,
    });
    assert_incompatible(
        &config,
        AiProviderDetection::NotRequired,
        AiProviderInvocationReadiness::Unverified,
    );
    config.ai_provider.llm_api_fallback = None;

    config
        .ai_provider
        .llm_api
        .as_mut()
        .expect("generic endpoint")
        .endpoint = "https://generic.example.test/v1/chat/completions".to_string();
    assert_incompatible(
        &config,
        AiProviderDetection::NotDetected,
        AiProviderInvocationReadiness::Unavailable,
    );
}

#[test]
fn ready_cli_under_api_key_mode_is_a_summary_mode_mismatch() {
    let provider = provider_snapshot(
        "provider_surface.openai.subprocess_cli",
        FeatureAvailability::Available,
        Some(ProviderCliReadiness::InvocationReady),
    );
    let mut config = ready_summary_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.llm_api = None;
    let readiness =
        build_ai_readiness_snapshot(&provider, &config, &config, &summary_consent(true));

    assert_eq!(
        readiness
            .find(AiCapabilityId::DailyNarrative)
            .expect("summary readiness")
            .reason_code,
        AiReadinessReasonCode::AccessModeMismatch
    );
}
