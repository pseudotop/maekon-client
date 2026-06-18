//! Secret projection adapter for MAEKON-managed temp files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::StorageError;
use async_trait::async_trait;
use maekon_core::config::AiProviderType;
use maekon_core::error::CoreError;
use maekon_core::ports::secret_projection::{
    provider_api_key_temp_file_consumer_id, ProjectionTarget, ProjectionTemplate,
    SecretProjectionPort, SecretProjectionRequest, SecretProjectionResult,
};
use maekon_core::ports::secret_store::SecretStore;
use maekon_core::provider_surface::provider_projection_for_type;
use tempfile::Builder;

const DEFAULT_PROJECTION_DIR_NAME: &str = "secret-projections";

#[derive(Clone)]
pub struct TempFileSecretProjection {
    secret_store: Arc<dyn SecretStore>,
    templates: Arc<HashMap<String, ProjectionTemplate>>,
    projection_dir: PathBuf,
    registry_path: PathBuf,
}

impl TempFileSecretProjection {
    pub fn new(
        secret_store: Arc<dyn SecretStore>,
        projection_dir: PathBuf,
        registry_path: PathBuf,
        templates: impl IntoIterator<Item = ProjectionTemplate>,
    ) -> Self {
        let templates = templates
            .into_iter()
            .map(|template| (template.consumer_id.clone(), template))
            .collect();

        Self {
            secret_store,
            templates: Arc::new(templates),
            projection_dir,
            registry_path,
        }
    }

    pub fn with_default_provider_api_key_cli_templates(
        secret_store: Arc<dyn SecretStore>,
        base_dir: &Path,
    ) -> Self {
        Self::new(
            secret_store,
            std::env::temp_dir().join(DEFAULT_PROJECTION_DIR_NAME),
            base_dir.join("temp-secret-projection-registry.json"),
            default_provider_api_key_temp_file_templates(),
        )
    }

    async fn load_registry(&self) -> Result<HashMap<String, PathBuf>, StorageError> {
        let registry_path = self.registry_path.clone();
        tokio::task::spawn_blocking(move || {
            if !registry_path.exists() {
                return Ok(HashMap::new());
            }

            let raw = std::fs::read_to_string(&registry_path).map_err(|e| {
                StorageError::Internal(format!(
                    "failed to read temp projection registry ({}): {}",
                    registry_path.display(),
                    e
                ))
            })?;

            let stored: HashMap<String, String> = serde_json::from_str(&raw).map_err(|e| {
                StorageError::Internal(format!(
                    "failed to parse temp projection registry ({}): {}",
                    registry_path.display(),
                    e
                ))
            })?;

            Ok(stored
                .into_iter()
                .map(|(consumer_id, path)| (consumer_id, PathBuf::from(path)))
                .collect())
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    async fn save_registry(&self, registry: &HashMap<String, PathBuf>) -> Result<(), StorageError> {
        let registry_path = self.registry_path.clone();
        let stored: HashMap<String, String> = registry
            .iter()
            .map(|(consumer_id, path)| (consumer_id.clone(), path.display().to_string()))
            .collect();

        tokio::task::spawn_blocking(move || {
            if let Some(parent) = registry_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    StorageError::Internal(format!(
                        "failed to create temp projection registry dir ({}): {}",
                        parent.display(),
                        e
                    ))
                })?;
            }

            let json = serde_json::to_string_pretty(&stored).map_err(|e| {
                StorageError::Internal(format!(
                    "failed to serialize temp projection registry ({}): {}",
                    registry_path.display(),
                    e
                ))
            })?;

            std::fs::write(&registry_path, json).map_err(|e| {
                StorageError::Internal(format!(
                    "failed to write temp projection registry ({}): {}",
                    registry_path.display(),
                    e
                ))
            })?;

            Ok(())
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }

    fn file_prefix(template: &ProjectionTemplate) -> String {
        let raw = template
            .file_name_hint
            .clone()
            .unwrap_or_else(|| template.consumer_id.clone());
        let normalized = raw
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() {
                    ch.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>();
        format!("maekon-{}-", normalized.trim_matches('-'))
    }

    async fn remove_file_if_exists(path: PathBuf) -> Result<(), StorageError> {
        tokio::task::spawn_blocking(move || match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(StorageError::Internal(format!(
                "failed to remove projected temp file ({}): {}",
                path.display(),
                err
            ))),
        })
        .await
        .map_err(|e| StorageError::Internal(format!("spawn_blocking join error: {e}")))?
    }
}

pub fn provider_api_key_temp_file_template(
    provider_type: AiProviderType,
    profile_id: &str,
) -> Result<ProjectionTemplate, StorageError> {
    let projection = provider_projection_for_type(provider_type).ok_or_else(|| {
        StorageError::Config(format!(
            "missing provider projection metadata for provider type '{provider_type:?}'"
        ))
    })?;

    Ok(ProjectionTemplate::temp_file(
        provider_api_key_temp_file_consumer_id(&projection.vendor_id, profile_id)?,
        format!(
            "{}-{profile_id}-api-key",
            projection.api_key_temp_file_prefix
        ),
    ))
}

pub fn default_provider_api_key_temp_file_templates() -> Vec<ProjectionTemplate> {
    let mut templates = Vec::new();

    for provider_type in [
        AiProviderType::OpenAi,
        AiProviderType::Anthropic,
        AiProviderType::Google,
        AiProviderType::Ollama,
        AiProviderType::Generic,
    ] {
        for profile_id in ["llm", "ocr"] {
            if let Ok(template) = provider_api_key_temp_file_template(provider_type, profile_id) {
                templates.push(template);
            }
        }
    }

    templates
}

#[async_trait]
impl SecretProjectionPort for TempFileSecretProjection {
    async fn project(
        &self,
        request: SecretProjectionRequest,
    ) -> Result<SecretProjectionResult, CoreError> {
        if request.target != ProjectionTarget::TempFile {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: "temp-file projection adapter only supports ProjectionTarget::TempFile"
                    .to_string(),
            });
        }

        let template =
            self.templates
                .get(&request.consumer_id)
                .ok_or_else(|| CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Invalid,
                    message: format!(
                        "no temp-file projection template registered for consumer '{}'",
                        request.consumer_id
                    ),
                })?;

        let secret = self
            .secret_store
            .retrieve(&request.namespace, &request.key)
            .await?
            .ok_or_else(|| CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!(
                    "secret not found for projection request {}:{}",
                    request.namespace, request.key
                ),
            })?;

        // F-RC-10: use tokio::fs::create_dir_all in async context
        tokio::fs::create_dir_all(&self.projection_dir)
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!(
                    "failed to create temp projection dir ({}): {}",
                    self.projection_dir.display(),
                    e
                ),
            })?;

        let mut registry = self.load_registry().await?;
        if let Some(existing_path) = registry.get(&request.consumer_id).cloned() {
            Self::remove_file_if_exists(existing_path).await?;
        }

        let temp_file = Builder::new()
            .prefix(&Self::file_prefix(template))
            .suffix(".secret")
            .tempfile_in(&self.projection_dir)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!(
                    "failed to create projected temp file in {}: {}",
                    self.projection_dir.display(),
                    e
                ),
            })?;
        temp_file
            .as_file()
            .set_len(0)
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("failed to initialize temp file: {e}"),
            })?;
        // #6131: On Windows the owner-only DACL MUST be applied while the file is
        // still EMPTY — BEFORE the cleartext secret is written. The `tempfile`
        // builder creates the file inheriting the parent-directory ACL, so if we
        // wrote the secret first there would be a window where the cleartext key
        // is exposed under the inherited (potentially world-readable) ACL. This
        // mirrors `EncryptionKey::save_to_file` (encryption.rs): create empty →
        // protect → write. DACL failure is a HARD error — we remove the file and
        // bail rather than leave a cleartext secret unprotected.
        //
        // On Unix the analogous protection (chmod 0o600) is applied to the
        // already-created tempfile after the write below; this is safe because
        // `tempfile` opens with mode 0o600 by default, so there is no
        // world-readable window on Unix.
        #[cfg(windows)]
        {
            let temp_path = temp_file.path().to_owned();
            tokio::task::spawn_blocking(move || {
                crate::encryption::set_owner_only_dacl(&temp_path).map_err(|e| {
                    let _ = std::fs::remove_file(&temp_path);
                    CoreError::Internal {
                        code: maekon_core::error_codes::InternalCode::Generic,
                        message: format!(
                            "failed to secure projected temp file ({}): {}",
                            temp_path.display(),
                            e
                        ),
                    }
                })
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking join error: {e}"),
            })??;
        }
        // F-RC-10: use tokio::fs::write in async context. On Windows the file is
        // already protected by the owner-only DACL applied above, so the
        // cleartext secret never touches disk under an inherited ACL (#6131).
        tokio::fs::write(temp_file.path(), secret)
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!(
                    "failed to write projected temp file ({}): {}",
                    temp_file.path().display(),
                    e
                ),
            })?;
        #[cfg(unix)]
        {
            let temp_path = temp_file.path().to_owned();
            tokio::task::spawn_blocking(move || {
                use std::os::unix::fs::PermissionsExt;
                let permissions = std::fs::Permissions::from_mode(0o600);
                std::fs::set_permissions(&temp_path, permissions).map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!(
                        "failed to secure projected temp file ({}): {}",
                        temp_path.display(),
                        e
                    ),
                })
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("spawn_blocking join error: {e}"),
            })??;
        }

        let (path, keep_error): (PathBuf, Option<std::io::Error>) = match temp_file.keep() {
            Ok(value) => (value.1, None),
            Err(err) => (err.file.path().to_path_buf(), Some(err.error)),
        };
        if let Some(err) = keep_error {
            return Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!(
                    "failed to persist projected temp file ({}): {}",
                    path.display(),
                    err
                ),
            });
        }

        registry.insert(request.consumer_id, path.clone());
        // #13: If persisting the registry fails AFTER `keep()` succeeded, the
        // cleartext-secret temp file is already materialized on disk but is NOT
        // recorded in the registry — `revoke_projection` (which only iterates the
        // registry) could then never clean it up, leaving an orphaned cleartext
        // secret. Best-effort remove the just-kept file before propagating the
        // error so a projection is either fully tracked or fully cleaned up. The
        // removal error is intentionally swallowed (we only log it): the original
        // save_registry failure is the more actionable one to surface.
        if let Err(save_err) = self.save_registry(&registry).await {
            if let Err(cleanup_err) = Self::remove_file_if_exists(path.clone()).await {
                tracing::warn!(
                    "failed to clean up orphaned projected temp file ({}) after registry save error: {}",
                    path.display(),
                    cleanup_err
                );
            }
            return Err(save_err.into());
        }

        Ok(SecretProjectionResult::TempFile {
            path,
            cleanup_required: true,
        })
    }

    async fn revoke_projection(&self, consumer_id: &str) -> Result<(), CoreError> {
        let mut registry = self.load_registry().await?;
        let Some(path) = registry.remove(consumer_id) else {
            return Ok(());
        };

        Self::remove_file_if_exists(path).await?;
        self.save_registry(&registry).await.map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::secret_projection::{
        provider_api_key_temp_file_consumer_id, ProjectionPurpose, SecretProjectionPort,
    };
    use maekon_core::ports::secret_store::SecretStore;
    use std::sync::Mutex;
    use tempfile::TempDir;

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
    async fn project_temp_file_materializes_secret_and_tracks_registry() {
        let temp_dir = TempDir::new().unwrap();
        let secret_store = Arc::new(TestSecretStore::new());
        secret_store
            .store("provider/openai/llm", "api_key", "sk-temp-file")
            .await
            .unwrap();

        let adapter = TempFileSecretProjection::new(
            secret_store,
            temp_dir.path().join("proj"),
            temp_dir.path().join("registry.json"),
            vec![ProjectionTemplate::temp_file(
                "provider/openai/llm/api-key-temp-file",
                "openai-llm-api-key",
            )],
        );

        let result = adapter
            .project(SecretProjectionRequest {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
                target: ProjectionTarget::TempFile,
                purpose:
                    maekon_core::ports::secret_projection::ProjectionPurpose::ProviderCliExecution,
                consumer_id: "provider/openai/llm/api-key-temp-file".to_string(),
            })
            .await
            .unwrap();

        let SecretProjectionResult::TempFile {
            path,
            cleanup_required,
        } = result
        else {
            panic!("expected temp file result");
        };

        assert!(cleanup_required);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "sk-temp-file");
        assert!(temp_dir.path().join("registry.json").exists());
    }

    /// #6080: on Unix the projected cleartext secret temp file must be
    /// owner-only (0o600). (The Windows owner-only DACL path is applied via the
    /// shared `encryption::set_owner_only_dacl` helper and exercised on Windows.)
    #[cfg(unix)]
    #[tokio::test]
    async fn projected_temp_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let secret_store = Arc::new(TestSecretStore::new());
        secret_store
            .store("provider/openai/llm", "api_key", "sk-temp-file")
            .await
            .unwrap();

        let adapter = TempFileSecretProjection::new(
            secret_store,
            temp_dir.path().join("proj"),
            temp_dir.path().join("registry.json"),
            vec![ProjectionTemplate::temp_file(
                "provider/openai/llm/api-key-temp-file",
                "openai-llm-api-key",
            )],
        );

        let result = adapter
            .project(SecretProjectionRequest {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
                target: ProjectionTarget::TempFile,
                purpose: ProjectionPurpose::ProviderCliExecution,
                consumer_id: "provider/openai/llm/api-key-temp-file".to_string(),
            })
            .await
            .unwrap();

        let SecretProjectionResult::TempFile { path, .. } = result else {
            panic!("expected temp file result");
        };

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        // Only the owner rw bits may be set; group/other must be clear.
        assert_eq!(
            mode & 0o777,
            0o600,
            "projected secret temp file must be 0o600, got {:o}",
            mode & 0o777
        );
    }

    #[tokio::test]
    async fn revoke_projection_removes_managed_temp_file() {
        let temp_dir = TempDir::new().unwrap();
        let secret_store = Arc::new(TestSecretStore::new());
        secret_store
            .store("provider/openai/llm", "api_key", "sk-temp-file")
            .await
            .unwrap();
        let consumer_id = provider_api_key_temp_file_consumer_id("openai", "llm").unwrap();

        let adapter = TempFileSecretProjection::new(
            secret_store,
            temp_dir.path().join("proj"),
            temp_dir.path().join("registry.json"),
            vec![ProjectionTemplate::temp_file(
                consumer_id.clone(),
                "openai-llm-api-key",
            )],
        );

        let result = adapter
            .project(SecretProjectionRequest {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
                target: ProjectionTarget::TempFile,
                purpose: ProjectionPurpose::ProviderCliExecution,
                consumer_id: consumer_id.clone(),
            })
            .await
            .unwrap();

        let path = match result {
            SecretProjectionResult::TempFile { path, .. } => path,
            _ => panic!("expected temp file result"),
        };
        assert!(path.exists());

        adapter.revoke_projection(&consumer_id).await.unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn project_temp_file_keeps_llm_and_ocr_consumers_separate() {
        let temp_dir = TempDir::new().unwrap();
        let secret_store = Arc::new(TestSecretStore::new());
        secret_store
            .store("provider/openai/llm", "api_key", "sk-llm")
            .await
            .unwrap();
        secret_store
            .store("provider/openai/ocr", "api_key", "sk-ocr")
            .await
            .unwrap();

        let adapter = TempFileSecretProjection::new(
            secret_store,
            temp_dir.path().join("proj"),
            temp_dir.path().join("registry.json"),
            vec![
                provider_api_key_temp_file_template(AiProviderType::OpenAi, "llm").unwrap(),
                provider_api_key_temp_file_template(AiProviderType::OpenAi, "ocr").unwrap(),
            ],
        );

        let llm_consumer_id = provider_api_key_temp_file_consumer_id("openai", "llm").unwrap();
        let ocr_consumer_id = provider_api_key_temp_file_consumer_id("openai", "ocr").unwrap();

        let llm_path = match adapter
            .project(SecretProjectionRequest {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
                target: ProjectionTarget::TempFile,
                purpose: ProjectionPurpose::ProviderCliExecution,
                consumer_id: llm_consumer_id,
            })
            .await
            .unwrap()
        {
            SecretProjectionResult::TempFile { path, .. } => path,
            _ => panic!("expected temp file result"),
        };
        let ocr_path = match adapter
            .project(SecretProjectionRequest {
                namespace: "provider/openai/ocr".to_string(),
                key: "api_key".to_string(),
                target: ProjectionTarget::TempFile,
                purpose: ProjectionPurpose::ProviderCliExecution,
                consumer_id: ocr_consumer_id,
            })
            .await
            .unwrap()
        {
            SecretProjectionResult::TempFile { path, .. } => path,
            _ => panic!("expected temp file result"),
        };

        assert!(llm_path.exists());
        assert!(ocr_path.exists());
        assert_ne!(llm_path, ocr_path);
        assert_eq!(std::fs::read_to_string(llm_path).unwrap(), "sk-llm");
        assert_eq!(std::fs::read_to_string(ocr_path).unwrap(), "sk-ocr");
    }

    #[tokio::test]
    async fn project_temp_file_errors_when_template_missing() {
        let temp_dir = TempDir::new().unwrap();
        let secret_store = Arc::new(TestSecretStore::new());
        let adapter = TempFileSecretProjection::new(
            secret_store,
            temp_dir.path().join("proj"),
            temp_dir.path().join("registry.json"),
            Vec::<ProjectionTemplate>::new(),
        );

        let err = adapter
            .project(SecretProjectionRequest {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
                target: ProjectionTarget::TempFile,
                purpose: ProjectionPurpose::ProviderCliExecution,
                consumer_id: "provider/openai/llm/api-key-temp-file".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, CoreError::Config { .. }));
    }

    /// #13: when `save_registry` fails AFTER the temp file is `keep()`-persisted,
    /// the orphaned cleartext-secret file must be cleaned up before the error is
    /// propagated — otherwise it stays on disk untracked (and thus uncleanable
    /// via `revoke_projection`, which only iterates the registry).
    #[tokio::test]
    async fn project_temp_file_cleans_up_when_registry_save_fails_after_keep() {
        let temp_dir = TempDir::new().unwrap();
        let secret_store = Arc::new(TestSecretStore::new());
        secret_store
            .store("provider/openai/llm", "api_key", "sk-temp-file")
            .await
            .unwrap();

        // Force save_registry to fail: place a *regular file* where the registry's
        // parent directory needs to be created. `create_dir_all(parent)` then
        // fails (NotADirectory), which happens AFTER keep() has materialized the
        // projected temp file.
        let parent_as_file = temp_dir.path().join("registry-parent");
        std::fs::write(&parent_as_file, b"not a dir").unwrap();
        let registry_path = parent_as_file.join("registry.json");

        let projection_dir = temp_dir.path().join("proj");
        let adapter = TempFileSecretProjection::new(
            secret_store,
            projection_dir.clone(),
            registry_path,
            vec![ProjectionTemplate::temp_file(
                "provider/openai/llm/api-key-temp-file",
                "openai-llm-api-key",
            )],
        );

        let err = adapter
            .project(SecretProjectionRequest {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
                target: ProjectionTarget::TempFile,
                purpose: ProjectionPurpose::ProviderCliExecution,
                consumer_id: "provider/openai/llm/api-key-temp-file".to_string(),
            })
            .await
            .unwrap_err();

        // The registry-save failure is surfaced (StorageError::Internal maps to
        // CoreError::Storage via `From`), not swallowed.
        assert!(
            matches!(err, CoreError::Storage { .. }),
            "expected Storage from registry-save failure, got {err:?}"
        );

        // No orphaned `.secret` file may remain in the projection dir.
        let leftover_secret = std::fs::read_dir(&projection_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .any(|e| e.path().extension().and_then(|s| s.to_str()) == Some("secret"))
            })
            .unwrap_or(false);
        assert!(
            !leftover_secret,
            "projected cleartext temp file must be cleaned up when registry save fails after keep()"
        );
    }

    #[test]
    fn provider_api_key_temp_file_template_maps_openai_to_expected_shape() {
        let template = provider_api_key_temp_file_template(AiProviderType::OpenAi, "llm").unwrap();
        assert_eq!(
            template.consumer_id,
            provider_api_key_temp_file_consumer_id("openai", "llm").unwrap()
        );
        assert_eq!(template.target, ProjectionTarget::TempFile);
        assert_eq!(
            template.file_name_hint.as_deref(),
            Some("openai-llm-api-key")
        );
    }
}
