use oneshim_api_contracts::settings::{
    AiProviderProfileConfig as ApiAiProviderProfileConfig, AppSettings,
    SavedAiProviderProfile as ApiSavedAiProviderProfile,
};
use oneshim_core::config::{
    AiProviderConfig, AiProviderProfileConfig, AppConfig, OcrValidationConfig,
    SavedAiProviderProfile, SceneActionOverrideConfig, SceneIntelligenceConfig,
};

use crate::error::ApiError;
use crate::services::settings_endpoint::{api_settings_to_endpoint, ApiEndpointKind};
use crate::services::settings_validation::{
    parse_ai_access_mode, parse_external_data_policy, parse_llm_provider, parse_ocr_provider,
    parse_optional_rfc3339_utc, parse_pii_filter_level, parse_sandbox_profile, parse_weekday,
    trim_to_option,
};

pub(crate) fn apply_settings_fields_to_config(
    config: &mut AppConfig,
    settings: &AppSettings,
) -> Result<(), ApiError> {
    apply_general_settings(config, settings)?;
    apply_ai_provider_settings(config, settings)?;
    apply_extended_settings(config, settings);
    Ok(())
}

fn apply_general_settings(config: &mut AppConfig, settings: &AppSettings) -> Result<(), ApiError> {
    config.storage.retention_days = settings.retention_days;
    config.storage.max_storage_mb = settings.max_storage_mb as u64;
    config.web.port = settings.web_port;
    config.web.allow_external = settings.allow_external;
    config.vision.capture_enabled = settings.capture_enabled;
    config.monitor.poll_interval_ms = (settings.metrics_interval_secs as u64) * 1000;
    config.monitor.idle_threshold_secs = settings.idle_threshold_secs as u64;
    config.monitor.process_interval_secs = settings.process_interval_secs as u64;
    config.notification.enabled = settings.notification.enabled;
    config.notification.idle_notification = settings.notification.idle_notification;
    config.notification.idle_notification_mins = settings.notification.idle_notification_mins;
    config.notification.long_session_notification = settings.notification.long_session_notification;
    config.notification.long_session_mins = settings.notification.long_session_mins;
    config.notification.high_usage_notification = settings.notification.high_usage_notification;
    config.notification.high_usage_threshold = settings.notification.high_usage_threshold;
    config.update.enabled = settings.update.enabled;
    config.update.check_interval_hours = settings.update.check_interval_hours;
    config.update.channel = match settings.update.channel.as_str() {
        "pre_release" | "prerelease" => oneshim_core::config::UpdateChannel::PreRelease,
        "nightly" => oneshim_core::config::UpdateChannel::Nightly,
        _ => oneshim_core::config::UpdateChannel::Stable,
    };
    config.update.include_prerelease = config.update.channel.includes_prerelease();
    config.update.auto_install = settings.update.auto_install;
    config.telemetry.enabled = settings.telemetry.enabled;
    config.telemetry.crash_reports = settings.telemetry.crash_reports;
    config.telemetry.usage_analytics = settings.telemetry.usage_analytics;
    config.telemetry.performance_metrics = settings.telemetry.performance_metrics;
    config.monitor.process_monitoring = settings.monitor.process_monitoring;
    config.monitor.input_activity = settings.monitor.input_activity;
    config.vision.privacy_mode = settings.monitor.privacy_mode;
    config.privacy.excluded_apps = settings.privacy.excluded_apps.clone();
    config.privacy.excluded_app_patterns = settings.privacy.excluded_app_patterns.clone();
    config.privacy.excluded_title_patterns = settings.privacy.excluded_title_patterns.clone();
    config.privacy.auto_exclude_sensitive = settings.privacy.auto_exclude_sensitive;
    config.privacy.pii_filter_level = parse_pii_filter_level(&settings.privacy.pii_filter_level)?;
    config.schedule.active_hours_enabled = settings.schedule.active_hours_enabled;
    config.schedule.active_start_hour = settings.schedule.active_start_hour;
    config.schedule.active_end_hour = settings.schedule.active_end_hour;
    config.schedule.active_days = settings
        .schedule
        .active_days
        .iter()
        .map(|day| parse_weekday(day))
        .collect::<Result<Vec<_>, _>>()?;
    config.schedule.pause_on_screen_lock = settings.schedule.pause_on_screen_lock;
    config.schedule.pause_on_battery_saver = settings.schedule.pause_on_battery_saver;
    config.automation.enabled = settings.automation.enabled;
    config.automation.sandbox.enabled = settings.sandbox.enabled;
    config.automation.sandbox.profile = parse_sandbox_profile(&settings.sandbox.profile)?;
    config.automation.sandbox.allowed_read_paths = settings.sandbox.allowed_read_paths.clone();
    config.automation.sandbox.allowed_write_paths = settings.sandbox.allowed_write_paths.clone();
    config.automation.sandbox.allow_network = settings.sandbox.allow_network;
    config.automation.sandbox.max_memory_bytes = settings.sandbox.max_memory_bytes;
    config.automation.sandbox.max_cpu_time_ms = settings.sandbox.max_cpu_time_ms;
    Ok(())
}

fn apply_ai_provider_settings(
    config: &mut AppConfig,
    settings: &AppSettings,
) -> Result<(), ApiError> {
    let existing_saved_profiles = config.ai_provider.saved_profiles.clone();
    config.ai_provider.access_mode = parse_ai_access_mode(&settings.ai_provider.access_mode)?;
    config.ai_provider.ocr_provider = parse_ocr_provider(&settings.ai_provider.ocr_provider)?;
    config.ai_provider.llm_provider = parse_llm_provider(&settings.ai_provider.llm_provider)?;
    config.ai_provider.external_data_policy =
        parse_external_data_policy(&settings.ai_provider.external_data_policy)?;
    config.ai_provider.allow_unredacted_external_ocr =
        settings.ai_provider.allow_unredacted_external_ocr;
    config.ai_provider.ocr_validation = OcrValidationConfig {
        enabled: settings.ai_provider.ocr_validation.enabled,
        min_confidence: settings.ai_provider.ocr_validation.min_confidence,
        max_invalid_ratio: settings.ai_provider.ocr_validation.max_invalid_ratio,
    };
    config.ai_provider.scene_action_override = SceneActionOverrideConfig {
        enabled: settings.ai_provider.scene_action_override.enabled,
        reason: trim_to_option(&settings.ai_provider.scene_action_override.reason),
        approved_by: trim_to_option(&settings.ai_provider.scene_action_override.approved_by),
        expires_at: parse_optional_rfc3339_utc(
            settings
                .ai_provider
                .scene_action_override
                .expires_at
                .as_deref(),
            "ai_provider.scene_action_override.expires_at",
        )?,
    };
    config.ai_provider.scene_intelligence = SceneIntelligenceConfig {
        enabled: settings.ai_provider.scene_intelligence.enabled,
        overlay_enabled: settings.ai_provider.scene_intelligence.overlay_enabled,
        allow_action_execution: settings
            .ai_provider
            .scene_intelligence
            .allow_action_execution,
        min_confidence: settings.ai_provider.scene_intelligence.min_confidence,
        max_elements: settings.ai_provider.scene_intelligence.max_elements as usize,
        calibration_enabled: settings.ai_provider.scene_intelligence.calibration_enabled,
        calibration_min_elements: settings
            .ai_provider
            .scene_intelligence
            .calibration_min_elements as usize,
        calibration_min_avg_confidence: settings
            .ai_provider
            .scene_intelligence
            .calibration_min_avg_confidence,
    };
    config.ai_provider.fallback_to_local = settings.ai_provider.fallback_to_local;

    if let Some(ref ocr_settings) = settings.ai_provider.ocr_api {
        let existing_endpoint = config.ai_provider.ocr_api.as_ref();
        config.ai_provider.ocr_api = Some(api_settings_to_endpoint(
            ocr_settings,
            existing_endpoint,
            config.ai_provider.access_mode,
            ApiEndpointKind::Ocr,
        )?);
    } else {
        config.ai_provider.ocr_api = None;
    }

    if let Some(ref llm_settings) = settings.ai_provider.llm_api {
        let existing_endpoint = config.ai_provider.llm_api.as_ref();
        config.ai_provider.llm_api = Some(api_settings_to_endpoint(
            llm_settings,
            existing_endpoint,
            config.ai_provider.access_mode,
            ApiEndpointKind::Llm,
        )?);
    } else {
        config.ai_provider.llm_api = None;
    }

    config.ai_provider.active_profile_id = trim_to_option(
        settings
            .ai_provider
            .active_profile_id
            .as_deref()
            .unwrap_or_default(),
    );
    config.ai_provider.saved_profiles = settings
        .ai_provider
        .saved_profiles
        .iter()
        .map(|profile| saved_ai_provider_profile_to_config(profile, &existing_saved_profiles))
        .collect::<Result<Vec<_>, _>>()?;
    sync_selected_saved_ai_provider_profile(&mut config.ai_provider);

    Ok(())
}

fn saved_ai_provider_profile_to_config(
    profile: &ApiSavedAiProviderProfile,
    existing_saved_profiles: &[SavedAiProviderProfile],
) -> Result<SavedAiProviderProfile, ApiError> {
    let existing_profile = existing_saved_profiles
        .iter()
        .find(|existing| existing.profile_id == profile.profile_id);

    Ok(SavedAiProviderProfile {
        profile_id: profile.profile_id.trim().to_string(),
        name: profile.name.trim().to_string(),
        ai_provider: ai_provider_profile_config_from_settings(
            &profile.ai_provider,
            existing_profile.map(|value| &value.ai_provider),
        )?,
        updated_at: trim_to_option(profile.updated_at.as_deref().unwrap_or_default()),
    })
}

fn ai_provider_profile_config_from_settings(
    settings: &ApiAiProviderProfileConfig,
    existing_profile: Option<&AiProviderProfileConfig>,
) -> Result<AiProviderProfileConfig, ApiError> {
    let access_mode = parse_ai_access_mode(&settings.access_mode)?;
    let ocr_provider = parse_ocr_provider(&settings.ocr_provider)?;
    let llm_provider = parse_llm_provider(&settings.llm_provider)?;

    let ocr_api = if let Some(ocr_settings) = settings.ocr_api.as_ref() {
        Some(api_settings_to_endpoint(
            ocr_settings,
            existing_profile.and_then(|profile| profile.ocr_api.as_ref()),
            access_mode,
            ApiEndpointKind::Ocr,
        )?)
    } else {
        None
    };

    let llm_api = if let Some(llm_settings) = settings.llm_api.as_ref() {
        Some(api_settings_to_endpoint(
            llm_settings,
            existing_profile.and_then(|profile| profile.llm_api.as_ref()),
            access_mode,
            ApiEndpointKind::Llm,
        )?)
    } else {
        None
    };

    Ok(AiProviderProfileConfig {
        access_mode,
        ocr_provider,
        llm_provider,
        ocr_api,
        llm_api,
        external_data_policy: parse_external_data_policy(&settings.external_data_policy)?,
        allow_unredacted_external_ocr: settings.allow_unredacted_external_ocr,
        ocr_validation: OcrValidationConfig {
            enabled: settings.ocr_validation.enabled,
            min_confidence: settings.ocr_validation.min_confidence,
            max_invalid_ratio: settings.ocr_validation.max_invalid_ratio,
        },
        scene_action_override: SceneActionOverrideConfig {
            enabled: settings.scene_action_override.enabled,
            reason: trim_to_option(&settings.scene_action_override.reason),
            approved_by: trim_to_option(&settings.scene_action_override.approved_by),
            expires_at: parse_optional_rfc3339_utc(
                settings.scene_action_override.expires_at.as_deref(),
                "ai_provider.saved_profiles[].ai_provider.scene_action_override.expires_at",
            )?,
        },
        scene_intelligence: SceneIntelligenceConfig {
            enabled: settings.scene_intelligence.enabled,
            overlay_enabled: settings.scene_intelligence.overlay_enabled,
            allow_action_execution: settings.scene_intelligence.allow_action_execution,
            min_confidence: settings.scene_intelligence.min_confidence,
            max_elements: settings.scene_intelligence.max_elements as usize,
            calibration_enabled: settings.scene_intelligence.calibration_enabled,
            calibration_min_elements: settings.scene_intelligence.calibration_min_elements as usize,
            calibration_min_avg_confidence: settings
                .scene_intelligence
                .calibration_min_avg_confidence,
        },
        fallback_to_local: settings.fallback_to_local,
    })
}

fn sync_selected_saved_ai_provider_profile(config: &mut AiProviderConfig) {
    let Some(active_profile_id) = config
        .active_profile_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    let active_profile = AiProviderProfileConfig {
        access_mode: config.access_mode,
        ocr_provider: config.ocr_provider,
        llm_provider: config.llm_provider,
        ocr_api: config.ocr_api.clone(),
        llm_api: config.llm_api.clone(),
        external_data_policy: config.external_data_policy,
        allow_unredacted_external_ocr: config.allow_unredacted_external_ocr,
        ocr_validation: config.ocr_validation.clone(),
        scene_action_override: config.scene_action_override.clone(),
        scene_intelligence: config.scene_intelligence.clone(),
        fallback_to_local: config.fallback_to_local,
    };

    if let Some(saved_profile) = config
        .saved_profiles
        .iter_mut()
        .find(|profile| profile.profile_id == active_profile_id)
    {
        saved_profile.ai_provider = active_profile;
    }
}

fn apply_extended_settings(config: &mut AppConfig, settings: &AppSettings) {
    // AI Session
    config.ai_session.max_concurrent_sessions = settings.ai_session.max_concurrent_sessions;
    config.ai_session.idle_timeout_secs = settings.ai_session.idle_timeout_secs;
    config.ai_session.session_timeout_secs = settings.ai_session.session_timeout_secs;
    config.ai_session.max_retries = settings.ai_session.max_retries;
    config.ai_session.max_history_turns = settings.ai_session.max_history_turns;
    config.ai_session.health_check_interval_secs = settings.ai_session.health_check_interval_secs;
    config.ai_session.max_output_tokens = settings.ai_session.max_output_tokens;
    config.ai_session.thinking = settings.ai_session.thinking.clone();

    // Suggestion
    config.suggestions.enabled = settings.suggestion.enabled;

    // Indicator
    config.indicator.show_border = settings.indicator.show_border;
    config.indicator.show_panel = settings.indicator.show_panel;
    config.indicator.border_opacity = settings.indicator.border_opacity;

    // Analysis
    config.analysis.enabled = settings.analysis.enabled;
    config.analysis.interval_secs = settings.analysis.interval_secs;
    config.analysis.min_confidence = settings.analysis.min_confidence;
    config.analysis.max_suggestions = settings.analysis.max_suggestions as usize;
    config.analysis.embedding.enabled = settings.analysis.embedding_enabled;
    config.analysis.gui_intelligence.enabled = settings.analysis.gui_intelligence_enabled;
    config.analysis.text_intelligence.enabled = settings.analysis.text_intelligence_enabled;
    config.analysis.tiered_memory.auto_tuning.enabled = settings.analysis.auto_tuner_enabled;

    // Network
    config.server.base_url = settings.network.server_base_url.clone();
    config.server.request_timeout_ms = settings.network.request_timeout_ms;
    config.grpc.grpc_endpoint = settings.network.grpc_endpoint.clone();
    config.tls.enabled = settings.network.tls_enabled;
    let grpc_enabled = settings.network.grpc_enabled;
    config.grpc.use_grpc_auth = grpc_enabled;
    config.grpc.use_grpc_context = grpc_enabled;

    // Coaching
    // Note: coaching.tone, coaching.overlay_mode are read-only on the wire —
    // serialized by the assembler for display, but not written back here.
    // They require enum parsing (CoachingTone, OverlayMode) and are managed
    // via the dedicated CoachingGoalsTab, not the Advanced tab.
    config.coaching.enabled = settings.coaching.enabled;
    config.coaching.locale = settings.coaching.locale.clone();

    // Integration
    // Note: integration.auth_profile_kind is read-only — enum requires
    // dedicated parsing (IntegrationAuthProfileKind).
    config.integration.enabled = settings.integration.enabled;
    config.integration.request_timeout_secs = settings.integration.request_timeout_secs;
    config.integration.sync_interval_secs = settings.integration.sync_interval_secs;

    // Sync
    // Note: sync.transport is read-only — enum requires dedicated parsing
    // (SyncTransportKind).
    config.sync.enabled = settings.sync.enabled;
    config.sync.interval_secs = settings.sync.interval_secs;
    config.sync.device_name = settings.sync.device_name.clone();
    config.sync.lan_advertise = settings.sync.lan_advertise;
    config.sync.compression_enabled = settings.sync.compression_enabled;
}
