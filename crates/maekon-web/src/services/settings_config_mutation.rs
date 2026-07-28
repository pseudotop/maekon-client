use maekon_api_contracts::settings::{
    AiProviderProfileConfig as ApiAiProviderProfileConfig, AiSessionSettings, AnalysisSettings,
    AppSettings, AudioSettings, CoachingSettings, FocusAutoSettings, IndicatorSettings,
    IntegrationSettings, MonitorControlSettings, NetworkSettings, NotificationSettings,
    PrivacySettings, SandboxSettings, SavedAiProviderProfile as ApiSavedAiProviderProfile,
    ScheduleSettings, SuggestionSettings, SyncSettings, TelemetrySettings, UpdateSettings,
};
use maekon_core::config::{
    AiProviderConfig, AiProviderProfileConfig, AppConfig, FocusSchedule, OcrValidationConfig,
    ProfileConfig, SavedAiProviderProfile, SceneActionOverrideConfig, SceneIntelligenceConfig,
    TimeRange,
};

use crate::error::ApiError;
use crate::services::settings_assembler::is_masked_key;
use crate::services::settings_endpoint::{api_settings_to_endpoint, ApiEndpointKind};
use crate::services::settings_validation::{
    parse_ai_access_mode, parse_external_data_policy, parse_llm_provider, parse_mic_input_mode,
    parse_ocr_provider, parse_optional_rfc3339_utc, parse_pii_filter_level, parse_sandbox_profile,
    parse_stt_language, parse_stt_provider, parse_weekday, parse_whisper_model_size,
    trim_to_option,
};

pub(crate) fn apply_settings_fields_to_config(
    config: &mut AppConfig,
    settings: &AppSettings,
) -> Result<(), ApiError> {
    // Layer 1 (D-1, #7740 write-guard slice): an exhaustive, all-27-field
    // destructure of `AppSettings`. This delegator itself touches ZERO fields
    // (it just forwards `settings` to the three `apply_*` functions below), so
    // adding a NEW top-level `AppSettings` field makes this pattern
    // non-exhaustive -> hard compile error (E0027) until the field is added
    // here. That is the ENTIRE guarantee this layer provides: a new top-level
    // section was ACKNOWLEDGED (someone had to touch this line). It does NOT
    // prove the field is actually WIRED to a `config.x = ...` assignment — the
    // bindings below are all `_`-prefixed, which `unused = "deny"`
    // (root Cargo.toml `[workspace.lints.rust]`) deliberately exempts.
    //
    // The real teeth are Layer 2: the per-section destructure blocks inside
    // `apply_general_settings`, `apply_audio_settings`, `apply_focus_auto_settings`,
    // and `apply_extended_settings` below, which bind REAL (non-`_`) field
    // names. There, the same `unused = "deny"` lint makes a bound-but-never-
    // assigned leaf a hard compile error, closing the dominant silent-drop case
    // ("new leaf added to an existing section, the write side forgot to
    // consume it").
    //
    // Do NOT duplicate this 27-field list anywhere else in this file.
    let AppSettings {
        retention_days: _retention_days,
        max_storage_mb: _max_storage_mb,
        web_port: _web_port,
        allow_external: _allow_external,
        capture_enabled: _capture_enabled,
        idle_threshold_secs: _idle_threshold_secs,
        metrics_interval_secs: _metrics_interval_secs,
        process_interval_secs: _process_interval_secs,
        notification: _notification,
        update: _update,
        telemetry: _telemetry,
        monitor: _monitor,
        privacy: _privacy,
        schedule: _schedule,
        automation: _automation,
        sandbox: _sandbox,
        ai_provider: _ai_provider,
        ai_session: _ai_session,
        suggestion: _suggestion,
        indicator: _indicator,
        analysis: _analysis,
        network: _network,
        coaching: _coaching,
        integration: _integration,
        sync: _sync,
        audio: _audio,
        focus_auto: _focus_auto,
    } = settings;

    apply_general_settings(config, settings)?;
    apply_ai_provider_settings(config, settings)?;
    apply_extended_settings(config, settings);
    Ok(())
}

fn apply_general_settings(config: &mut AppConfig, settings: &AppSettings) -> Result<(), ApiError> {
    // These 8 fields are top-level AppSettings scalars (not nested structs), so
    // Layer 1 above already forces a touch on this function when a new one is
    // added. No per-field destructure block is needed for a flat scalar.
    config.storage.retention_days = settings.retention_days;
    config.storage.max_storage_mb = settings.max_storage_mb as u64;
    config.web.port = settings.web_port;
    config.web.allow_external = settings.allow_external;
    config.vision.capture_enabled = settings.capture_enabled;
    config.monitor.poll_interval_ms = (settings.metrics_interval_secs as u64) * 1000;
    config.monitor.idle_threshold_secs = settings.idle_threshold_secs as u64;
    config.monitor.process_interval_secs = settings.process_interval_secs as u64;

    // Notification
    {
        let NotificationSettings {
            enabled,
            idle_notification,
            idle_notification_mins,
            long_session_notification,
            long_session_mins,
            high_usage_notification,
            high_usage_threshold,
        } = &settings.notification;
        config.notification.enabled = *enabled;
        config.notification.idle_notification = *idle_notification;
        config.notification.idle_notification_mins = *idle_notification_mins;
        config.notification.long_session_notification = *long_session_notification;
        config.notification.long_session_mins = *long_session_mins;
        config.notification.high_usage_notification = *high_usage_notification;
        config.notification.high_usage_threshold = *high_usage_threshold;
    }

    // Update
    // Note: `include_prerelease` is a legacy field superseded by `channel`
    // (kept only so old settings payloads still deserialize, see
    // `maekon-api-contracts::settings::UpdateSettings`). The config value below
    // is derived FROM the just-written `channel`, never from this field — a
    // conscious, named non-write, not an oversight.
    {
        let UpdateSettings {
            enabled,
            check_interval_hours,
            channel,
            include_prerelease: _legacy_include_prerelease,
            auto_install,
        } = &settings.update;
        config.update.enabled = *enabled;
        config.update.check_interval_hours = *check_interval_hours;
        config.update.channel = match channel.as_str() {
            "pre_release" | "prerelease" => maekon_core::config::UpdateChannel::PreRelease,
            "nightly" => maekon_core::config::UpdateChannel::Nightly,
            _ => maekon_core::config::UpdateChannel::Stable,
        };
        config.update.include_prerelease = config.update.channel.includes_prerelease();
        config.update.auto_install = *auto_install;
    }

    // Telemetry
    {
        let TelemetrySettings {
            enabled,
            crash_reports,
            usage_analytics,
            performance_metrics,
        } = &settings.telemetry;
        config.telemetry.enabled = *enabled;
        config.telemetry.crash_reports = *crash_reports;
        config.telemetry.usage_analytics = *usage_analytics;
        config.telemetry.performance_metrics = *performance_metrics;
    }

    // Monitor
    // Note: `privacy_mode` is a pre-existing cross-section remap — it arrives
    // on the wire under `MonitorControlSettings` but is written into
    // `config.vision.privacy_mode`, not `config.monitor`.
    {
        let MonitorControlSettings {
            process_monitoring,
            input_activity,
            privacy_mode,
        } = &settings.monitor;
        config.monitor.process_monitoring = *process_monitoring;
        config.monitor.input_activity = *input_activity;
        config.vision.privacy_mode = *privacy_mode;
    }

    // Privacy
    {
        let PrivacySettings {
            excluded_apps,
            excluded_app_patterns,
            excluded_title_patterns,
            auto_exclude_sensitive,
            pii_filter_level,
        } = &settings.privacy;
        config.privacy.excluded_apps = excluded_apps.clone();
        config.privacy.excluded_app_patterns = excluded_app_patterns.clone();
        config.privacy.excluded_title_patterns = excluded_title_patterns.clone();
        config.privacy.auto_exclude_sensitive = *auto_exclude_sensitive;
        config.privacy.pii_filter_level = parse_pii_filter_level(pii_filter_level)?;
    }

    // Schedule
    {
        let ScheduleSettings {
            active_hours_enabled,
            active_start_hour,
            active_end_hour,
            active_days,
            pause_on_screen_lock,
            pause_on_battery_saver,
        } = &settings.schedule;
        config.schedule.active_hours_enabled = *active_hours_enabled;
        config.schedule.active_start_hour = *active_start_hour;
        config.schedule.active_end_hour = *active_end_hour;
        config.schedule.active_days = active_days
            .iter()
            .map(|day| parse_weekday(day))
            .collect::<Result<Vec<_>, _>>()?;
        config.schedule.pause_on_screen_lock = *pause_on_screen_lock;
        config.schedule.pause_on_battery_saver = *pause_on_battery_saver;
    }

    // Automation: single-field settings struct (`enabled` only) — not part of
    // the D-1 per-section block list (a 1-field destructure adds no coverage
    // beyond what the direct assignment already gives).
    config.automation.enabled = settings.automation.enabled;

    // Sandbox
    {
        let SandboxSettings {
            enabled,
            profile,
            allowed_read_paths,
            allowed_write_paths,
            allow_network,
            max_memory_bytes,
            max_cpu_time_ms,
        } = &settings.sandbox;
        config.automation.sandbox.enabled = *enabled;
        config.automation.sandbox.profile = parse_sandbox_profile(profile)?;
        config.automation.sandbox.allowed_read_paths = allowed_read_paths.clone();
        config.automation.sandbox.allowed_write_paths = allowed_write_paths.clone();
        config.automation.sandbox.allow_network = *allow_network;
        config.automation.sandbox.max_memory_bytes = *max_memory_bytes;
        config.automation.sandbox.max_cpu_time_ms = *max_cpu_time_ms;
    }

    apply_audio_settings(config, settings)?;
    apply_focus_auto_settings(config, settings)?;
    Ok(())
}

fn apply_audio_settings(config: &mut AppConfig, settings: &AppSettings) -> Result<(), ApiError> {
    // Layer 2 (D-1, #7740): real-named destructure of the source
    // `settings.audio` — `unused = "deny"` fails the build if a new
    // `AudioSettings` leaf is added but never bound/consumed below.
    let AudioSettings {
        enabled,
        whisper_model_path,
        language,
        max_recording_secs,
        model_size,
        stt_provider,
        cloud_api_key,
        cloud_stt_endpoint,
        cloud_timeout_secs,
        mic_input_mode,
        vad_threshold,
        vad_silence_ms,
        vad_min_speech_ms,
    } = &settings.audio;

    config.audio.enabled = *enabled;
    config.audio.whisper_model_path = whisper_model_path.clone();
    config.audio.language = parse_stt_language(language)?;
    config.audio.max_recording_secs = *max_recording_secs;
    config.audio.model_size = parse_whisper_model_size(model_size)?;
    config.audio.stt_provider = parse_stt_provider(stt_provider)?;
    // SECURITY (#7066): the GET path returns a MASKED sentinel for the cloud STT
    // secret, so an unchanged resubmit carries that sentinel (or an empty value)
    // rather than the raw key. Treat the masked sentinel / empty value as
    // 'unchanged' and only overwrite when a genuinely new plaintext key is
    // supplied — mirroring the AI-provider key write path
    // (`api_settings_to_endpoint`). Otherwise a round-trip save would clobber the
    // stored secret with its own mask.
    // CRITICAL (#7740 D-1): `cloud_api_key` is bound from the destructure above
    // (rename-only from the former `submitted_cloud_api_key` local) — the `if`
    // condition itself is UNCHANGED. Do not turn this into an unconditional
    // assignment; that would reintroduce the masked-sentinel-clobbers-secret bug
    // (#7066/#6281).
    if !cloud_api_key.is_empty() && !is_masked_key(cloud_api_key) {
        config.audio.cloud_api_key = cloud_api_key.clone();
    }
    config.audio.cloud_stt_endpoint = cloud_stt_endpoint.clone();
    config.audio.cloud_timeout_secs = *cloud_timeout_secs;
    config.audio.mic_input_mode = parse_mic_input_mode(mic_input_mode)?;
    config.audio.vad_threshold = *vad_threshold;
    config.audio.vad_silence_ms = *vad_silence_ms;
    config.audio.vad_min_speech_ms = *vad_min_speech_ms;
    Ok(())
}

fn apply_focus_auto_settings(
    config: &mut AppConfig,
    settings: &AppSettings,
) -> Result<(), ApiError> {
    // Layer 2 (D-1, #7740): real-named destructure of the source
    // `settings.focus_auto` — `unused = "deny"` fails the build if a new
    // `FocusAutoSettings` leaf is added but never bound/consumed below.
    let FocusAutoSettings {
        enabled,
        duration_minutes,
        trigger_apps,
        trigger_schedules,
        cooldown_secs,
    } = &settings.focus_auto;

    config.focus_auto.enabled = *enabled;
    config.focus_auto.duration_minutes = *duration_minutes;
    config.focus_auto.trigger_apps = trigger_apps.clone();
    config.focus_auto.trigger_schedules = trigger_schedules
        .iter()
        .map(|schedule| {
            Ok(FocusSchedule {
                time_range: TimeRange {
                    start: schedule.start.clone(),
                    end: schedule.end.clone(),
                },
                days: schedule
                    .days
                    .iter()
                    .map(|day| parse_weekday(day))
                    .collect::<Result<Vec<_>, _>>()?,
            })
        })
        .collect::<Result<Vec<_>, ApiError>>()?;
    config.focus_auto.cooldown_secs = *cooldown_secs;
    Ok(())
}

// NOTE (#7740 D-1 scope): `apply_ai_provider_settings` is deliberately OUT of
// the Layer-2 destructure-guard scope for this write-guard slice (see the F1
// plan's D-1 section). Its transform logic (endpoint credential merging,
// saved-profile diffing, active-profile sync) is materially more involved than
// a flat field copy, and bringing it in would grow this PR well past a
// write-guard slice. `ai_provider` still gets the weak Layer-1 top-level ack
// above; a follow-up can extend Layer 2 here.
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
    config.ai_provider.bypass_pii_filter_for_external_ocr =
        settings.ai_provider.bypass_pii_filter_for_external_ocr;
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
        bypass_pii_filter_for_external_ocr: settings.bypass_pii_filter_for_external_ocr,
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
        bypass_pii_filter_for_external_ocr: config.bypass_pii_filter_for_external_ocr,
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
    {
        let AiSessionSettings {
            max_concurrent_sessions,
            idle_timeout_secs,
            session_timeout_secs,
            max_retries,
            max_history_turns,
            health_check_interval_secs,
            max_output_tokens,
            thinking,
        } = &settings.ai_session;
        config.ai_session.max_concurrent_sessions = *max_concurrent_sessions;
        config.ai_session.idle_timeout_secs = *idle_timeout_secs;
        config.ai_session.session_timeout_secs = *session_timeout_secs;
        config.ai_session.max_retries = *max_retries;
        config.ai_session.max_history_turns = *max_history_turns;
        config.ai_session.health_check_interval_secs = *health_check_interval_secs;
        config.ai_session.max_output_tokens = *max_output_tokens;
        config.ai_session.thinking = thinking.clone();
    }

    // Suggestion
    {
        let SuggestionSettings { enabled } = &settings.suggestion;
        config.suggestions.enabled = *enabled;
    }

    // Indicator
    {
        let IndicatorSettings {
            show_border,
            show_panel,
            border_opacity,
        } = &settings.indicator;
        config.indicator.show_border = *show_border;
        config.indicator.show_panel = *show_panel;
        config.indicator.border_opacity = *border_opacity;
    }

    // Analysis
    {
        let AnalysisSettings {
            enabled,
            interval_secs,
            min_confidence,
            max_suggestions,
            embedding_enabled,
            llm_summary_enabled,
            gui_intelligence_enabled,
            text_intelligence_enabled,
            auto_tuner_enabled,
        } = &settings.analysis;
        config.analysis.enabled = *enabled;
        config.analysis.interval_secs = *interval_secs;
        config.analysis.min_confidence = *min_confidence;
        config.analysis.max_suggestions = *max_suggestions as usize;
        config.analysis.embedding.enabled = *embedding_enabled;
        config.analysis.embedding.llm_summary_enabled = *llm_summary_enabled;
        config.analysis.gui_intelligence.enabled = *gui_intelligence_enabled;
        config.analysis.text_intelligence.enabled = *text_intelligence_enabled;
        config.analysis.tiered_memory.auto_tuning.enabled = *auto_tuner_enabled;
    }

    // Network
    {
        let NetworkSettings {
            server_base_url,
            request_timeout_ms,
            grpc_enabled,
            grpc_endpoint,
            tls_enabled,
        } = &settings.network;
        config.server.base_url = server_base_url.clone();
        config.server.request_timeout_ms = *request_timeout_ms;
        config.grpc.grpc_endpoint = grpc_endpoint.clone();
        config.tls.enabled = *tls_enabled;
        config.grpc.use_grpc_auth = *grpc_enabled;
        config.grpc.use_grpc_context = *grpc_enabled;
    }

    // Coaching
    // Note: coaching.tone, coaching.overlay_mode, and regime_goals are
    // display-only, not persisted here — read-only on the wire (serialized by
    // the assembler for display) but not written back. tone/overlay_mode
    // require enum parsing (CoachingTone, OverlayMode).
    //
    // #8083/#8052: regime_goals is owned EXCLUSIVELY by the dedicated coaching
    // goals endpoint (Tauri `update_regime_goals` / REST `PUT /api/coaching/goals`),
    // which is its single writer. Writing it back from the settings payload let a
    // stale, goal-less Advanced-tab form clobber goals just added via that
    // endpoint — both in config here AND, via the `apply_config` hot-reload in
    // `settings_update_flow`, the live engine tracker. That was the QC CJ-03-01
    // "goal disappears after Save Settings" regression. Preserving the existing
    // config value (this function starts from a clone of the current config)
    // keeps goals intact; the same preserved value flows into `apply_config`, so
    // the engine tracker is re-seeded with the CORRECT goals, not the stale set.
    {
        let CoachingSettings {
            enabled,
            tone: _display_only_tone,
            locale,
            overlay_mode: _display_only_overlay_mode,
            quiet_hours,
            profiles,
            regime_goals: _dedicated_endpoint_owned_regime_goals,
        } = &settings.coaching;
        config.coaching.enabled = *enabled;
        config.coaching.locale = locale.clone();
        config.coaching.quiet_hours = quiet_hours
            .iter()
            .map(|range| TimeRange {
                start: range.start.clone(),
                end: range.end.clone(),
            })
            .collect();
        config.coaching.profiles = profiles
            .iter()
            .map(|(name, profile)| {
                (
                    name.clone(),
                    ProfileConfig {
                        enabled: profile.enabled,
                        min_interval_secs: profile.min_interval_secs,
                    },
                )
            })
            .collect();
        // regime_goals intentionally NOT written here — see the note above.
    }

    // Integration
    // Note: integration.auth_profile_kind is display-only, not persisted —
    // read-only on the wire; enum requires dedicated parsing
    // (IntegrationAuthProfileKind).
    {
        let IntegrationSettings {
            enabled,
            auth_profile_kind: _display_only_auth_profile_kind,
            request_timeout_secs,
            sync_interval_secs,
        } = &settings.integration;
        config.integration.enabled = *enabled;
        config.integration.request_timeout_secs = *request_timeout_secs;
        config.integration.sync_interval_secs = *sync_interval_secs;
    }

    // Sync
    // Note: sync.transport is display-only, not persisted — read-only on the
    // wire; enum requires dedicated parsing (SyncTransportKind).
    {
        let SyncSettings {
            enabled,
            transport: _display_only_transport,
            interval_secs,
            device_name,
            lan_advertise,
            compression_enabled,
        } = &settings.sync;
        config.sync.enabled = *enabled;
        config.sync.interval_secs = *interval_secs;
        config.sync.device_name = device_name.clone();
        config.sync.lan_advertise = *lan_advertise;
        config.sync.compression_enabled = *compression_enabled;
    }
}
