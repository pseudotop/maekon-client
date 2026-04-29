use crate::services::settings_assembler::config_to_settings;
use oneshim_core::config::{
    AiAccessMode, AiProviderProfileConfig, AiProviderType, CredentialAuthMode,
    CredentialBackendKind, CredentialBinding, ExternalApiEndpoint, LlmProviderType,
    OcrProviderType, SavedAiProviderProfile, SecretRef,
};

#[test]
fn config_to_settings_maps_plaintext_api_keys_to_current_default_backend() {
    let mut config = oneshim_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.example.com/v1".to_string(),
        api_key: "sk-test-1234567890".to_string(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 45,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "api_key");
    assert_eq!(llm_api.backend_kind, "os_secret_store");
    assert!(llm_api.has_secret);
    assert!(llm_api.can_edit_secret);
    assert_eq!(llm_api.secret_display_hint.as_deref(), Some("sk...7890"));
    assert!(!llm_api.projection_enabled);
}

#[test]
fn config_to_settings_marks_ollama_surface_as_no_auth() {
    let mut config = oneshim_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "http://localhost:11434/v1/responses".to_string(),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "api_key");
    assert_eq!(llm_api.backend_kind, "unavailable");
    assert!(!llm_api.has_secret);
    assert!(!llm_api.can_edit_secret);
    assert!(llm_api.api_key_masked.is_empty());
}

#[test]
fn config_to_settings_marks_provider_oauth_as_managed_oauth_metadata() {
    let mut config = oneshim_core::config::AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderOAuth;
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "managed_oauth");
    assert_eq!(llm_api.backend_kind, "os_secret_store");
    assert_eq!(
        llm_api.surface_id.as_deref(),
        Some("provider_surface.openai.managed_oauth")
    );
    assert!(!llm_api.has_secret);
    assert!(!llm_api.can_edit_secret);
    assert_eq!(llm_api.secret_display_hint, None);
}

#[test]
fn config_to_settings_roundtrips_saved_ai_provider_profiles() {
    let mut config = oneshim_core::config::AppConfig::default_config();
    config.ai_provider.active_profile_id = Some("anthropic-prod".to_string());
    config.ai_provider.saved_profiles = vec![SavedAiProviderProfile {
        profile_id: "anthropic-prod".to_string(),
        name: "Anthropic Prod".to_string(),
        ai_provider: AiProviderProfileConfig {
            access_mode: AiAccessMode::ProviderApiKey,
            ocr_provider: OcrProviderType::Local,
            llm_provider: LlmProviderType::Remote,
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.anthropic.com/v1/messages".to_string(),
                api_key: String::new(),
                model: Some("claude-3-7-sonnet-latest".to_string()),
                timeout_secs: 45,
                provider_type: AiProviderType::Anthropic,
                surface_id: Some("provider_surface.anthropic.direct_api".to_string()),
                credential: Some(CredentialBinding {
                    auth_mode: CredentialAuthMode::ApiKey,
                    backend_kind: CredentialBackendKind::OsSecretStore,
                    secret_ref: Some(SecretRef {
                        namespace: "provider/anthropic/anthropic-prod".to_string(),
                        key: "api_key".to_string(),
                    }),
                    projection_enabled: false,
                }),
            }),
            ..AiProviderProfileConfig::default()
        },
        updated_at: Some("2026-03-17T00:00:00Z".to_string()),
    }];

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);

    assert_eq!(
        settings.ai_provider.active_profile_id.as_deref(),
        Some("anthropic-prod")
    );
    assert_eq!(settings.ai_provider.saved_profiles.len(), 1);

    let saved_profile = &settings.ai_provider.saved_profiles[0];
    assert_eq!(saved_profile.profile_id, "anthropic-prod");
    assert_eq!(saved_profile.name, "Anthropic Prod");
    assert_eq!(
        saved_profile.updated_at.as_deref(),
        Some("2026-03-17T00:00:00Z")
    );
    assert_eq!(saved_profile.ai_provider.access_mode, "ProviderApiKey");
    assert_eq!(saved_profile.ai_provider.llm_provider, "Remote");

    let llm_api = saved_profile
        .ai_provider
        .llm_api
        .as_ref()
        .expect("saved profile llm api");
    assert_eq!(llm_api.provider_type, "Anthropic");
    assert_eq!(
        llm_api.surface_id.as_deref(),
        Some("provider_surface.anthropic.direct_api")
    );
    assert_eq!(llm_api.timeout_secs, 45);
    assert!(llm_api.has_secret);
    assert_eq!(llm_api.backend_kind, "os_secret_store");
}

#[test]
fn config_to_settings_marks_cli_subscription_as_cli_bridge_metadata() {
    let mut config = oneshim_core::config::AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_provider = LlmProviderType::Local;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: String::new(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "cli_bridge");
    assert_eq!(llm_api.backend_kind, "bridge_managed");
    assert_eq!(
        llm_api.surface_id.as_deref(),
        Some("provider_surface.openai.subprocess_cli")
    );
    assert!(!llm_api.can_edit_secret);
    assert_eq!(llm_api.secret_display_hint, None);
}

#[test]
fn config_to_settings_keeps_backend_managed_api_key_without_fake_mask() {
    let mut config = oneshim_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: false,
        }),
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.backend_kind, "os_secret_store");
    assert!(llm_api.has_secret);
    assert_eq!(llm_api.api_key_masked, "");
    assert_eq!(llm_api.secret_display_hint, None);
}

#[test]
fn config_to_settings_marks_env_bound_api_key_as_present_without_secret_ref() {
    let mut config = oneshim_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::Env,
            secret_ref: None,
            projection_enabled: false,
        }),
    });

    let settings = config_to_settings(&config, CredentialBackendKind::Env);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.backend_kind, "env");
    assert!(llm_api.has_secret);
    assert!(!llm_api.can_edit_secret);
    assert_eq!(llm_api.api_key_masked, "");
    assert_eq!(llm_api.secret_display_hint, None);
}
