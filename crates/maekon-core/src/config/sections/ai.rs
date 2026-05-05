// AI 프로바이더 설정 — AiProviderConfig 및 외부 엔드포인트 검증 오케스트레이터
use super::super::enums::*;
use super::ai_validation::{
    CredentialAuthMode, ExternalApiEndpoint, OcrValidationConfig, SceneActionOverrideConfig,
    SceneIntelligenceConfig,
};
use crate::error::CoreError;
use crate::provider_surface::{
    provider_surface_spec, provider_surface_supports_llm, provider_surface_supports_ocr,
    provider_surface_uses_no_auth, ProviderSurfaceTransport,
};
use serde::{Deserialize, Serialize};

// ── AiProviderConfig ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderConfig {
    #[serde(default)]
    pub access_mode: AiAccessMode,
    #[serde(default)]
    pub ocr_provider: OcrProviderType,
    #[serde(default)]
    pub llm_provider: LlmProviderType,
    #[serde(default)]
    pub ocr_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub llm_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub llm_api_fallback: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub external_data_policy: ExternalDataPolicy,
    #[serde(default)]
    pub allow_unredacted_external_ocr: bool,
    #[serde(default)]
    pub ocr_validation: OcrValidationConfig,
    #[serde(default)]
    pub scene_action_override: SceneActionOverrideConfig,
    #[serde(default)]
    pub scene_intelligence: SceneIntelligenceConfig,
    #[serde(default = "default_true")]
    pub fallback_to_local: bool,
    #[serde(default)]
    pub active_profile_id: Option<String>,
    #[serde(default)]
    pub saved_profiles: Vec<SavedAiProviderProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiProviderProfileConfig {
    #[serde(default)]
    pub access_mode: AiAccessMode,
    #[serde(default)]
    pub ocr_provider: OcrProviderType,
    #[serde(default)]
    pub llm_provider: LlmProviderType,
    #[serde(default)]
    pub ocr_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub llm_api: Option<ExternalApiEndpoint>,
    #[serde(default)]
    pub external_data_policy: ExternalDataPolicy,
    #[serde(default)]
    pub allow_unredacted_external_ocr: bool,
    #[serde(default)]
    pub ocr_validation: OcrValidationConfig,
    #[serde(default)]
    pub scene_action_override: SceneActionOverrideConfig,
    #[serde(default)]
    pub scene_intelligence: SceneIntelligenceConfig,
    #[serde(default = "default_true")]
    pub fallback_to_local: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedAiProviderProfile {
    pub profile_id: String,
    pub name: String,
    #[serde(default)]
    pub ai_provider: AiProviderProfileConfig,
    #[serde(default)]
    pub updated_at: Option<String>,
}

impl Default for AiProviderConfig {
    fn default() -> Self {
        Self {
            access_mode: AiAccessMode::default(),
            ocr_provider: OcrProviderType::default(),
            llm_provider: LlmProviderType::default(),
            ocr_api: None,
            llm_api: None,
            llm_api_fallback: None,
            external_data_policy: ExternalDataPolicy::default(),
            allow_unredacted_external_ocr: false,
            ocr_validation: OcrValidationConfig::default(),
            scene_action_override: SceneActionOverrideConfig::default(),
            scene_intelligence: SceneIntelligenceConfig::default(),
            fallback_to_local: true,
            active_profile_id: None,
            saved_profiles: Vec::new(),
        }
    }
}

impl Default for AiProviderProfileConfig {
    fn default() -> Self {
        Self {
            access_mode: AiAccessMode::default(),
            ocr_provider: OcrProviderType::default(),
            llm_provider: LlmProviderType::default(),
            ocr_api: None,
            llm_api: None,
            external_data_policy: ExternalDataPolicy::default(),
            allow_unredacted_external_ocr: false,
            ocr_validation: OcrValidationConfig::default(),
            scene_action_override: SceneActionOverrideConfig::default(),
            scene_intelligence: SceneIntelligenceConfig::default(),
            fallback_to_local: true,
        }
    }
}

impl Default for SavedAiProviderProfile {
    fn default() -> Self {
        Self {
            profile_id: "ai-profile".to_string(),
            name: "AI Profile".to_string(),
            ai_provider: AiProviderProfileConfig::default(),
            updated_at: None,
        }
    }
}

impl AiProviderConfig {
    pub fn active_secret_profile_id_or<'a>(&'a self, fallback_profile_id: &'a str) -> &'a str {
        self.active_profile_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback_profile_id)
    }

    pub fn validate_selected_remote_endpoints(&self) -> Result<(), CoreError> {
        self.ocr_validation.validate()?;
        self.scene_action_override.validate()?;
        self.scene_intelligence.validate()?;

        match self.access_mode.normalized_for_ai_surfaces() {
            AiAccessMode::ProviderApiKey => {
                if self.ocr_provider == OcrProviderType::Remote {
                    validate_remote_endpoint(self.ocr_api.as_ref(), "ocr_api")?;
                }
                if self.llm_provider == LlmProviderType::Remote {
                    validate_remote_endpoint(self.llm_api.as_ref(), "llm_api")?;
                }
            }
            AiAccessMode::LocalModel => {}
            AiAccessMode::ProviderSubscriptionCli => {
                if self.ocr_provider == OcrProviderType::Remote {
                    validate_subprocess_or_remote_endpoint(
                        self.ocr_api.as_ref(),
                        "ocr_api",
                        ProviderSurfaceTransport::SubprocessCli,
                        provider_surface_supports_ocr,
                    )?;
                }
                if self.llm_provider == LlmProviderType::Remote {
                    validate_non_http_surface_endpoint(
                        self.llm_api.as_ref(),
                        "llm_api",
                        ProviderSurfaceTransport::SubprocessCli,
                        provider_surface_supports_llm,
                    )?;
                }
            }
            AiAccessMode::ProviderOAuth => {
                if self.llm_provider == LlmProviderType::Remote {
                    validate_managed_or_remote_endpoint(
                        self.llm_api.as_ref(),
                        "llm_api",
                        ProviderSurfaceTransport::ManagedOAuth,
                        provider_surface_supports_llm,
                    )?;
                }
                if self.ocr_provider == OcrProviderType::Remote {
                    validate_managed_or_remote_endpoint(
                        self.ocr_api.as_ref(),
                        "ocr_api",
                        ProviderSurfaceTransport::ManagedOAuth,
                        provider_surface_supports_ocr,
                    )?;
                }
            }
        }
        Ok(())
    }
}

// ── validate_remote_endpoint (모듈-내부 헬퍼) ─────────────────────

fn validate_remote_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
) -> Result<(), CoreError> {
    let endpoint = endpoint.ok_or_else(|| CoreError::Config {
        code: crate::error_codes::ConfigCode::Missing,
        message: format!("`{field_name}` is required when a remote provider is selected."),
    })?;

    let endpoint_url = endpoint.endpoint.trim();
    if endpoint_url.is_empty() {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}.endpoint` must not be empty."),
        });
    }
    if !(endpoint_url.starts_with("http://") || endpoint_url.starts_with("https://")) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("`{field_name}.endpoint` must be an http:// or https:// URL."),
        });
    }

    let has_api_key_binding = endpoint.credential.as_ref().is_some_and(|binding| {
        binding.auth_mode == CredentialAuthMode::ApiKey && binding.secret_ref.is_some()
    });

    let requires_plaintext_api_key = endpoint
        .surface_id
        .as_deref()
        .map(|surface_id| !provider_surface_uses_no_auth(surface_id))
        .unwrap_or(true);

    if requires_plaintext_api_key && endpoint.api_key.trim().is_empty() && !has_api_key_binding {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!(
            "`{field_name}.api_key` must not be empty unless a credential binding is configured."
        ),
        });
    }

    if endpoint.timeout_secs == 0 {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::OutOfRange,
            message: format!("`{field_name}.timeout_secs` must be >= 1."),
        });
    }

    if let Some(model) = endpoint
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let decision = crate::ai_model_lifecycle_policy::evaluate_model_lifecycle_now_for_surface(
            endpoint.provider_type,
            endpoint.surface_id.as_deref(),
            model,
        )?;
        if let crate::ai_model_lifecycle_policy::ModelLifecycleDecision::Block { message, .. } =
            decision
        {
            return Err(CoreError::PolicyDenied {
                code: crate::error_codes::PolicyCode::Denied,
                message,
            });
        }
    }

    Ok(())
}

fn validate_non_http_surface_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
    expected_transport: ProviderSurfaceTransport,
    capability_check: fn(&str) -> bool,
) -> Result<(), CoreError> {
    let endpoint = endpoint.ok_or_else(|| CoreError::Config {
        code: crate::error_codes::ConfigCode::Missing,
        message: format!("`{field_name}` is required when a managed provider surface is selected."),
    })?;

    let surface_id = endpoint
        .surface_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!(
                "`{field_name}.surface_id` is required for managed provider surfaces."
            ),
        })?;

    let spec = provider_surface_spec(surface_id).ok_or_else(|| CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message: format!("`{field_name}.surface_id` references an unknown provider surface."),
    })?;

    if spec.provider_type != endpoint.provider_type {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("`{field_name}.surface_id` must match `{field_name}.provider_type`."),
        });
    }

    if spec.transport != expected_transport {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "`{field_name}.surface_id` is incompatible with the selected access mode."
            ),
        });
    }

    if !capability_check(surface_id) {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!(
                "`{field_name}.surface_id` does not support this endpoint capability."
            ),
        });
    }

    Ok(())
}

fn validate_subprocess_or_remote_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
    managed_transport: ProviderSurfaceTransport,
    capability_check: fn(&str) -> bool,
) -> Result<(), CoreError> {
    let Some(endpoint) = endpoint else {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}` is required when a remote provider is selected."),
        });
    };

    if let Some(surface_id) = endpoint
        .surface_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(surface) = provider_surface_spec(surface_id) {
            if surface.transport == managed_transport {
                return validate_non_http_surface_endpoint(
                    Some(endpoint),
                    field_name,
                    managed_transport,
                    capability_check,
                );
            }
        }
    }

    validate_remote_endpoint(Some(endpoint), field_name)
}

fn validate_managed_or_remote_endpoint(
    endpoint: Option<&ExternalApiEndpoint>,
    field_name: &str,
    managed_transport: ProviderSurfaceTransport,
    capability_check: fn(&str) -> bool,
) -> Result<(), CoreError> {
    let Some(endpoint) = endpoint else {
        return Err(CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: format!("`{field_name}` is required when a remote provider is selected."),
        });
    };

    if let Some(surface_id) = endpoint
        .surface_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if let Some(surface) = provider_surface_spec(surface_id) {
            if surface.transport == managed_transport {
                return validate_non_http_surface_endpoint(
                    Some(endpoint),
                    field_name,
                    managed_transport,
                    capability_check,
                );
            }
        }
    }

    validate_remote_endpoint(Some(endpoint), field_name)
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}
