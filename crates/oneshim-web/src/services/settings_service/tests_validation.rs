use super::*;
use crate::error::ApiError;
use crate::services::settings_assembler::config_to_settings;
use oneshim_api_contracts::settings::{AiProviderSettings, AppSettings, ExternalApiSettings};
use oneshim_core::config::{
    AiAccessMode, AiProviderType, AppConfig, CredentialAuthMode, CredentialBackendKind,
    CredentialBinding, ExternalApiEndpoint, LlmProviderType, OcrProviderType, SecretRef,
};

#[test]
fn apply_settings_to_config_rejects_allow_external_without_integration_token() {
    let mut config = AppConfig::default_config();
    let settings = AppSettings {
        allow_external: true,
        ..AppSettings::default()
    };

    let err = apply_settings_to_config(&mut config, &settings)
        .expect_err("external access should require integration token");
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("integration_auth_token"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn apply_settings_to_config_preserves_projection_enabled_on_existing_binding() {
    let mut config = AppConfig::default_config();
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

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.as_mut().expect("llm settings");
    llm_api.projection_enabled = true;

    apply_settings_to_config(&mut config, &settings).expect("config update");

    let binding = config
        .ai_provider
        .llm_api
        .as_ref()
        .and_then(|endpoint| endpoint.credential.as_ref())
        .expect("binding");
    assert!(binding.projection_enabled);
}

#[test]
fn apply_settings_to_config_rejects_projection_enabled_for_env_backend() {
    let mut config = AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::Env);
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key_masked: String::new(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "openai".to_string(),
        surface_id: None,
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "env".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: true,
    });

    let err = apply_settings_to_config(&mut config, &settings).expect_err("projection guard");
    assert!(matches!(err, ApiError::BadRequest(_)));
}

#[test]
fn apply_settings_to_config_rejects_projection_enabled_for_managed_oauth() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderOAuth;
    config.ai_provider.llm_provider = LlmProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key_masked: String::new(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "openai".to_string(),
        surface_id: None,
        timeout_secs: 30,
        auth_mode: "managed_oauth".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: true,
    });

    let err = apply_settings_to_config(&mut config, &settings).expect_err("projection guard");
    assert!(matches!(err, ApiError::BadRequest(_)));
}

#[test]
fn apply_settings_to_config_rejects_incompatible_surface_id_for_oauth_mode() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderOAuth;
    config.ai_provider.llm_provider = LlmProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key_masked: String::new(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "openai".to_string(),
        surface_id: Some("provider_surface.openai.direct_api".to_string()),
        timeout_secs: 30,
        auth_mode: "managed_oauth".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err = apply_settings_to_config(&mut config, &settings).expect_err("surface guard");
    assert!(matches!(err, ApiError::BadRequest(_)));
}

#[test]
fn apply_settings_to_config_rejects_projection_enabled_for_bridge_managed_backend() {
    let mut config = AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;

    let settings = AppSettings {
        ai_provider: AiProviderSettings {
            llm_provider: "Remote".to_string(),
            llm_api: Some(ExternalApiSettings {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key_masked: String::new(),
                model: Some("gpt-5.4".to_string()),
                provider_type: "OpenAi".to_string(),
                surface_id: None,
                timeout_secs: 30,
                auth_mode: "api_key".to_string(),
                backend_kind: "bridge_managed".to_string(),
                has_secret: true,
                can_edit_secret: false,
                secret_display_hint: None,
                projection_enabled: true,
            }),
            ..AiProviderSettings::default()
        },
        ..AppSettings::default()
    };

    let err = apply_settings_to_config(&mut config, &settings).unwrap_err();
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("Projection is not supported"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn apply_settings_to_config_rewrites_binding_when_switching_to_oauth_mode() {
    let mut config = AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: "sk-test-123456".to_string(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: Some("provider_surface.openai.direct_api".to_string()),
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: true,
        }),
    });

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.access_mode = "ProviderOAuth".to_string();
    let llm_api = settings
        .ai_provider
        .llm_api
        .as_mut()
        .expect("llm api settings");
    llm_api.surface_id = Some("provider_surface.openai.managed_oauth".to_string());
    llm_api.auth_mode = "managed_oauth".to_string();
    llm_api.backend_kind = "os_secret_store".to_string();
    llm_api.projection_enabled = false;

    apply_settings_to_config(&mut config, &settings).expect("oauth mode rewrite");

    let endpoint = config.ai_provider.llm_api.as_ref().expect("llm endpoint");
    let binding = endpoint.credential.as_ref().expect("credential binding");
    assert_eq!(binding.auth_mode, CredentialAuthMode::ManagedOAuth);
    assert_eq!(binding.backend_kind, CredentialBackendKind::OsSecretStore);
    assert!(binding.secret_ref.is_none());
    assert!(!binding.projection_enabled);
    assert!(endpoint.api_key.is_empty());
}

#[test]
fn apply_settings_to_config_rewrites_binding_when_switching_from_cli_bridge_to_api_key() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_provider = LlmProviderType::Local;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: String::new(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: Some("provider_surface.openai.subprocess_cli".to_string()),
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::CliBridge,
            backend_kind: CredentialBackendKind::BridgeManaged,
            secret_ref: None,
            projection_enabled: false,
        }),
    });

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.access_mode = "ProviderApiKey".to_string();
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1/responses".to_string(),
        api_key_masked: String::new(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: Some("provider_surface.openai.direct_api".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: false,
        can_edit_secret: true,
        secret_display_hint: None,
        projection_enabled: false,
    });

    apply_settings_to_config(&mut config, &settings).expect("api key mode rewrite");

    let endpoint = config.ai_provider.llm_api.as_ref().expect("llm endpoint");
    let binding = endpoint.credential.as_ref().expect("credential binding");
    assert_eq!(binding.auth_mode, CredentialAuthMode::ApiKey);
    assert_eq!(binding.backend_kind, CredentialBackendKind::OsSecretStore);
    assert!(binding.secret_ref.is_none());
    assert_eq!(
        endpoint.surface_id.as_deref(),
        Some("provider_surface.openai.direct_api")
    );
}

#[test]
fn apply_settings_to_config_allows_direct_ocr_surface_in_cli_mode() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.access_mode = "ProviderSubscriptionCli".to_string();
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
        api_key_masked: "sk-ocr-123456".to_string(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: Some("provider_surface.openai.direct_api".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: true,
        can_edit_secret: true,
        secret_display_hint: None,
        projection_enabled: false,
    });

    apply_settings_to_config(&mut config, &settings).expect("cli mode should keep direct OCR");

    let endpoint = config.ai_provider.ocr_api.as_ref().expect("ocr endpoint");
    assert_eq!(
        endpoint.surface_id.as_deref(),
        Some("provider_surface.openai.direct_api")
    );
    assert_eq!(endpoint.api_key, "sk-ocr-123456");
}

#[test]
fn apply_settings_to_config_rejects_cli_auth_mode_for_ocr_in_api_key_mode() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.access_mode = "ProviderApiKey".to_string();
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: String::new(),
        api_key_masked: String::new(),
        model: None,
        provider_type: "OpenAi".to_string(),
        surface_id: Some("provider_surface.openai.subprocess_cli".to_string()),
        timeout_secs: 30,
        auth_mode: "cli_bridge".to_string(),
        backend_kind: "bridge_managed".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err = apply_settings_to_config(&mut config, &settings).expect_err("ocr cli mode guard");
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("ProviderApiKey"));
            assert!(message.contains("OCR"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn apply_settings_to_config_allows_subprocess_ocr_surface_in_cli_mode() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.access_mode = "ProviderSubscriptionCli".to_string();
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: String::new(),
        api_key_masked: String::new(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: Some("provider_surface.openai.subprocess_cli".to_string()),
        timeout_secs: 30,
        auth_mode: "cli_bridge".to_string(),
        backend_kind: "bridge_managed".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    apply_settings_to_config(&mut config, &settings).expect("ocr subprocess surface");

    let endpoint = config.ai_provider.ocr_api.as_ref().expect("ocr endpoint");
    assert_eq!(
        endpoint.surface_id.as_deref(),
        Some("provider_surface.openai.subprocess_cli")
    );
    let binding = endpoint.credential.as_ref().expect("credential binding");
    assert_eq!(binding.auth_mode, CredentialAuthMode::CliBridge);
    assert_eq!(binding.backend_kind, CredentialBackendKind::BridgeManaged);
}

#[test]
fn apply_settings_to_config_rejects_managed_ocr_surface_in_cli_mode() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.access_mode = "ProviderSubscriptionCli".to_string();
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: String::new(),
        api_key_masked: String::new(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: Some("provider_surface.openai.managed_oauth".to_string()),
        timeout_secs: 30,
        auth_mode: "managed_oauth".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err = apply_settings_to_config(&mut config, &settings).expect_err("ocr managed guard");
    assert!(matches!(err, ApiError::BadRequest(_)));
}

#[test]
fn apply_settings_to_config_rejects_managed_ocr_surface_in_oauth_mode() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderOAuth;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.access_mode = "ProviderOAuth".to_string();
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: String::new(),
        api_key_masked: String::new(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: Some("provider_surface.openai.managed_oauth".to_string()),
        timeout_secs: 30,
        auth_mode: "managed_oauth".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err =
        apply_settings_to_config(&mut config, &settings).expect_err("oauth ocr managed guard");
    assert!(matches!(err, ApiError::BadRequest(_)));
}

#[test]
fn apply_settings_to_config_rejects_text_only_ollama_model_for_ocr() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: "http://localhost:11434/v1/chat/completions".to_string(),
        api_key_masked: String::new(),
        model: Some("qwen3:8b".to_string()),
        provider_type: "Ollama".to_string(),
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "unavailable".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err = apply_settings_to_config(&mut config, &settings).expect_err("ollama OCR model guard");
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("OCR-capable"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn apply_settings_to_config_rejects_google_ocr_model_override() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: "https://vision.googleapis.com/v1/images:annotate".to_string(),
        api_key_masked: "goog-key-123456".to_string(),
        model: Some("gemini-2.5-flash".to_string()),
        provider_type: "Google".to_string(),
        surface_id: Some("provider_surface.google.direct_api".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: true,
        can_edit_secret: true,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err = apply_settings_to_config(&mut config, &settings).expect_err("google OCR model guard");
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("does not support configurable OCR model selection"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn apply_settings_to_config_requires_explicit_model_for_local_openai_compatible_surface() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.llm_provider = LlmProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "http://127.0.0.1:1234/v1/chat/completions".to_string(),
        api_key_masked: String::new(),
        model: None,
        provider_type: "Generic".to_string(),
        surface_id: Some("provider_surface.generic.local_openai_compatible".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "unavailable".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err = apply_settings_to_config(&mut config, &settings)
        .expect_err("surface should require explicit model selection");
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("requires an explicit LLM model selection"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn apply_settings_to_config_rejects_non_structured_model_for_local_openai_compatible_ocr() {
    let mut config = AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderApiKey;
    config.ai_provider.ocr_provider = OcrProviderType::Remote;

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.ocr_provider = "Remote".to_string();
    settings.ai_provider.ocr_api = Some(ExternalApiSettings {
        endpoint: "http://127.0.0.1:1234/v1/chat/completions".to_string(),
        api_key_masked: String::new(),
        model: Some("text-embedding-3-small".to_string()),
        provider_type: "Generic".to_string(),
        surface_id: Some("provider_surface.generic.local_openai_compatible".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "unavailable".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let err = apply_settings_to_config(&mut config, &settings)
        .expect_err("surface should reject non-structured OCR model");
    match err {
        ApiError::BadRequest(message) => {
            assert!(
                message.contains("structured JSON output")
                    || message.contains("OCR-capable")
                    || message.contains("not marked as OCR-capable")
            );
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn apply_settings_to_config_clears_secret_binding_for_ollama_surface() {
    let mut config = AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1/responses".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: Some("provider_surface.openai.direct_api".to_string()),
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: true,
        }),
    });

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "http://localhost:11434/v1/responses".to_string(),
        api_key_masked: String::new(),
        model: Some("qwen3:8b".to_string()),
        provider_type: "Ollama".to_string(),
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "unavailable".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    apply_settings_to_config(&mut config, &settings).expect("ollama no-auth save should work");

    let endpoint = config.ai_provider.llm_api.expect("saved endpoint");
    assert_eq!(endpoint.provider_type, AiProviderType::Ollama);
    assert_eq!(
        endpoint.surface_id.as_deref(),
        Some("provider_surface.ollama.local_http")
    );
    assert!(endpoint.api_key.is_empty());
    assert!(endpoint.credential.is_none());
}

#[test]
fn apply_settings_to_config_rejects_backend_change_for_existing_binding() {
    let mut config = AppConfig::default_config();
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

    let mut settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings
        .ai_provider
        .llm_api
        .as_mut()
        .expect("llm api settings");
    llm_api.backend_kind = "file_secret_store".to_string();

    let err = apply_settings_to_config(&mut config, &settings).unwrap_err();
    match err {
        ApiError::BadRequest(message) => {
            assert!(message.contains("Changing provider credential auth mode or backend"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}
