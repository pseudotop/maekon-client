use chrono::{DateTime, Utc};
use oneshim_api_contracts::settings::AppSettings;
use oneshim_core::config::{
    AiAccessMode, AiProviderType, CredentialAuthMode, CredentialBackendKind, ExternalDataPolicy,
    LlmProviderType, OcrProviderType, PiiFilterLevel, SandboxProfile, Weekday,
};
use oneshim_core::ports::secret_store::validate_secret_segment;
use oneshim_core::provider_surface::provider_type_from_vendor_id;
use std::collections::HashSet;

use crate::error::ApiError;

pub(crate) fn validate_settings_input(settings: &AppSettings) -> Result<(), ApiError> {
    if settings.retention_days == 0 || settings.retention_days > 365 {
        return Err(ApiError::BadRequest(
            "Retention period must be between 1 and 365 days.".to_string(),
        ));
    }
    if settings.max_storage_mb < 100 || settings.max_storage_mb > 10000 {
        return Err(ApiError::BadRequest(
            "Maximum storage size must be between 100 MB and 10 GB.".to_string(),
        ));
    }
    if settings.web_port < 1024 {
        return Err(ApiError::BadRequest(
            "web_port must be 1024 or higher.".to_string(),
        ));
    }
    if !settings
        .ai_provider
        .ocr_validation
        .min_confidence
        .is_finite()
        || !(0.0..=1.0).contains(&settings.ai_provider.ocr_validation.min_confidence)
    {
        return Err(ApiError::BadRequest(
            "ai_provider.ocr_validation.min_confidence must be within 0.0..=1.0.".to_string(),
        ));
    }
    if !settings
        .ai_provider
        .ocr_validation
        .max_invalid_ratio
        .is_finite()
        || !(0.0..=1.0).contains(&settings.ai_provider.ocr_validation.max_invalid_ratio)
    {
        return Err(ApiError::BadRequest(
            "ai_provider.ocr_validation.max_invalid_ratio must be within 0.0..=1.0.".to_string(),
        ));
    }
    if !settings
        .ai_provider
        .scene_intelligence
        .min_confidence
        .is_finite()
        || !(0.0..=1.0).contains(&settings.ai_provider.scene_intelligence.min_confidence)
    {
        return Err(ApiError::BadRequest(
            "ai_provider.scene_intelligence.min_confidence must be within 0.0..=1.0.".to_string(),
        ));
    }
    if settings.ai_provider.scene_intelligence.max_elements == 0
        || settings.ai_provider.scene_intelligence.max_elements > 1000
    {
        return Err(ApiError::BadRequest(
            "ai_provider.scene_intelligence.max_elements must be within 1..=1000.".to_string(),
        ));
    }
    if settings
        .ai_provider
        .scene_intelligence
        .calibration_min_elements
        == 0
        || settings
            .ai_provider
            .scene_intelligence
            .calibration_min_elements
            > 1000
    {
        return Err(ApiError::BadRequest(
            "ai_provider.scene_intelligence.calibration_min_elements must be within 1..=1000."
                .to_string(),
        ));
    }
    if !settings
        .ai_provider
        .scene_intelligence
        .calibration_min_avg_confidence
        .is_finite()
        || !(0.0..=1.0).contains(
            &settings
                .ai_provider
                .scene_intelligence
                .calibration_min_avg_confidence,
        )
    {
        return Err(ApiError::BadRequest(
            "ai_provider.scene_intelligence.calibration_min_avg_confidence must be within 0.0..=1.0."
                .to_string(),
        ));
    }
    validate_ai_provider_profiles_input(settings)?;
    Ok(())
}

fn validate_ai_provider_profiles_input(settings: &AppSettings) -> Result<(), ApiError> {
    let mut seen_profile_ids = HashSet::new();

    for profile in settings.ai_provider.saved_profiles.iter() {
        validate_secret_segment(
            &profile.profile_id,
            "ai_provider.saved_profiles[].profile_id",
        )
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;

        if profile.name.trim().is_empty() {
            return Err(ApiError::BadRequest(
                "ai_provider.saved_profiles[].name must not be empty.".to_string(),
            ));
        }

        if !seen_profile_ids.insert(profile.profile_id.as_str()) {
            return Err(ApiError::BadRequest(format!(
                "Duplicate ai_provider.saved_profiles profile_id: {}",
                profile.profile_id
            )));
        }
    }

    if let Some(active_profile_id) = settings
        .ai_provider
        .active_profile_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        if !settings
            .ai_provider
            .saved_profiles
            .iter()
            .any(|profile| profile.profile_id == active_profile_id)
        {
            return Err(ApiError::BadRequest(format!(
                "ai_provider.active_profile_id references an unknown saved profile: {active_profile_id}"
            )));
        }
    }

    Ok(())
}

pub(crate) fn parse_pii_filter_level(value: &str) -> Result<PiiFilterLevel, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" => Ok(PiiFilterLevel::Off),
        "basic" => Ok(PiiFilterLevel::Basic),
        "standard" => Ok(PiiFilterLevel::Standard),
        "strict" => Ok(PiiFilterLevel::Strict),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid privacy.pii_filter_level value: {value}"
        ))),
    }
}

pub(crate) fn parse_weekday(value: &str) -> Result<Weekday, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "mon" => Ok(Weekday::Mon),
        "tue" => Ok(Weekday::Tue),
        "wed" => Ok(Weekday::Wed),
        "thu" => Ok(Weekday::Thu),
        "fri" => Ok(Weekday::Fri),
        "sat" => Ok(Weekday::Sat),
        "sun" => Ok(Weekday::Sun),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid schedule.active_days value: {value}"
        ))),
    }
}

pub(crate) fn parse_sandbox_profile(value: &str) -> Result<SandboxProfile, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "permissive" => Ok(SandboxProfile::Permissive),
        "standard" | "balanced" => Ok(SandboxProfile::Standard),
        "strict" => Ok(SandboxProfile::Strict),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid sandbox.profile value: {value}"
        ))),
    }
}

pub(crate) fn parse_ocr_provider(value: &str) -> Result<OcrProviderType, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "local" => Ok(OcrProviderType::Local),
        "remote" => Ok(OcrProviderType::Remote),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid ai_provider.ocr_provider value: {value}"
        ))),
    }
}

pub(crate) fn parse_ai_access_mode(value: &str) -> Result<AiAccessMode, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "providerapikey" | "provider_api_key" | "api" | "apikey" => {
            Ok(AiAccessMode::ProviderApiKey)
        }
        "localmodel" | "local_model" | "local" => Ok(AiAccessMode::LocalModel),
        "providersubscriptioncli" | "provider_subscription_cli" | "cli" | "subscription" => {
            Ok(AiAccessMode::ProviderSubscriptionCli)
        }
        "provideroauth" | "provider_oauth" | "oauth" => Ok(AiAccessMode::ProviderOAuth),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid ai_provider.access_mode value: {value}"
        ))),
    }
}

pub(crate) fn parse_ai_provider_type(value: &str) -> Result<AiProviderType, ApiError> {
    provider_type_from_vendor_id(value).ok_or_else(|| {
        ApiError::BadRequest(format!(
            "Invalid ai_provider.api.provider_type value: {value}"
        ))
    })
}

pub(crate) fn parse_llm_provider(value: &str) -> Result<LlmProviderType, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "local" => Ok(LlmProviderType::Local),
        "remote" => Ok(LlmProviderType::Remote),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid ai_provider.llm_provider value: {value}"
        ))),
    }
}

pub(crate) fn parse_external_data_policy(value: &str) -> Result<ExternalDataPolicy, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "piifilterstrict" => Ok(ExternalDataPolicy::PiiFilterStrict),
        "piifilterstandard" => Ok(ExternalDataPolicy::PiiFilterStandard),
        "allowfiltered" => Ok(ExternalDataPolicy::AllowFiltered),
        "disabled" => Ok(ExternalDataPolicy::PiiFilterStrict),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid ai_provider.external_data_policy value: {value}"
        ))),
    }
}

pub(crate) fn parse_credential_auth_mode(value: &str) -> Result<CredentialAuthMode, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "api_key" | "apikey" => Ok(CredentialAuthMode::ApiKey),
        "managed_oauth" | "managedoauth" => Ok(CredentialAuthMode::ManagedOAuth),
        "cli_bridge" | "clibridge" => Ok(CredentialAuthMode::CliBridge),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid ai_provider.api.auth_mode value: {value}"
        ))),
    }
}

pub(crate) fn parse_credential_backend_kind(
    value: &str,
) -> Result<CredentialBackendKind, ApiError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "os_secret_store" | "ossecretstore" => Ok(CredentialBackendKind::OsSecretStore),
        "file_secret_store" | "filesecretstore" => Ok(CredentialBackendKind::FileSecretStore),
        "env" => Ok(CredentialBackendKind::Env),
        "bridge_managed" | "bridgemanaged" => Ok(CredentialBackendKind::BridgeManaged),
        "unavailable" => Ok(CredentialBackendKind::Unavailable),
        _ => Err(ApiError::BadRequest(format!(
            "Invalid ai_provider.api.backend_kind value: {value}"
        ))),
    }
}

pub(crate) fn is_managed_auth_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "managed_oauth" | "cli_bridge"
    )
}

pub(crate) fn trim_to_option(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(crate) fn parse_optional_rfc3339_utc(
    value: Option<&str>,
    field_name: &str,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }

    let parsed = DateTime::parse_from_rfc3339(trimmed).map_err(|_| {
        ApiError::BadRequest(format!(
            "{field_name} must use RFC3339 format. Example: 2026-02-24T03:00:00Z"
        ))
    })?;

    Ok(Some(parsed.with_timezone(&Utc)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Timelike};
    use oneshim_api_contracts::settings::AppSettings;

    fn valid_settings() -> AppSettings {
        AppSettings::default()
    }

    // ── validate_settings_input ─────────────────────────────────────

    #[test]
    fn validate_accepts_default_settings() {
        assert!(validate_settings_input(&valid_settings()).is_ok());
    }

    #[test]
    fn validate_rejects_zero_retention_days() {
        let mut s = valid_settings();
        s.retention_days = 0;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_retention_days_above_365() {
        let mut s = valid_settings();
        s.retention_days = 366;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_accepts_retention_days_boundary() {
        for days in [1, 365] {
            let mut s = valid_settings();
            s.retention_days = days;
            assert!(validate_settings_input(&s).is_ok());
        }
    }

    #[test]
    fn validate_rejects_max_storage_below_100() {
        let mut s = valid_settings();
        s.max_storage_mb = 99;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_max_storage_above_10000() {
        let mut s = valid_settings();
        s.max_storage_mb = 10001;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_accepts_max_storage_boundary() {
        for mb in [100, 10000] {
            let mut s = valid_settings();
            s.max_storage_mb = mb;
            assert!(validate_settings_input(&s).is_ok());
        }
    }

    #[test]
    fn validate_rejects_web_port_below_1024() {
        let mut s = valid_settings();
        s.web_port = 1023;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_accepts_web_port_1024() {
        let mut s = valid_settings();
        s.web_port = 1024;
        assert!(validate_settings_input(&s).is_ok());
    }

    #[test]
    fn validate_rejects_ocr_min_confidence_above_1() {
        let mut s = valid_settings();
        s.ai_provider.ocr_validation.min_confidence = 1.01;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_ocr_min_confidence_negative() {
        let mut s = valid_settings();
        s.ai_provider.ocr_validation.min_confidence = -0.01;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_ocr_min_confidence_nan() {
        let mut s = valid_settings();
        s.ai_provider.ocr_validation.min_confidence = f64::NAN;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_ocr_min_confidence_inf() {
        let mut s = valid_settings();
        s.ai_provider.ocr_validation.min_confidence = f64::INFINITY;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_accepts_ocr_min_confidence_boundary() {
        for val in [0.0, 1.0] {
            let mut s = valid_settings();
            s.ai_provider.ocr_validation.min_confidence = val;
            assert!(validate_settings_input(&s).is_ok());
        }
    }

    #[test]
    fn validate_rejects_ocr_max_invalid_ratio_above_1() {
        let mut s = valid_settings();
        s.ai_provider.ocr_validation.max_invalid_ratio = 1.001;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_ocr_max_invalid_ratio_nan() {
        let mut s = valid_settings();
        s.ai_provider.ocr_validation.max_invalid_ratio = f64::NAN;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_scene_min_confidence_above_1() {
        let mut s = valid_settings();
        s.ai_provider.scene_intelligence.min_confidence = 1.5;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_scene_min_confidence_neg_inf() {
        let mut s = valid_settings();
        s.ai_provider.scene_intelligence.min_confidence = f64::NEG_INFINITY;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_scene_max_elements_zero() {
        let mut s = valid_settings();
        s.ai_provider.scene_intelligence.max_elements = 0;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_scene_max_elements_above_1000() {
        let mut s = valid_settings();
        s.ai_provider.scene_intelligence.max_elements = 1001;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_accepts_scene_max_elements_boundary() {
        for val in [1, 1000] {
            let mut s = valid_settings();
            s.ai_provider.scene_intelligence.max_elements = val;
            assert!(validate_settings_input(&s).is_ok());
        }
    }

    #[test]
    fn validate_rejects_calibration_min_elements_zero() {
        let mut s = valid_settings();
        s.ai_provider.scene_intelligence.calibration_min_elements = 0;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_calibration_min_elements_above_1000() {
        let mut s = valid_settings();
        s.ai_provider.scene_intelligence.calibration_min_elements = 1001;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_calibration_min_avg_confidence_above_1() {
        let mut s = valid_settings();
        s.ai_provider
            .scene_intelligence
            .calibration_min_avg_confidence = 1.1;
        assert!(validate_settings_input(&s).is_err());
    }

    #[test]
    fn validate_rejects_calibration_min_avg_confidence_nan() {
        let mut s = valid_settings();
        s.ai_provider
            .scene_intelligence
            .calibration_min_avg_confidence = f64::NAN;
        assert!(validate_settings_input(&s).is_err());
    }

    // ── parse_pii_filter_level ──────────────────────────────────────

    #[test]
    fn parse_pii_filter_level_valid_lowercase() {
        assert_eq!(parse_pii_filter_level("off").unwrap(), PiiFilterLevel::Off);
        assert_eq!(
            parse_pii_filter_level("basic").unwrap(),
            PiiFilterLevel::Basic
        );
        assert_eq!(
            parse_pii_filter_level("standard").unwrap(),
            PiiFilterLevel::Standard
        );
        assert_eq!(
            parse_pii_filter_level("strict").unwrap(),
            PiiFilterLevel::Strict
        );
    }

    #[test]
    fn parse_pii_filter_level_case_insensitive() {
        assert_eq!(parse_pii_filter_level("OFF").unwrap(), PiiFilterLevel::Off);
        assert_eq!(
            parse_pii_filter_level("Basic").unwrap(),
            PiiFilterLevel::Basic
        );
    }

    #[test]
    fn parse_pii_filter_level_trims_whitespace() {
        assert_eq!(
            parse_pii_filter_level("  strict  ").unwrap(),
            PiiFilterLevel::Strict
        );
    }

    #[test]
    fn parse_pii_filter_level_invalid() {
        assert!(parse_pii_filter_level("maximum").is_err());
    }

    // ── parse_weekday ───────────────────────────────────────────────

    #[test]
    fn parse_weekday_valid_lowercase() {
        let cases = [
            ("mon", Weekday::Mon),
            ("tue", Weekday::Tue),
            ("wed", Weekday::Wed),
            ("thu", Weekday::Thu),
            ("fri", Weekday::Fri),
            ("sat", Weekday::Sat),
            ("sun", Weekday::Sun),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_weekday(input).unwrap(), expected);
        }
    }

    #[test]
    fn parse_weekday_case_insensitive() {
        assert_eq!(parse_weekday("MON").unwrap(), Weekday::Mon);
        assert_eq!(parse_weekday("Sun").unwrap(), Weekday::Sun);
    }

    #[test]
    fn parse_weekday_invalid() {
        assert!(parse_weekday("monday").is_err());
        assert!(parse_weekday("").is_err());
    }

    // ── parse_sandbox_profile ───────────────────────────────────────

    #[test]
    fn parse_sandbox_profile_valid() {
        assert_eq!(
            parse_sandbox_profile("permissive").unwrap(),
            SandboxProfile::Permissive
        );
        assert_eq!(
            parse_sandbox_profile("standard").unwrap(),
            SandboxProfile::Standard
        );
        assert_eq!(
            parse_sandbox_profile("balanced").unwrap(),
            SandboxProfile::Standard
        );
        assert_eq!(
            parse_sandbox_profile("strict").unwrap(),
            SandboxProfile::Strict
        );
    }

    #[test]
    fn parse_sandbox_profile_invalid() {
        assert!(parse_sandbox_profile("relaxed").is_err());
    }

    // ── parse_ocr_provider ──────────────────────────────────────────

    #[test]
    fn parse_ocr_provider_valid() {
        assert_eq!(parse_ocr_provider("local").unwrap(), OcrProviderType::Local);
        assert_eq!(
            parse_ocr_provider("remote").unwrap(),
            OcrProviderType::Remote
        );
    }

    #[test]
    fn parse_ocr_provider_invalid() {
        assert!(parse_ocr_provider("cloud").is_err());
    }

    // ── parse_ai_access_mode ────────────────────────────────────────

    #[test]
    fn parse_ai_access_mode_api_key_aliases() {
        for alias in ["providerapikey", "provider_api_key", "api", "apikey"] {
            assert_eq!(
                parse_ai_access_mode(alias).unwrap(),
                AiAccessMode::ProviderApiKey
            );
        }
    }

    #[test]
    fn parse_ai_access_mode_local_model_aliases() {
        for alias in ["localmodel", "local_model", "local"] {
            assert_eq!(
                parse_ai_access_mode(alias).unwrap(),
                AiAccessMode::LocalModel
            );
        }
    }

    #[test]
    fn parse_ai_access_mode_subscription_cli_aliases() {
        for alias in [
            "providersubscriptioncli",
            "provider_subscription_cli",
            "cli",
            "subscription",
        ] {
            assert_eq!(
                parse_ai_access_mode(alias).unwrap(),
                AiAccessMode::ProviderSubscriptionCli
            );
        }
    }

    #[test]
    fn parse_ai_access_mode_oauth_aliases() {
        for alias in ["provideroauth", "provider_oauth", "oauth"] {
            assert_eq!(
                parse_ai_access_mode(alias).unwrap(),
                AiAccessMode::ProviderOAuth
            );
        }
    }

    #[test]
    fn parse_ai_access_mode_invalid() {
        assert!(parse_ai_access_mode("bearer").is_err());
    }

    // ── parse_llm_provider ──────────────────────────────────────────

    #[test]
    fn parse_llm_provider_valid() {
        assert_eq!(parse_llm_provider("local").unwrap(), LlmProviderType::Local);
        assert_eq!(
            parse_llm_provider("remote").unwrap(),
            LlmProviderType::Remote
        );
    }

    #[test]
    fn parse_llm_provider_invalid() {
        assert!(parse_llm_provider("hybrid").is_err());
    }

    // ── parse_external_data_policy ──────────────────────────────────

    #[test]
    fn parse_external_data_policy_valid() {
        assert_eq!(
            parse_external_data_policy("piifilterstrict").unwrap(),
            ExternalDataPolicy::PiiFilterStrict
        );
        assert_eq!(
            parse_external_data_policy("piifilterstandard").unwrap(),
            ExternalDataPolicy::PiiFilterStandard
        );
        assert_eq!(
            parse_external_data_policy("allowfiltered").unwrap(),
            ExternalDataPolicy::AllowFiltered
        );
        assert_eq!(
            parse_external_data_policy("disabled").unwrap(),
            ExternalDataPolicy::PiiFilterStrict
        );
    }

    #[test]
    fn parse_external_data_policy_invalid() {
        assert!(parse_external_data_policy("allow_all").is_err());
    }

    // ── parse_credential_auth_mode ──────────────────────────────────

    #[test]
    fn parse_credential_auth_mode_api_key_aliases() {
        for alias in ["api_key", "apikey"] {
            assert_eq!(
                parse_credential_auth_mode(alias).unwrap(),
                CredentialAuthMode::ApiKey
            );
        }
    }

    #[test]
    fn parse_credential_auth_mode_managed_oauth_aliases() {
        for alias in ["managed_oauth", "managedoauth"] {
            assert_eq!(
                parse_credential_auth_mode(alias).unwrap(),
                CredentialAuthMode::ManagedOAuth
            );
        }
    }

    #[test]
    fn parse_credential_auth_mode_cli_bridge_aliases() {
        for alias in ["cli_bridge", "clibridge"] {
            assert_eq!(
                parse_credential_auth_mode(alias).unwrap(),
                CredentialAuthMode::CliBridge
            );
        }
    }

    #[test]
    fn parse_credential_auth_mode_invalid() {
        assert!(parse_credential_auth_mode("basic_auth").is_err());
    }

    // ── parse_credential_backend_kind ───────────────────────────────

    #[test]
    fn parse_credential_backend_kind_os_secret_store_aliases() {
        for alias in ["os_secret_store", "ossecretstore"] {
            assert_eq!(
                parse_credential_backend_kind(alias).unwrap(),
                CredentialBackendKind::OsSecretStore
            );
        }
    }

    #[test]
    fn parse_credential_backend_kind_file_secret_store_aliases() {
        for alias in ["file_secret_store", "filesecretstore"] {
            assert_eq!(
                parse_credential_backend_kind(alias).unwrap(),
                CredentialBackendKind::FileSecretStore
            );
        }
    }

    #[test]
    fn parse_credential_backend_kind_env() {
        assert_eq!(
            parse_credential_backend_kind("env").unwrap(),
            CredentialBackendKind::Env
        );
    }

    #[test]
    fn parse_credential_backend_kind_bridge_managed_aliases() {
        for alias in ["bridge_managed", "bridgemanaged"] {
            assert_eq!(
                parse_credential_backend_kind(alias).unwrap(),
                CredentialBackendKind::BridgeManaged
            );
        }
    }

    #[test]
    fn parse_credential_backend_kind_unavailable() {
        assert_eq!(
            parse_credential_backend_kind("unavailable").unwrap(),
            CredentialBackendKind::Unavailable
        );
    }

    #[test]
    fn parse_credential_backend_kind_invalid() {
        assert!(parse_credential_backend_kind("memory").is_err());
    }

    // ── is_managed_auth_mode ────────────────────────────────────────

    #[test]
    fn is_managed_auth_mode_returns_true() {
        assert!(is_managed_auth_mode("managed_oauth"));
        assert!(is_managed_auth_mode("cli_bridge"));
    }

    #[test]
    fn is_managed_auth_mode_returns_false() {
        assert!(!is_managed_auth_mode("api_key"));
        assert!(!is_managed_auth_mode("apikey"));
        assert!(!is_managed_auth_mode(""));
    }

    #[test]
    fn is_managed_auth_mode_trims_and_lowercases() {
        assert!(is_managed_auth_mode("  Managed_OAuth  "));
        assert!(is_managed_auth_mode(" CLI_BRIDGE "));
    }

    // ── trim_to_option ──────────────────────────────────────────────

    #[test]
    fn trim_to_option_empty_returns_none() {
        assert_eq!(trim_to_option(""), None);
    }

    #[test]
    fn trim_to_option_whitespace_only_returns_none() {
        assert_eq!(trim_to_option("   "), None);
    }

    #[test]
    fn trim_to_option_non_empty_returns_trimmed() {
        assert_eq!(trim_to_option("  hello  "), Some("hello".to_string()));
    }

    #[test]
    fn trim_to_option_no_trimming_needed() {
        assert_eq!(trim_to_option("value"), Some("value".to_string()));
    }

    // ── parse_optional_rfc3339_utc ──────────────────────────────────

    #[test]
    fn parse_optional_rfc3339_utc_none_returns_ok_none() {
        assert_eq!(parse_optional_rfc3339_utc(None, "field").unwrap(), None);
    }

    #[test]
    fn parse_optional_rfc3339_utc_empty_returns_ok_none() {
        assert_eq!(parse_optional_rfc3339_utc(Some(""), "field").unwrap(), None);
    }

    #[test]
    fn parse_optional_rfc3339_utc_whitespace_returns_ok_none() {
        assert_eq!(
            parse_optional_rfc3339_utc(Some("   "), "field").unwrap(),
            None
        );
    }

    #[test]
    fn parse_optional_rfc3339_utc_valid_returns_datetime() {
        let result = parse_optional_rfc3339_utc(Some("2026-02-24T03:00:00Z"), "field").unwrap();
        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 2);
        assert_eq!(dt.day(), 24);
    }

    #[test]
    fn parse_optional_rfc3339_utc_valid_with_offset() {
        let result =
            parse_optional_rfc3339_utc(Some("2026-02-24T12:00:00+09:00"), "field").unwrap();
        assert!(result.is_some());
        // Converted to UTC: 2026-02-24T03:00:00Z
        assert_eq!(result.unwrap().hour(), 3);
    }

    #[test]
    fn parse_optional_rfc3339_utc_invalid_format() {
        let err = parse_optional_rfc3339_utc(Some("not-a-date"), "expires_at")
            .expect_err("should reject invalid format");
        match err {
            ApiError::BadRequest(msg) => {
                assert!(msg.contains("expires_at"));
                assert!(msg.contains("RFC3339"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
