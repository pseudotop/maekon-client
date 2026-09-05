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

fn summary_consent(granted: bool) -> maekon_core::consent::ConsentPermissions {
    maekon_core::consent::ConsentPermissions {
        activity_pattern_learning: granted,
        ..Default::default()
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
