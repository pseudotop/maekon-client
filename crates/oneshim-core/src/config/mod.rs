mod enums;
mod sections;

// ── Public re-exports (external API) ────────────────────────────────
pub use enums::*;
pub use sections::*;

use serde::{Deserialize, Serialize};
use std::time::Duration;

// Use default functions from sections for AppConfig::default_config()
use sections::{
    default_capture_enabled, default_capture_throttle_ms, default_heartbeat_interval_ms,
    default_idle_threshold_secs, default_max_storage_mb, default_poll_interval_ms,
    default_process_interval_secs, default_request_timeout_ms, default_retention_days,
    default_sse_max_retry_secs, default_sync_interval_ms, default_thumbnail_height,
    default_thumbnail_width,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub monitor: MonitorConfig,
    pub storage: StorageConfig,
    pub vision: VisionConfig,
    #[serde(default)]
    pub update: UpdateConfig,
    #[serde(default)]
    pub integrity: IntegrityConfig,
    #[serde(default)]
    pub web: WebConfig,
    #[serde(default)]
    pub notification: NotificationConfig,
    #[serde(default)]
    pub grpc: GrpcConfig,
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    #[serde(default)]
    pub file_access: FileAccessConfig,
    #[serde(default)]
    pub automation: AutomationConfig,
    #[serde(default)]
    pub ai_provider: AiProviderConfig,
    #[serde(default)]
    pub ai_session: AiSessionConfig,
    #[serde(default)]
    pub integration: IntegrationConfig,
    /// 아웃바운드 TLS 설정 — 기본값: 활성화
    #[serde(default)]
    pub tls: TlsConfig,
    #[serde(default)]
    pub analysis: AnalysisConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub coaching: CoachingConfig,
    #[serde(default)]
    pub indicator: IndicatorConfig,
    #[serde(default)]
    pub suggestions: SuggestionConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub focus_auto: FocusAutoConfig,
    #[serde(default)]
    pub external_grpc: ExternalGrpcConfig,
    /// Tracking schedule — wall-clock mute windows (Phase 9 PR-A).
    #[serde(default)]
    pub tracking_schedule: TrackingScheduleConfig,
    /// Autostart — onboarding state for cross-platform autostart feature (Phase 9 PR-B1).
    #[serde(default)]
    pub autostart: AutostartConfig,
}

// AppConfig impl

impl AppConfig {
    pub fn default_config() -> Self {
        Self {
            server: ServerConfig {
                base_url: "http://localhost:8000".to_string(),
                request_timeout_ms: default_request_timeout_ms(),
                sse_max_retry_secs: default_sse_max_retry_secs(),
            },
            monitor: MonitorConfig {
                poll_interval_ms: default_poll_interval_ms(),
                sync_interval_ms: default_sync_interval_ms(),
                heartbeat_interval_ms: default_heartbeat_interval_ms(),
                idle_threshold_secs: default_idle_threshold_secs(),
                process_interval_secs: default_process_interval_secs(),
                process_monitoring: true,
                input_activity: true,
                upload_enabled: false,
            },
            storage: StorageConfig {
                db_path: None,
                retention_days: default_retention_days(),
                max_storage_mb: default_max_storage_mb(),
            },
            vision: VisionConfig {
                capture_enabled: default_capture_enabled(),
                capture_throttle_ms: default_capture_throttle_ms(),
                thumbnail_width: default_thumbnail_width(),
                thumbnail_height: default_thumbnail_height(),
                ocr_enabled: false,
                privacy_mode: false,
            },
            update: UpdateConfig::default(),
            integrity: IntegrityConfig::default(),
            web: WebConfig::default(),
            notification: NotificationConfig::default(),
            grpc: GrpcConfig::default(),
            telemetry: TelemetryConfig::default(),
            privacy: PrivacyConfig::default(),
            schedule: ScheduleConfig::default(),
            file_access: FileAccessConfig::default(),
            automation: AutomationConfig::default(),
            ai_provider: AiProviderConfig::default(),
            ai_session: AiSessionConfig::default(),
            integration: IntegrationConfig::default(),
            tls: TlsConfig::default(),
            analysis: AnalysisConfig::default(),
            sync: SyncConfig::default(),
            coaching: CoachingConfig::default(),
            indicator: IndicatorConfig::default(),
            suggestions: SuggestionConfig::default(),
            audio: AudioConfig::default(),
            focus_auto: FocusAutoConfig::default(),
            external_grpc: ExternalGrpcConfig::default(),
            tracking_schedule: TrackingScheduleConfig::default(),
            autostart: AutostartConfig::default(),
        }
    }

    /// Validate that all config sections have values within acceptable bounds.
    pub fn validate_bounds(&self) -> Result<(), String> {
        self.storage.validate_bounds()?;
        self.vision.validate_bounds()?;
        Ok(())
    }

    pub fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.server.request_timeout_ms)
    }

    pub fn poll_interval(&self) -> Duration {
        Duration::from_millis(self.monitor.poll_interval_ms)
    }

    pub fn sync_interval(&self) -> Duration {
        Duration::from_millis(self.monitor.sync_interval_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
    use chrono::Utc;
    use serde_json::json;

    #[test]
    fn tls_config_default_enables_tls_and_rejects_self_signed() {
        let config = TlsConfig::default();
        assert!(config.enabled, "TLS 기본값은 활성화여야 함");
        assert!(
            !config.allow_self_signed,
            "자체 서명 인증서는 기본값으로 비허용"
        );
    }

    #[test]
    fn tls_config_deserializes_from_json() {
        let payload = json!({ "enabled": false, "allow_self_signed": true });
        let parsed: TlsConfig = serde_json::from_value(payload).unwrap();
        assert!(!parsed.enabled);
        assert!(parsed.allow_self_signed);
    }

    #[test]
    fn tls_config_empty_json_uses_defaults() {
        let parsed: TlsConfig = serde_json::from_value(json!({})).unwrap();
        assert!(parsed.enabled);
        assert!(!parsed.allow_self_signed);
    }

    #[test]
    fn app_config_default_includes_tls_enabled() {
        let config = AppConfig::default_config();
        assert!(config.tls.enabled, "AppConfig 기본값: TLS 활성화");
        assert!(!config.tls.allow_self_signed);
        assert!(!config.integration.enabled);
    }

    #[test]
    fn update_integrity_policy_rejects_disabled_signature_verification() {
        let config = UpdateConfig {
            enabled: true,
            require_signature_verification: false,
            ..UpdateConfig::default()
        };

        let result = config.validate_integrity_policy();
        assert!(result.is_err());
    }

    #[test]
    fn update_integrity_policy_rejects_invalid_public_key_length() {
        let config = UpdateConfig {
            enabled: true,
            require_signature_verification: true,
            signature_public_key: BASE64.encode([1u8; 16]),
            ..UpdateConfig::default()
        };

        let result = config.validate_integrity_policy();
        assert!(result.is_err());
    }

    #[test]
    fn update_integrity_policy_accepts_valid_key() {
        let config = UpdateConfig {
            enabled: true,
            require_signature_verification: true,
            signature_public_key: BASE64.encode([7u8; 32]),
            ..UpdateConfig::default()
        };

        let result = config.validate_integrity_policy();
        assert!(result.is_ok());
    }

    #[test]
    fn update_integrity_policy_rejects_invalid_version_floor() {
        let config = UpdateConfig {
            enabled: true,
            require_signature_verification: true,
            signature_public_key: BASE64.encode([7u8; 32]),
            min_allowed_version: Some("not-semver".to_string()),
            ..UpdateConfig::default()
        };

        let result = config.validate_integrity_policy();
        assert!(result.is_err());
    }

    #[test]
    fn grpc_config_defaults_disable_mtls() {
        let config = GrpcConfig::default();
        assert!(!config.use_tls);
        assert!(!config.mtls_enabled);
        assert!(config.tls_domain_name.is_none());
    }

    #[test]
    fn grpc_config_deserializes_mtls_fields() {
        let payload = json!({
            "use_grpc_auth": true,
            "use_grpc_context": true,
            "grpc_endpoint": "https://grpc.example.com:50051",
            "grpc_fallback_ports": [50052, 50053],
            "connect_timeout_secs": 5,
            "request_timeout_secs": 20,
            "use_tls": true,
            "mtls_enabled": true,
            "tls_domain_name": "grpc.example.com",
            "tls_ca_cert_path": "/etc/oneshim/ca.pem",
            "tls_client_cert_path": "/etc/oneshim/client.pem",
            "tls_client_key_path": "/etc/oneshim/client.key"
        });

        let parsed: GrpcConfig = serde_json::from_value(payload).expect("grpc config must parse");
        assert!(parsed.use_tls);
        assert!(parsed.mtls_enabled);
        assert_eq!(parsed.tls_domain_name.as_deref(), Some("grpc.example.com"));
    }

    #[test]
    fn ai_provider_validation_rejects_missing_remote_api_key() {
        let config = AiProviderConfig {
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Local,
            ocr_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.example.com/ocr".to_string(),
                api_key: "".to_string(),
                model: None,
                timeout_secs: 30,
                provider_type: AiProviderType::Generic,
                surface_id: None,
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("api_key"));
    }

    #[test]
    fn ai_provider_validation_accepts_remote_api_key_secret_binding() {
        let config = AiProviderConfig {
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Local,
            ocr_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.example.com/ocr".to_string(),
                api_key: "".to_string(),
                model: None,
                timeout_secs: 30,
                provider_type: AiProviderType::Generic,
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
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
    }

    #[test]
    fn ai_provider_validation_accepts_valid_remote_settings() {
        let config = AiProviderConfig {
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Remote,
            ocr_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.example.com/ocr".to_string(),
                api_key: "ocr-key".to_string(),
                model: None,
                timeout_secs: 30,
                provider_type: AiProviderType::Generic,
                surface_id: None,
                credential: None,
            }),
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.example.com/llm".to_string(),
                api_key: "llm-key".to_string(),
                model: Some("model-a".to_string()),
                timeout_secs: 30,
                provider_type: AiProviderType::Generic,
                surface_id: None,
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
    }

    #[test]
    fn ai_provider_validation_rejects_retired_model_by_policy() {
        let config = AiProviderConfig {
            llm_provider: LlmProviderType::Remote,
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.openai.com/v1/chat/completions".to_string(),
                api_key: "llm-key".to_string(),
                model: Some("gpt-3.5-turbo".to_string()),
                timeout_secs: 30,
                provider_type: AiProviderType::OpenAi,
                surface_id: None,
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("retired as of"));
    }

    #[test]
    fn ai_provider_validation_accepts_remote_ocr_in_cli_subscription_mode() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderSubscriptionCli,
            ocr_provider: OcrProviderType::Remote,
            llm_provider: LlmProviderType::Local,
            ocr_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.example.com/ocr".to_string(),
                api_key: "ocr-key".to_string(),
                model: None,
                timeout_secs: 30,
                provider_type: AiProviderType::Generic,
                surface_id: None,
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
    }

    #[test]
    fn ai_provider_validation_accepts_local_llm_in_oauth_mode_when_no_managed_llm_surface() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderOAuth,
            llm_provider: LlmProviderType::Local,
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
    }

    #[test]
    fn ai_provider_validation_accepts_direct_llm_surface_in_oauth_mode() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderOAuth,
            llm_provider: LlmProviderType::Remote,
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.anthropic.com/v1/messages".to_string(),
                api_key: "api-key".to_string(),
                model: Some("claude-opus-4-1-20250805".to_string()),
                timeout_secs: 30,
                provider_type: AiProviderType::Anthropic,
                surface_id: Some("provider_surface.anthropic.direct_api".to_string()),
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
    }

    #[test]
    fn ai_provider_validation_accepts_openai_managed_llm_surface_in_oauth_mode() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderOAuth,
            llm_provider: LlmProviderType::Remote,
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.openai.com/v1".to_string(),
                api_key: "".to_string(),
                model: Some("gpt-5.4".to_string()),
                timeout_secs: 30,
                provider_type: AiProviderType::OpenAi,
                surface_id: Some("provider_surface.openai.managed_oauth".to_string()),
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
    }

    #[test]
    fn ai_provider_validation_accepts_google_managed_ocr_surface_in_oauth_mode() {
        let config = AiProviderConfig {
            access_mode: AiAccessMode::ProviderOAuth,
            llm_provider: LlmProviderType::Local,
            ocr_provider: OcrProviderType::Remote,
            ocr_api: Some(ExternalApiEndpoint {
                endpoint: "https://vision.googleapis.com/v1/images:annotate".to_string(),
                api_key: "".to_string(),
                model: None,
                timeout_secs: 30,
                provider_type: AiProviderType::Google,
                surface_id: Some("provider_surface.google.managed_oauth".to_string()),
                credential: None,
            }),
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
    }

    #[test]
    fn ai_provider_validation_rejects_invalid_ocr_min_confidence() {
        let config = AiProviderConfig {
            ocr_validation: OcrValidationConfig {
                enabled: true,
                min_confidence: 1.5,
                max_invalid_ratio: 0.5,
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("min_confidence"));
    }

    #[test]
    fn ai_provider_validation_rejects_invalid_ocr_invalid_ratio() {
        let config = AiProviderConfig {
            ocr_validation: OcrValidationConfig {
                enabled: true,
                min_confidence: 0.3,
                max_invalid_ratio: -0.1,
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("max_invalid_ratio"));
    }

    #[test]
    fn ai_provider_validation_rejects_invalid_scene_min_confidence() {
        let config = AiProviderConfig {
            scene_intelligence: SceneIntelligenceConfig {
                min_confidence: 1.2,
                ..SceneIntelligenceConfig::default()
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("scene_intelligence"));
    }

    #[test]
    fn ai_provider_validation_rejects_invalid_scene_max_elements() {
        let config = AiProviderConfig {
            scene_intelligence: SceneIntelligenceConfig {
                max_elements: 0,
                ..SceneIntelligenceConfig::default()
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("max_elements"));
    }

    #[test]
    fn scene_action_override_validation_rejects_missing_reason() {
        let config = AiProviderConfig {
            scene_action_override: SceneActionOverrideConfig {
                enabled: true,
                reason: None,
                approved_by: Some("sec-review".to_string()),
                expires_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("reason"));
    }

    #[test]
    fn scene_action_override_validation_rejects_missing_approver() {
        let config = AiProviderConfig {
            scene_action_override: SceneActionOverrideConfig {
                enabled: true,
                reason: Some("OCR confidence fallback".to_string()),
                approved_by: None,
                expires_at: Some(Utc::now() + chrono::Duration::minutes(30)),
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("approved_by"));
    }

    #[test]
    fn scene_action_override_validation_rejects_expired_ttl() {
        let config = AiProviderConfig {
            scene_action_override: SceneActionOverrideConfig {
                enabled: true,
                reason: Some("incident investigation".to_string()),
                approved_by: Some("oncall-lead".to_string()),
                expires_at: Some(Utc::now() - chrono::Duration::minutes(1)),
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must be in the future"));
    }

    #[test]
    fn scene_action_override_validation_accepts_valid_config() {
        let config = AiProviderConfig {
            scene_action_override: SceneActionOverrideConfig {
                enabled: true,
                reason: Some("high-fidelity replay calibration".to_string()),
                approved_by: Some("security-reviewer".to_string()),
                expires_at: Some(Utc::now() + chrono::Duration::minutes(45)),
            },
            ..AiProviderConfig::default()
        };

        let result = config.validate_selected_remote_endpoints();
        assert!(result.is_ok());
        assert!(config
            .scene_action_override
            .is_active_at(Utc::now() + chrono::Duration::minutes(1)));
    }

    // ── Task 27a: AppConfig serde round-trip tests ──────────────────

    #[test]
    fn default_config_round_trips_through_json() {
        let original = AppConfig::default_config();
        let json_str =
            serde_json::to_string_pretty(&original).expect("default config must serialize to JSON");
        let restored: AppConfig =
            serde_json::from_str(&json_str).expect("serialized default config must deserialize");

        // Compare via serde_json::Value to tolerate HashMap key ordering
        // differences (e.g., coaching.profiles HashMap).
        let val_original: serde_json::Value =
            serde_json::from_str(&json_str).expect("parse original");
        let json_restored =
            serde_json::to_string_pretty(&restored).expect("restored config must re-serialize");
        let val_restored: serde_json::Value =
            serde_json::from_str(&json_restored).expect("parse restored");
        assert_eq!(
            val_original, val_restored,
            "JSON values must be identical after a serialize→deserialize→serialize round-trip"
        );
    }

    #[test]
    fn config_with_unknown_fields_deserializes_without_error() {
        let mut original = AppConfig::default_config();
        let mut json_val = serde_json::to_value(&original).expect("default config must serialize");

        // Inject unknown top-level field
        json_val
            .as_object_mut()
            .unwrap()
            .insert("unknown_future_section".into(), json!({ "beta": true }));

        // Inject unknown field inside an existing section
        json_val
            .get_mut("server")
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("experimental_flag".into(), json!(42));

        let parsed: AppConfig =
            serde_json::from_value(json_val).expect("unknown fields should be silently ignored");

        // Verify known fields survived
        original.server.base_url = parsed.server.base_url.clone(); // avoid lifetime issues
        assert_eq!(
            parsed.server.request_timeout_ms,
            original.server.request_timeout_ms
        );
    }

    #[test]
    fn each_section_has_sensible_defaults() {
        let config = AppConfig::default_config();

        // Server
        assert!(
            !config.server.base_url.is_empty(),
            "server.base_url must not be empty"
        );
        assert!(
            config.server.request_timeout_ms > 0,
            "server.request_timeout_ms must be positive"
        );
        assert!(
            config.server.sse_max_retry_secs > 0,
            "server.sse_max_retry_secs must be positive"
        );

        // Monitor
        assert!(
            config.monitor.poll_interval_ms > 0,
            "monitor.poll_interval_ms must be positive"
        );
        assert!(
            config.monitor.sync_interval_ms > 0,
            "monitor.sync_interval_ms must be positive"
        );
        assert!(
            config.monitor.heartbeat_interval_ms > 0,
            "monitor.heartbeat_interval_ms must be positive"
        );
        assert!(
            config.monitor.idle_threshold_secs > 0,
            "monitor.idle_threshold_secs must be positive"
        );

        // Storage
        assert!(
            config.storage.retention_days > 0,
            "storage.retention_days must be positive"
        );
        assert!(
            config.storage.max_storage_mb > 0,
            "storage.max_storage_mb must be positive"
        );

        // Vision
        assert!(
            config.vision.thumbnail_width > 0,
            "vision.thumbnail_width must be positive"
        );
        assert!(
            config.vision.thumbnail_height > 0,
            "vision.thumbnail_height must be positive"
        );
        assert!(
            config.vision.capture_throttle_ms > 0,
            "vision.capture_throttle_ms must be positive"
        );

        // Web
        assert!(
            config.web.port > 0,
            "web.port must be a valid non-zero port"
        );

        // Update
        assert!(
            !config.update.repo_owner.is_empty(),
            "update.repo_owner must not be empty"
        );
        assert!(
            !config.update.repo_name.is_empty(),
            "update.repo_name must not be empty"
        );
        assert!(
            config.update.check_interval_hours > 0,
            "update.check_interval_hours must be positive"
        );

        // Notification
        assert!(
            config.notification.idle_notification_mins > 0,
            "notification.idle_notification_mins must be positive"
        );
        assert!(
            config.notification.long_session_mins > 0,
            "notification.long_session_mins must be positive"
        );

        // gRPC
        assert!(
            !config.grpc.grpc_endpoint.is_empty(),
            "grpc.grpc_endpoint must not be empty"
        );
        assert!(
            config.grpc.connect_timeout_secs > 0,
            "grpc.connect_timeout_secs must be positive"
        );
        assert!(
            config.grpc.request_timeout_secs > 0,
            "grpc.request_timeout_secs must be positive"
        );

        // Duration helpers
        assert!(
            config.request_timeout().as_millis() > 0,
            "request_timeout() must be non-zero"
        );
        assert!(
            config.poll_interval().as_millis() > 0,
            "poll_interval() must be non-zero"
        );
        assert!(
            config.sync_interval().as_millis() > 0,
            "sync_interval() must be non-zero"
        );
    }

    // ── Config bounds validation tests ────────────────────────────────

    #[test]
    fn storage_validate_bounds_rejects_zero_retention_days() {
        let config = StorageConfig {
            db_path: None,
            retention_days: 0,
            max_storage_mb: 500,
        };
        assert!(config.validate_bounds().is_err());
    }

    #[test]
    fn storage_validate_bounds_rejects_low_max_storage_mb() {
        let config = StorageConfig {
            db_path: None,
            retention_days: 30,
            max_storage_mb: 5,
        };
        let err = config.validate_bounds().unwrap_err();
        assert!(err.contains("max_storage_mb"));
    }

    #[test]
    fn storage_validate_bounds_accepts_valid_config() {
        let config = StorageConfig {
            db_path: None,
            retention_days: 1,
            max_storage_mb: 10,
        };
        assert!(config.validate_bounds().is_ok());
    }

    #[test]
    fn vision_validate_bounds_rejects_zero_throttle() {
        let config = VisionConfig {
            capture_enabled: true,
            capture_throttle_ms: 0,
            thumbnail_width: 480,
            thumbnail_height: 270,
            ocr_enabled: false,
            privacy_mode: false,
        };
        assert!(config.validate_bounds().is_err());
    }

    #[test]
    fn vision_validate_bounds_rejects_low_throttle() {
        let config = VisionConfig {
            capture_enabled: true,
            capture_throttle_ms: 50,
            thumbnail_width: 480,
            thumbnail_height: 270,
            ocr_enabled: false,
            privacy_mode: false,
        };
        let err = config.validate_bounds().unwrap_err();
        assert!(err.contains("capture_throttle_ms"));
    }

    #[test]
    fn vision_validate_bounds_accepts_min_throttle() {
        let config = VisionConfig {
            capture_enabled: true,
            capture_throttle_ms: 100,
            thumbnail_width: 480,
            thumbnail_height: 270,
            ocr_enabled: false,
            privacy_mode: false,
        };
        assert!(config.validate_bounds().is_ok());
    }

    #[test]
    fn app_config_validate_bounds_default_passes() {
        let config = AppConfig::default_config();
        assert!(config.validate_bounds().is_ok());
    }

    #[test]
    fn analysis_llm_work_type_enabled_defaults_true() {
        let payload = json!({});
        let config: AnalysisConfig = serde_json::from_value(payload).expect("must parse");
        assert!(config.llm_work_type_enabled);
    }

    #[test]
    fn analysis_llm_work_type_enabled_explicit_false() {
        let payload = json!({"llm_work_type_enabled": false});
        let config: AnalysisConfig = serde_json::from_value(payload).expect("must parse");
        assert!(!config.llm_work_type_enabled);
    }
}
