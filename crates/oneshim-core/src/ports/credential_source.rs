//! Credential source abstraction for provider authentication.
//!
//! Replaces direct `String api_key` dependency with a resolution-time
//! abstraction that supports both BYOK API keys and managed OAuth tokens.

use std::sync::Arc;

use crate::config::{CredentialAuthMode, CredentialBackendKind, ExternalApiEndpoint};
use crate::error::CoreError;
use crate::ports::oauth::OAuthPort;
use crate::ports::secret_store::{provider_api_key_secret_ref, SecretStore};
use crate::provider_surface::{provider_surface_uses_no_auth, provider_vendor_id_or_default};

// --- Type definitions ---

/// Source of authentication credentials for AI provider requests.
///
/// # Errors
/// Resolution methods on this enum emit (verified against
/// `CredentialSource::from_api_key_endpoint_for_profile` + `resolve_bearer_token`):
/// - `CoreError::Config` with `ConfigCode::Invalid` (wire:
///   `config.invalid`) — secret store not initialized when the backend
///   requires one. The condition is a broken-config semantic ("your
///   credential backend declares it needs a secret store, but one isn't
///   wired") rather than a Missing-field semantic.
/// - `CoreError::Config` with `ConfigCode::Missing` (wire:
///   `config.missing`) — `profile_id` required for env-backed
///   credentials but not supplied (iter-99); or the terminal fallback
///   "AI provider API key is not configured in a supported secret
///   backend" path (no matching credential binding at all).
/// - `CoreError::OAuthError` (wire: `oauth.failed`) when managed-OAuth
///   token lookup fails (provider not registered, token expired beyond
///   refresh window).
/// - `CoreError::Auth` (wire: `auth.failed`) when the resolved secret
///   entry is absent from the backend at resolve time
///   (`StoredSecret::retrieve` returns `Ok(None)`).
#[derive(Clone)]
pub enum CredentialSource {
    /// No credential is required for this transport surface.
    NoAuth,

    /// User-provided API key (current default, BYOK).
    ApiKey(String),

    /// Managed OAuth token resolved at request time via OAuthPort.
    ///
    /// `api_base_url` is the provider's API endpoint for OAuth-authenticated
    /// requests. For OpenAI ChatGPT OAuth this is `chatgpt.com/backend-api/codex`,
    /// NOT the standard `api.openai.com/v1`.
    ManagedOAuth {
        provider_id: String,
        oauth_port: Arc<dyn OAuthPort>,
        /// API base URL for authenticated requests (differs per auth mode).
        api_base_url: String,
    },

    /// Secret resolved from a backend-managed secret store.
    StoredSecret {
        namespace: String,
        key: String,
        secret_store: Arc<dyn SecretStore>,
    },
}

// --- Resolution logic ---

impl CredentialSource {
    /// Build an API-key credential source from the configured secret backend.
    pub fn from_api_key_endpoint(
        endpoint: &ExternalApiEndpoint,
        secret_store: Option<Arc<dyn SecretStore>>,
    ) -> Result<Self, CoreError> {
        Self::from_api_key_endpoint_for_profile(endpoint, None, secret_store)
    }

    pub fn from_api_key_endpoint_for_profile(
        endpoint: &ExternalApiEndpoint,
        profile_id: Option<&str>,
        secret_store: Option<Arc<dyn SecretStore>>,
    ) -> Result<Self, CoreError> {
        if endpoint
            .surface_id
            .as_deref()
            .is_some_and(provider_surface_uses_no_auth)
        {
            return Ok(Self::NoAuth);
        }

        if let Some(binding) = endpoint.credential.as_ref() {
            if binding.auth_mode == CredentialAuthMode::ApiKey {
                let secret_store = secret_store.ok_or_else(|| CoreError::Config {
                    code: crate::error_codes::ConfigCode::Invalid,
                    message: format!(
                        "provider credential backend {:?} requires an initialized secret store",
                        binding.backend_kind
                    ),
                })?;

                if let Some(secret_ref) = binding.secret_ref.as_ref() {
                    return Ok(Self::StoredSecret {
                        namespace: secret_ref.namespace.clone(),
                        key: secret_ref.key.clone(),
                        secret_store,
                    });
                }

                if binding.backend_kind == CredentialBackendKind::Env {
                    let profile_id = profile_id.ok_or_else(|| CoreError::Config {
                        code: crate::error_codes::ConfigCode::Missing,
                        message:
                            "profile_id is required to resolve env-backed provider credentials"
                                .to_string(),
                    })?;
                    let (namespace, key) = provider_api_key_secret_ref(
                        provider_vendor_id_or_default(endpoint.provider_type),
                        profile_id,
                    )?;
                    return Ok(Self::StoredSecret {
                        namespace,
                        key: key.to_string(),
                        secret_store,
                    });
                }
            }
        }

        Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: "AI provider API key is not configured in a supported secret backend. Save it through Settings or configure an environment-backed credential source."
                .to_string(),
        })
    }

    /// Resolve to a bearer token string at request time.
    ///
    /// For `ApiKey`, returns the key directly.
    /// For `ManagedOAuth`, calls `OAuthPort::get_access_token` (may trigger refresh).
    pub async fn resolve_bearer_token(&self) -> Result<String, CoreError> {
        match self {
            Self::NoAuth => Ok(String::new()),
            Self::ApiKey(key) => Ok(key.clone()),
            Self::ManagedOAuth {
                provider_id,
                oauth_port,
                ..
            } => oauth_port
                .get_access_token(provider_id)
                .await?
                .ok_or_else(|| CoreError::OAuthError {
                    code: crate::error_codes::OAuthCode::Failed,
                    provider: provider_id.clone(),
                    message: "not authenticated — please connect via OAuth".into(),
                }),
            Self::StoredSecret {
                namespace,
                key,
                secret_store,
            } => {
                if let Some(secret) = secret_store.retrieve(namespace, key).await? {
                    return Ok(secret);
                }

                Err(CoreError::Auth {
                    code: crate::error_codes::AuthCode::Failed,
                    message: format!("credential backend entry not found for {namespace}.{key}"),
                })
            }
        }
    }

    // --- Helper functions ---

    /// Whether this source is a managed OAuth credential.
    pub fn is_managed(&self) -> bool {
        matches!(self, Self::ManagedOAuth { .. })
    }

    /// API base URL for the credential source.
    ///
    /// Returns `Some(url)` for `ManagedOAuth`, `None` for `ApiKey`
    /// (API key users configure their own endpoint).
    pub fn api_base_url(&self) -> Option<&str> {
        match self {
            Self::NoAuth => None,
            Self::ApiKey(_) => None,
            Self::ManagedOAuth { api_base_url, .. } => Some(api_base_url),
            Self::StoredSecret { .. } => None,
        }
    }
}

impl std::fmt::Debug for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAuth => write!(f, "CredentialSource::NoAuth"),
            Self::ApiKey(_) => write!(f, "CredentialSource::ApiKey(****)"),
            Self::ManagedOAuth { provider_id, .. } => {
                write!(f, "CredentialSource::ManagedOAuth({provider_id})")
            }
            Self::StoredSecret { namespace, key, .. } => {
                write!(f, "CredentialSource::StoredSecret({namespace}.{key})")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AiProviderType, CredentialBackendKind, CredentialBinding, ExternalApiEndpoint, SecretRef,
    };
    use crate::ports::secret_store::SecretStore;
    use async_trait::async_trait;
    use std::collections::HashMap;
    use std::sync::Mutex;

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

    #[tokio::test]
    async fn api_key_resolves_directly() {
        let source = CredentialSource::ApiKey("sk-test-key".to_string());
        let token = source.resolve_bearer_token().await.unwrap();
        assert_eq!(token, "sk-test-key");
        assert!(!source.is_managed());
    }

    #[test]
    fn debug_redacts_api_key() {
        let source = CredentialSource::ApiKey("sk-secret".to_string());
        let debug = format!("{source:?}");
        assert!(!debug.contains("sk-secret"));
        assert!(debug.contains("****"));
    }

    #[tokio::test]
    async fn stored_secret_prefers_backend_value() {
        let store = Arc::new(TestSecretStore::new());
        store
            .store("provider/openai/default", "api_key", "sk-backend")
            .await
            .unwrap();

        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "sk-legacy".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ApiKey,
                backend_kind: CredentialBackendKind::OsSecretStore,
                secret_ref: Some(SecretRef {
                    namespace: "provider/openai/default".to_string(),
                    key: "api_key".to_string(),
                }),
                projection_enabled: false,
            }),
        };

        let source = CredentialSource::from_api_key_endpoint(&endpoint, Some(store)).unwrap();
        let token = source.resolve_bearer_token().await.unwrap();
        assert_eq!(token, "sk-backend");
    }

    #[tokio::test]
    async fn stored_secret_errors_when_backend_entry_is_missing() {
        let store = Arc::new(TestSecretStore::new());
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "sk-legacy".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ApiKey,
                backend_kind: CredentialBackendKind::OsSecretStore,
                secret_ref: Some(SecretRef {
                    namespace: "provider/openai/default".to_string(),
                    key: "api_key".to_string(),
                }),
                projection_enabled: false,
            }),
        };

        let source = CredentialSource::from_api_key_endpoint(&endpoint, Some(store)).unwrap();
        let err = source.resolve_bearer_token().await.unwrap_err();
        assert!(matches!(err, CoreError::Auth { .. }));
    }

    #[test]
    fn plaintext_inline_api_keys_are_rejected_without_supported_binding() {
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "sk-inline".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: None,
        };

        let err = CredentialSource::from_api_key_endpoint(&endpoint, None).unwrap_err();
        assert!(matches!(err, CoreError::Config { .. }));
    }

    #[test]
    fn secret_bound_backend_requires_initialized_store() {
        let endpoint = ExternalApiEndpoint {
            endpoint: "https://api.openai.com/v1".to_string(),
            api_key: "sk-legacy".to_string(),
            model: Some("gpt-5.4".to_string()),
            timeout_secs: 30,
            provider_type: AiProviderType::OpenAi,
            surface_id: None,
            credential: Some(CredentialBinding {
                auth_mode: CredentialAuthMode::ApiKey,
                backend_kind: CredentialBackendKind::OsSecretStore,
                secret_ref: Some(SecretRef {
                    namespace: "provider/openai/default".to_string(),
                    key: "api_key".to_string(),
                }),
                projection_enabled: false,
            }),
        };

        let err = CredentialSource::from_api_key_endpoint(&endpoint, None).unwrap_err();
        assert!(matches!(err, CoreError::Config { .. }));
    }

    #[tokio::test]
    async fn env_backed_secret_without_secret_ref_uses_profile_context() {
        let store = Arc::new(TestSecretStore::new());
        store
            .store("provider/openai/llm", "api_key", "sk-env")
            .await
            .unwrap();

        let endpoint = ExternalApiEndpoint {
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
        };

        let source = CredentialSource::from_api_key_endpoint_for_profile(
            &endpoint,
            Some("llm"),
            Some(store),
        )
        .unwrap();
        let token = source.resolve_bearer_token().await.unwrap();
        assert_eq!(token, "sk-env");
    }
}
