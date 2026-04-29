// AI 검증 설정 — OCR 신뢰도, 장면 행동 오버라이드, 장면 지능 검증 설정
use super::super::enums::AiProviderType;
use crate::error::CoreError;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Credential metadata ─────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBackendKind {
    OsSecretStore,
    FileSecretStore,
    Env,
    BridgeManaged,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialAuthMode {
    ApiKey,
    ManagedOAuth,
    CliBridge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    pub namespace: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialBinding {
    pub auth_mode: CredentialAuthMode,
    pub backend_kind: CredentialBackendKind,
    #[serde(default)]
    pub secret_ref: Option<SecretRef>,
    #[serde(default)]
    pub projection_enabled: bool,
}

// ── ExternalApiEndpoint ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalApiEndpoint {
    pub endpoint: String,
    #[serde(default)]
    pub api_key: String,
    pub model: Option<String>,
    #[serde(default = "default_api_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub provider_type: AiProviderType,
    #[serde(default)]
    pub surface_id: Option<String>,
    #[serde(default)]
    pub credential: Option<CredentialBinding>,
}

// ── OcrValidationConfig ────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrValidationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_ocr_min_confidence")]
    pub min_confidence: f64,
    #[serde(default = "default_ocr_max_invalid_ratio")]
    pub max_invalid_ratio: f64,
}

impl Default for OcrValidationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_confidence: default_ocr_min_confidence(),
            max_invalid_ratio: default_ocr_max_invalid_ratio(),
        }
    }
}

impl OcrValidationConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.enabled {
            return Ok(());
        }

        if !self.min_confidence.is_finite() || !(0.0..=1.0).contains(&self.min_confidence) {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::OutOfRange,
                message: "`ai_provider.ocr_validation.min_confidence` must be within 0.0..=1.0."
                    .to_string(),
            });
        }

        if !self.max_invalid_ratio.is_finite() || !(0.0..=1.0).contains(&self.max_invalid_ratio) {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::OutOfRange,
                message: "`ai_provider.ocr_validation.max_invalid_ratio` must be within 0.0..=1.0."
                    .to_string(),
            });
        }

        Ok(())
    }
}

// ── SceneActionOverrideConfig ──────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SceneActionOverrideConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub approved_by: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
}

impl SceneActionOverrideConfig {
    pub fn is_active_at(&self, now: DateTime<Utc>) -> bool {
        if !self.enabled {
            return false;
        }

        let reason = self.reason.as_deref().map(str::trim).unwrap_or_default();
        let approved_by = self
            .approved_by
            .as_deref()
            .map(str::trim)
            .unwrap_or_default();
        let Some(expires_at) = self.expires_at else {
            return false;
        };

        !reason.is_empty() && !approved_by.is_empty() && expires_at > now
    }

    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.enabled {
            return Ok(());
        }

        let reason = self.reason.as_deref().map(str::trim).unwrap_or_default();
        if reason.is_empty() {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::Missing,
                message: "`ai_provider.scene_action_override.reason` is required.".to_string(),
            });
        }

        let approved_by = self
            .approved_by
            .as_deref()
            .map(str::trim)
            .unwrap_or_default();
        if approved_by.is_empty() {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::Missing,
                message: "`ai_provider.scene_action_override.approved_by` is required.".to_string(),
            });
        }

        let expires_at = self.expires_at.ok_or_else(|| CoreError::Config {
            code: crate::error_codes::ConfigCode::Missing,
            message: "`ai_provider.scene_action_override.expires_at` is required.".to_string(),
        })?;

        if expires_at <= Utc::now() {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message: "`ai_provider.scene_action_override.expires_at` must be in the future."
                    .to_string(),
            });
        }

        Ok(())
    }
}

// ── SceneIntelligenceConfig ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneIntelligenceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub overlay_enabled: bool,
    #[serde(default = "default_false")]
    pub allow_action_execution: bool,
    #[serde(default = "default_scene_min_confidence")]
    pub min_confidence: f64,
    #[serde(default = "default_scene_max_elements")]
    pub max_elements: usize,
    #[serde(default = "default_true")]
    pub calibration_enabled: bool,
    #[serde(default = "default_scene_calibration_min_elements")]
    pub calibration_min_elements: usize,
    #[serde(default = "default_scene_calibration_min_avg_confidence")]
    pub calibration_min_avg_confidence: f64,
}

impl Default for SceneIntelligenceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            overlay_enabled: true,
            allow_action_execution: default_false(),
            min_confidence: default_scene_min_confidence(),
            max_elements: default_scene_max_elements(),
            calibration_enabled: true,
            calibration_min_elements: default_scene_calibration_min_elements(),
            calibration_min_avg_confidence: default_scene_calibration_min_avg_confidence(),
        }
    }
}

impl SceneIntelligenceConfig {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.min_confidence.is_finite() || !(0.0..=1.0).contains(&self.min_confidence) {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::OutOfRange,
                message:
                    "`ai_provider.scene_intelligence.min_confidence` must be within 0.0..=1.0."
                        .to_string(),
            });
        }
        if self.max_elements == 0 || self.max_elements > 1000 {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::OutOfRange,
                message: "`ai_provider.scene_intelligence.max_elements` must be within 1..=1000."
                    .to_string(),
            });
        }
        if self.calibration_min_elements == 0 || self.calibration_min_elements > 1000 {
            return Err(CoreError::Config { code: crate::error_codes::ConfigCode::OutOfRange, message: "`ai_provider.scene_intelligence.calibration_min_elements` must be within 1..=1000."
                    .to_string() });
        }
        if !self.calibration_min_avg_confidence.is_finite()
            || !(0.0..=1.0).contains(&self.calibration_min_avg_confidence)
        {
            return Err(CoreError::Config { code: crate::error_codes::ConfigCode::OutOfRange, message: "`ai_provider.scene_intelligence.calibration_min_avg_confidence` must be within 0.0..=1.0."
                    .to_string() });
        }
        Ok(())
    }
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_api_timeout_secs() -> u64 {
    30
}

fn default_ocr_min_confidence() -> f64 {
    0.25
}

fn default_ocr_max_invalid_ratio() -> f64 {
    0.6
}

fn default_scene_min_confidence() -> f64 {
    0.35
}

fn default_scene_max_elements() -> usize {
    120
}

fn default_scene_calibration_min_elements() -> usize {
    8
}

fn default_scene_calibration_min_avg_confidence() -> f64 {
    0.55
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard: range-check validation errors emit the
    /// `config.out_of_range` wire code, not `config.invalid`. Pre-iter-89
    /// these were incorrectly labelled Invalid, conflating range violations
    /// with format/unsupported-value violations in Grafana group-by.
    #[test]
    fn ocr_min_confidence_out_of_range_emits_out_of_range_code() {
        let cfg = OcrValidationConfig {
            enabled: true,
            min_confidence: 1.5, // out of 0.0..=1.0
            max_invalid_ratio: 0.5,
        };
        let err = cfg.validate().expect_err("expected range-check error");
        assert_eq!(err.code(), "config.out_of_range");
    }

    #[test]
    fn ocr_max_invalid_ratio_out_of_range_emits_out_of_range_code() {
        let cfg = OcrValidationConfig {
            enabled: true,
            min_confidence: 0.5,
            max_invalid_ratio: 2.0, // out of 0.0..=1.0
        };
        let err = cfg.validate().expect_err("expected range-check error");
        assert_eq!(err.code(), "config.out_of_range");
    }

    #[test]
    fn scene_max_elements_out_of_range_emits_out_of_range_code() {
        let cfg = SceneIntelligenceConfig {
            max_elements: 5000, // out of 1..=1000
            ..SceneIntelligenceConfig::default()
        };
        let err = cfg.validate().expect_err("expected range-check error");
        assert_eq!(err.code(), "config.out_of_range");
    }

    /// Regression guard: required-field-absent validation errors emit the
    /// `config.missing` wire code, not `config.invalid`. Pre-iter-89 these
    /// were incorrectly labelled Invalid across ai.rs, helpers.rs, and
    /// config_manager.rs env-var lookups.
    #[test]
    fn scene_action_override_missing_reason_emits_missing_code() {
        let cfg = SceneActionOverrideConfig {
            enabled: true,
            reason: None,
            approved_by: Some("alice@example.com".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        };
        let err = cfg.validate().expect_err("expected missing-field error");
        assert_eq!(err.code(), "config.missing");
    }

    #[test]
    fn scene_action_override_missing_approved_by_emits_missing_code() {
        let cfg = SceneActionOverrideConfig {
            enabled: true,
            reason: Some("maintenance override".to_string()),
            approved_by: None,
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
        };
        let err = cfg.validate().expect_err("expected missing-field error");
        assert_eq!(err.code(), "config.missing");
    }

    #[test]
    fn scene_action_override_missing_expires_at_emits_missing_code() {
        let cfg = SceneActionOverrideConfig {
            enabled: true,
            reason: Some("maintenance override".to_string()),
            approved_by: Some("alice@example.com".to_string()),
            expires_at: None,
        };
        let err = cfg.validate().expect_err("expected missing-field error");
        assert_eq!(err.code(), "config.missing");
    }

    /// Regression guard: expires_at in the PAST is semantically Invalid
    /// (value present but illegal), distinct from the Missing cases above.
    /// Keeps both codes exercised so future drift doesn't collapse them.
    #[test]
    fn scene_action_override_expired_time_emits_invalid_code() {
        let cfg = SceneActionOverrideConfig {
            enabled: true,
            reason: Some("maintenance override".to_string()),
            approved_by: Some("alice@example.com".to_string()),
            expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
        };
        let err = cfg.validate().expect_err("expected invalid-value error");
        assert_eq!(err.code(), "config.invalid");
    }
}
