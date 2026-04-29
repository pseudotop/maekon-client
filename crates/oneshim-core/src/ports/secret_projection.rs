//! Port for projecting secrets into process-env, temp files, or bridge files for external tool consumption.

use std::path::PathBuf;

use async_trait::async_trait;

use crate::error::CoreError;
use crate::ports::secret_store::{provider_api_key_secret_ref, validate_secret_segment};

/// Projection target for compatibility output generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionTarget {
    ProcessEnv,
    TempFile,
    PersistentBridgeFile,
}

/// Why a projection is being created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionPurpose {
    ProviderCliExecution,
    ToolBridge,
    UserExportedCompatibilityArtifact,
}

/// Provider/tool-specific projection template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionTemplate {
    pub consumer_id: String,
    pub target: ProjectionTarget,
    pub env_names: Vec<String>,
    pub file_name_hint: Option<String>,
}

impl ProjectionTemplate {
    pub fn process_env(consumer_id: impl Into<String>, env_names: Vec<String>) -> Self {
        Self {
            consumer_id: consumer_id.into(),
            target: ProjectionTarget::ProcessEnv,
            env_names,
            file_name_hint: None,
        }
    }

    pub fn temp_file(consumer_id: impl Into<String>, file_name_hint: impl Into<String>) -> Self {
        Self {
            consumer_id: consumer_id.into(),
            target: ProjectionTarget::TempFile,
            env_names: Vec::new(),
            file_name_hint: Some(file_name_hint.into()),
        }
    }
}

pub fn provider_api_key_cli_consumer_id(provider_id: &str) -> Result<String, CoreError> {
    let provider_id = validate_secret_segment(provider_id, "provider_id")?;
    Ok(format!("provider/{provider_id}/api-key-cli"))
}

pub fn provider_api_key_temp_file_consumer_id(
    provider_id: &str,
    profile_id: &str,
) -> Result<String, CoreError> {
    let provider_id = validate_secret_segment(provider_id, "provider_id")?;
    let profile_id = validate_secret_segment(profile_id, "profile_id")?;
    Ok(format!(
        "provider/{provider_id}/{profile_id}/api-key-temp-file"
    ))
}

/// Request to project a canonical secret into a compatibility target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretProjectionRequest {
    pub namespace: String,
    pub key: String,
    pub target: ProjectionTarget,
    pub purpose: ProjectionPurpose,
    pub consumer_id: String,
}

impl SecretProjectionRequest {
    pub fn provider_api_key_process_env(
        provider_id: &str,
        profile_id: &str,
        consumer_id: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let (namespace, key) = provider_api_key_secret_ref(provider_id, profile_id)?;
        Ok(Self {
            namespace,
            key: key.to_string(),
            target: ProjectionTarget::ProcessEnv,
            purpose: ProjectionPurpose::ProviderCliExecution,
            consumer_id: consumer_id.into(),
        })
    }

    pub fn provider_api_key_temp_file(
        provider_id: &str,
        profile_id: &str,
        consumer_id: impl Into<String>,
    ) -> Result<Self, CoreError> {
        let (namespace, key) = provider_api_key_secret_ref(provider_id, profile_id)?;
        Ok(Self {
            namespace,
            key: key.to_string(),
            target: ProjectionTarget::TempFile,
            purpose: ProjectionPurpose::ProviderCliExecution,
            consumer_id: consumer_id.into(),
        })
    }
}

/// Result of projecting a secret for compatibility use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretProjectionResult {
    EnvVars(Vec<(String, String)>),
    TempFile {
        path: PathBuf,
        cleanup_required: bool,
    },
    PersistentFile {
        path: PathBuf,
    },
}

/// Compatibility projection port.
///
/// # Errors
/// - `CoreError::NotFound` (wire: `resource.not_found`,
///   `resource_type = "secret"`) — requested secret (`namespace` +
///   `key`) does not exist in the backing SecretStore.
/// - `CoreError::Config` with `ConfigCode::Invalid` (wire:
///   `config.invalid`) — namespace/key/consumer_id segment fails
///   `validate_secret_segment` (empty, non-alphanumeric, reserved char).
/// - `CoreError::Io` (wire: `internal.io`) — temp-file / bridge-file
///   creation or write failure; permission denied on projection
///   target directory.
/// - `CoreError::Internal` (wire: `internal.generic`) — unsupported
///   projection target for the requested purpose, platform-specific
///   keychain/credential-store failures propagated from the
///   underlying SecretStore.
/// - `revoke_projection` for an unknown `consumer_id` is treated as a
///   no-op (`Ok(())`) by the current adapter rather than a distinct
///   NotFound.
#[async_trait]
pub trait SecretProjectionPort: Send + Sync {
    async fn project(
        &self,
        request: SecretProjectionRequest,
    ) -> Result<SecretProjectionResult, CoreError>;

    async fn revoke_projection(&self, consumer_id: &str) -> Result<(), CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_env_template_sets_target_and_names() {
        let template = ProjectionTemplate::process_env(
            "provider/openai/api-key-cli",
            vec!["OPENAI_API_KEY".to_string()],
        );

        assert_eq!(template.consumer_id, "provider/openai/api-key-cli");
        assert_eq!(template.target, ProjectionTarget::ProcessEnv);
        assert_eq!(template.env_names, vec!["OPENAI_API_KEY".to_string()]);
        assert!(template.file_name_hint.is_none());
    }

    #[test]
    fn temp_file_template_sets_target_and_hint() {
        let template = ProjectionTemplate::temp_file(
            "provider/openai/llm/api-key-temp-file",
            "openai-llm-api-key",
        );

        assert_eq!(
            template.consumer_id,
            "provider/openai/llm/api-key-temp-file"
        );
        assert_eq!(template.target, ProjectionTarget::TempFile);
        assert!(template.env_names.is_empty());
        assert_eq!(
            template.file_name_hint.as_deref(),
            Some("openai-llm-api-key")
        );
    }

    #[test]
    fn provider_api_key_cli_consumer_id_uses_stable_shape() {
        let consumer_id = provider_api_key_cli_consumer_id("openai").unwrap();
        assert_eq!(consumer_id, "provider/openai/api-key-cli");
    }

    #[test]
    fn provider_api_key_temp_file_consumer_id_uses_stable_shape() {
        let consumer_id = provider_api_key_temp_file_consumer_id("openai", "llm").unwrap();
        assert_eq!(consumer_id, "provider/openai/llm/api-key-temp-file");
    }

    #[test]
    fn provider_api_key_process_env_request_uses_secret_ref_shape() {
        let request = SecretProjectionRequest::provider_api_key_process_env(
            "openai",
            "llm",
            "provider/openai/api-key-cli",
        )
        .unwrap();

        assert_eq!(request.namespace, "provider/openai/llm");
        assert_eq!(request.key, "api_key");
        assert_eq!(request.target, ProjectionTarget::ProcessEnv);
        assert_eq!(request.purpose, ProjectionPurpose::ProviderCliExecution);
        assert_eq!(request.consumer_id, "provider/openai/api-key-cli");
    }

    #[test]
    fn provider_api_key_temp_file_request_uses_secret_ref_shape() {
        let request = SecretProjectionRequest::provider_api_key_temp_file(
            "openai",
            "llm",
            "provider/openai/llm/api-key-temp-file",
        )
        .unwrap();

        assert_eq!(request.namespace, "provider/openai/llm");
        assert_eq!(request.key, "api_key");
        assert_eq!(request.target, ProjectionTarget::TempFile);
        assert_eq!(request.purpose, ProjectionPurpose::ProviderCliExecution);
        assert_eq!(request.consumer_id, "provider/openai/llm/api-key-temp-file");
    }
}
