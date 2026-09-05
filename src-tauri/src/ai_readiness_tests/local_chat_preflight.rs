use super::*;

#[cfg(not(feature = "analysis"))]
#[tokio::test]
async fn non_analysis_build_fails_closed_for_local_chat_preflight() {
    let config = AppConfig::default_config();

    let preflight = probe_local_chat_preflight(&config.ai_provider).await;

    assert_eq!(preflight.detection, AiProviderDetection::NotDetected);
    assert_eq!(preflight.auth, AiProviderAuthReadiness::NotRequired);
    assert_eq!(
        preflight.invocation,
        AiProviderInvocationReadiness::Unavailable
    );
    assert_eq!(
        preflight.model_availability,
        AiModelAvailability::Unavailable
    );
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn local_chat_is_ready_only_after_bounded_daemon_and_model_preflight() {
    let mut server = mockito::Server::new_async().await;
    let _version = server
        .mock("GET", "/api/version")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"version":"0.11.0"}"#)
        .expect(1)
        .create_async()
        .await;
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"models":[{"name":"qwen3:8b"}]}"#)
        .expect(1)
        .create_async()
        .await;
    let provider = provider_snapshot(
        "provider_surface.ollama.local_http",
        FeatureAvailability::Available,
        None,
    );
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: format!("{}/v1/responses", server.url()),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    });

    let preflight = probe_local_chat_preflight(&config.ai_provider).await;
    let readiness = build_ai_readiness_snapshot_with_local_preflight(
        &provider,
        &config,
        &config,
        &maekon_core::consent::ConsentPermissions::default(),
        Some(preflight),
    );
    let local = readiness
        .find(AiCapabilityId::ChatLocalLlm)
        .expect("local Chat readiness");

    assert_eq!(local.status, AiReadinessStatus::Ready);
    assert_eq!(local.reason_code, AiReadinessReasonCode::Ready);
    assert_eq!(
        local.dimensions.model_availability,
        AiModelAvailability::Available
    );
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn local_chat_preflight_reports_a_missing_model_before_session_creation() {
    let mut server = mockito::Server::new_async().await;
    let _version = server
        .mock("GET", "/api/version")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"version":"0.11.0"}"#)
        .create_async()
        .await;
    let _models = server
        .mock("GET", "/api/tags")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"models":[{"name":"mistral:7b"}]}"#)
        .create_async()
        .await;
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: format!("{}/v1/responses", server.url()),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    });

    let preflight = probe_local_chat_preflight(&config.ai_provider).await;

    assert_eq!(
        preflight.model_availability,
        AiModelAvailability::Unavailable
    );
}

#[cfg(feature = "analysis")]
#[tokio::test]
async fn local_chat_preflight_refuses_non_loopback_targets_without_egress() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::LocalModel;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://ollama.example.test/v1/responses".to_string(),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    });

    let preflight = probe_local_chat_preflight(&config.ai_provider).await;

    assert_eq!(preflight.detection, AiProviderDetection::NotDetected);
    assert_eq!(
        preflight.invocation,
        AiProviderInvocationReadiness::Unavailable
    );
}
