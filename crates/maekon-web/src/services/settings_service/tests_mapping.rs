use crate::services::settings_assembler::config_to_settings;
use maekon_core::config::{
    AiAccessMode, AiProviderProfileConfig, AiProviderType, CredentialAuthMode,
    CredentialBackendKind, CredentialBinding, ExternalApiEndpoint, FocusSchedule, LlmProviderType,
    MicInputMode, OcrProviderType, SavedAiProviderProfile, SecretRef, SttLanguage, SttProviderKind,
    TimeRange, Weekday, WhisperModelSize,
};

#[test]
fn config_to_settings_maps_plaintext_api_keys_to_current_default_backend() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.example.com/v1".to_string(),
        api_key: "sk-test-1234567890".to_string(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 45,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "api_key");
    assert_eq!(llm_api.backend_kind, "os_secret_store");
    assert!(llm_api.has_secret);
    assert!(llm_api.can_edit_secret);
    assert_eq!(llm_api.secret_display_hint.as_deref(), Some("sk...7890"));
    assert!(!llm_api.projection_enabled);
}

#[test]
fn config_to_settings_marks_ollama_surface_as_no_auth() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "http://localhost:11434/v1/responses".to_string(),
        api_key: String::new(),
        model: Some("qwen3:8b".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::Ollama,
        surface_id: Some("provider_surface.ollama.local_http".to_string()),
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "api_key");
    assert_eq!(llm_api.backend_kind, "unavailable");
    assert!(!llm_api.has_secret);
    assert!(!llm_api.can_edit_secret);
    assert!(llm_api.api_key_masked.is_empty());
}

#[test]
fn config_to_settings_marks_provider_oauth_as_managed_oauth_metadata() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderOAuth;
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "managed_oauth");
    assert_eq!(llm_api.backend_kind, "os_secret_store");
    assert_eq!(
        llm_api.surface_id.as_deref(),
        Some("provider_surface.openai.managed_oauth")
    );
    assert!(!llm_api.has_secret);
    assert!(!llm_api.can_edit_secret);
    assert_eq!(llm_api.secret_display_hint, None);
}

#[test]
fn config_to_settings_roundtrips_saved_ai_provider_profiles() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.ai_provider.active_profile_id = Some("anthropic-prod".to_string());
    config.ai_provider.saved_profiles = vec![SavedAiProviderProfile {
        profile_id: "anthropic-prod".to_string(),
        name: "Anthropic Prod".to_string(),
        ai_provider: AiProviderProfileConfig {
            access_mode: AiAccessMode::ProviderApiKey,
            ocr_provider: OcrProviderType::Local,
            llm_provider: LlmProviderType::Remote,
            llm_api: Some(ExternalApiEndpoint {
                endpoint: "https://api.anthropic.com/v1/messages".to_string(),
                api_key: String::new(),
                model: Some("claude-3-7-sonnet-latest".to_string()),
                timeout_secs: 45,
                provider_type: AiProviderType::Anthropic,
                surface_id: Some("provider_surface.anthropic.direct_api".to_string()),
                credential: Some(CredentialBinding {
                    auth_mode: CredentialAuthMode::ApiKey,
                    backend_kind: CredentialBackendKind::OsSecretStore,
                    secret_ref: Some(SecretRef {
                        namespace: "provider/anthropic/anthropic-prod".to_string(),
                        key: "api_key".to_string(),
                    }),
                    projection_enabled: false,
                }),
            }),
            ..AiProviderProfileConfig::default()
        },
        updated_at: Some("2026-03-17T00:00:00Z".to_string()),
    }];

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);

    assert_eq!(
        settings.ai_provider.active_profile_id.as_deref(),
        Some("anthropic-prod")
    );
    assert_eq!(settings.ai_provider.saved_profiles.len(), 1);

    let saved_profile = &settings.ai_provider.saved_profiles[0];
    assert_eq!(saved_profile.profile_id, "anthropic-prod");
    assert_eq!(saved_profile.name, "Anthropic Prod");
    assert_eq!(
        saved_profile.updated_at.as_deref(),
        Some("2026-03-17T00:00:00Z")
    );
    assert_eq!(saved_profile.ai_provider.access_mode, "provider_api_key");
    assert_eq!(saved_profile.ai_provider.llm_provider, "Remote");

    let llm_api = saved_profile
        .ai_provider
        .llm_api
        .as_ref()
        .expect("saved profile llm api");
    assert_eq!(llm_api.provider_type, "anthropic");
    assert_eq!(
        llm_api.surface_id.as_deref(),
        Some("provider_surface.anthropic.direct_api")
    );
    assert_eq!(llm_api.timeout_secs, 45);
    assert!(llm_api.has_secret);
    assert_eq!(llm_api.backend_kind, "os_secret_store");
}

#[test]
fn config_to_settings_marks_cli_subscription_as_cli_bridge_metadata() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.ai_provider.access_mode = AiAccessMode::ProviderSubscriptionCli;
    config.ai_provider.llm_provider = LlmProviderType::Local;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: String::new(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: None,
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.auth_mode, "cli_bridge");
    assert_eq!(llm_api.backend_kind, "bridge_managed");
    assert_eq!(
        llm_api.surface_id.as_deref(),
        Some("provider_surface.openai.subprocess_cli")
    );
    assert!(!llm_api.can_edit_secret);
    assert_eq!(llm_api.secret_display_hint, None);
}

#[test]
fn config_to_settings_keeps_backend_managed_api_key_without_fake_mask() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::OsSecretStore,
            secret_ref: Some(SecretRef {
                namespace: "provider/openai/llm".to_string(),
                key: "api_key".to_string(),
            }),
            projection_enabled: false,
        }),
    });

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.backend_kind, "os_secret_store");
    assert!(llm_api.has_secret);
    assert_eq!(llm_api.api_key_masked, "");
    assert_eq!(llm_api.secret_display_hint, None);
}

#[test]
fn config_to_settings_marks_env_bound_api_key_as_present_without_secret_ref() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.ai_provider.llm_provider = LlmProviderType::Remote;
    config.ai_provider.llm_api = Some(ExternalApiEndpoint {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key: String::new(),
        model: Some("gpt-5.4".to_string()),
        timeout_secs: 30,
        provider_type: AiProviderType::OpenAi,
        surface_id: None,
        credential: Some(CredentialBinding {
            auth_mode: CredentialAuthMode::ApiKey,
            backend_kind: CredentialBackendKind::Env,
            secret_ref: None,
            projection_enabled: false,
        }),
    });

    let settings = config_to_settings(&config, CredentialBackendKind::Env);
    let llm_api = settings.ai_provider.llm_api.expect("llm api settings");

    assert_eq!(llm_api.backend_kind, "env");
    assert!(llm_api.has_secret);
    assert!(!llm_api.can_edit_secret);
    assert_eq!(llm_api.api_key_masked, "");
    assert_eq!(llm_api.secret_display_hint, None);
}

#[test]
fn config_to_settings_maps_coaching_profiles_and_quiet_hours() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.coaching.enabled = true;
    config.coaching.quiet_hours = vec![TimeRange {
        start: "22:00".to_string(),
        end: "07:30".to_string(),
    }];
    config.coaching.profiles.clear();
    config.coaching.profiles.insert(
        "FocusGuard".to_string(),
        maekon_core::config::ProfileConfig {
            enabled: true,
            min_interval_secs: 120,
        },
    );
    config.coaching.profiles.insert(
        "TimeAware".to_string(),
        maekon_core::config::ProfileConfig {
            enabled: false,
            min_interval_secs: 900,
        },
    );
    config
        .coaching
        .regime_goals
        .insert("deep_work".to_string(), 180);

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);

    assert!(settings.coaching.enabled);
    assert_eq!(settings.coaching.quiet_hours.len(), 1);
    assert_eq!(settings.coaching.quiet_hours[0].start, "22:00");
    assert_eq!(settings.coaching.quiet_hours[0].end, "07:30");
    assert_eq!(
        settings
            .coaching
            .profiles
            .get("FocusGuard")
            .expect("FocusGuard profile")
            .min_interval_secs,
        120
    );
    assert!(
        !settings
            .coaching
            .profiles
            .get("TimeAware")
            .expect("TimeAware profile")
            .enabled
    );
    assert_eq!(
        settings.coaching.regime_goals.get("deep_work").copied(),
        Some(180)
    );
}

#[test]
fn config_to_settings_maps_audio_and_focus_auto_sections() {
    let mut config = maekon_core::config::AppConfig::default_config();
    config.audio.enabled = true;
    config.audio.whisper_model_path = "/models/ggml-small.bin".to_string();
    config.audio.language = SttLanguage::Ko;
    config.audio.max_recording_secs = 45;
    config.audio.model_size = WhisperModelSize::Small;
    config.audio.stt_provider = SttProviderKind::Cloud;
    config.audio.cloud_api_key = "sk-audio-test".to_string();
    config.audio.cloud_stt_endpoint = "https://stt.example.com/v1/transcriptions".to_string();
    config.audio.cloud_timeout_secs = 22;
    config.audio.mic_input_mode = MicInputMode::VoiceActivity;
    config.audio.vad_threshold = 0.05;
    config.audio.vad_silence_ms = 1200;
    config.audio.vad_min_speech_ms = 450;
    config.focus_auto.enabled = true;
    config.focus_auto.duration_minutes = 50;
    config.focus_auto.trigger_apps = vec!["Code".to_string(), "Terminal".to_string()];
    config.focus_auto.trigger_schedules = vec![FocusSchedule {
        time_range: TimeRange {
            start: "09:00".to_string(),
            end: "12:00".to_string(),
        },
        days: vec![Weekday::Mon, Weekday::Wed],
    }];
    config.focus_auto.cooldown_secs = 900;

    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);

    assert!(settings.audio.enabled);
    assert_eq!(settings.audio.whisper_model_path, "/models/ggml-small.bin");
    assert_eq!(settings.audio.language, "ko");
    assert_eq!(settings.audio.max_recording_secs, 45);
    assert_eq!(settings.audio.model_size, "small");
    assert_eq!(settings.audio.stt_provider, "cloud");
    // #7066: the cloud STT BYOK secret must be MASKED on the read/GET path
    // (mirroring the AI-provider `api_key_masked` keys), NEVER returned in
    // cleartext. "sk-audio-test" (13 chars) masks to "sk...test".
    assert_ne!(
        settings.audio.cloud_api_key, "sk-audio-test",
        "raw cloud_api_key must not be returned by config_to_settings"
    );
    assert_eq!(settings.audio.cloud_api_key, "sk...test");
    assert_eq!(
        settings.audio.cloud_stt_endpoint,
        "https://stt.example.com/v1/transcriptions"
    );
    assert_eq!(settings.audio.cloud_timeout_secs, 22);
    assert_eq!(settings.audio.mic_input_mode, "voice_activity");
    assert_eq!(settings.audio.vad_threshold, 0.05);
    assert_eq!(settings.audio.vad_silence_ms, 1200);
    assert_eq!(settings.audio.vad_min_speech_ms, 450);

    assert!(settings.focus_auto.enabled);
    assert_eq!(settings.focus_auto.duration_minutes, 50);
    assert_eq!(
        settings.focus_auto.trigger_apps,
        vec!["Code".to_string(), "Terminal".to_string()]
    );
    assert_eq!(settings.focus_auto.trigger_schedules.len(), 1);
    let schedule = &settings.focus_auto.trigger_schedules[0];
    assert_eq!(schedule.start, "09:00");
    assert_eq!(schedule.end, "12:00");
    assert_eq!(schedule.days, vec!["Mon".to_string(), "Wed".to_string()]);
    assert_eq!(settings.focus_auto.cooldown_secs, 900);
}

/// #7066: a configured cloud STT secret is masked on the GET path, and an empty
/// (unconfigured) key stays empty so the UI can distinguish "no key" from "set".
#[test]
fn config_to_settings_masks_audio_cloud_api_key() {
    let mut config = maekon_core::config::AppConfig::default_config();

    // Configured key -> masked sentinel, never the raw secret.
    config.audio.cloud_api_key = "sk-1234567890abcdef".to_string();
    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    assert_ne!(
        settings.audio.cloud_api_key, "sk-1234567890abcdef",
        "raw cloud_api_key must never be returned on the read path"
    );
    assert_eq!(settings.audio.cloud_api_key, "sk...cdef");

    // No key configured -> empty (no spurious "***" sentinel).
    config.audio.cloud_api_key = String::new();
    let settings = config_to_settings(&config, CredentialBackendKind::OsSecretStore);
    assert_eq!(settings.audio.cloud_api_key, "");
}
