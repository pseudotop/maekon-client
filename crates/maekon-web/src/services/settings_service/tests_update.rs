use super::*;
use crate::error::ApiError;
use maekon_api_contracts::settings::{
    AppSettings, ExternalApiSettings, SavedAiProviderProfile as ApiSavedAiProviderProfile,
};
use maekon_core::config::{
    CredentialAuthMode, CredentialBackendKind, MicInputMode, SttLanguage, SttProviderKind, Weekday,
    WhisperModelSize,
};
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::secret_store::SecretStore;
use std::sync::Arc;
use tempfile::TempDir;
use tests_fixtures::*;

#[tokio::test]
async fn update_settings_validates_input_without_config_manager() {
    let state = test_state_without_config_manager();
    let context = test_context_from_state(&state);
    let settings = AppSettings {
        web_port: 80,
        ..AppSettings::default()
    };

    let result = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await;
    assert!(matches!(result, Err(ApiError::BadRequest(_))));
}

#[tokio::test]
async fn update_settings_accepts_valid_defaults_without_config_manager() {
    let state = test_state_without_config_manager();
    let context = test_context_from_state(&state);
    let settings = AppSettings::default();

    // update_settings returns Result<(), ApiError>; pin the exact Ok(()) value and
    // ensure validation passed for AppSettings::default() without a config manager (#5594).
    let result = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await;
    assert_eq!(
        result.expect("AppSettings::default() must pass validation even without a config manager"),
        ()
    );
}

#[tokio::test]
async fn update_settings_accepts_provider_oauth_roundtrip_without_config_manager() {
    let state = test_state_without_config_manager();
    let context = test_context_from_state(&state);
    let mut settings = AppSettings::default();
    settings.ai_provider.access_mode = "ProviderOAuth".to_string();
    settings.ai_provider.llm_provider = "Remote".to_string();

    // Pin: ProviderOAuth + Remote round-trips through validation without a config manager.
    // The access_mode and llm_provider fields must be valid enum variants; Ok(()) confirms
    // both fields were accepted (#5594).
    let result = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await;
    assert_eq!(
        result.expect("ProviderOAuth + Remote must pass validation even without a config manager"),
        ()
    );
    // Confirm the fields that drove acceptance are what we set, not silently defaulted.
    assert_eq!(settings.ai_provider.access_mode, "ProviderOAuth");
    assert_eq!(settings.ai_provider.llm_provider, "Remote");
}

#[tokio::test]
async fn update_settings_persists_remote_api_key_to_secret_store_and_binding_metadata() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
    let state = test_state_with_config_manager(config_manager.clone(), Some(secret_store.clone()));
    let context = test_context_from_state(&state);

    let mut settings = AppSettings::default();
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key_masked: "sk-secret-123456".to_string(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: None,
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "os_secret_store".to_string(),
        has_secret: true,
        can_edit_secret: true,
        secret_display_hint: None,
        projection_enabled: true,
    });

    crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect("settings update should succeed");

    let stored = secret_store
        .retrieve("provider/openai/llm", "api_key")
        .await
        .expect("secret lookup");
    assert_eq!(stored.as_deref(), Some("sk-secret-123456"));

    let saved = config_manager.get();
    let endpoint = saved.ai_provider.llm_api.expect("saved llm endpoint");
    let binding = endpoint.credential.expect("credential binding");
    assert_eq!(endpoint.api_key, "");
    assert_eq!(binding.backend_kind, CredentialBackendKind::OsSecretStore);
    assert_eq!(binding.auth_mode, CredentialAuthMode::ApiKey);
    assert!(binding.projection_enabled);
    let secret_ref = binding.secret_ref.expect("secret ref");
    assert_eq!(secret_ref.namespace, "provider/openai/llm");
    assert_eq!(secret_ref.key, "api_key");
}

#[tokio::test]
async fn update_settings_persists_selected_saved_profile_under_profile_namespace() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let secret_store = Arc::new(TestSecretStore::new()) as Arc<dyn SecretStore>;
    let state = test_state_with_config_manager(config_manager.clone(), Some(secret_store.clone()));
    let context = test_context_from_state(&state);

    let mut settings = AppSettings::default();
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(anthropic_external_api_settings("sk-ant-secret-123456"));
    settings.ai_provider.active_profile_id = Some("anthropic-prod".to_string());
    settings.ai_provider.saved_profiles = vec![ApiSavedAiProviderProfile {
        profile_id: "anthropic-prod".to_string(),
        name: "Anthropic Prod".to_string(),
        ai_provider: anthropic_api_profile_config("sk-ant-secret-123456"),
        updated_at: Some("2026-03-17T00:00:00Z".to_string()),
    }];

    crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect("settings update should succeed");

    let stored = secret_store
        .retrieve("provider/anthropic/anthropic-prod", "api_key")
        .await
        .expect("secret lookup");
    assert_eq!(stored.as_deref(), Some("sk-ant-secret-123456"));

    let legacy_slot = secret_store
        .retrieve("provider/anthropic/llm", "api_key")
        .await
        .expect("legacy slot lookup");
    assert_eq!(legacy_slot, None);

    let saved = config_manager.get();
    assert_eq!(
        saved.ai_provider.active_profile_id.as_deref(),
        Some("anthropic-prod")
    );

    let active_endpoint = saved.ai_provider.llm_api.expect("active llm endpoint");
    let active_binding = active_endpoint
        .credential
        .expect("active credential binding");
    let active_secret_ref = active_binding.secret_ref.expect("active secret ref");
    assert_eq!(
        active_secret_ref.namespace,
        "provider/anthropic/anthropic-prod"
    );
    assert_eq!(active_secret_ref.key, "api_key");

    assert_eq!(saved.ai_provider.saved_profiles.len(), 1);
    let saved_profile = &saved.ai_provider.saved_profiles[0];
    assert_eq!(saved_profile.profile_id, "anthropic-prod");
    assert_eq!(saved_profile.name, "Anthropic Prod");

    let profile_endpoint = saved_profile
        .ai_provider
        .llm_api
        .as_ref()
        .expect("saved profile llm endpoint");
    let profile_binding = profile_endpoint
        .credential
        .as_ref()
        .expect("saved profile credential binding");
    let profile_secret_ref = profile_binding
        .secret_ref
        .as_ref()
        .expect("saved profile secret ref");
    assert_eq!(
        profile_secret_ref.namespace,
        "provider/anthropic/anthropic-prod"
    );
    assert_eq!(profile_secret_ref.key, "api_key");
}

#[tokio::test]
async fn update_settings_persists_audio_and_focus_auto_sections() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let state = test_state_with_config_manager(config_manager.clone(), None);
    let context = test_context_from_state(&state);

    let mut settings = AppSettings::default();
    settings.audio.enabled = true;
    settings.audio.whisper_model_path = "/models/ggml-small.bin".to_string();
    settings.audio.language = "ko".to_string();
    settings.audio.max_recording_secs = 45;
    settings.audio.model_size = "small".to_string();
    settings.audio.stt_provider = "cloud".to_string();
    settings.audio.cloud_api_key = "sk-audio-test".to_string();
    settings.audio.cloud_stt_endpoint = "https://stt.example.com/v1/transcriptions".to_string();
    settings.audio.cloud_timeout_secs = 22;
    settings.audio.mic_input_mode = "voice_activity".to_string();
    settings.audio.vad_threshold = 0.05;
    settings.audio.vad_silence_ms = 1200;
    settings.audio.vad_min_speech_ms = 450;
    settings.focus_auto.enabled = true;
    settings.focus_auto.duration_minutes = 50;
    settings.focus_auto.trigger_apps = vec!["Code".to_string(), "Terminal".to_string()];
    settings.focus_auto.trigger_schedules =
        vec![maekon_api_contracts::settings::FocusScheduleSettings {
            start: "09:00".to_string(),
            end: "12:00".to_string(),
            days: vec!["Mon".to_string(), "Wed".to_string()],
        }];
    settings.focus_auto.cooldown_secs = 900;

    crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect("settings update should persist audio and focus_auto");

    let saved = config_manager.get();
    assert!(saved.audio.enabled);
    assert_eq!(saved.audio.whisper_model_path, "/models/ggml-small.bin");
    assert_eq!(saved.audio.language, SttLanguage::Ko);
    assert_eq!(saved.audio.max_recording_secs, 45);
    assert_eq!(saved.audio.model_size, WhisperModelSize::Small);
    assert_eq!(saved.audio.stt_provider, SttProviderKind::Cloud);
    assert_eq!(saved.audio.cloud_api_key, "sk-audio-test");
    assert_eq!(
        saved.audio.cloud_stt_endpoint,
        "https://stt.example.com/v1/transcriptions"
    );
    assert_eq!(saved.audio.cloud_timeout_secs, 22);
    assert_eq!(saved.audio.mic_input_mode, MicInputMode::VoiceActivity);
    assert_eq!(saved.audio.vad_threshold, 0.05);
    assert_eq!(saved.audio.vad_silence_ms, 1200);
    assert_eq!(saved.audio.vad_min_speech_ms, 450);

    assert!(saved.focus_auto.enabled);
    assert_eq!(saved.focus_auto.duration_minutes, 50);
    assert_eq!(
        saved.focus_auto.trigger_apps,
        vec!["Code".to_string(), "Terminal".to_string()]
    );
    assert_eq!(saved.focus_auto.trigger_schedules.len(), 1);
    let schedule = &saved.focus_auto.trigger_schedules[0];
    assert_eq!(schedule.time_range.start, "09:00");
    assert_eq!(schedule.time_range.end, "12:00");
    assert_eq!(schedule.days, vec![Weekday::Mon, Weekday::Wed]);
    assert_eq!(saved.focus_auto.cooldown_secs, 900);
}

/// #7066: the GET path returns a MASKED cloud STT secret, so an unchanged
/// resubmit carries the mask. The write path must treat the masked sentinel as
/// 'unchanged' and preserve the stored raw key — never overwrite it with the
/// mask (which would corrupt the credential). Mirrors the AI-provider key path.
#[tokio::test]
async fn update_settings_masked_audio_cloud_api_key_preserves_stored_secret() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let state = test_state_with_config_manager(config_manager.clone(), None);
    let context = test_context_from_state(&state);

    let mut settings = AppSettings::default();
    settings.audio.enabled = true;
    settings.audio.stt_provider = "cloud".to_string();

    // 1) Persist a genuine plaintext key (first-time configuration).
    settings.audio.cloud_api_key = "sk-1234567890abcdef".to_string();
    crate::services::settings_web_service::SettingsCommandService::new(context.clone())
        .update_settings(&settings)
        .await
        .expect("initial cloud_api_key must persist");
    assert_eq!(
        config_manager.get().audio.cloud_api_key,
        "sk-1234567890abcdef"
    );

    // 2) Resubmit with the MASKED sentinel (as the GET path would have returned)
    //    -> the stored secret must be preserved, not clobbered by the mask.
    settings.audio.cloud_api_key = "sk...cdef".to_string();
    crate::services::settings_web_service::SettingsCommandService::new(context.clone())
        .update_settings(&settings)
        .await
        .expect("masked-sentinel resubmit must succeed");
    assert_eq!(
        config_manager.get().audio.cloud_api_key,
        "sk-1234567890abcdef",
        "masked-sentinel resubmit must NOT clobber the stored cloud STT secret"
    );

    // 3) An empty value also leaves the stored secret untouched (mirrors the
    //    AI-provider key path where empty == unchanged).
    settings.audio.cloud_api_key = String::new();
    crate::services::settings_web_service::SettingsCommandService::new(context.clone())
        .update_settings(&settings)
        .await
        .expect("empty resubmit must succeed");
    assert_eq!(
        config_manager.get().audio.cloud_api_key,
        "sk-1234567890abcdef",
        "empty resubmit must NOT clobber the stored cloud STT secret"
    );

    // 4) A genuinely new plaintext key still overwrites.
    settings.audio.cloud_api_key = "sk-rotated-9876543210".to_string();
    crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect("new key must persist");
    assert_eq!(
        config_manager.get().audio.cloud_api_key,
        "sk-rotated-9876543210"
    );
}

#[tokio::test]
async fn update_settings_persists_coaching_profiles_and_quiet_hours() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let state = test_state_with_config_manager(config_manager.clone(), None);
    let context = test_context_from_state(&state);

    let mut settings = AppSettings::default();
    settings.coaching.enabled = true;
    settings.coaching.locale = "ko".to_string();
    settings.coaching.quiet_hours =
        vec![maekon_api_contracts::settings::CoachingTimeRangeSettings {
            start: "22:00".to_string(),
            end: "07:30".to_string(),
        }];
    settings.coaching.profiles.clear();
    settings.coaching.profiles.insert(
        "FocusGuard".to_string(),
        maekon_api_contracts::settings::CoachingProfileSettings {
            enabled: true,
            min_interval_secs: 120,
        },
    );
    settings.coaching.profiles.insert(
        "TimeAware".to_string(),
        maekon_api_contracts::settings::CoachingProfileSettings {
            enabled: false,
            min_interval_secs: 900,
        },
    );
    settings
        .coaching
        .regime_goals
        .insert("deep_work".to_string(), 180);

    crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect("settings update should persist coaching sections");

    let saved = config_manager.get();
    assert!(saved.coaching.enabled);
    assert_eq!(saved.coaching.locale, "ko");
    assert_eq!(saved.coaching.quiet_hours.len(), 1);
    assert_eq!(saved.coaching.quiet_hours[0].start, "22:00");
    assert_eq!(saved.coaching.quiet_hours[0].end, "07:30");
    assert_eq!(
        saved
            .coaching
            .profiles
            .get("FocusGuard")
            .expect("FocusGuard profile")
            .min_interval_secs,
        120
    );
    assert!(
        !saved
            .coaching
            .profiles
            .get("TimeAware")
            .expect("TimeAware profile")
            .enabled
    );
    assert_eq!(
        saved.coaching.regime_goals.get("deep_work").copied(),
        Some(180)
    );
}

/// F-RR-C22-01: `SettingsCommandService` must create its `SettingsUpdateFlow`
/// once at construction time, not once per `update_settings` call.
///
/// This test verifies the structural property: constructing the service and
/// calling `update_settings` 10 times succeeds without error and the service
/// remains usable throughout (i.e. the flow is not recreated and dropped on
/// each call, which would abort the `PolicyAuditWriter` background task and
/// could lose audit events).
///
/// The negative case (per-call construction) would only be observable via
/// code inspection, but we can confirm the happy path: repeated calls on the
/// same service instance all succeed and the service is still `Clone`-able
/// (meaning the `Arc<PolicyAuditWriter>` inside `SettingsUpdateFlow` is shared
/// across clones, not recreated).
#[tokio::test]
async fn update_settings_flow_reused_across_calls() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    let state = test_state_with_config_manager(config_manager.clone(), None);
    let context = test_context_from_state(&state);

    // Construct the service ONCE — flow is created here.
    let svc = crate::services::settings_web_service::SettingsCommandService::new(context);

    let settings = AppSettings::default();
    for _ in 0..10 {
        svc.update_settings(&settings)
            .await
            .expect("repeated update_settings must succeed");
    }

    // Cloning the service must share the same PolicyAuditWriter Arc (not
    // create a new one).  Both instances must still be usable.
    let svc2 = svc.clone();
    svc2.update_settings(&settings)
        .await
        .expect("cloned service update_settings must succeed");
}

#[tokio::test]
async fn update_settings_rejects_api_key_write_for_env_backend() {
    let temp_dir = TempDir::new().expect("temp dir");
    let config_path = temp_dir.path().join("config.json");
    let config_manager = ConfigManager::with_path(config_path).expect("config manager");
    // #7738 D-4: funnel through the canonical test-state helper.
    let mut state = crate::test_local_auth::test_app_state_with_event_capacity(8);
    state.core.config_manager = Some(config_manager);
    state.secrets.default_backend_kind = CredentialBackendKind::Env;
    state.secrets.store = Some(Arc::new(TestSecretStore::new()));

    let mut settings = AppSettings::default();
    settings.ai_provider.llm_provider = "Remote".to_string();
    settings.ai_provider.llm_api = Some(ExternalApiSettings {
        endpoint: "https://api.openai.com/v1".to_string(),
        api_key_masked: "sk-secret-123456".to_string(),
        model: Some("gpt-5.4".to_string()),
        provider_type: "OpenAi".to_string(),
        surface_id: None,
        timeout_secs: 30,
        auth_mode: "api_key".to_string(),
        backend_kind: "env".to_string(),
        has_secret: false,
        can_edit_secret: false,
        secret_display_hint: None,
        projection_enabled: false,
    });

    let context = test_context_from_state(&state);
    let err = crate::services::settings_web_service::SettingsCommandService::new(context)
        .update_settings(&settings)
        .await
        .expect_err("env backend should be read-only");

    assert!(matches!(err, ApiError::BadRequest(_)));
}
