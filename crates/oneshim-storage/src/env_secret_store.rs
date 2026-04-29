//! Environment-backed secret store.
//!
//! This backend is read-only and intended for explicit headless or CI use.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::secret_store::{secret_env_var_name, SecretStore};

#[derive(Clone)]
pub struct EnvSecretStore {
    snapshot: Arc<HashMap<String, String>>,
}

impl EnvSecretStore {
    pub fn from_current_process() -> Self {
        Self {
            snapshot: Arc::new(std::env::vars().collect()),
        }
    }

    pub fn from_snapshot(snapshot: HashMap<String, String>) -> Self {
        Self {
            snapshot: Arc::new(snapshot),
        }
    }

    fn read_only_error() -> CoreError {
        CoreError::SecretStoreError { code: oneshim_core::error_codes::SecretCode::Failed, message: "environment-backed secret store is read-only; modify the environment source instead"
                .to_string(), }
    }
}

#[async_trait]
impl SecretStore for EnvSecretStore {
    async fn store(&self, _namespace: &str, _key: &str, _value: &str) -> Result<(), CoreError> {
        Err(Self::read_only_error())
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        let env_name = secret_env_var_name(namespace, key);
        Ok(self.snapshot.get(&env_name).cloned())
    }

    async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), CoreError> {
        Err(Self::read_only_error())
    }

    async fn delete_namespace(&self, _namespace: &str) -> Result<(), CoreError> {
        Err(Self::read_only_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::ports::secret_store::SecretStore;

    #[tokio::test]
    async fn env_secret_store_reads_canonical_env_name() {
        let mut snapshot = HashMap::new();
        snapshot.insert(
            "ONESHIM_SECRET_PROVIDER_OPENAI_DEFAULT_API_KEY".to_string(),
            "sk-test".to_string(),
        );
        let store = EnvSecretStore::from_snapshot(snapshot);

        let value = store
            .retrieve("provider/openai/default", "api_key")
            .await
            .unwrap();
        assert_eq!(value.as_deref(), Some("sk-test"));
    }

    #[tokio::test]
    async fn env_secret_store_is_read_only() {
        let store = EnvSecretStore::from_snapshot(HashMap::new());
        let err = store
            .store("provider/openai/default", "api_key", "sk-test")
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::SecretStoreError { .. }));
    }
}
