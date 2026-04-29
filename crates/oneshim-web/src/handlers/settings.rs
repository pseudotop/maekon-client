use crate::{
    error::ApiError,
    services::{
        settings_query_service::SettingsQueryService, settings_web_service::SettingsCommandService,
        web_contexts::SettingsWebContext,
    },
};
use axum::{extract::State, Json};
use oneshim_api_contracts::settings::{AppSettings, StorageStats};

pub async fn get_storage_stats(
    State(context): State<SettingsWebContext>,
) -> Result<Json<StorageStats>, ApiError> {
    Ok(Json(
        SettingsQueryService::new(context).get_storage_stats()?,
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
    SettingsCommandService::new(context)
        .update_settings(&settings)
        .await?;
    Ok(Json(settings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{settings_assembler, settings_service};
    use oneshim_api_contracts::settings::ExternalApiSettings;
    use oneshim_core::config::AppConfig;

    #[test]
    fn default_settings_valid() {
        let settings = AppSettings::default();
        assert_eq!(settings.retention_days, 30);
        assert_eq!(settings.max_storage_mb, 500);
        assert_eq!(settings.web_port, oneshim_core::config::DEFAULT_WEB_PORT);
        assert!(!settings.allow_external);
        assert!(settings.capture_enabled);
    }

    #[test]
    fn default_settings_includes_automation() {
        let settings = AppSettings::default();
        assert!(!settings.automation.enabled);
        assert!(!settings.sandbox.enabled);
        assert_eq!(settings.sandbox.profile, "Standard");
        assert_eq!(settings.ai_provider.access_mode, "ProviderApiKey");
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
        let result = app_config.ai_provider.validate_selected_remote_endpoints();
        assert!(result.is_err());
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
}
