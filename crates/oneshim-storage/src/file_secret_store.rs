//! Explicit file-backed secret store.
//!
//! This backend is intended for headless, CI, or explicit fallback scenarios.
//! It is not the desktop-default backend.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::StorageError;
use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::secret_store::SecretStore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileSecretRegistry {
    version: u32,
    namespaces: HashMap<String, HashMap<String, String>>,
}

impl FileSecretRegistry {
    fn new() -> Self {
        Self {
            version: 1,
            namespaces: HashMap::new(),
        }
    }

    fn load_or_default(path: &Path) -> Result<Self, StorageError> {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents)
                .map_err(|e| StorageError::SecretStore(format!("file secret registry parse: {e}"))),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::new()),
            Err(err) => Err(err.into()),
        }
    }

    fn save(&self, path: &Path) -> Result<(), StorageError> {
        let serialized = serde_json::to_string_pretty(self).map_err(|e| {
            StorageError::SecretStore(format!("file secret registry serialization: {e}"))
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp_path = path.with_extension("tmp");
        std::fs::write(&temp_path, &serialized)?;

        // Set owner-only permissions before atomic rename so the file is never
        // world-readable on disk.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o600)).map_err(
                |e| {
                    StorageError::SecretStore(format!(
                        "file secret store permission set failed: {e}"
                    ))
                },
            )?;
        }

        std::fs::rename(&temp_path, path)?;
        Ok(())
    }

    fn store(&mut self, namespace: &str, key: &str, value: &str) {
        self.namespaces
            .entry(namespace.to_string())
            .or_default()
            .insert(key.to_string(), value.to_string());
    }

    fn retrieve(&self, namespace: &str, key: &str) -> Option<String> {
        self.namespaces
            .get(namespace)
            .and_then(|keys| keys.get(key))
            .cloned()
    }

    fn delete(&mut self, namespace: &str, key: &str) {
        if let Some(keys) = self.namespaces.get_mut(namespace) {
            keys.remove(key);
            if keys.is_empty() {
                self.namespaces.remove(namespace);
            }
        }
    }

    fn delete_namespace(&mut self, namespace: &str) {
        self.namespaces.remove(namespace);
    }
}

struct FileSecretInner {
    registry_path: PathBuf,
    registry: parking_lot::Mutex<FileSecretRegistry>,
}

impl FileSecretInner {
    fn new(registry_path: PathBuf) -> Result<Self, StorageError> {
        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let registry = FileSecretRegistry::load_or_default(&registry_path)?;
        Ok(Self {
            registry_path,
            registry: parking_lot::Mutex::new(registry),
        })
    }

    fn store_sync(&self, namespace: &str, key: &str, value: &str) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.store(namespace, key, value);
        registry.save(&self.registry_path)
    }

    fn retrieve_sync(&self, namespace: &str, key: &str) -> Option<String> {
        self.registry.lock().retrieve(namespace, key)
    }

    fn delete_sync(&self, namespace: &str, key: &str) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.delete(namespace, key);
        registry.save(&self.registry_path)
    }

    fn delete_namespace_sync(&self, namespace: &str) -> Result<(), StorageError> {
        let mut registry = self.registry.lock();
        registry.delete_namespace(namespace);
        registry.save(&self.registry_path)
    }
}

#[derive(Clone)]
pub struct FileSecretStore {
    inner: Arc<FileSecretInner>,
}

impl FileSecretStore {
    pub fn new(registry_path: PathBuf) -> Result<Self, StorageError> {
        Ok(Self {
            inner: Arc::new(FileSecretInner::new(registry_path)?),
        })
    }
}

#[async_trait]
impl SecretStore for FileSecretStore {
    async fn store(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        let value = value.to_string();
        tokio::task::spawn_blocking(move || inner.store_sync(&namespace, &key, &value))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: oneshim_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn retrieve(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || Ok(inner.retrieve_sync(&namespace, &key)))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: oneshim_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
    }

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || inner.delete_sync(&namespace, &key))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: oneshim_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<(), CoreError> {
        let inner = self.inner.clone();
        let namespace = namespace.to_string();
        tokio::task::spawn_blocking(move || inner.delete_namespace_sync(&namespace))
            .await
            .map_err(|e| CoreError::SecretStoreError {
                code: oneshim_core::error_codes::SecretCode::Failed,
                message: format!("spawn_blocking: {e}"),
            })?
            .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::ports::secret_store::SecretStore;

    #[tokio::test]
    async fn file_secret_store_roundtrips_values() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(temp_dir.path().join("secrets.json")).unwrap();

        store
            .store("provider/openai/default", "api_key", "sk-test")
            .await
            .unwrap();

        let value = store
            .retrieve("provider/openai/default", "api_key")
            .await
            .unwrap();
        assert_eq!(value.as_deref(), Some("sk-test"));
    }

    #[tokio::test]
    async fn file_secret_store_persists_to_disk() {
        let temp_dir = tempfile::tempdir().unwrap();
        let path = temp_dir.path().join("secrets.json");

        FileSecretStore::new(path.clone())
            .unwrap()
            .store("provider/openai/default", "api_key", "sk-test")
            .await
            .unwrap();

        let reloaded = FileSecretStore::new(path).unwrap();
        let value = reloaded
            .retrieve("provider/openai/default", "api_key")
            .await
            .unwrap();
        assert_eq!(value.as_deref(), Some("sk-test"));
    }

    #[tokio::test]
    async fn file_secret_store_delete_namespace_removes_only_target_namespace() {
        let temp_dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(temp_dir.path().join("secrets.json")).unwrap();

        store
            .store("provider/openai/default", "api_key", "a")
            .await
            .unwrap();
        store
            .store("provider/anthropic/default", "api_key", "b")
            .await
            .unwrap();

        store
            .delete_namespace("provider/openai/default")
            .await
            .unwrap();

        assert_eq!(
            store
                .retrieve("provider/openai/default", "api_key")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .retrieve("provider/anthropic/default", "api_key")
                .await
                .unwrap()
                .as_deref(),
            Some("b")
        );
    }
}
