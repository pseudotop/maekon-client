use crate::{
    error::ApiError,
    services::{
        settings_query_service::SettingsQueryService, settings_web_service::SettingsCommandService,
        web_contexts::SettingsWebContext,
    },
};
use axum::{extract::State, Json};
use maekon_api_contracts::settings::{AppSettings, StorageStats};

pub async fn get_storage_stats(
    State(context): State<SettingsWebContext>,
) -> Result<Json<StorageStats>, ApiError> {
    Ok(Json(
        SettingsQueryService::new(context)
            .get_storage_stats()
            .await?,
    ))
}

pub async fn get_settings(
    State(context): State<SettingsWebContext>,
) -> Result<Json<AppSettings>, ApiError> {
    Ok(Json(SettingsQueryService::new(context).get_settings()))
}

pub async fn update_settings(
    State(context): State<SettingsWebContext>,
    Json(settings): Json<AppSettings>,
) -> Result<Json<AppSettings>, ApiError> {
    SettingsCommandService::new(context.clone())
        .update_settings(&settings)
        .await?;
    Ok(Json(SettingsQueryService::new(context).get_settings()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{settings_assembler, settings_service};
    use maekon_api_contracts::settings::ExternalApiSettings;
    use maekon_core::config::AppConfig;
    use maekon_core::config::UpdateChannel;
    use maekon_core::config_manager::ConfigManager;
    use tempfile::TempDir;

    #[test]
    fn default_settings_valid() {
        let settings = AppSettings::default();
        assert_eq!(settings.retention_days, 30);
        assert_eq!(settings.max_storage_mb, 500);
        assert_eq!(settings.web_port, maekon_core::config::DEFAULT_WEB_PORT);
        assert!(!settings.allow_external);
        assert!(settings.capture_enabled);
    }

    #[test]
    fn default_settings_includes_automation() {
        let settings = AppSettings::default();
        assert!(!settings.automation.enabled);
        assert!(!settings.sandbox.enabled);
        assert_eq!(settings.sandbox.profile, "Standard");
        assert_eq!(settings.ai_provider.access_mode, "provider_api_key");
        assert_eq!(settings.ai_provider.ocr_provider, "Local");
        assert_eq!(settings.ai_provider.llm_provider, "Local");
        assert!(settings.ai_provider.fallback_to_local);
    }

    #[test]
    fn settings_serde_roundtrip() {
        let settings = AppSettings::default();
        let json = serde_json::to_string(&settings).unwrap();
        let deser: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.automation.enabled, settings.automation.enabled);
        assert_eq!(deser.sandbox.profile, settings.sandbox.profile);
        assert_eq!(
            deser.ai_provider.ocr_provider,
            settings.ai_provider.ocr_provider
        );
    }

    #[test]
    fn mask_api_key_works() {
        assert_eq!(
            settings_assembler::mask_api_key("sk-1234567890abcdef"),
            "sk...cdef"
        );
        assert_eq!(settings_assembler::mask_api_key("short"), "***");
        assert_eq!(settings_assembler::mask_api_key("12345678"), "***");
        assert_eq!(settings_assembler::mask_api_key("123456789"), "12...6789");
    }

    #[test]
    fn is_masked_key_detection() {
        assert!(settings_service::is_masked_key("sk...cdef"));
        assert!(settings_service::is_masked_key("ab...1234"));
        assert!(!settings_service::is_masked_key("sk-1234567890abcdef"));
        assert!(!settings_service::is_masked_key(""));
    }

    #[test]
    fn storage_stats_serializes() {
        let stats = StorageStats {
            db_size_bytes: 1024 * 1024,
            frames_size_bytes: 5 * 1024 * 1024,
            total_size_bytes: 6 * 1024 * 1024,
            frame_count: 100,
            event_count: 500,
            metric_count: 1000,
            oldest_data_date: Some("2024-01-01T00:00:00Z".to_string()),
            newest_data_date: Some("2024-01-30T23:59:59Z".to_string()),
        };

        let json = serde_json::to_string(&stats).unwrap();
        assert!(json.contains("db_size_bytes"));
        assert!(json.contains("frame_count"));
    }

    /// #9146: save-response parity — the value a save persists into the config
    /// must come back byte-identical from the settings assembler. The old
    /// Display-based assembly returned lowercase "strict", which matched no
    /// `<select>` option in the UI and rendered as "Off" after a successful
    /// save while `config.json` correctly held Strict.
    #[test]
    fn pii_level_save_response_round_trips_canonical_token() {
        let mut app_config = AppConfig::default_config();
        let mut settings = AppSettings::default();
        settings.privacy.pii_filter_level = "Strict".to_string();

        settings_service::apply_settings_to_config(&mut app_config, &settings).unwrap();
        assert_eq!(
            app_config.privacy.pii_filter_level,
            maekon_core::config::PiiFilterLevel::Strict
        );

        let assembled = settings_assembler::config_to_settings(
            &app_config,
            maekon_core::config::CredentialBackendKind::Unavailable,
        );
        assert_eq!(assembled.privacy.pii_filter_level, "Strict");

        // The parser accepts the assembled token back (full UI round trip).
        let mut second_config = AppConfig::default_config();
        settings_service::apply_settings_to_config(&mut second_config, &assembled).unwrap();
        assert_eq!(
            second_config.privacy.pii_filter_level,
            maekon_core::config::PiiFilterLevel::Strict
        );
    }

    #[test]
    fn apply_settings_to_config_validates_remote_ai_requirements() {
        let mut app_config = AppConfig::default_config();
        let mut settings = AppSettings::default();

        settings.ai_provider.ocr_provider = "Remote".to_string();
        settings.ai_provider.ocr_api = Some(ExternalApiSettings {
            endpoint: "https://api.example.com/ocr".to_string(),
            api_key_masked: "".to_string(),
            model: None,
            provider_type: "Generic".to_string(),
            surface_id: None,
            timeout_secs: 30,
            auth_mode: "api_key".to_string(),
            backend_kind: "unavailable".to_string(),
            has_secret: false,
            can_edit_secret: true,
            secret_display_hint: None,
            projection_enabled: false,
        });

        settings_service::apply_settings_to_config(&mut app_config, &settings).unwrap();
        let err = app_config
            .ai_provider
            .validate_selected_remote_endpoints()
            .unwrap_err();
        assert!(
            matches!(err, maekon_core::error::CoreError::Config { .. }),
            "missing API key must return CoreError::Config, got: {err:?}"
        );
    }

    #[test]
    fn apply_settings_to_config_wires_llm_summary_enabled() {
        // #8059 G2a: the new flat `analysis.llm_summary_enabled` contract field
        // must map onto the nested `analysis.embedding.llm_summary_enabled`
        // config (write path). This mirrors the existing `embedding_enabled`
        // mapping so the AdvancedTab "Enable AI features" master toggle can
        // actually turn the AI daily-digest narrative on end-to-end.
        let mut app_config = AppConfig::default_config();
        let mut settings = AppSettings::default();

        // Flip away from the config defaults to prove the write is real.
        settings.analysis.embedding_enabled = true;
        settings.analysis.llm_summary_enabled = true;
        settings_service::apply_settings_to_config(&mut app_config, &settings).unwrap();
        assert!(app_config.analysis.embedding.llm_summary_enabled);

        settings.analysis.llm_summary_enabled = false;
        settings_service::apply_settings_to_config(&mut app_config, &settings).unwrap();
        assert!(!app_config.analysis.embedding.llm_summary_enabled);
    }

    #[test]
    fn apply_settings_to_config_rejects_unknown_sandbox_profile() {
        let mut app_config = AppConfig::default_config();
        let mut settings = AppSettings::default();
        settings.sandbox.profile = "Unknown".to_string();

        let result = settings_service::apply_settings_to_config(&mut app_config, &settings);
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn apply_settings_to_config_rejects_unknown_weekday() {
        let mut app_config = AppConfig::default_config();
        let mut settings = AppSettings::default();
        settings.schedule.active_days = vec!["Mon".to_string(), "Funday".to_string()];

        let result = settings_service::apply_settings_to_config(&mut app_config, &settings);
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[tokio::test]
    async fn update_settings_returns_canonical_reloaded_settings() {
        let temp_dir = TempDir::new().expect("temp dir");
        let config_path = temp_dir.path().join("config.json");
        let config_manager = ConfigManager::with_path(config_path).expect("config manager");
        // #7738 D-4: funnel through the canonical test-state helper.
        let mut state = crate::test_local_auth::test_app_state_with_event_capacity(8);
        state.core.config_manager = Some(config_manager.clone());
        let context = SettingsWebContext::from_state(&state);

        let mut settings = AppSettings::default();
        settings.update.channel = "prerelease".to_string();

        let Json(response) = update_settings(State(context), Json(settings))
            .await
            .expect("settings update should succeed");

        assert_eq!(
            config_manager.get().update.channel,
            UpdateChannel::PreRelease
        );
        assert_eq!(response.update.channel, "pre_release");
    }

    #[tokio::test]
    async fn update_settings_returns_canonical_coaching_profiles_and_quiet_hours() {
        let temp_dir = TempDir::new().expect("temp dir");
        let config_path = temp_dir.path().join("config.json");
        let config_manager = ConfigManager::with_path(config_path).expect("config manager");
        // #7738 D-4: funnel through the canonical test-state helper.
        let mut state = crate::test_local_auth::test_app_state_with_event_capacity(8);
        state.core.config_manager = Some(config_manager);
        let context = SettingsWebContext::from_state(&state);

        let mut settings = AppSettings::default();
        settings.coaching.quiet_hours =
            vec![maekon_api_contracts::settings::CoachingTimeRangeSettings {
                start: "22:00".to_string(),
                end: "07:30".to_string(),
            }];
        settings
            .coaching
            .profiles
            .get_mut("FocusGuard")
            .expect("default FocusGuard profile")
            .min_interval_secs = 120;

        let Json(response) = update_settings(State(context), Json(settings))
            .await
            .expect("settings update should succeed");

        assert_eq!(response.coaching.quiet_hours.len(), 1);
        assert_eq!(response.coaching.quiet_hours[0].start, "22:00");
        assert_eq!(response.coaching.quiet_hours[0].end, "07:30");
        assert_eq!(
            response
                .coaching
                .profiles
                .get("FocusGuard")
                .expect("FocusGuard profile")
                .min_interval_secs,
            120
        );
    }
}
