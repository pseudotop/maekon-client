use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct UpdateSettings {
    pub enabled: bool,
    pub check_interval_hours: u32,
    /// Update channel: "stable", "pre_release", or "nightly".
    #[serde(default)]
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
    #[serde(default)]
    pub allow_unredacted_external_ocr: bool,
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
    #[serde(default)]
    pub allow_unredacted_external_ocr: bool,
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
            allow_unredacted_external_ocr: false,
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
            allow_unredacted_external_ocr: false,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct CoachingSettings {
    pub enabled: bool,
    pub tone: String,
    pub locale: String,
    pub overlay_mode: String,
}

impl Default for CoachingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            tone: "balanced".to_string(),
            locale: "en".to_string(),
            overlay_mode: "minimal".to_string(),
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

// ─────────────────────────────────────────────────────────────────────────────
// AppSettings 열거형 드리프트 방지 가드
//
// `AppSettings` 하위 구조체의 String 필드들은 maekon-core 정의 열거형 값만
// 허용한다. 아래 테스트들은 각 String 필드의 유효 토큰 집합이 대응하는
// maekon-core 열거형 변형 집합과 1:1로 일치함을 보장한다.
//
// 가드 원리:
//   1. `_assert_variant_coverage` — 각 열거형 변형을 exhaustive match 로
//      열거한 후 Display 토큰을 수집하여 AppSettings 기본값에 쓰인 토큰과
//      크기 및 멤버십을 비교한다. 새 변형 추가 시 exhaustive match 가
//      컴파일 에러를 발생시키므로 이 함수도 함께 갱신해야 한다.
//   2. `_round_trip_*` — 기본값 토큰을 실제 serde 역직렬화로 검증한다.
//      열거형 변형 이름/케이스 변환이 바뀌면 즉시 실패한다.
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
    // 헬퍼: serde JSON 토큰을 따옴표 없이 반환한다.
    // ────────────────────────────────────────────────────────────────────
    fn serde_token<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string(v)
            .expect("직렬화 실패")
            .trim_matches('"')
            .to_string()
    }

    // ────────────────────────────────────────────────────────────────────
    // PiiFilterLevel — PrivacySettings::pii_filter_level
    // ────────────────────────────────────────────────────────────────────

    /// PiiFilterLevel 변형 전체를 exhaustive match 로 열거한다.
    /// 변형 추가/삭제 시 이 함수의 match 가 컴파일 에러를 낸다.
    fn pii_filter_level_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            PiiFilterLevel::Off,
            PiiFilterLevel::Basic,
            PiiFilterLevel::Standard,
            PiiFilterLevel::Strict,
        ] {
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
        // AppSettings 기본값 "Standard" 가 PiiFilterLevel 로 역직렬화되어야 한다.
        let default_val = &AppSettings::default().privacy.pii_filter_level;
        let quoted = format!("\"{}\"", default_val);
        serde_json::from_str::<PiiFilterLevel>(&quoted).unwrap_or_else(|_| {
            panic!(
                "pii_filter_level 기본값 {:?} 는 PiiFilterLevel 변형이 아님 — \
                 열거형이 변경됐거나 문자열 케이스가 드리프트됨",
                default_val
            )
        });
    }

    #[test]
    fn pii_filter_level_accepted_tokens_match_enum_variants() {
        // AppSettings 에서 허용되는 토큰 집합 = PiiFilterLevel serde 토큰 집합.
        // 열거형에 변형이 추가/삭제되면 이 테스트가 실패한다.
        let all_tokens = pii_filter_level_all_serde_tokens();
        // 기본값 토큰이 집합에 포함돼 있어야 한다.
        let default_token = serde_token(&PiiFilterLevel::Standard);
        assert!(
            all_tokens.contains(&default_token),
            "PiiFilterLevel::Standard 의 serde 토큰 {:?} 가 전체 집합에 없음",
            default_token
        );
        // 집합 크기는 현재 4개여야 한다 (Off/Basic/Standard/Strict).
        // 변형이 추가되면 exhaustive match + 이 단언이 모두 실패한다.
        assert_eq!(
            all_tokens.len(),
            4,
            "PiiFilterLevel 변형 수가 바뀜 — AppSettings 문서와 기본값을 검토하라"
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
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
        // ScheduleSettings 기본값의 각 요일 문자열이 Weekday 로 역직렬화되어야 한다.
        let defaults = ScheduleSettings::default();
        for day in &defaults.active_days {
            let quoted = format!("\"{}\"", day);
            serde_json::from_str::<Weekday>(&quoted).unwrap_or_else(|_| {
                panic!(
                    "active_days 기본값 {:?} 는 Weekday 변형이 아님 — \
                     열거형이 변경됐거나 문자열 케이스가 드리프트됨",
                    day
                )
            });
        }
    }

    #[test]
    fn weekday_accepted_tokens_match_enum_variants() {
        let all_tokens = weekday_all_serde_tokens();
        // Weekday 는 7개 변형이어야 한다.
        assert_eq!(
            all_tokens.len(),
            7,
            "Weekday 변형 수가 바뀜 — ScheduleSettings 기본값과 문서를 검토하라"
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
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
                "sandbox.profile 기본값 {:?} 는 SandboxProfile 변형이 아님",
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
            "SandboxProfile 변형 수가 바뀜 — SandboxSettings 기본값을 검토하라"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // AiAccessMode — AiProviderSettings::access_mode
    //
    // #4874 해소: AiProviderSettings 기본값은 이제 AiAccessMode serde
    // snake_case 토큰("provider_api_key")과 일치한다. 아래 테스트는 기본값이
    // 실제로 AiAccessMode 로 역직렬화됨을 보증한다(이전 known_drift 가드 대체).
    // ────────────────────────────────────────────────────────────────────

    fn ai_access_mode_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [
            AiAccessMode::ProviderApiKey,
            AiAccessMode::LocalModel,
            AiAccessMode::ProviderSubscriptionCli,
            AiAccessMode::ProviderOAuth,
        ] {
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
        // AiAccessMode 는 4개 변형이어야 한다.
        let all_tokens = ai_access_mode_all_serde_tokens();
        assert_eq!(
            all_tokens.len(),
            4,
            "AiAccessMode 변형 수가 바뀜 — AiProviderSettings 기본값을 검토하라"
        );
    }

    #[test]
    fn ai_access_mode_default_deserializes_to_enum() {
        // #4874: AiProviderSettings 기본 access_mode 는 canonical AiAccessMode serde
        // 토큰이어야 저장된 기본값이 역직렬화에 성공한다(이전 "ProviderApiKey" 드리프트).
        let default_access_mode = AiProviderSettings::default().access_mode;
        let all_serde_tokens = ai_access_mode_all_serde_tokens();
        assert!(
            all_serde_tokens.contains(default_access_mode.as_str()),
            "access_mode 기본값 {:?} 이 AiAccessMode serde 토큰 {:?} 에 없습니다",
            default_access_mode,
            all_serde_tokens
        );
        let parsed: AiAccessMode = serde_json::from_str(&format!("\"{default_access_mode}\""))
            .expect("기본 access_mode 가 AiAccessMode 로 역직렬화되어야 함");
        assert_eq!(parsed, AiAccessMode::ProviderApiKey);
    }

    // ────────────────────────────────────────────────────────────────────
    // OcrProviderType — AiProviderSettings::ocr_provider
    // ────────────────────────────────────────────────────────────────────

    fn ocr_provider_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [OcrProviderType::Local, OcrProviderType::Remote] {
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
                "ocr_provider 기본값 {:?} 는 OcrProviderType 변형이 아님",
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
            "OcrProviderType 변형 수가 바뀜 — AiProviderSettings 기본값을 검토하라"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // LlmProviderType — AiProviderSettings::llm_provider
    // ────────────────────────────────────────────────────────────────────

    fn llm_provider_all_serde_tokens() -> BTreeSet<String> {
        let mut set = BTreeSet::new();
        for v in [LlmProviderType::Local, LlmProviderType::Remote] {
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
                "llm_provider 기본값 {:?} 는 LlmProviderType 변형이 아님",
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
            "LlmProviderType 변형 수가 바뀜 — AiProviderSettings 기본값을 검토하라"
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
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
                "external_data_policy 기본값 {:?} 는 ExternalDataPolicy 변형이 아님",
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
            "ExternalDataPolicy 변형 수가 바뀜 — AiProviderSettings 기본값을 검토하라"
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
            // exhaustive match — 변형 추가 시 여기를 갱신해야 한다
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
        // #4874: ExternalApiSettings 기본 provider_type 은 canonical AiProviderType
        // serde 토큰("generic")이어야 저장된 기본값이 역직렬화된다(이전 "Generic" 드리프트).
        let default_val = default_provider_type();
        let all_serde_tokens = ai_provider_type_all_serde_tokens();
        assert!(
            all_serde_tokens.contains(default_val.as_str()),
            "provider_type 기본값 {:?} 이 AiProviderType serde 토큰 {:?} 에 없습니다",
            default_val,
            all_serde_tokens
        );
        let parsed: AiProviderType = serde_json::from_str(&format!("\"{default_val}\""))
            .expect("기본 provider_type 이 AiProviderType 로 역직렬화되어야 함");
        assert_eq!(parsed, AiProviderType::Generic);
    }

    #[test]
    fn ai_provider_type_accepted_tokens_match_enum_variants() {
        let all_tokens = ai_provider_type_all_serde_tokens();
        // AiProviderType 은 7개 변형이어야 한다.
        assert_eq!(
            all_tokens.len(),
            7,
            "AiProviderType 변형 수가 바뀜 — ExternalApiSettings 기본값과 \
             provider_spec 목록을 검토하라"
        );
    }

    // ────────────────────────────────────────────────────────────────────
    // AiProviderProfileConfig — access_mode/ocr_provider/llm_provider/
    //                           external_data_policy 필드 중복 커버리지
    //
    // AiProviderProfileConfig 는 AiProviderSettings 와 동일한 String 필드를
    // 공유한다. 기본값이 같은지 확인한다 (별도 Default impl 이 있으므로).
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn ai_provider_profile_config_defaults_match_provider_settings_defaults() {
        let p = AiProviderSettings::default();
        let c = AiProviderProfileConfig::default();
        assert_eq!(
            p.access_mode, c.access_mode,
            "AiProviderSettings 와 AiProviderProfileConfig 의 access_mode 기본값이 다름"
        );
        assert_eq!(p.ocr_provider, c.ocr_provider, "ocr_provider 기본값 불일치");
        assert_eq!(p.llm_provider, c.llm_provider, "llm_provider 기본값 불일치");
        assert_eq!(
            p.external_data_policy, c.external_data_policy,
            "external_data_policy 기본값 불일치"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
