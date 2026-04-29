use crate::services::web_contexts::SettingsWebContext;
use crate::AppState;
use async_trait::async_trait;
use oneshim_api_contracts::settings::{
    AiProviderProfileConfig as ApiAiProviderProfileConfig, AppSettings, ExternalApiSettings,
    OcrValidationSettings, SceneActionOverrideSettings, SceneIntelligenceSettings,
};
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::error::CoreError;
use oneshim_core::ports::secret_store::SecretStore;
use oneshim_storage::sqlite::SqliteStorage;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use tokio::sync::broadcast;

pub(super) struct TestSecretStore {
    values: Mutex<HashMap<(String, String), String>>,
}

impl TestSecretStore {
    pub(super) fn new() -> Self {
        Self {
            values: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl SecretStore for TestSecretStore {
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .unwrap()
            .insert((namespace.to_string(), key.to_string()), value.to_string());
        Ok(())
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(&(namespace.to_string(), key.to_string()))
            .cloned())
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .unwrap()
            .remove(&(namespace.to_string(), key.to_string()));
        Ok(())
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
        self.values
            .lock()
            .unwrap()
            .retain(|(existing_namespace, _), _| existing_namespace != namespace);
        Ok(())
    }
}

pub(super) fn test_state_without_config_manager() -> AppState {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
    let (event_tx, _) = broadcast::channel(8);
    AppState::with_core(storage, event_tx)
}

pub(super) fn test_state_with_config_manager(
    config_manager: ConfigManager,
    secret_store: Option<Arc<dyn SecretStore>>,
) -> AppState {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
    let (event_tx, _) = broadcast::channel(8);
    let mut state = AppState::with_core(storage, event_tx);
    state.core.config_manager = Some(config_manager);
    state.secrets.default_backend_kind = oneshim_core::config::CredentialBackendKind::OsSecretStore;
    state.secrets.store = secret_store;
    state
}

pub(super) fn test_context_from_state(state: &AppState) -> SettingsWebContext {
    SettingsWebContext::from_state(state)
}

pub(super) fn anthropic_external_api_settings(api_key: &str) -> ExternalApiSettings {
    ExternalApiSettings {
        endpoint: "https://api.anthropic.com/v1/messages".to_string(),
        api_key_masked: api_key.to_string(),
        model: Some("claude-3-7-sonnet-latest".to_string()),
        provider_type: "Anthropic".to_string(),
        surface_id: Some("provider_surface.anthropic.direct_api".to_string()),
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: !api_key.is_empty(),
        can_edit_secret: true,
        secret_display_hint: None,
        projection_enabled: false,
    }
}

pub(super) fn anthropic_api_profile_config(api_key: &str) -> ApiAiProviderProfileConfig {
    let defaults = AppSettings::default();
    ApiAiProviderProfileConfig {
        access_mode: defaults.ai_provider.access_mode,
        ocr_provider: defaults.ai_provider.ocr_provider,
        llm_provider: "Remote".to_string(),
        external_data_policy: defaults.ai_provider.external_data_policy,
        allow_unredacted_external_ocr: defaults.ai_provider.allow_unredacted_external_ocr,
        ocr_validation: OcrValidationSettings::default(),
        scene_action_override: SceneActionOverrideSettings::default(),
        scene_intelligence: SceneIntelligenceSettings::default(),
        fallback_to_local: defaults.ai_provider.fallback_to_local,
        ocr_api: None,
        llm_api: Some(anthropic_external_api_settings(api_key)),
    }
}
