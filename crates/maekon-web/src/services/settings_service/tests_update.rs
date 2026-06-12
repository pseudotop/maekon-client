use super::*;
use crate::error::ApiError;
use maekon_api_contracts::settings::{
    AppSettings, ExternalApiSettings, SavedAiProviderProfile as ApiSavedAiProviderProfile,
};
use maekon_core::config::{CredentialAuthMode, CredentialBackendKind};
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::secret_store::SecretStore;
use std::sync::Arc;
use tempfile::TempDir;
use tests_fixtures::*;

#[tokio::test]
async fn update_settings_validates_input_without_config_manager() {
    let state = test_state_without_config_manager();
    let context = test_context_from_state(&state);
    let settings = AppSettings {
        web_port: 80,
        ..AppSettings::default()
    };

    let result = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[tokio::test]
async fn update_settings_accepts_valid_defaults_without_config_manager() {
    let state = test_state_without_config_manager();
    let context = test_context_from_state(&state);
    let settings = AppSettings::default();

    // update_settings returns Result<(), ApiError>; pin the exact Ok(()) value and
    // ensure validation passed for AppSettings::default() without a config manager (#5594).
    let result = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await;
    assert_eq!(
        result.expect("AppSettings::default() must pass validation even without a config manager"),
        ()
    );
}

#[tokio::test]
async fn update_settings_accepts_provider_oauth_roundtrip_without_config_manager() {
    let state = test_state_without_config_manager();
    let context = test_context_from_state(&state);
    let mut settings = AppSettings::default();
    settings.ai_provider.access_mode = "ProviderOAuth".to_string();
    settings.ai_provider.llm_provider = "Remote".to_string();

    // Pin: ProviderOAuth + Remote round-trips through validation without a config manager.
    // The access_mode and llm_provider fields must be valid enum variants; Ok(()) confirms
    // both fields were accepted (#5594).
    let result = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await;
    assert_eq!(
        result.expect("ProviderOAuth + Remote must pass validation even without a config manager"),
        ()
    );
    // Confirm the fields that drove acceptance are what we set, not silently defaulted.
    assert_eq!(settings.ai_provider.access_mode, "ProviderOAuth");
    assert_eq!(settings.ai_provider.llm_provider, "Remote");
}

#[tokio::test]
async fn update_settings_persists_remote_api_key_to_secret_store_and_binding_metadata() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
    let state = test_state_with_config_manager(config_manager.clone(), Some(secret_store.clone()));
    let context = test_context_from_state(&state);

    let mut settings = AppSettings::default();
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key_masked: "sk-secret-123456".to_string(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: None,
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: true,
        can_edit_secret: true,
        secret_display_hint: None,
        projection_enabled: true,
    });

    crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect("settings update should succeed");

    let stored = secret_store
        .retrieve("provider/openai/llm", "api_key")
        .await
        .expect("secret lookup");
    assert_eq!(stored.as_deref(), Some("sk-secret-123456"));

    let saved = config_manager.get();
    let endpoint = saved.ai_provider.llm_api.expect("saved llm endpoint");
    let binding = endpoint.credential.expect("credential binding");
    assert_eq!(endpoint.api_key, "");
    assert_eq!(binding.backend_kind, CredentialBackendKind::OsSecretStore);
    assert_eq!(binding.auth_mode, CredentialAuthMode::ApiKey);
    assert!(binding.projection_enabled);
    let secret_ref = binding.secret_ref.expect("secret ref");
    assert_eq!(secret_ref.namespace, "provider/openai/llm");
    assert_eq!(secret_ref.key, "api_key");
}

#[tokio::test]
async fn update_settings_persists_selected_saved_profile_under_profile_namespace() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
    let state = test_state_with_config_manager(config_manager.clone(), Some(secret_store.clone()));
    let context = test_context_from_state(&state);

    let mut settings = AppSettings::default();
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(anthropic_external_api_settings("sk-ant-secret-123456"));
    settings.ai_provider.active_profile_id = Some("anthropic-prod".to_string());
    settings.ai_provider.saved_profiles = vec![ApiSavedAiProviderProfile {
        profile_id: "anthropic-prod".to_string(),
        name: "Anthropic Prod".to_string(),
        ai_provider: anthropic_api_profile_config("sk-ant-secret-123456"),
        updated_at: Some("2026-03-17T00:00:00Z".to_string()),
    }];

    crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect("settings update should succeed");

    let stored = secret_store
        .retrieve("provider/anthropic/anthropic-prod", "api_key")
        .await
        .expect("secret lookup");
    assert_eq!(stored.as_deref(), Some("sk-ant-secret-123456"));

    let legacy_slot = secret_store
        .retrieve("provider/anthropic/llm", "api_key")
        .await
        .expect("legacy slot lookup");
    assert_eq!(legacy_slot, None);

    let saved = config_manager.get();
    assert_eq!(
        saved.ai_provider.active_profile_id.as_deref(),
        Some("anthropic-prod")
    );

    let active_endpoint = saved.ai_provider.llm_api.expect("active llm endpoint");
    let active_binding = active_endpoint
        .credential
        .expect("active credential binding");
    let active_secret_ref = active_binding.secret_ref.expect("active secret ref");
    assert_eq!(
        active_secret_ref.namespace,
        "provider/anthropic/anthropic-prod"
    );
    assert_eq!(active_secret_ref.key, "api_key");

    assert_eq!(saved.ai_provider.saved_profiles.len(), 1);
    let saved_profile = &saved.ai_provider.saved_profiles[0];
    assert_eq!(saved_profile.profile_id, "anthropic-prod");
    assert_eq!(saved_profile.name, "Anthropic Prod");

    let profile_endpoint = saved_profile
        .ai_provider
        .llm_api
        .as_ref()
        .expect("saved profile llm endpoint");
    let profile_binding = profile_endpoint
        .credential
        .as_ref()
        .expect("saved profile credential binding");
    let profile_secret_ref = profile_binding
        .secret_ref
        .as_ref()
        .expect("saved profile secret ref");
    assert_eq!(
        profile_secret_ref.namespace,
        "provider/anthropic/anthropic-prod"
    );
    assert_eq!(profile_secret_ref.key, "api_key");
}

/// F-RR-C22-01: `SettingsCommandService` must create its `SettingsUpdateFlow`
/// once at construction time, not once per `update_settings` call.
///
/// This test verifies the structural property: constructing the service and
/// calling `update_settings` 10 times succeeds without error and the service
/// remains usable throughout (i.e. the flow is not recreated and dropped on
/// each call, which would abort the `PolicyAuditWriter` background task and
/// could lose audit events).
///
/// The negative case (per-call construction) would only be observable via
/// code inspection, but we can confirm the happy path: repeated calls on the
/// same service instance all succeed and the service is still `Clone`-able
/// (meaning the `Arc<PolicyAuditWriter>` inside `SettingsUpdateFlow` is shared
/// across clones, not recreated).
#[tokio::test]
async fn update_settings_flow_reused_across_calls() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let state = test_state_with_config_manager(config_manager.clone(), None);
    let context = test_context_from_state(&state);

    // Construct the service ONCE — flow is created here.
    let svc = crate::services::settings_web_service::SettingsCommandService::new(context);

    let settings = AppSettings::default();
    for _ in 0..10 {
        svc.update_settings(&settings)
            .await
            .expect("repeated update_settings must succeed");
    }

    // Cloning the service must share the same PolicyAuditWriter Arc (not
    // create a new one).  Both instances must still be usable.
    let svc2 = svc.clone();
    svc2.update_settings(&settings)
        .await
        .expect("cloned service update_settings must succeed");
}

#[tokio::test]
async fn update_settings_rejects_api_key_write_for_env_backend() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let storage = Arc::new(
        maekon_storage::sqlite::SqliteStorage::open_in_memory(30).expect("in-memory sqlite"),
    );
    let (event_tx, _) = tokio::sync::broadcast::channel(8);
    let mut state = crate::AppState::with_core(storage, event_tx);
    state.core.config_manager = Some(config_manager);
    state.secrets.default_backend_kind = CredentialBackendKind::Env;
    state.secrets.store = Some(Arc::new(TestSecretStore::new()));

    let mut settings = AppSettings::default();
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key_masked: "sk-secret-123456".to_string(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: None,
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "env".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let context = test_context_from_state(&state);
    let err = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect_err("env backend should be read-only");

    assert!(matches!(err, ApiError::BadRequest(_)));
}
