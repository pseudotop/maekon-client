//! Secret projection adapter for ONESHIM-managed temp files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::error::StorageError;
use async_trait::async_trait;
use oneshim_core::config::AiProviderType;
use oneshim_core::error::CoreError;
use oneshim_core::ports::secret_projection::{
    provider_api_key_temp_file_consumer_id, ProjectionTarget, ProjectionTemplate,
    SecretProjectionPort, SecretProjectionRequest, SecretProjectionResult,
};
use oneshim_core::ports::secret_store::SecretStore;
use oneshim_core::provider_surface::provider_projection_for_type;
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

    fn load_registry(&self) -> Result<HashMap<String, PathBuf>, StorageError> {
        if !self.registry_path.exists() {
            return Ok(HashMap::new());
        }

        let raw = std::fs::read_to_string(&self.registry_path).map_err(|e| {
            StorageError::Internal(format!(
                "failed to read temp projection registry ({}): {}",
                self.registry_path.display(),
                e
            ))
        })?;

        let stored: HashMap<String, String> = serde_json::from_str(&raw).map_err(|e| {
            StorageError::Internal(format!(
                "failed to parse temp projection registry ({}): {}",
                self.registry_path.display(),
                e
            ))
        })?;

        Ok(stored
            .into_iter()
            .map(|(consumer_id, path)| (consumer_id, PathBuf::from(path)))
            .collect())
    }

    fn save_registry(&self, registry: &HashMap<String, PathBuf>) -> Result<(), StorageError> {
        if let Some(parent) = self.registry_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                StorageError::Internal(format!(
                    "failed to create temp projection registry dir ({}): {}",
                    parent.display(),
                    e
                ))
            })?;
        }

        let stored: HashMap<String, String> = registry
            .iter()
            .map(|(consumer_id, path)| (consumer_id.clone(), path.display().to_string()))
            .collect();

        let json = serde_json::to_string_pretty(&stored).map_err(|e| {
            StorageError::Internal(format!(
                "failed to serialize temp projection registry ({}): {}",
                self.registry_path.display(),
                e
            ))
        })?;

        std::fs::write(&self.registry_path, json).map_err(|e| {
            StorageError::Internal(format!(
                "failed to write temp projection registry ({}): {}",
                self.registry_path.display(),
                e
            ))
        })?;

        Ok(())
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
        format!("oneshim-{}-", normalized.trim_matches('-'))
    }

    fn remove_file_if_exists(path: &Path) -> Result<(), StorageError> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(StorageError::Internal(format!(
                "failed to remove projected temp file ({}): {}",
                path.display(),
                err
            ))),
        }
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
                code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
                message: "temp-file projection adapter only supports ProjectionTarget::TempFile"
                    .to_string(),
            });
        }

        let template =
            self.templates
                .get(&request.consumer_id)
                .ok_or_else(|| CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Invalid,
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
                code: oneshim_core::error_codes::AuthCode::Failed,
                message: format!(
                    "secret not found for projection request {}:{}",
                    request.namespace, request.key
                ),
            })?;

        std::fs::create_dir_all(&self.projection_dir).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!(
                "failed to create temp projection dir ({}): {}",
                self.projection_dir.display(),
                e
            ),
        })?;

        let mut registry = self.load_registry()?;
        if let Some(existing_path) = registry.get(&request.consumer_id).cloned() {
            Self::remove_file_if_exists(&existing_path)?;
        }

        let temp_file = Builder::new()
            .prefix(&Self::file_prefix(template))
            .suffix(".secret")
            .tempfile_in(&self.projection_dir)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
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
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("failed to initialize temp file: {e}"),
            })?;
        std::fs::write(temp_file.path(), secret).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!(
                "failed to write projected temp file ({}): {}",
                temp_file.path().display(),
                e
            ),
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let permissions = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(temp_file.path(), permissions).map_err(|e| {
                CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!(
                        "failed to secure projected temp file ({}): {}",
                        temp_file.path().display(),
                        e
                    ),
                }
            })?;
        }

        let (path, keep_error): (PathBuf, Option<std::io::Error>) = match temp_file.keep() {
            Ok(value) => (value.1, None),
            Err(err) => (err.file.path().to_path_buf(), Some(err.error)),
        };
        if let Some(err) = keep_error {
            return Err(CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!(
                    "failed to persist projected temp file ({}): {}",
                    path.display(),
                    err
                ),
            });
        }

        registry.insert(request.consumer_id, path.clone());
        self.save_registry(&registry)?;

        Ok(SecretProjectionResult::TempFile {
            path,
            cleanup_required: true,
        })
    }

    async fn revoke_projection(&self, consumer_id: &str) -> Result<(), CoreError> {
        let mut registry = self.load_registry()?;
        let Some(path) = registry.remove(consumer_id) else {
            return Ok(());
        };

        Self::remove_file_if_exists(&path)?;
        self.save_registry(&registry).map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::ports::secret_projection::{
        provider_api_key_temp_file_consumer_id, ProjectionPurpose, SecretProjectionPort,
    };
    use oneshim_core::ports::secret_store::SecretStore;
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
                    oneshim_core::ports::secret_projection::ProjectionPurpose::ProviderCliExecution,
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
