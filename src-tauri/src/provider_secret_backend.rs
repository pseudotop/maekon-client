use std::path::{Path, PathBuf};
use std::sync::Arc;

use oneshim_core::config::{CredentialBackendKind, CredentialBinding};
use oneshim_core::error::CoreError;
use oneshim_core::ports::secret_store::SecretStore;
#[cfg(feature = "server")]
use oneshim_core::ports::secret_store::SecretStoreSet;
use oneshim_storage::env_secret_store::EnvSecretStore;
use oneshim_storage::file_secret_store::FileSecretStore;
use oneshim_storage::keychain::{KeychainOps, KeychainSecretStore};

pub const ONESHIM_PROVIDER_SECRET_BACKEND_ENV: &str = "ONESHIM_PROVIDER_SECRET_BACKEND";
pub const ONESHIM_PROVIDER_SECRET_FILE_ENV: &str = "ONESHIM_PROVIDER_SECRET_FILE";
pub const FILE_SECRET_STORE_NAME: &str = "oneshim-secrets.json";
const KEYCHAIN_REGISTRY_FILE_NAME: &str = "oneshim-keychain-registry.json";

#[derive(Clone)]
#[allow(dead_code)] // fields exposed for diagnostic logging in future
pub struct ProviderSecretBackendResolution {
    pub secret_store: Option<Arc<dyn SecretStore>>,
    pub backend_kind: CredentialBackendKind,
    pub fallback_backend_kind: CredentialBackendKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestedProviderSecretBackend {
    Auto,
    OsSecretStore,
    FileSecretStore,
    Env,
}

pub fn create_os_secret_store(config_dir: &Path) -> Option<Arc<dyn SecretStore>> {
    let registry_path = config_dir.join(KEYCHAIN_REGISTRY_FILE_NAME);
    match KeychainOps::new(registry_path) {
        Ok(ops) => Some(Arc::new(KeychainSecretStore::new(Arc::new(ops))) as Arc<dyn SecretStore>),
        Err(_) => None,
    }
}

pub fn create_file_secret_store(config_dir: &Path) -> Result<Arc<dyn SecretStore>, CoreError> {
    Ok(Arc::new(FileSecretStore::new(file_secret_store_path(config_dir))?) as Arc<dyn SecretStore>)
}

pub fn create_env_secret_store() -> Arc<dyn SecretStore> {
    Arc::new(EnvSecretStore::from_current_process()) as Arc<dyn SecretStore>
}

pub fn resolve_provider_secret_backend(
    config_dir: &Path,
    os_secret_store: Option<Arc<dyn SecretStore>>,
) -> Result<ProviderSecretBackendResolution, CoreError> {
    let requested_backend = requested_provider_secret_backend()?;

    let resolution = match requested_backend {
        RequestedProviderSecretBackend::Auto | RequestedProviderSecretBackend::OsSecretStore => {
            if let Some(secret_store) = os_secret_store {
                ProviderSecretBackendResolution {
                    secret_store: Some(secret_store),
                    backend_kind: CredentialBackendKind::OsSecretStore,
                    fallback_backend_kind: CredentialBackendKind::Unavailable,
                }
            } else {
                ProviderSecretBackendResolution {
                    secret_store: None,
                    backend_kind: CredentialBackendKind::Unavailable,
                    fallback_backend_kind: CredentialBackendKind::Unavailable,
                }
            }
        }
        RequestedProviderSecretBackend::FileSecretStore => ProviderSecretBackendResolution {
            secret_store: Some(Arc::new(FileSecretStore::new(file_secret_store_path(
                config_dir,
            ))?)),
            backend_kind: CredentialBackendKind::FileSecretStore,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        },
        RequestedProviderSecretBackend::Env => ProviderSecretBackendResolution {
            secret_store: Some(Arc::new(EnvSecretStore::from_current_process())),
            backend_kind: CredentialBackendKind::Env,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        },
    };

    Ok(resolution)
}

pub fn create_secret_store_for_binding(
    binding: Option<&CredentialBinding>,
    config_dir: &Path,
    os_secret_store: Option<Arc<dyn SecretStore>>,
) -> Result<Option<Arc<dyn SecretStore>>, CoreError> {
    let Some(binding) = binding else {
        return Ok(resolve_provider_secret_backend(config_dir, os_secret_store)?.secret_store);
    };

    match binding.backend_kind {
        CredentialBackendKind::OsSecretStore => Ok(os_secret_store),
        CredentialBackendKind::FileSecretStore => Ok(Some(create_file_secret_store(config_dir)?)),
        CredentialBackendKind::Env => Ok(Some(create_env_secret_store())),
        CredentialBackendKind::BridgeManaged | CredentialBackendKind::Unavailable => Ok(None),
    }
}

#[cfg(feature = "server")]
pub fn build_provider_secret_store_set(
    config_dir: &Path,
    os_secret_store: Option<Arc<dyn SecretStore>>,
    resolution: &ProviderSecretBackendResolution,
) -> SecretStoreSet {
    let file_secret_store = if resolution.backend_kind == CredentialBackendKind::FileSecretStore {
        resolution.secret_store.clone()
    } else {
        create_file_secret_store(config_dir).ok()
    };
    let env_secret_store = if resolution.backend_kind == CredentialBackendKind::Env {
        resolution.secret_store.clone()
    } else {
        Some(create_env_secret_store())
    };

    SecretStoreSet {
        os_secret_store,
        file_secret_store,
        env_secret_store,
        default_backend_kind: resolution.backend_kind,
        fallback_backend_kind: resolution.fallback_backend_kind,
    }
}

#[cfg(all(feature = "server", test))]
pub fn is_writable_backend_kind(backend_kind: CredentialBackendKind) -> bool {
    matches!(
        backend_kind,
        CredentialBackendKind::OsSecretStore | CredentialBackendKind::FileSecretStore
    )
}

fn requested_provider_secret_backend() -> Result<RequestedProviderSecretBackend, CoreError> {
    let Some(raw) = std::env::var(ONESHIM_PROVIDER_SECRET_BACKEND_ENV).ok() else {
        return Ok(RequestedProviderSecretBackend::Auto);
    };

    match raw.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => Ok(RequestedProviderSecretBackend::Auto),
        "os_secret_store" | "os" | "keychain" => Ok(RequestedProviderSecretBackend::OsSecretStore),
        "file_secret_store" | "file" => Ok(RequestedProviderSecretBackend::FileSecretStore),
        "env" => Ok(RequestedProviderSecretBackend::Env),
        other => Err(CoreError::Config { code: oneshim_core::error_codes::ConfigCode::Invalid, message: format!(
            "Unsupported provider secret backend '{other}'. Expected auto, os_secret_store, file_secret_store, or env."
        ) }),
    }
}

fn file_secret_store_path(config_dir: &Path) -> PathBuf {
    std::env::var_os(ONESHIM_PROVIDER_SECRET_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir.join(FILE_SECRET_STORE_NAME))
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::{CredentialAuthMode, SecretRef};
    use tempfile::TempDir;

    #[test]
    fn file_secret_store_path_defaults_to_config_dir() {
        let temp_dir = TempDir::new().unwrap();
        let path = file_secret_store_path(temp_dir.path());
        assert_eq!(path, temp_dir.path().join(FILE_SECRET_STORE_NAME));
    }

    #[cfg(feature = "server")]
    #[test]
    fn writable_backend_kind_matches_os_and_file() {
        assert!(is_writable_backend_kind(
            CredentialBackendKind::OsSecretStore
        ));
        assert!(is_writable_backend_kind(
            CredentialBackendKind::FileSecretStore
        ));
        assert!(!is_writable_backend_kind(CredentialBackendKind::Env));
        assert!(!is_writable_backend_kind(
            CredentialBackendKind::Unavailable
        ));
    }

    #[test]
    fn binding_store_resolution_respects_binding_backend() {
        let temp_dir = TempDir::new().unwrap();
        let binding = CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::FileSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/default".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: false,
        };

        let store = create_secret_store_for_binding(Some(&binding), temp_dir.path(), None).unwrap();
        assert!(store.is_some());
    }
}
