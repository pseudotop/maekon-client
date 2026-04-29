//! Secret store port — canonical storage for provider credentials.
//!
//! Secrets (OAuth tokens, API keys, bridge credentials) must not default to
//! plaintext config storage. This port abstracts OS keychain, explicit file
//! backends, environment-backed sources, or in-memory test doubles.

use std::sync::Arc;

use async_trait::async_trait;

use crate::config::{CredentialBackendKind, CredentialBinding};
use crate::error::CoreError;

pub const DEFAULT_SECRET_PROFILE: &str = "default";
pub const PROVIDER_API_KEY_SECRET_KEY: &str = "api_key";
pub const PROVIDER_OAUTH_SESSION_SECRET_KEY: &str = "oauth_session";
pub const INTEGRATION_AUTH_SECRET_NAMESPACE: &str = "integration/auth/default";
pub const INTEGRATION_ACCESS_TOKEN_SECRET_KEY: &str = "access_token";
pub const INTEGRATION_REFRESH_TOKEN_SECRET_KEY: &str = "refresh_token";
pub const INTEGRATION_EXPIRES_AT_SECRET_KEY: &str = "expires_at";
pub const INTEGRATION_DPOP_SIGNING_KEY_SECRET_KEY: &str = "dpop_signing_key";

pub fn validate_secret_segment(raw: &str, field_name: &str) -> Result<String, CoreError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CoreError::InvalidArguments {
            code: crate::error_codes::ValidationCode::InvalidArguments,
            message: format!("{field_name} must not be empty"),
        });
    }

    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(CoreError::InvalidArguments {
            code: crate::error_codes::ValidationCode::InvalidArguments,
            message: format!(
                "{field_name} must contain only ASCII alphanumeric characters, '.', '_' or '-'"
            ),
        });
    }

    Ok(trimmed.to_string())
}

pub fn provider_secret_namespace(provider_id: &str, profile_id: &str) -> Result<String, CoreError> {
    let provider_id = validate_secret_segment(provider_id, "provider_id")?;
    let profile_id = validate_secret_segment(profile_id, "profile_id")?;
    Ok(format!("provider/{provider_id}/{profile_id}"))
}

pub fn provider_api_key_secret_ref(
    provider_id: &str,
    profile_id: &str,
) -> Result<(String, &'static str), CoreError> {
    Ok((
        provider_secret_namespace(provider_id, profile_id)?,
        PROVIDER_API_KEY_SECRET_KEY,
    ))
}

pub fn provider_oauth_session_secret_ref(
    provider_id: &str,
    profile_id: &str,
) -> Result<(String, &'static str), CoreError> {
    Ok((
        provider_secret_namespace(provider_id, profile_id)?,
        PROVIDER_OAUTH_SESSION_SECRET_KEY,
    ))
}

pub fn secret_env_var_name(namespace: &str, key: &str) -> String {
    let normalized_namespace = namespace
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    let normalized_key = key
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("ONESHIM_SECRET_{normalized_namespace}_{normalized_key}")
}

/// Secure secret storage abstraction.
///
/// Implementations may use OS keychain (macOS Keychain, Windows Credential
/// Manager, Linux Secret Service) or an in-memory fallback.
///
/// # Errors
/// - `CoreError::SecretStoreError` (wire: `secret.failed`) for backend
///   failures: keychain unavailable, read-only env backend, file secret
///   store I/O. Env-backed store rejects writes as `secret.failed` with
///   an informative message ("read-only; modify the environment source
///   instead").
/// - `CoreError::InvalidArguments` (wire: `validation.invalid_arguments`)
///   from the `validate_secret_segment` helper when namespace/key
///   contains non-ASCII-alphanumeric characters or is empty.
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Store a secret value under a namespaced key.
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError>;

    /// Retrieve a secret value. Returns `None` if not found.
    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError>;

    /// Delete a secret value. No-op if key does not exist.
    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError>;

    /// Delete all secrets under a namespace.
    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError>;
}

#[derive(Clone)]
pub struct SecretStoreSet {
    pub os_secret_store: Option<Arc<dyn SecretStore>>,
    pub file_secret_store: Option<Arc<dyn SecretStore>>,
    pub env_secret_store: Option<Arc<dyn SecretStore>>,
    pub default_backend_kind: CredentialBackendKind,
    pub fallback_backend_kind: CredentialBackendKind,
}

impl SecretStoreSet {
    pub fn for_backend_kind(
        &self,
        backend_kind: CredentialBackendKind,
    ) -> Option<Arc<dyn SecretStore>> {
        match backend_kind {
            CredentialBackendKind::OsSecretStore => self.os_secret_store.clone(),
            CredentialBackendKind::FileSecretStore => self.file_secret_store.clone(),
            CredentialBackendKind::Env => self.env_secret_store.clone(),
            CredentialBackendKind::BridgeManaged | CredentialBackendKind::Unavailable => None,
        }
    }

    pub fn for_binding(&self, binding: Option<&CredentialBinding>) -> Option<Arc<dyn SecretStore>> {
        let backend_kind = binding
            .map(|value| value.backend_kind)
            .unwrap_or(self.default_backend_kind);
        self.for_backend_kind(backend_kind)
    }

    pub fn default_store(&self) -> Option<Arc<dyn SecretStore>> {
        self.for_backend_kind(self.default_backend_kind)
    }
}

impl Default for SecretStoreSet {
    fn default() -> Self {
        Self {
            os_secret_store: None,
            file_secret_store: None,
            env_secret_store: None,
            default_backend_kind: CredentialBackendKind::Unavailable,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// In-memory secret store for testing.
    pub struct InMemorySecretStore {
        store: Mutex<HashMap<String, String>>,
    }

    impl InMemorySecretStore {
        pub fn new() -> Self {
            Self {
                store: Mutex::new(HashMap::new()),
            }
        }

        fn make_key(namespace: &str, key: &str) -> String {
            format!("{namespace}.{key}")
        }
    }

    #[async_trait]
    impl SecretStore for InMemorySecretStore {
        async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
            let mut map = self.store.lock().unwrap();
            map.insert(Self::make_key(namespace, key), value.to_string());
            Ok(())
        }

        async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
            let map = self.store.lock().unwrap();
            Ok(map.get(&Self::make_key(namespace, key)).cloned())
        }

        async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
            let mut map = self.store.lock().unwrap();
            map.remove(&Self::make_key(namespace, key));
            Ok(())
        }

        async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
            let mut map = self.store.lock().unwrap();
            let prefix = format!("{namespace}.");
            map.retain(|k, _| !k.starts_with(&prefix));
            Ok(())
        }
    }

    #[tokio::test]
    async fn store_and_retrieve() {
        let store = InMemorySecretStore::new();
        store
            .store("openai", "access_token", "tok_abc")
            .await
            .unwrap();
        let val = store.retrieve("openai", "access_token").await.unwrap();
        assert_eq!(val, Some("tok_abc".to_string()));
    }

    #[tokio::test]
    async fn retrieve_missing_returns_none() {
        let store = InMemorySecretStore::new();
        let val = store.retrieve("openai", "missing").await.unwrap();
        assert_eq!(val, None);
    }

    #[tokio::test]
    async fn delete_removes_key() {
        let store = InMemorySecretStore::new();
        store.store("openai", "token", "val").await.unwrap();
        store.delete("openai", "token").await.unwrap();
        let val = store.retrieve("openai", "token").await.unwrap();
        assert_eq!(val, None);
    }

    #[tokio::test]
    async fn delete_namespace_removes_all_keys() {
        let store = InMemorySecretStore::new();
        store.store("openai", "access", "a").await.unwrap();
        store.store("openai", "refresh", "b").await.unwrap();
        store.store("other", "key", "c").await.unwrap();
        store.delete_namespace("openai").await.unwrap();
        assert_eq!(store.retrieve("openai", "access").await.unwrap(), None);
        assert_eq!(store.retrieve("openai", "refresh").await.unwrap(), None);
        assert_eq!(
            store.retrieve("other", "key").await.unwrap(),
            Some("c".to_string())
        );
    }

    #[tokio::test]
    async fn namespace_isolation() {
        let store = InMemorySecretStore::new();
        store.store("openai", "token", "openai_tok").await.unwrap();
        store.store("openrouter", "token", "or_tok").await.unwrap();
        assert_eq!(
            store.retrieve("openai", "token").await.unwrap(),
            Some("openai_tok".to_string())
        );
        assert_eq!(
            store.retrieve("openrouter", "token").await.unwrap(),
            Some("or_tok".to_string())
        );
    }

    #[test]
    fn provider_namespace_uses_stable_shape() {
        let namespace = provider_secret_namespace("openai", DEFAULT_SECRET_PROFILE).unwrap();
        assert_eq!(namespace, "provider/openai/default");
    }

    #[test]
    fn provider_secret_ref_uses_api_key_key_name() {
        let (namespace, key) = provider_api_key_secret_ref("openrouter", "team").unwrap();
        assert_eq!(namespace, "provider/openrouter/team");
        assert_eq!(key, PROVIDER_API_KEY_SECRET_KEY);
    }

    #[test]
    fn provider_namespace_rejects_invalid_segments() {
        let err = provider_secret_namespace("openai/codex", "default").unwrap_err();
        assert!(matches!(err, CoreError::InvalidArguments { .. }));
    }

    #[test]
    fn env_secret_var_name_normalizes_namespace_and_key() {
        let env_name = secret_env_var_name("provider/openai/default", "api_key");
        assert_eq!(env_name, "ONESHIM_SECRET_PROVIDER_OPENAI_DEFAULT_API_KEY");
    }

    #[test]
    fn secret_store_set_uses_binding_backend_kind() {
        let os_store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
        let file_store: Arc<dyn SecretStore> = Arc::new(InMemorySecretStore::new());
        let stores = SecretStoreSet {
            os_secret_store: Some(os_store.clone()),
            file_secret_store: Some(file_store.clone()),
            env_secret_store: None,
            default_backend_kind: CredentialBackendKind::OsSecretStore,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        };
        let binding = CredentialBinding {
            auth_mode: crate::config::CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::FileSecretStore,
            secret_ref: None,
            projection_enabled: false,
        };

        let selected = stores.for_binding(Some(&binding)).expect("selected store");
        assert!(Arc::ptr_eq(&selected, &file_store));
        assert!(Arc::ptr_eq(
            &stores.default_store().expect("default store"),
            &os_store
        ));
    }
}
