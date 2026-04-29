use oneshim_api_contracts::settings::{
    AiProviderProfileConfig as ApiAiProviderProfileConfig, AiProviderSettings, AiSessionSettings,
    AnalysisSettings, AppSettings, AutomationSettings, CoachingSettings, ExternalApiSettings,
    IndicatorSettings, IntegrationSettings, MonitorControlSettings, NetworkSettings,
    NotificationSettings, OcrValidationSettings, PrivacySettings, SandboxSettings,
    SavedAiProviderProfile as ApiSavedAiProviderProfile, SceneActionOverrideSettings,
    SceneIntelligenceSettings, ScheduleSettings, SuggestionSettings, SyncSettings,
    TelemetrySettings, UpdateSettings,
};
use oneshim_core::config::{
    AiAccessMode, AiProviderProfileConfig, AppConfig, CredentialAuthMode, CredentialBackendKind,
    ExternalApiEndpoint, SavedAiProviderProfile,
};

use crate::services::settings_endpoint::{
    can_edit_secret, credential_auth_mode_to_wire, credential_backend_kind_to_wire,
    default_surface_id_for_endpoint, derive_credential_auth_mode, derive_credential_backend_kind,
    normalize_ai_access_mode_for_settings, surface_uses_no_auth, ApiEndpointKind,
};

pub(crate) fn config_to_settings(
    config: &AppConfig,
    default_secret_backend_kind: CredentialBackendKind,
) -> AppSettings {
    AppSettings {
        retention_days: config.storage.retention_days,
        max_storage_mb: config.storage.max_storage_mb as u32,
        web_port: config.web.port,
        allow_external: config.web.allow_external,
        capture_enabled: config.vision.capture_enabled,
        idle_threshold_secs: config.monitor.idle_threshold_secs as u32,
        metrics_interval_secs: (config.monitor.poll_interval_ms / 1000) as u32,
        process_interval_secs: config.monitor.process_interval_secs as u32,
        notification: NotificationSettings {
            enabled: config.notification.enabled,
            idle_notification: config.notification.idle_notification,
            idle_notification_mins: config.notification.idle_notification_mins,
            long_session_notification: config.notification.long_session_notification,
            long_session_mins: config.notification.long_session_mins,
            high_usage_notification: config.notification.high_usage_notification,
            high_usage_threshold: config.notification.high_usage_threshold,
        },
        update: UpdateSettings {
            enabled: config.update.enabled,
            check_interval_hours: config.update.check_interval_hours,
            channel: format!("{:?}", config.update.effective_channel()).to_lowercase(),
            include_prerelease: config.update.include_prerelease,
            auto_install: config.update.auto_install,
        },
        telemetry: TelemetrySettings {
            enabled: config.telemetry.enabled,
            crash_reports: config.telemetry.crash_reports,
            usage_analytics: config.telemetry.usage_analytics,
            performance_metrics: config.telemetry.performance_metrics,
        },
        monitor: MonitorControlSettings {
            process_monitoring: config.monitor.process_monitoring,
            input_activity: config.monitor.input_activity,
            privacy_mode: config.vision.privacy_mode,
        },
        privacy: PrivacySettings {
            excluded_apps: config.privacy.excluded_apps.clone(),
            excluded_app_patterns: config.privacy.excluded_app_patterns.clone(),
            excluded_title_patterns: config.privacy.excluded_title_patterns.clone(),
            auto_exclude_sensitive: config.privacy.auto_exclude_sensitive,
            pii_filter_level: format!("{:?}", config.privacy.pii_filter_level),
        },
        schedule: ScheduleSettings {
            active_hours_enabled: config.schedule.active_hours_enabled,
            active_start_hour: config.schedule.active_start_hour,
            active_end_hour: config.schedule.active_end_hour,
            active_days: config
                .schedule
                .active_days
                .iter()
                .map(|d| format!("{:?}", d))
                .collect(),
            pause_on_screen_lock: config.schedule.pause_on_screen_lock,
            pause_on_battery_saver: config.schedule.pause_on_battery_saver,
        },
        automation: AutomationSettings {
            enabled: config.automation.enabled,
        },
        sandbox: SandboxSettings {
            enabled: config.automation.sandbox.enabled,
            profile: format!("{:?}", config.automation.sandbox.profile),
            allowed_read_paths: config.automation.sandbox.allowed_read_paths.clone(),
            allowed_write_paths: config.automation.sandbox.allowed_write_paths.clone(),
            allow_network: config.automation.sandbox.allow_network,
            max_memory_bytes: config.automation.sandbox.max_memory_bytes,
            max_cpu_time_ms: config.automation.sandbox.max_cpu_time_ms,
        },
        ai_provider: ai_provider_settings_from_config(
            &config.ai_provider,
            default_secret_backend_kind,
        ),
        ai_session: AiSessionSettings {
            max_concurrent_sessions: config.ai_session.max_concurrent_sessions,
            idle_timeout_secs: config.ai_session.idle_timeout_secs,
            session_timeout_secs: config.ai_session.session_timeout_secs,
            max_retries: config.ai_session.max_retries,
            max_history_turns: config.ai_session.max_history_turns,
            health_check_interval_secs: config.ai_session.health_check_interval_secs,
            max_output_tokens: config.ai_session.max_output_tokens,
            thinking: config.ai_session.thinking.clone(),
        },
        suggestion: SuggestionSettings {
            enabled: config.suggestions.enabled,
        },
        indicator: IndicatorSettings {
            show_border: config.indicator.show_border,
            show_panel: config.indicator.show_panel,
            border_opacity: config.indicator.border_opacity,
        },
        analysis: AnalysisSettings {
            enabled: config.analysis.enabled,
            interval_secs: config.analysis.interval_secs,
            min_confidence: config.analysis.min_confidence,
            max_suggestions: config.analysis.max_suggestions as u32,
            embedding_enabled: config.analysis.embedding.enabled,
            gui_intelligence_enabled: config.analysis.gui_intelligence.enabled,
            text_intelligence_enabled: config.analysis.text_intelligence.enabled,
            auto_tuner_enabled: config.analysis.tiered_memory.auto_tuning.enabled,
        },
        network: NetworkSettings {
            server_base_url: config.server.base_url.clone(),
            request_timeout_ms: config.server.request_timeout_ms,
            grpc_enabled: config.grpc.use_grpc_auth || config.grpc.use_grpc_context,
            grpc_endpoint: config.grpc.grpc_endpoint.clone(),
            tls_enabled: config.tls.enabled,
        },
        coaching: CoachingSettings {
            enabled: config.coaching.enabled,
            tone: format!("{:?}", config.coaching.tone),
            locale: config.coaching.locale.clone(),
            overlay_mode: format!("{:?}", config.coaching.overlay_mode),
        },
        integration: IntegrationSettings {
            enabled: config.integration.enabled,
            auth_profile_kind: format!("{:?}", config.integration.auth_profile_kind),
            request_timeout_secs: config.integration.request_timeout_secs,
            sync_interval_secs: config.integration.sync_interval_secs,
        },
        sync: SyncSettings {
            enabled: config.sync.enabled,
            transport: format!("{:?}", config.sync.transport),
            interval_secs: config.sync.interval_secs,
            device_name: config.sync.device_name.clone(),
            lan_advertise: config.sync.lan_advertise,
            compression_enabled: config.sync.compression_enabled,
        },
    }
}

pub(crate) fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "***".to_string();
    }
    let prefix: String = chars[..2].iter().collect();
    let suffix: String = chars[chars.len() - 4..].iter().collect();
    format!("{prefix}...{suffix}")
}

pub(crate) fn is_masked_key(value: &str) -> bool {
    value.contains("...") && value.len() <= 12
}

fn endpoint_to_api_settings(
    endpoint: &ExternalApiEndpoint,
    access_mode: AiAccessMode,
    endpoint_kind: ApiEndpointKind,
    default_backend_kind: CredentialBackendKind,
) -> ExternalApiSettings {
    let access_mode = normalize_ai_access_mode_for_settings(access_mode);
    let no_auth_surface = endpoint
        .surface_id
        .as_deref()
        .is_some_and(surface_uses_no_auth);
    let binding = endpoint.credential.as_ref();
    let auth_mode = endpoint
        .credential
        .as_ref()
        .map(|binding| binding.auth_mode)
        .unwrap_or_else(|| {
            derive_credential_auth_mode(endpoint.surface_id.as_deref(), access_mode, endpoint_kind)
        });
    let has_plaintext_secret = !endpoint.api_key.trim().is_empty();
    let backend_kind = endpoint
        .credential
        .as_ref()
        .map(|binding| binding.backend_kind)
        .unwrap_or_else(|| derive_credential_backend_kind(auth_mode, default_backend_kind));
    let has_secret = !no_auth_surface
        && (has_plaintext_secret
            || binding
                .and_then(|value| value.secret_ref.as_ref())
                .is_some()
            || binding.is_some_and(|value| {
                value.auth_mode == CredentialAuthMode::ApiKey
                    && value.backend_kind == CredentialBackendKind::Env
            }));
    let masked_plaintext_secret = has_plaintext_secret.then(|| mask_api_key(&endpoint.api_key));

    ExternalApiSettings {
        endpoint: endpoint.endpoint.clone(),
        api_key_masked: masked_plaintext_secret.clone().unwrap_or_default(),
        model: endpoint.model.clone(),
        provider_type: format!("{:?}", endpoint.provider_type),
        surface_id: endpoint.surface_id.clone().or_else(|| {
            default_surface_id_for_endpoint(endpoint.provider_type, access_mode, endpoint_kind)
                .map(str::to_string)
        }),
        timeout_secs: endpoint.timeout_secs,
        auth_mode: credential_auth_mode_to_wire(auth_mode).to_string(),
        backend_kind: credential_backend_kind_to_wire(if no_auth_surface {
            CredentialBackendKind::Unavailable
        } else {
            backend_kind
        })
        .to_string(),
        has_secret,
        can_edit_secret: !no_auth_surface && can_edit_secret(auth_mode, backend_kind),
        secret_display_hint: masked_plaintext_secret,
        projection_enabled: binding
            .map(|value| value.projection_enabled)
            .unwrap_or(false),
    }
}

fn ai_provider_settings_from_config(
    config: &oneshim_core::config::AiProviderConfig,
    default_secret_backend_kind: CredentialBackendKind,
) -> AiProviderSettings {
    let active = ai_provider_profile_settings_from_config(
        &AiProviderProfileConfig {
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
        },
        default_secret_backend_kind,
    );

    AiProviderSettings {
        access_mode: active.access_mode,
        ocr_provider: active.ocr_provider,
        llm_provider: active.llm_provider,
        external_data_policy: active.external_data_policy,
        allow_unredacted_external_ocr: active.allow_unredacted_external_ocr,
        ocr_validation: active.ocr_validation,
        scene_action_override: active.scene_action_override,
        scene_intelligence: active.scene_intelligence,
        fallback_to_local: active.fallback_to_local,
        ocr_api: active.ocr_api,
        llm_api: active.llm_api,
        active_profile_id: config.active_profile_id.clone(),
        saved_profiles: config
            .saved_profiles
            .iter()
            .map(|profile| saved_profile_settings_from_config(profile, default_secret_backend_kind))
            .collect(),
    }
}

fn saved_profile_settings_from_config(
    profile: &SavedAiProviderProfile,
    default_secret_backend_kind: CredentialBackendKind,
) -> ApiSavedAiProviderProfile {
    ApiSavedAiProviderProfile {
        profile_id: profile.profile_id.clone(),
        name: profile.name.clone(),
        ai_provider: ai_provider_profile_settings_from_config(
            &profile.ai_provider,
            default_secret_backend_kind,
        ),
        updated_at: profile.updated_at.clone(),
    }
}

fn ai_provider_profile_settings_from_config(
    config: &AiProviderProfileConfig,
    default_secret_backend_kind: CredentialBackendKind,
) -> ApiAiProviderProfileConfig {
    ApiAiProviderProfileConfig {
        access_mode: format!(
            "{:?}",
            normalize_ai_access_mode_for_settings(config.access_mode)
        ),
        ocr_provider: format!("{:?}", config.ocr_provider),
        llm_provider: format!("{:?}", config.llm_provider),
        external_data_policy: format!("{:?}", config.external_data_policy),
        allow_unredacted_external_ocr: config.allow_unredacted_external_ocr,
        ocr_validation: OcrValidationSettings {
            enabled: config.ocr_validation.enabled,
            min_confidence: config.ocr_validation.min_confidence,
            max_invalid_ratio: config.ocr_validation.max_invalid_ratio,
        },
        scene_action_override: SceneActionOverrideSettings {
            enabled: config.scene_action_override.enabled,
            reason: config
                .scene_action_override
                .reason
                .clone()
                .unwrap_or_default(),
            approved_by: config
                .scene_action_override
                .approved_by
                .clone()
                .unwrap_or_default(),
            expires_at: config
                .scene_action_override
                .expires_at
                .map(|v| v.to_rfc3339()),
        },
        scene_intelligence: SceneIntelligenceSettings {
            enabled: config.scene_intelligence.enabled,
            overlay_enabled: config.scene_intelligence.overlay_enabled,
            allow_action_execution: config.scene_intelligence.allow_action_execution,
            min_confidence: config.scene_intelligence.min_confidence,
            max_elements: config.scene_intelligence.max_elements as u32,
            calibration_enabled: config.scene_intelligence.calibration_enabled,
            calibration_min_elements: config.scene_intelligence.calibration_min_elements as u32,
            calibration_min_avg_confidence: config
                .scene_intelligence
                .calibration_min_avg_confidence,
        },
        fallback_to_local: config.fallback_to_local,
        ocr_api: config.ocr_api.as_ref().map(|endpoint| {
            endpoint_to_api_settings(
                endpoint,
                config.access_mode,
                ApiEndpointKind::Ocr,
                default_secret_backend_kind,
            )
        }),
        llm_api: config.llm_api.as_ref().map(|endpoint| {
            endpoint_to_api_settings(
                endpoint,
                config.access_mode,
                ApiEndpointKind::Llm,
                default_secret_backend_kind,
            )
        }),
    }
}
