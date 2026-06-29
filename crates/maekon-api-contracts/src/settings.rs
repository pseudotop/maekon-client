use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct StorageStats {
    pub db_size_bytes: u64,
    pub frames_size_bytes: u64,
    pub total_size_bytes: u64,
    pub frame_count: u64,
    pub event_count: u64,
    pub metric_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_data_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_data_date: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AppSettings {
    pub retention_days: u32,
    pub max_storage_mb: u32,
    pub web_port: u16,
    pub allow_external: bool,
    pub capture_enabled: bool,
    pub idle_threshold_secs: u32,
    pub metrics_interval_secs: u32,
    pub process_interval_secs: u32,
    #[serde(default)]
    pub notification: NotificationSettings,
    #[serde(default)]
    pub update: UpdateSettings,
    #[serde(default)]
    pub telemetry: TelemetrySettings,
    #[serde(default)]
    pub monitor: MonitorControlSettings,
    #[serde(default)]
    pub privacy: PrivacySettings,
    #[serde(default)]
    pub schedule: ScheduleSettings,
    #[serde(default)]
    pub automation: AutomationSettings,
    #[serde(default)]
    pub sandbox: SandboxSettings,
    #[serde(default)]
    pub ai_provider: AiProviderSettings,
    #[serde(default)]
    pub ai_session: AiSessionSettings,
    #[serde(default)]
    pub suggestion: SuggestionSettings,
    #[serde(default)]
    pub indicator: IndicatorSettings,
    #[serde(default)]
    pub analysis: AnalysisSettings,
    #[serde(default)]
    pub network: NetworkSettings,
    #[serde(default)]
    pub coaching: CoachingSettings,
    #[serde(default)]
    pub integration: IntegrationSettings,
    #[serde(default)]
    pub sync: SyncSettings,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub focus_auto: FocusAutoSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NotificationSettings {
    pub enabled: bool,
    pub idle_notification: bool,
    pub idle_notification_mins: u32,
    pub long_session_notification: bool,
    pub long_session_mins: u32,
    pub high_usage_notification: bool,
    pub high_usage_threshold: u32,
}

/// serde default for `UpdateSettings::channel`: a missing channel (e.g. legacy
/// settings persisted before the field existed) deserializes to the valid
/// "stable" channel rather than an empty string (which is not a valid channel).
/// Matches the struct `Default` and the TS/openapi contract, which treat
/// `channel` as a required, always-present field.
fn default_update_channel() -> String {
    "stable".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateSettings {
    pub enabled: bool,
    pub check_interval_hours: u32,
    /// Update channel: "stable", "pre_release", or "nightly".
    #[serde(default = "default_update_channel")]
    pub channel: String,
    /// Legacy field — kept for backward compatibility. New code uses `channel`.
    #[serde(default, skip_serializing)]
    pub include_prerelease: bool,
    pub auto_install: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_hours: 24,
            channel: "stable".to_string(),
            include_prerelease: false,
            auto_install: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct TelemetrySettings {
    pub enabled: bool,
    pub crash_reports: bool,
    pub usage_analytics: bool,
    pub performance_metrics: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MonitorControlSettings {
    pub process_monitoring: bool,
    pub input_activity: bool,
    pub privacy_mode: bool,
}

impl Default for MonitorControlSettings {
    fn default() -> Self {
        Self {
            process_monitoring: true,
            input_activity: true,
            privacy_mode: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PrivacySettings {
    pub excluded_apps: Vec<String>,
    pub excluded_app_patterns: Vec<String>,
    pub excluded_title_patterns: Vec<String>,
    pub auto_exclude_sensitive: bool,
    pub pii_filter_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ScheduleSettings {
    pub active_hours_enabled: bool,
    pub active_start_hour: u8,
    pub active_end_hour: u8,
    pub active_days: Vec<String>,
    pub pause_on_screen_lock: bool,
    pub pause_on_battery_saver: bool,
}

impl Default for ScheduleSettings {
    fn default() -> Self {
        Self {
            active_hours_enabled: false,
            active_start_hour: 9,
            active_end_hour: 18,
            active_days: vec![
                "Mon".to_string(),
                "Tue".to_string(),
                "Wed".to_string(),
                "Thu".to_string(),
                "Fri".to_string(),
            ],
            pause_on_screen_lock: true,
            pause_on_battery_saver: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutomationSettings {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SandboxSettings {
    pub enabled: bool,
    pub profile: String,
    pub allowed_read_paths: Vec<String>,
    pub allowed_write_paths: Vec<String>,
    pub allow_network: bool,
    pub max_memory_bytes: u64,
    pub max_cpu_time_ms: u64,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: "Standard".to_string(),
            allowed_read_paths: Vec::new(),
            allowed_write_paths: Vec::new(),
            allow_network: false,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AiProviderSettings {
    pub access_mode: String,
    pub ocr_provider: String,
    pub llm_provider: String,
    pub external_data_policy: String,
    // #5966: renamed from `allow_unredacted_external_ocr`. The serde alias keeps
    // existing settings payloads (and older frontend builds) deserializing.
    #[serde(default, alias = "allow_unredacted_external_ocr")]
    pub bypass_pii_filter_for_external_ocr: bool,
    #[serde(default)]
    pub ocr_validation: OcrValidationSettings,
    #[serde(default)]
    pub scene_action_override: SceneActionOverrideSettings,
    #[serde(default)]
    pub scene_intelligence: SceneIntelligenceSettings,
    pub fallback_to_local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_api: Option<ExternalApiSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_api: Option<ExternalApiSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_profile_id: Option<String>,
    #[serde(default)]
    pub saved_profiles: Vec<SavedAiProviderProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AiProviderProfileConfig {
    pub access_mode: String,
    pub ocr_provider: String,
    pub llm_provider: String,
    pub external_data_policy: String,
    // #5966: renamed from `allow_unredacted_external_ocr` (see `AiProviderSettings`).
    #[serde(default, alias = "allow_unredacted_external_ocr")]
    pub bypass_pii_filter_for_external_ocr: bool,
    #[serde(default)]
    pub ocr_validation: OcrValidationSettings,
    #[serde(default)]
    pub scene_action_override: SceneActionOverrideSettings,
    #[serde(default)]
    pub scene_intelligence: SceneIntelligenceSettings,
    pub fallback_to_local: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_api: Option<ExternalApiSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_api: Option<ExternalApiSettings>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SavedAiProviderProfile {
    pub profile_id: String,
    pub name: String,
    #[serde(default)]
    pub ai_provider: AiProviderProfileConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OcrValidationSettings {
    pub enabled: bool,
    pub min_confidence: f64,
    pub max_invalid_ratio: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SceneActionOverrideSettings {
    pub enabled: bool,
    pub reason: String,
    pub approved_by: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SceneIntelligenceSettings {
    pub enabled: bool,
    pub overlay_enabled: bool,
    pub allow_action_execution: bool,
    pub min_confidence: f64,
    pub max_elements: u32,
    pub calibration_enabled: bool,
    pub calibration_min_elements: u32,
    pub calibration_min_avg_confidence: f64,
}

impl Default for OcrValidationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            min_confidence: 0.25,
            max_invalid_ratio: 0.6,
        }
    }
}

impl Default for SceneIntelligenceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            overlay_enabled: true,
            allow_action_execution: false,
            min_confidence: 0.35,
            max_elements: 120,
            calibration_enabled: true,
            calibration_min_elements: 8,
            calibration_min_avg_confidence: 0.55,
        }
    }
}

impl Default for AiProviderSettings {
    fn default() -> Self {
        Self {
            access_mode: "provider_api_key".to_string(),
            ocr_provider: "Local".to_string(),
            llm_provider: "Local".to_string(),
            external_data_policy: "PiiFilterStrict".to_string(),
            bypass_pii_filter_for_external_ocr: false,
            ocr_validation: OcrValidationSettings::default(),
            scene_action_override: SceneActionOverrideSettings::default(),
            scene_intelligence: SceneIntelligenceSettings::default(),
            fallback_to_local: true,
            ocr_api: None,
            llm_api: None,
            active_profile_id: None,
            saved_profiles: Vec::new(),
        }
    }
}

impl Default for AiProviderProfileConfig {
    fn default() -> Self {
        Self {
            access_mode: "provider_api_key".to_string(),
            ocr_provider: "Local".to_string(),
            llm_provider: "Local".to_string(),
            external_data_policy: "PiiFilterStrict".to_string(),
            bypass_pii_filter_for_external_ocr: false,
            ocr_validation: OcrValidationSettings::default(),
            scene_action_override: SceneActionOverrideSettings::default(),
            scene_intelligence: SceneIntelligenceSettings::default(),
            fallback_to_local: true,
            ocr_api: None,
            llm_api: None,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExternalApiSettings {
    pub endpoint: String,
    pub api_key_masked: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default = "default_provider_type")]
    pub provider_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_id: Option<String>,
    #[serde(default = "default_external_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_credential_auth_mode")]
    pub auth_mode: String,
    #[serde(default = "default_credential_backend_kind")]
    pub backend_kind: String,
    #[serde(default)]
    pub has_secret: bool,
    #[serde(default = "default_true")]
    pub can_edit_secret: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_display_hint: Option<String>,
    #[serde(default)]
    pub projection_enabled: bool,
}

fn default_external_timeout() -> u64 {
    30
}

fn default_provider_type() -> String {
    "generic".to_string()
}

fn default_credential_auth_mode() -> String {
    "api_key".to_string()
}

fn default_credential_backend_kind() -> String {
    "unavailable".to_string()
}

fn default_true() -> bool {
    true
}

impl Default for ExternalApiSettings {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            api_key_masked: String::new(),
            model: None,
            provider_type: default_provider_type(),
            surface_id: None,
            timeout_secs: default_external_timeout(),
            auth_mode: default_credential_auth_mode(),
            backend_kind: default_credential_backend_kind(),
            has_secret: false,
            can_edit_secret: default_true(),
            secret_display_hint: None,
            projection_enabled: false,
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            retention_days: 30,
            max_storage_mb: 500,
            web_port: maekon_core::config::DEFAULT_WEB_PORT,
            allow_external: false,
            capture_enabled: true,
            idle_threshold_secs: 300,
            metrics_interval_secs: 5,
            process_interval_secs: 10,
            notification: NotificationSettings {
                enabled: true,
                idle_notification: true,
                idle_notification_mins: 30,
                long_session_notification: true,
                long_session_mins: 60,
                high_usage_notification: false,
                high_usage_threshold: 90,
            },
            update: UpdateSettings::default(),
            telemetry: TelemetrySettings::default(),
            monitor: MonitorControlSettings::default(),
            privacy: PrivacySettings {
                auto_exclude_sensitive: true,
                pii_filter_level: "Standard".to_string(),
                ..Default::default()
            },
            schedule: ScheduleSettings::default(),
            automation: AutomationSettings::default(),
            sandbox: SandboxSettings::default(),
            ai_provider: AiProviderSettings::default(),
            ai_session: AiSessionSettings::default(),
            suggestion: SuggestionSettings::default(),
            indicator: IndicatorSettings::default(),
            analysis: AnalysisSettings::default(),
            network: NetworkSettings::default(),
            coaching: CoachingSettings::default(),
            integration: IntegrationSettings::default(),
            sync: SyncSettings::default(),
            audio: AudioSettings::default(),
            focus_auto: FocusAutoSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AiSessionSettings {
    pub max_concurrent_sessions: u32,
    pub idle_timeout_secs: u64,
    pub session_timeout_secs: u64,
    pub max_retries: u32,
    pub max_history_turns: u32,
    pub health_check_interval_secs: u64,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<serde_json::Value>,
}

fn default_max_output_tokens() -> u32 {
    4096
}

impl Default for AiSessionSettings {
    fn default() -> Self {
        Self {
            max_concurrent_sessions: 3,
            idle_timeout_secs: 300,
            session_timeout_secs: 600,
            max_retries: 3,
            max_history_turns: 100,
            health_check_interval_secs: 30,
            max_output_tokens: default_max_output_tokens(),
            thinking: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestionSettings {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IndicatorSettings {
    pub show_border: bool,
    pub show_panel: bool,
    pub border_opacity: f32,
}

impl Default for IndicatorSettings {
    fn default() -> Self {
        Self {
            show_border: true,
            show_panel: true,
            border_opacity: 0.6,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AnalysisSettings {
    pub enabled: bool,
    pub interval_secs: u64,
    pub min_confidence: f64,
    pub max_suggestions: u32,
    pub embedding_enabled: bool,
    pub gui_intelligence_enabled: bool,
    pub text_intelligence_enabled: bool,
    /// Whether the EMA-based auto-tuner (drift detection + re-clustering) is active.
    pub auto_tuner_enabled: bool,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 60,
            min_confidence: 0.5,
            max_suggestions: 5,
            embedding_enabled: true,
            gui_intelligence_enabled: true,
            text_intelligence_enabled: true,
            auto_tuner_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct NetworkSettings {
    pub server_base_url: String,
    pub request_timeout_ms: u64,
    pub grpc_enabled: bool,
    pub grpc_endpoint: String,
    pub tls_enabled: bool,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            server_base_url: "http://localhost:8000".to_string(),
            request_timeout_ms: 30000,
            grpc_enabled: false,
            grpc_endpoint: "http://localhost:50051".to_string(),
            tls_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CoachingSettings {
    pub enabled: bool,
    pub tone: String,
    pub locale: String,
    pub overlay_mode: String,
    #[serde(default)]
    pub quiet_hours: Vec<CoachingTimeRangeSettings>,
    #[serde(default)]
    pub profiles: HashMap<String, CoachingProfileSettings>,
    #[serde(default)]
    pub regime_goals: HashMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CoachingTimeRangeSettings {
    pub start: String,
    pub end: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CoachingProfileSettings {
    pub enabled: bool,
    pub min_interval_secs: u64,
}

impl Default for CoachingSettings {
    fn default() -> Self {
        let default = maekon_core::config::CoachingConfig::default();
        Self {
            enabled: default.enabled,
            tone: format!("{}", default.tone),
            locale: default.locale,
            overlay_mode: format!("{}", default.overlay_mode),
            quiet_hours: default
                .quiet_hours
                .into_iter()
                .map(|range| CoachingTimeRangeSettings {
                    start: range.start,
                    end: range.end,
                })
                .collect(),
            profiles: default
                .profiles
                .into_iter()
                .map(|(name, profile)| {
                    (
                        name,
                        CoachingProfileSettings {
                            enabled: profile.enabled,
                            min_interval_secs: profile.min_interval_secs,
                        },
                    )
                })
                .collect(),
            regime_goals: default.regime_goals,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IntegrationSettings {
    pub enabled: bool,
    pub auth_profile_kind: String,
    pub request_timeout_secs: u64,
    pub sync_interval_secs: u64,
}

impl Default for IntegrationSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            auth_profile_kind: "none".to_string(),
            request_timeout_secs: 30,
            sync_interval_secs: 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SyncSettings {
    pub enabled: bool,
    pub transport: String,
    pub interval_secs: u64,
    pub device_name: String,
    pub lan_advertise: bool,
    /// Whether to compress changeset payloads before encryption.
    pub compression_enabled: bool,
}

impl Default for SyncSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: "none".to_string(),
            interval_secs: 300,
            device_name: String::new(),
            lan_advertise: false,
            compression_enabled: true,
        }
    }
}

// NOTE: Debug is hand-written (not derived) to mask `cloud_api_key` (#7066,
// mirroring the `ProviderModelsRequest` #5639 mitigation). `cloud_api_key` is a
// cloud STT BYOK secret; a derived Debug would emit it verbatim under any
// `{:?}`, so a single error-path `?settings` (including `{:?}` of the enclosing
// `AppSettings`) would leak the key to the file/OTel log sink.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AudioSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub whisper_model_path: String,
    #[serde(default = "default_audio_language")]
    pub language: String,
    #[serde(default = "default_audio_max_recording_secs")]
    pub max_recording_secs: u32,
    #[serde(default = "default_audio_model_size")]
    pub model_size: String,
    #[serde(default = "default_audio_stt_provider")]
    pub stt_provider: String,
    // SECURITY (#7066): cloud STT BYOK secret. On the read/GET path the
    // assembler returns a MASKED sentinel here (never the raw key), mirroring the
    // AI-provider `api_key_masked` convention. On the write/update path the
    // masked sentinel is treated as 'unchanged' so an unmodified resubmit does
    // not clobber the stored secret. See `maekon-web` settings_assembler /
    // settings_config_mutation.
    #[serde(default)]
    pub cloud_api_key: String,
    #[serde(default = "default_audio_cloud_stt_endpoint")]
    pub cloud_stt_endpoint: String,
    #[serde(default = "default_audio_cloud_timeout_secs")]
    pub cloud_timeout_secs: u32,
    #[serde(default = "default_audio_mic_input_mode")]
    pub mic_input_mode: String,
    #[serde(default = "default_audio_vad_threshold")]
    pub vad_threshold: f32,
    #[serde(default = "default_audio_vad_silence_ms")]
    pub vad_silence_ms: u32,
    #[serde(default = "default_audio_vad_min_speech_ms")]
    pub vad_min_speech_ms: u32,
}

impl std::fmt::Debug for AudioSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioSettings")
            .field("enabled", &self.enabled)
            .field("whisper_model_path", &self.whisper_model_path)
            .field("language", &self.language)
            .field("max_recording_secs", &self.max_recording_secs)
            .field("model_size", &self.model_size)
            .field("stt_provider", &self.stt_provider)
            .field("cloud_api_key", &"[REDACTED]")
            .field("cloud_stt_endpoint", &self.cloud_stt_endpoint)
            .field("cloud_timeout_secs", &self.cloud_timeout_secs)
            .field("mic_input_mode", &self.mic_input_mode)
            .field("vad_threshold", &self.vad_threshold)
            .field("vad_silence_ms", &self.vad_silence_ms)
            .field("vad_min_speech_ms", &self.vad_min_speech_ms)
            .finish()
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        let defaults = maekon_core::config::AudioConfig::default();
        Self {
            enabled: defaults.enabled,
            whisper_model_path: defaults.whisper_model_path,
            language: defaults.language.to_string(),
            max_recording_secs: defaults.max_recording_secs,
            model_size: defaults.model_size.to_string(),
            stt_provider: defaults.stt_provider.to_string(),
            cloud_api_key: defaults.cloud_api_key,
            cloud_stt_endpoint: defaults.cloud_stt_endpoint,
            cloud_timeout_secs: defaults.cloud_timeout_secs,
            mic_input_mode: defaults.mic_input_mode.to_string(),
            vad_threshold: defaults.vad_threshold,
            vad_silence_ms: defaults.vad_silence_ms,
            vad_min_speech_ms: defaults.vad_min_speech_ms,
        }
    }
}

fn default_audio_language() -> String {
    maekon_core::config::SttLanguage::default().to_string()
}

fn default_audio_max_recording_secs() -> u32 {
    maekon_core::config::AudioConfig::default().max_recording_secs
}

fn default_audio_model_size() -> String {
    maekon_core::config::WhisperModelSize::default().to_string()
}

fn default_audio_stt_provider() -> String {
    maekon_core::config::SttProviderKind::default().to_string()
}

fn default_audio_cloud_stt_endpoint() -> String {
    maekon_core::config::AudioConfig::default().cloud_stt_endpoint
}

fn default_audio_cloud_timeout_secs() -> u32 {
    maekon_core::config::AudioConfig::default().cloud_timeout_secs
}

fn default_audio_mic_input_mode() -> String {
    maekon_core::config::MicInputMode::default().to_string()
}

fn default_audio_vad_threshold() -> f32 {
    maekon_core::config::AudioConfig::default().vad_threshold
}

fn default_audio_vad_silence_ms() -> u32 {
    maekon_core::config::AudioConfig::default().vad_silence_ms
}

fn default_audio_vad_min_speech_ms() -> u32 {
    maekon_core::config::AudioConfig::default().vad_min_speech_ms
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FocusAutoSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_focus_auto_duration_minutes")]
    pub duration_minutes: u32,
    #[serde(default)]
    pub trigger_apps: Vec<String>,
    #[serde(default)]
    pub trigger_schedules: Vec<FocusScheduleSettings>,
    #[serde(default = "default_focus_auto_cooldown_secs")]
    pub cooldown_secs: u64,
}

impl Default for FocusAutoSettings {
    fn default() -> Self {
        let defaults = maekon_core::config::FocusAutoConfig::default();
        Self {
            enabled: defaults.enabled,
            duration_minutes: defaults.duration_minutes,
            trigger_apps: defaults.trigger_apps,
            trigger_schedules: defaults
                .trigger_schedules
                .into_iter()
                .map(FocusScheduleSettings::from)
                .collect(),
            cooldown_secs: defaults.cooldown_secs,
        }
    }
}

fn default_focus_auto_duration_minutes() -> u32 {
    maekon_core::config::FocusAutoConfig::default().duration_minutes
}

fn default_focus_auto_cooldown_secs() -> u64 {
    maekon_core::config::FocusAutoConfig::default().cooldown_secs
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FocusScheduleSettings {
    #[serde(default)]
    pub start: String,
    #[serde(default)]
    pub end: String,
    #[serde(default)]
    pub days: Vec<String>,
}

impl From<maekon_core::config::FocusSchedule> for FocusScheduleSettings {
    fn from(value: maekon_core::config::FocusSchedule) -> Self {
        Self {
            start: value.time_range.start,
            end: value.time_range.end,
            days: value.days.into_iter().map(|day| day.to_string()).collect(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// AppSettings enum drift-prevention guards
//
// The String fields of `AppSettings` sub-structs only accept the enum values
// defined in maekon-core. The tests below guarantee that the set of valid tokens
// for each String field matches the corresponding maekon-core enum's variant set
// 1:1.
//
// Guard principle:
//   1. `_assert_variant_coverage` — enumerate every enum variant via an
//      exhaustive match, then collect the Display tokens and compare both size
//      and membership against the tokens used in the AppSettings defaults. Adding
//      a new variant makes the exhaustive match a compile error, so this function
//      must be updated alongside it.
//   2. `_round_trip_*` — validate the default tokens via real serde
//      deserialization. They fail immediately if an enum variant name / case
//      conversion changes.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod enum_drift_guard {
    use super::*;
    use maekon_core::config::{
        AiAccessMode, AiProviderType, ExternalDataPolicy, LlmProviderType, OcrProviderType,
        PiiFilterLevel, SandboxProfile, Weekday,
    };
    use std::collections::BTreeSet;

    // ────────────────────────────────────────────────────────────────────
    // Helper: return the serde JSON token without surrounding quotes.
    // ────────────────────────────────────────────────────────────────────
    fn serde_token<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string(v)
            .expect("serialization failed")
            .trim_matches('"')
            .to_string()
    }

    // ────────────────────────────────────────────────────────────────────
    // PiiFilterLevel — PrivacySettings::pii_filter_level
    // ────────────────────────────────────────────────────────────────────

    /// Enumerate every PiiFilterLevel variant via an exhaustive match.
    /// Adding/removing a variant makes this function's match a compile error.
    fn pii_filter_level_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            PiiFilterLevel::Off,
            PiiFilterLevel::Basic,
            PiiFilterLevel::Standard,
            PiiFilterLevel::Strict,
        ] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                PiiFilterLevel::Off => "guard",
                PiiFilterLevel::Basic => "guard",
                PiiFilterLevel::Standard => "guard",
                PiiFilterLevel::Strict => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn pii_filter_level_default_is_valid_variant() {
        // The AppSettings default "Standard" must deserialize as a PiiFilterLevel.
        let default_val = &AppSettings::default().privacy.pii_filter_level;
        let quoted = format!("\"{}\"", default_val);
        serde_json::from_str::<PiiFilterLevel>(&quoted).unwrap_or_else(|_| {
            panic!(
                "pii_filter_level default {:?} is not a PiiFilterLevel variant — \
                 the enum changed or the string case drifted",
                default_val
            )
        });
    }

    #[test]
    fn pii_filter_level_accepted_tokens_match_enum_variants() {
        // Set of tokens accepted by AppSettings = set of PiiFilterLevel serde tokens.
        // This test fails if a variant is added to / removed from the enum.
        let all_tokens = pii_filter_level_all_serde_tokens();
        // The default token must be contained in the set.
        let default_token = serde_token(&PiiFilterLevel::Standard);
        assert!(
            all_tokens.contains(&default_token),
            "the serde token {:?} for PiiFilterLevel::Standard is not in the full set",
            default_token
        );
        // The set size must currently be 4 (Off/Basic/Standard/Strict).
        // Adding a variant makes both the exhaustive match and this assertion fail.
        assert_eq!(
            all_tokens.len(),
            4,
            "the PiiFilterLevel variant count changed — review the AppSettings docs and defaults"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // Weekday — ScheduleSettings::active_days
    // ────────────────────────────────────────────────────────────────────

    fn weekday_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
            Weekday::Sat,
            Weekday::Sun,
        ] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                Weekday::Mon => "guard",
                Weekday::Tue => "guard",
                Weekday::Wed => "guard",
                Weekday::Thu => "guard",
                Weekday::Fri => "guard",
                Weekday::Sat => "guard",
                Weekday::Sun => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn schedule_active_days_defaults_are_valid_weekday_variants() {
        // Each weekday string in the ScheduleSettings defaults must deserialize as a Weekday.
        let defaults = ScheduleSettings::default();
        for day in &defaults.active_days {
            let quoted = format!("\"{}\"", day);
            serde_json::from_str::<Weekday>(&quoted).unwrap_or_else(|_| {
                panic!(
                    "active_days default {:?} is not a Weekday variant — \
                     the enum changed or the string case drifted",
                    day
                )
            });
        }
    }

    #[test]
    fn weekday_accepted_tokens_match_enum_variants() {
        let all_tokens = weekday_all_serde_tokens();
        // Weekday must have 7 variants.
        assert_eq!(
            all_tokens.len(),
            7,
            "the Weekday variant count changed — review the ScheduleSettings defaults and docs"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // SandboxProfile — SandboxSettings::profile
    // ────────────────────────────────────────────────────────────────────

    fn sandbox_profile_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            SandboxProfile::Permissive,
            SandboxProfile::Standard,
            SandboxProfile::Strict,
        ] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                SandboxProfile::Permissive => "guard",
                SandboxProfile::Standard => "guard",
                SandboxProfile::Strict => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn sandbox_profile_default_is_valid_variant() {
        let default_val = &SandboxSettings::default().profile;
        let quoted = format!("\"{}\"", default_val);
        serde_json::from_str::<SandboxProfile>(&quoted).unwrap_or_else(|_| {
            panic!(
                "sandbox.profile default {:?} is not a SandboxProfile variant",
                default_val
            )
        });
    }

    #[test]
    fn sandbox_profile_accepted_tokens_match_enum_variants() {
        let all_tokens = sandbox_profile_all_serde_tokens();
        assert_eq!(
            all_tokens.len(),
            3,
            "the SandboxProfile variant count changed — review the SandboxSettings defaults"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // AiAccessMode — AiProviderSettings::access_mode
    //
    // #4874 resolution: the AiProviderSettings default now matches the AiAccessMode
    // serde snake_case token ("provider_api_key"). The test below guarantees the
    // default actually deserializes as an AiAccessMode (replacing the former
    // known_drift guard).
    // ────────────────────────────────────────────────────────────────────

    fn ai_access_mode_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            AiAccessMode::ProviderApiKey,
            AiAccessMode::LocalModel,
            AiAccessMode::ProviderSubscriptionCli,
            AiAccessMode::ProviderOAuth,
        ] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                AiAccessMode::ProviderApiKey => "guard",
                AiAccessMode::LocalModel => "guard",
                AiAccessMode::ProviderSubscriptionCli => "guard",
                AiAccessMode::ProviderOAuth => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn ai_access_mode_accepted_tokens_match_enum_variants() {
        // AiAccessMode must have 4 variants.
        let all_tokens = ai_access_mode_all_serde_tokens();
        assert_eq!(
            all_tokens.len(),
            4,
            "the AiAccessMode variant count changed — review the AiProviderSettings defaults"
        );
    }

    #[test]
    fn ai_access_mode_default_deserializes_to_enum() {
        // #4874: the AiProviderSettings default access_mode must be the canonical AiAccessMode
        // serde token so the stored default deserializes successfully (previously the
        // "ProviderApiKey" drift).
        let default_access_mode = AiProviderSettings::default().access_mode;
        let all_serde_tokens = ai_access_mode_all_serde_tokens();
        assert!(
            all_serde_tokens.contains(default_access_mode.as_str()),
            "access_mode default {:?} is not in the AiAccessMode serde tokens {:?}",
            default_access_mode,
            all_serde_tokens
        );
        let parsed: AiAccessMode = serde_json::from_str(&format!("\"{default_access_mode}\""))
            .expect("the default access_mode must deserialize as an AiAccessMode");
        assert_eq!(parsed, AiAccessMode::ProviderApiKey);
    }

    // ────────────────────────────────────────────────────────────────────
    // OcrProviderType — AiProviderSettings::ocr_provider
    // ────────────────────────────────────────────────────────────────────

    fn ocr_provider_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [OcrProviderType::Local, OcrProviderType::Remote] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                OcrProviderType::Local => "guard",
                OcrProviderType::Remote => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn ocr_provider_default_is_valid_variant() {
        let default_val = &AiProviderSettings::default().ocr_provider;
        let quoted = format!("\"{}\"", default_val);
        serde_json::from_str::<OcrProviderType>(&quoted).unwrap_or_else(|_| {
            panic!(
                "ocr_provider default {:?} is not an OcrProviderType variant",
                default_val
            )
        });
    }

    #[test]
    fn ocr_provider_accepted_tokens_match_enum_variants() {
        let all_tokens = ocr_provider_all_serde_tokens();
        assert_eq!(
            all_tokens.len(),
            2,
            "the OcrProviderType variant count changed — review the AiProviderSettings defaults"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // LlmProviderType — AiProviderSettings::llm_provider
    // ────────────────────────────────────────────────────────────────────

    fn llm_provider_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [LlmProviderType::Local, LlmProviderType::Remote] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                LlmProviderType::Local => "guard",
                LlmProviderType::Remote => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn llm_provider_default_is_valid_variant() {
        let default_val = &AiProviderSettings::default().llm_provider;
        let quoted = format!("\"{}\"", default_val);
        serde_json::from_str::<LlmProviderType>(&quoted).unwrap_or_else(|_| {
            panic!(
                "llm_provider default {:?} is not an LlmProviderType variant",
                default_val
            )
        });
    }

    #[test]
    fn llm_provider_accepted_tokens_match_enum_variants() {
        let all_tokens = llm_provider_all_serde_tokens();
        assert_eq!(
            all_tokens.len(),
            2,
            "the LlmProviderType variant count changed — review the AiProviderSettings defaults"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // ExternalDataPolicy — AiProviderSettings::external_data_policy
    // ────────────────────────────────────────────────────────────────────

    fn external_data_policy_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            ExternalDataPolicy::PiiFilterStrict,
            ExternalDataPolicy::PiiFilterStandard,
            ExternalDataPolicy::AllowFiltered,
        ] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                ExternalDataPolicy::PiiFilterStrict => "guard",
                ExternalDataPolicy::PiiFilterStandard => "guard",
                ExternalDataPolicy::AllowFiltered => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn external_data_policy_default_is_valid_variant() {
        let default_val = &AiProviderSettings::default().external_data_policy;
        let quoted = format!("\"{}\"", default_val);
        serde_json::from_str::<ExternalDataPolicy>(&quoted).unwrap_or_else(|_| {
            panic!(
                "external_data_policy default {:?} is not an ExternalDataPolicy variant",
                default_val
            )
        });
    }

    #[test]
    fn external_data_policy_accepted_tokens_match_enum_variants() {
        let all_tokens = external_data_policy_all_serde_tokens();
        assert_eq!(
            all_tokens.len(),
            3,
            "the ExternalDataPolicy variant count changed — review the AiProviderSettings defaults"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // AiProviderType — ExternalApiSettings::provider_type
    // ────────────────────────────────────────────────────────────────────

    fn ai_provider_type_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            AiProviderType::Anthropic,
            AiProviderType::OpenAi,
            AiProviderType::Google,
            AiProviderType::Ollama,
            AiProviderType::Bedrock,
            AiProviderType::Copilot,
            AiProviderType::Generic,
        ] {
            // exhaustive match — must be updated here when a variant is added
            let _ = match v {
                AiProviderType::Anthropic => "guard",
                AiProviderType::OpenAi => "guard",
                AiProviderType::Google => "guard",
                AiProviderType::Ollama => "guard",
                AiProviderType::Bedrock => "guard",
                AiProviderType::Copilot => "guard",
                AiProviderType::Generic => "guard",
            };
            set.insert(serde_token(&v));
        }
        set
    }

    #[test]
    fn ai_provider_type_default_provider_type_deserializes_to_enum() {
        // #4874: the ExternalApiSettings default provider_type must be the canonical
        // AiProviderType serde token ("generic") so the stored default deserializes
        // (previously the "Generic" drift).
        let default_val = default_provider_type();
        let all_serde_tokens = ai_provider_type_all_serde_tokens();
        assert!(
            all_serde_tokens.contains(default_val.as_str()),
            "provider_type default {:?} is not in the AiProviderType serde tokens {:?}",
            default_val,
            all_serde_tokens
        );
        let parsed: AiProviderType = serde_json::from_str(&format!("\"{default_val}\""))
            .expect("the default provider_type must deserialize as an AiProviderType");
        assert_eq!(parsed, AiProviderType::Generic);
    }

    #[test]
    fn ai_provider_type_accepted_tokens_match_enum_variants() {
        let all_tokens = ai_provider_type_all_serde_tokens();
        // AiProviderType must have 7 variants.
        assert_eq!(
            all_tokens.len(),
            7,
            "the AiProviderType variant count changed — review the ExternalApiSettings defaults \
             and the provider_spec list"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // AiProviderProfileConfig — overlapping coverage of the
    //                           access_mode/ocr_provider/llm_provider/external_data_policy fields
    //
    // AiProviderProfileConfig shares the same String fields as AiProviderSettings.
    // Confirm their defaults match (since it has a separate Default impl).
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn ai_provider_profile_config_defaults_match_provider_settings_defaults() {
        let p = AiProviderSettings::default();
        let c = AiProviderProfileConfig::default();
        assert_eq!(
            p.access_mode, c.access_mode,
            "the access_mode defaults of AiProviderSettings and AiProviderProfileConfig differ"
        );
        assert_eq!(
            p.ocr_provider, c.ocr_provider,
            "ocr_provider default mismatch"
        );
        assert_eq!(
            p.llm_provider, c.llm_provider,
            "llm_provider default mismatch"
        );
        assert_eq!(
            p.external_data_policy, c.external_data_policy,
            "external_data_policy default mismatch"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #5966: legacy IPC/settings payloads use the old
    /// `allow_unredacted_external_ocr` key; the serde alias must keep them
    /// deserializing so an older frontend build never silently loses the setting.
    #[test]
    fn ai_provider_settings_deserializes_legacy_allow_unredacted_external_ocr_alias() {
        let legacy_json = r#"{
            "access_mode": "provider_api_key",
            "ocr_provider": "Local",
            "llm_provider": "Local",
            "external_data_policy": "PiiFilterStrict",
            "allow_unredacted_external_ocr": true,
            "fallback_to_local": true
        }"#;
        let settings: AiProviderSettings = serde_json::from_str(legacy_json).unwrap();
        assert!(
            settings.bypass_pii_filter_for_external_ocr,
            "legacy `allow_unredacted_external_ocr` must map onto the renamed field"
        );
    }

    /// Serialization emits the renamed (canonical) key.
    #[test]
    fn ai_provider_settings_serializes_with_renamed_key() {
        let settings = AiProviderSettings {
            bypass_pii_filter_for_external_ocr: true,
            ..AiProviderSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(
            json.contains("bypass_pii_filter_for_external_ocr"),
            "serialized settings must use the renamed key, got: {json}"
        );
    }

    #[test]
    fn round_trip_storage_stats() {
        let original = StorageStats {
            db_size_bytes: 1024,
            frames_size_bytes: 2048,
            total_size_bytes: 3072,
            frame_count: 100,
            event_count: 500,
            metric_count: 200,
            oldest_data_date: Some("2026-01-01".to_string()),
            newest_data_date: Some("2026-04-11".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: StorageStats = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_notification_settings() {
        let original = NotificationSettings {
            enabled: true,
            idle_notification: true,
            idle_notification_mins: 30,
            long_session_notification: true,
            long_session_mins: 90,
            high_usage_notification: false,
            high_usage_threshold: 80,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: NotificationSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_privacy_settings() {
        let original = PrivacySettings {
            excluded_apps: vec!["Signal".to_string(), "1Password".to_string()],
            excluded_app_patterns: vec!["*password*".to_string()],
            excluded_title_patterns: vec!["*private*".to_string()],
            auto_exclude_sensitive: true,
            pii_filter_level: "Standard".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: PrivacySettings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_telemetry_settings() {
        let original = TelemetrySettings {
            enabled: true,
            crash_reports: true,
            usage_analytics: false,
            performance_metrics: true,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: TelemetrySettings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn round_trip_coaching_settings_profiles_and_quiet_hours() {
        let mut profiles = std::collections::HashMap::new();
        profiles.insert(
            "FocusGuard".to_string(),
            CoachingProfileSettings {
                enabled: true,
                min_interval_secs: 120,
            },
        );
        profiles.insert(
            "TimeAware".to_string(),
            CoachingProfileSettings {
                enabled: false,
                min_interval_secs: 900,
            },
        );

        let mut regime_goals = std::collections::HashMap::new();
        regime_goals.insert("deep_work".to_string(), 180);

        let original = CoachingSettings {
            enabled: true,
            tone: "Gentle".to_string(),
            locale: "ko".to_string(),
            overlay_mode: "Minimal".to_string(),
            quiet_hours: vec![CoachingTimeRangeSettings {
                start: "22:00".to_string(),
                end: "07:30".to_string(),
            }],
            profiles,
            regime_goals,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: CoachingSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn storage_stats_none_dates_roundtrip() {
        let original = StorageStats {
            db_size_bytes: 0,
            frames_size_bytes: 0,
            total_size_bytes: 0,
            frame_count: 0,
            event_count: 0,
            metric_count: 0,
            oldest_data_date: None,
            newest_data_date: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: StorageStats = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
        // skip_serializing_if means optional None fields should not appear
        assert!(!json.contains("oldest_data_date"));
        assert!(!json.contains("newest_data_date"));
    }

    /// #7066: the hand-written `Debug` for `AudioSettings` must redact the cloud
    /// STT BYOK secret so a `{:?}` cannot leak it to a log/OTel sink (same threat
    /// class #5639 closed for `ProviderModelsRequest`).
    #[test]
    fn audio_settings_debug_redacts_cloud_api_key() {
        let audio = AudioSettings {
            cloud_api_key: "sk-super-secret-cloud-stt-key".to_string(),
            ..AudioSettings::default()
        };
        let rendered = format!("{audio:?}");
        assert!(
            rendered.contains("[REDACTED]"),
            "Debug must mark cloud_api_key as redacted, got: {rendered}"
        );
        assert!(
            !rendered.contains("sk-super-secret-cloud-stt-key"),
            "Debug must NOT leak the raw cloud_api_key, got: {rendered}"
        );
    }

    /// #7066: the enclosing `AppSettings` derives `Debug`, which delegates to the
    /// hand-written `AudioSettings::fmt`, so `{:?}` of the whole settings tree
    /// also redacts the cloud STT secret.
    #[test]
    fn app_settings_debug_redacts_audio_cloud_api_key() {
        let mut settings = AppSettings::default();
        settings.audio.cloud_api_key = "sk-super-secret-cloud-stt-key".to_string();
        let rendered = format!("{settings:?}");
        assert!(
            !rendered.contains("sk-super-secret-cloud-stt-key"),
            "AppSettings Debug must NOT leak the raw audio cloud_api_key"
        );
        assert!(
            rendered.contains("[REDACTED]"),
            "AppSettings Debug must mark the audio cloud_api_key as redacted"
        );
    }
}
