const MAX_ERROR_SNIPPET_CHARS: usize = 220;

pub(crate) fn truncate_error(raw: &str) -> String {
    let compact = raw.replace(['\n', '\r'], " ");
    let compact = compact.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(MAX_ERROR_SNIPPET_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use crate::error::ApiError;
    use crate::services::ai_model_catalog_assembler::{
        build_model_details, parse_google_models, parse_standard_models, ParsedModelRecord,
    };
    use crate::services::ai_model_catalog_auth::{
        resolve_model_discovery_api_key, resolve_saved_model_discovery_api_key,
    };
    use crate::services::ai_model_catalog_endpoint::{
        derive_model_catalog_endpoint_from_surface, resolve_models_endpoint,
    };
    use crate::services::web_contexts::AiModelCatalogWebContext;
    use crate::AppState;
    use async_trait::async_trait;
    use oneshim_api_contracts::ai_providers::{ProviderModelSupportStatus, ProviderModelsRequest};
    use oneshim_api_contracts::provider_specs::resolved_surface_spec;
    use oneshim_core::config::{
        AiProviderConfig, AiProviderType, AppConfig, CredentialAuthMode, CredentialBackendKind,
        CredentialBinding, ExternalApiEndpoint, SecretRef,
    };
    use oneshim_core::config_manager::ConfigManager;
    use oneshim_core::error::CoreError;
    use oneshim_core::ports::secret_store::SecretStore;
    use oneshim_storage::sqlite::SqliteStorage;
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;
    use tempfile::TempDir;
    use tokio::sync::broadcast;

    struct TestSecretStore {
        values: Mutex<HashMap<(String, String), String>>,
    }

    impl TestSecretStore {
        fn new() -> Self {
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

    fn test_state_with_saved_secret(
        config: AppConfig,
        secret_store: Arc<dyn SecretStore>,
    ) -> AppState {
        let temp_dir = TempDir::new().expect("temp dir");
        let config_path = temp_dir.path().join("config.json");
        let config_manager = ConfigManager::with_path(config_path).expect("config manager");
        config_manager.update(config).expect("save config");
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"));
        let (event_tx, _) = broadcast::channel(8);
        let mut state = AppState::with_core(storage, event_tx);
        state.core.config_manager = Some(config_manager);
        state.secrets.store = Some(secret_store);
        state
    }

    fn test_context_with_saved_secret(
        config: AppConfig,
        secret_store: Arc<dyn SecretStore>,
    ) -> AiModelCatalogWebContext {
        AiModelCatalogWebContext::from_state(&test_state_with_saved_secret(config, secret_store))
    }

    #[test]
    fn derives_google_models_endpoint_from_generate_content_url() {
        let surface = resolved_surface_spec(
            AiProviderType::Google,
            Some("provider_surface.google.direct_api"),
        )
        .expect("google surface should resolve");
        let endpoint =
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash:generateContent";
        let derived = derive_model_catalog_endpoint_from_surface(surface, endpoint).unwrap();
        assert_eq!(
            derived,
            "https://generativelanguage.googleapis.com/v1beta/models"
        );
    }

    #[test]
    fn derives_openai_models_endpoint_from_chat_completions_url() {
        let surface = resolved_surface_spec(
            AiProviderType::OpenAi,
            Some("provider_surface.openai.direct_api"),
        )
        .expect("openai surface should resolve");
        let endpoint = "https://api.openai.com/v1/chat/completions";
        let derived = derive_model_catalog_endpoint_from_surface(surface, endpoint).unwrap();
        assert_eq!(derived, "https://api.openai.com/v1/models");
    }

    #[test]
    fn derives_openai_models_endpoint_from_responses_url() {
        let surface = resolved_surface_spec(
            AiProviderType::OpenAi,
            Some("provider_surface.openai.direct_api"),
        )
        .expect("openai surface should resolve");
        let endpoint = "https://api.openai.com/v1/responses";
        let derived = derive_model_catalog_endpoint_from_surface(surface, endpoint).unwrap();
        assert_eq!(derived, "https://api.openai.com/v1/models");
    }

    #[test]
    fn derives_ollama_models_endpoint_from_responses_url() {
        let surface = resolved_surface_spec(
            AiProviderType::Ollama,
            Some("provider_surface.ollama.local_http"),
        )
        .expect("ollama surface should resolve");
        let endpoint = "http://localhost:11434/v1/responses";
        let derived = derive_model_catalog_endpoint_from_surface(surface, endpoint).unwrap();
        assert_eq!(derived, "http://localhost:11434/api/tags");
    }

    #[test]
    fn derives_generic_local_openai_compatible_models_endpoint_from_v1_base() {
        let surface = resolved_surface_spec(
            AiProviderType::Generic,
            Some("provider_surface.generic.local_openai_compatible"),
        )
        .expect("generic local openai-compatible surface should resolve");
        let endpoint = "http://127.0.0.1:1234/v1";
        let derived = derive_model_catalog_endpoint_from_surface(surface, endpoint).unwrap();
        assert_eq!(derived, "http://127.0.0.1:1234/v1/models");
    }

    #[test]
    fn parses_google_model_catalog() {
        let body = r#"{
          "models": [
            {
              "name": "models/gemini-2.5-flash",
              "displayName": "Gemini 2.5 Flash",
              "supportedGenerationMethods": ["generateContent"]
            },
            {
              "name": "models/text-embedding-004",
              "supportedGenerationMethods": ["embedContent"]
            }
          ]
        }"#;
        let value: Value = serde_json::from_str(body).unwrap();
        let parsed = parse_google_models(&value).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "gemini-2.5-flash");
        assert_eq!(parsed[0].display_name.as_deref(), Some("Gemini 2.5 Flash"));
    }

    #[test]
    fn parses_standard_model_catalog() {
        let body = r#"{
          "data": [
            {"id": "gpt-5.4"},
            {"id": "gpt-5.2"}
          ]
        }"#;
        let value: Value = serde_json::from_str(body).unwrap();
        let parsed = parse_standard_models(&value).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].id, "gpt-5.4");
        assert_eq!(parsed[1].id, "gpt-5.2");
    }

    #[test]
    fn builds_model_details_from_known_surface_models() {
        let details = build_model_details(
            AiProviderType::OpenAi,
            Some("provider_surface.openai.direct_api"),
            &[ParsedModelRecord {
                id: "text-embedding-3-small".to_string(),
                display_name: Some("Text Embedding 3 Small".to_string()),
            }],
        )
        .expect("model details should build");
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0].llm_support,
            Some(ProviderModelSupportStatus::Unsupported)
        );
        assert_eq!(
            details[0].ocr_support,
            Some(ProviderModelSupportStatus::Unsupported)
        );
        assert_eq!(details[0].supports_ocr, Some(false));
        assert_eq!(
            details[0].image_input_support,
            Some(ProviderModelSupportStatus::Unsupported)
        );
        assert_eq!(
            details[0].structured_output_support,
            Some(ProviderModelSupportStatus::Unsupported)
        );
    }

    #[test]
    fn builds_google_image_input_support_from_known_models() {
        let details = build_model_details(
            AiProviderType::Google,
            Some("provider_surface.google.direct_api"),
            &[ParsedModelRecord {
                id: "gemini-2.5-flash".to_string(),
                display_name: Some("Gemini 2.5 Flash".to_string()),
            }],
        )
        .expect("google model details should build");
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0].image_input_support,
            Some(ProviderModelSupportStatus::Supported)
        );
        assert_eq!(
            details[0].ocr_support,
            Some(ProviderModelSupportStatus::Unsupported)
        );
    }

    #[test]
    fn builds_capability_rule_details_for_local_openai_compatible_models() {
        let details = build_model_details(
            AiProviderType::Generic,
            Some("provider_surface.generic.local_openai_compatible"),
            &[ParsedModelRecord {
                id: "qwen2.5-vl-7b-instruct".to_string(),
                display_name: Some("Qwen 2.5 VL 7B".to_string()),
            }],
        )
        .expect("local openai-compatible model details should build");
        assert_eq!(details.len(), 1);
        assert_eq!(
            details[0].ocr_support,
            Some(ProviderModelSupportStatus::Supported)
        );
        assert_eq!(
            details[0].image_input_support,
            Some(ProviderModelSupportStatus::Supported)
        );
        assert_eq!(
            details[0].capability_source.as_deref(),
            Some("capability_rules")
        );
    }

    #[tokio::test]
    async fn resolve_model_discovery_api_key_uses_saved_secret_binding() {
        let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
        secret_store
            .store("provider/openai/llm", "api_key", "sk-saved")
            .await
            .unwrap();

        let mut config = AppConfig::default_config();
        config.ai_provider = AiProviderConfig {
            llm_api: Some(ExternalApiEndpoint {
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
            }),
            ..AiProviderConfig::default()
        };
        let context = test_context_with_saved_secret(config, secret_store);
        let request = ProviderModelsRequest {
            provider_type: "OpenAi".to_string(),
            api_key: String::new(),
            endpoint: Some("https://api.openai.com/v1".to_string()),
            surface: Some("llm_api".to_string()),
            surface_id: None,
            use_saved_secret: true,
        };

        let resolved = resolve_model_discovery_api_key(&request, &context, AiProviderType::OpenAi)
            .await
            .unwrap();
        assert_eq!(resolved, "sk-saved");
    }

    #[tokio::test]
    async fn resolve_model_discovery_api_key_accepts_legacy_default_surface_id() {
        let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
        secret_store
            .store("provider/openai/llm", "api_key", "sk-legacy-surface")
            .await
            .unwrap();

        let mut config = AppConfig::default_config();
        config.ai_provider = AiProviderConfig {
            llm_api: Some(ExternalApiEndpoint {
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
            }),
            ..AiProviderConfig::default()
        };

        let context = test_context_with_saved_secret(config, secret_store);
        let request = ProviderModelsRequest {
            provider_type: "OpenAi".to_string(),
            api_key: String::new(),
            endpoint: Some("https://api.openai.com/v1".to_string()),
            surface: Some("llm_api".to_string()),
            surface_id: Some("provider_surface.openai.direct_api".to_string()),
            use_saved_secret: true,
        };

        let resolved = resolve_model_discovery_api_key(&request, &context, AiProviderType::OpenAi)
            .await
            .unwrap();
        assert_eq!(resolved, "sk-legacy-surface");
    }

    #[test]
    fn resolve_models_endpoint_rejects_unsupported_surface_catalog() {
        let error = resolve_models_endpoint(
            AiProviderType::OpenAi,
            Some("provider_surface.openai.managed_oauth"),
            None,
        )
        .expect_err("managed oauth should not expose model discovery");

        assert!(matches!(error, ApiError::BadRequest(_)));
    }

    #[tokio::test]
    async fn resolve_model_discovery_api_key_rejects_endpoint_mismatch_for_saved_secret() {
        let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
        let mut config = AppConfig::default_config();
        config.ai_provider = AiProviderConfig {
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: "sk-legacy".to_string(),
                model: Some("gpt-5.4".to_string()),
                timeout_secs: 30,
                provider_type: AiProviderType::OpenAi,
                surface_id: None,
                credential: None,
            }),
            ..AiProviderConfig::default()
        };
        let context = test_context_with_saved_secret(config, secret_store);
        let request = ProviderModelsRequest {
            provider_type: "OpenAi".to_string(),
            api_key: String::new(),
            endpoint: Some("https://proxy.example.com/v1".to_string()),
            surface: Some("llm_api".to_string()),
            surface_id: None,
            use_saved_secret: true,
        };

        let resolved =
            resolve_saved_model_discovery_api_key(&request, &context, AiProviderType::OpenAi)
                .await
                .unwrap();
        assert!(resolved.is_none());
    }

    #[tokio::test]
    async fn resolve_model_discovery_api_key_uses_env_backend_without_secret_ref() {
        let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
        secret_store
            .store("provider/openai/llm", "api_key", "sk-env")
            .await
            .unwrap();

        let mut config = AppConfig::default_config();
        config.ai_provider = AiProviderConfig {
            llm_api: Some(ExternalApiEndpoint {
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
            }),
            ..AiProviderConfig::default()
        };
        let context = test_context_with_saved_secret(config, secret_store);
        let request = ProviderModelsRequest {
            provider_type: "OpenAi".to_string(),
            api_key: String::new(),
            endpoint: Some("https://api.openai.com/v1".to_string()),
            surface: Some("llm_api".to_string()),
            surface_id: None,
            use_saved_secret: true,
        };

        let resolved = resolve_model_discovery_api_key(&request, &context, AiProviderType::OpenAi)
            .await
            .unwrap();
        assert_eq!(resolved, "sk-env");
    }
}
