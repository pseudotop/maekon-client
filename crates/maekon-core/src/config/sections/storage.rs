// Storage/integrity/notification/update/telemetry config — data lifecycle and system-state management
use crate::error::CoreError;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── StorageConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub db_path: Option<PathBuf>,
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,
    #[serde(default = "default_max_storage_mb")]
    pub max_storage_mb: u64,
}

/// Floor for `storage.retention_days` (#6169).
pub(crate) const STORAGE_RETENTION_DAYS_FLOOR: u32 = 1;
/// Floor for `storage.max_storage_mb` (#6169).
pub(crate) const STORAGE_MAX_STORAGE_MB_FLOOR: u64 = 10;

impl StorageConfig {
    /// Validate that storage configuration values are within acceptable bounds.
    pub fn validate_bounds(&self) -> Result<(), String> {
        if self.retention_days < STORAGE_RETENTION_DAYS_FLOOR {
            return Err(format!(
                "storage.retention_days must be >= {STORAGE_RETENTION_DAYS_FLOOR}"
            ));
        }
        if self.max_storage_mb < STORAGE_MAX_STORAGE_MB_FLOOR {
            return Err(format!(
                "storage.max_storage_mb must be >= {STORAGE_MAX_STORAGE_MB_FLOOR}"
            ));
        }
        Ok(())
    }

    /// Raise any sub-floor storage value to its floor in place, returning the
    /// dotted-path identities of the fields that were clamped (#6169). Used by
    /// the fail-open INITIAL-LOAD path so the clamped config also satisfies
    /// [`Self::validate_bounds`].
    pub(crate) fn clamp_bounds(&mut self) -> Vec<&'static str> {
        let mut clamped = Vec::new();
        if self.retention_days < STORAGE_RETENTION_DAYS_FLOOR {
            self.retention_days = STORAGE_RETENTION_DAYS_FLOOR;
            clamped.push("storage.retention_days");
        }
        if self.max_storage_mb < STORAGE_MAX_STORAGE_MB_FLOOR {
            self.max_storage_mb = STORAGE_MAX_STORAGE_MB_FLOOR;
            clamped.push("storage.max_storage_mb");
        }
        clamped
    }
}

// ── IntegrityConfig ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityConfig {
    #[serde(default = "default_integrity_enabled")]
    pub enabled: bool,
    // Fail-closed by default: a config that omits this field must inherit the
    // same `true` as `IntegrityConfig::default()`, so the serde default and the
    // struct Default agree (a plain `#[serde(default)]` would yield `false`).
    #[serde(default = "default_require_signed_policy_bundle")]
    pub require_signed_policy_bundle: bool,
    #[serde(default)]
    pub policy_file_path: Option<String>,
    #[serde(default)]
    pub policy_signature_path: Option<String>,
    #[serde(default)]
    pub policy_public_key: Option<String>,
}

impl Default for IntegrityConfig {
    fn default() -> Self {
        Self {
            enabled: default_integrity_enabled(),
            require_signed_policy_bundle: true,
            policy_file_path: None,
            policy_signature_path: None,
            policy_public_key: None,
        }
    }
}

// ── TelemetryConfig ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryConfig {
    #[serde(default = "default_telemetry_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub crash_reports: bool,
    #[serde(default)]
    pub usage_analytics: bool,
    #[serde(default)]
    pub performance_metrics: bool,
    /// OTLP exporter endpoint. `None` falls back to the `OTEL_EXPORTER_OTLP_ENDPOINT`
    /// environment variable, and finally to `http://localhost:4318` (OTLP/HTTP default).
    #[serde(default)]
    pub otlp_endpoint: Option<String>,
    /// Sampling rate for spans, 0.0–1.0. Defaults to 1.0 (no sampling on top of
    /// tracing's own `EnvFilter`).
    #[serde(default = "default_telemetry_sample_rate")]
    pub sample_rate: f64,
    /// `service.name` OTel resource attribute. Identifies the client binary in
    /// the collector; not a user identifier.
    #[serde(default = "default_telemetry_service_name")]
    pub service_name: String,
}

fn default_telemetry_enabled() -> bool {
    true
}

fn default_telemetry_sample_rate() -> f64 {
    1.0
}

fn default_telemetry_service_name() -> String {
    "maekon-client".to_string()
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: default_telemetry_enabled(),
            crash_reports: false,
            usage_analytics: false,
            performance_metrics: false,
            otlp_endpoint: None,
            sample_rate: default_telemetry_sample_rate(),
            service_name: default_telemetry_service_name(),
        }
    }
}

impl TelemetryConfig {
    /// Force the exporter intent off for pre-consent bootstrap paths.
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }
}

// ── NotificationConfig ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    #[serde(default = "default_notification_enabled")]
    pub enabled: bool,
    #[serde(default = "default_idle_notification")]
    pub idle_notification: bool,
    #[serde(default = "default_idle_notification_mins")]
    pub idle_notification_mins: u32,
    #[serde(default = "default_long_session_notification")]
    pub long_session_notification: bool,
    #[serde(default = "default_long_session_mins")]
    pub long_session_mins: u32,
    #[serde(default = "default_high_usage_notification")]
    pub high_usage_notification: bool,
    #[serde(default = "default_high_usage_threshold")]
    pub high_usage_threshold: u32,
    #[serde(default)]
    pub daily_summary_notification: bool,
    /// Whether to fire a desktop notification when the tracking-schedule window
    /// is entered or exited. Defaults to `true` (CONS-M05).
    #[serde(default = "default_true")]
    pub tracking_schedule_enabled: bool,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: default_notification_enabled(),
            idle_notification: default_idle_notification(),
            idle_notification_mins: default_idle_notification_mins(),
            long_session_notification: default_long_session_notification(),
            long_session_mins: default_long_session_mins(),
            high_usage_notification: default_high_usage_notification(),
            high_usage_threshold: default_high_usage_threshold(),
            daily_summary_notification: false,
            tracking_schedule_enabled: default_true(),
        }
    }
}

// ── UpdateChannel ──────────────────────────────────────────────────

/// Update channel selection. Controls which GitHub Releases the updater
/// considers when checking for new versions.
///
/// - `Stable`: only non-prerelease releases (`/releases/latest`)
/// - `PreRelease`: RC and beta releases (`/releases?per_page=1`)
/// - `Nightly`: nightly builds (future — currently behaves like PreRelease)
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    #[serde(alias = "rc", alias = "beta")]
    PreRelease,
    Nightly,
}

impl UpdateChannel {
    /// Whether this channel includes prerelease versions.
    pub fn includes_prerelease(self) -> bool {
        matches!(self, Self::PreRelease | Self::Nightly)
    }
}

impl std::fmt::Display for UpdateChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stable => f.write_str("stable"),
            Self::PreRelease => f.write_str("pre_release"),
            Self::Nightly => f.write_str("nightly"),
        }
    }
}

// ── UpdateConfig ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateConfig {
    #[serde(default = "default_update_enabled")]
    pub enabled: bool,
    #[serde(default = "default_repo_owner")]
    pub repo_owner: String,
    #[serde(default = "default_repo_name")]
    pub repo_name: String,
    #[serde(default = "default_check_interval_hours")]
    pub check_interval_hours: u32,
    /// Update channel selection. Replaces the legacy `include_prerelease` boolean.
    /// Backward-compatible: old configs with `include_prerelease: true` are
    /// migrated to `channel: pre_release` on load.
    #[serde(default)]
    pub channel: UpdateChannel,
    /// Legacy field — kept for backward-compatible deserialization only.
    /// New code should use `channel` instead.
    #[serde(default, skip_serializing)]
    pub include_prerelease: bool,
    #[serde(default)]
    pub auto_install: bool,
    /// Unique per-installation identifier for staged rollout bucketing.
    /// Auto-generated on first launch if absent.
    #[serde(default)]
    pub installation_id: Option<String>,
    #[serde(default = "default_update_require_signature")]
    pub require_signature_verification: bool,
    #[serde(default = "default_update_signature_public_key")]
    pub signature_public_key: String,
    #[serde(default)]
    pub min_allowed_version: Option<String>,
    /// Update **ceiling** — managed-config / MDM update kill-switch (#4836).
    /// When set, the auto-updater will NOT offer or install any release **above**
    /// this version; the device is held at its current version (reported as
    /// up-to-date, not an error). Combined with an MDM lock
    /// (`ManagedUpdate.max_allowed_version`) this freezes a fleet at a known-good
    /// version — a soft kill-switch that, unlike `update.enabled = false`, still
    /// permits updates *up to* the ceiling. Empty/None = no ceiling. Must be
    /// valid semver and `>= min_allowed_version` when both are set.
    #[serde(default)]
    pub max_allowed_version: Option<String>,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: default_update_enabled(),
            repo_owner: default_repo_owner(),
            repo_name: default_repo_name(),
            check_interval_hours: default_check_interval_hours(),
            channel: UpdateChannel::Stable,
            include_prerelease: false,
            auto_install: false,
            installation_id: None,
            require_signature_verification: default_update_require_signature(),
            signature_public_key: default_update_signature_public_key(),
            min_allowed_version: None,
            max_allowed_version: None,
        }
    }
}

impl UpdateConfig {
    /// Resolve the effective channel, migrating legacy `include_prerelease`
    /// if `channel` is still at its default.
    pub fn effective_channel(&self) -> UpdateChannel {
        if self.channel == UpdateChannel::Stable && self.include_prerelease {
            UpdateChannel::PreRelease
        } else {
            self.channel
        }
    }
}

impl UpdateConfig {
    pub fn validate_integrity_policy(&self) -> Result<(), CoreError> {
        if !self.enabled {
            return Ok(());
        }

        if !self.require_signature_verification {
            return Err(CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message:
                    "update.require_signature_verification must be true when updates are enabled"
                        .to_string(),
            });
        }

        // D9 (Phase 4): The built-in TRUSTED_PUBLIC_KEYS array (lives in
        // src-tauri/src/updater/trusted_keys.rs) is the authoritative trust
        // source. `signature_public_key` here is now an optional user override
        // (e.g., dev self-signing). Empty is allowed; when non-empty, we only
        // validate the base64 + 32-byte shape so a malformed override doesn't
        // silently succeed at runtime.
        if let Some(key_b64) = self
            .signature_public_key
            .split_whitespace()
            .next()
            .filter(|k| !k.trim().is_empty())
        {
            let key_bytes = BASE64.decode(key_b64).map_err(|e| CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message: format!("update.signature_public_key must be valid base64: {}", e),
            })?;

            if key_bytes.len() != 32 {
                return Err(CoreError::Config {
                    code: crate::error_codes::ConfigCode::Invalid,
                    message: format!(
                        "update.signature_public_key must decode to 32 bytes, got {}",
                        key_bytes.len()
                    ),
                });
            }
        }

        if let Some(version_floor) = self
            .min_allowed_version
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            semver::Version::parse(version_floor).map_err(|e| CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message: format!("update.min_allowed_version must be valid semver: {}", e),
            })?;
        }

        // #4836: the optional update ceiling must be valid semver, and when a
        // floor is also set the window must be non-empty (ceiling >= floor) —
        // a ceiling below the floor would reject every release, which is a
        // config mistake rather than an intentional freeze, so fail loudly.
        if let Some(version_ceiling) = self
            .max_allowed_version
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            let ceiling =
                semver::Version::parse(version_ceiling).map_err(|e| CoreError::Config {
                    code: crate::error_codes::ConfigCode::Invalid,
                    message: format!("update.max_allowed_version must be valid semver: {}", e),
                })?;

            if let Some(version_floor) = self
                .min_allowed_version
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            {
                // The floor was already validated as parseable above.
                let floor =
                    semver::Version::parse(version_floor).map_err(|e| CoreError::Config {
                        code: crate::error_codes::ConfigCode::Invalid,
                        message: format!("update.min_allowed_version must be valid semver: {}", e),
                    })?;
                if ceiling < floor {
                    return Err(CoreError::Config {
                        code: crate::error_codes::ConfigCode::Invalid,
                        message: format!(
                            "update.max_allowed_version ({}) must be >= update.min_allowed_version ({})",
                            ceiling, floor
                        ),
                    });
                }
            }
        }

        Ok(())
    }
}

// ── Default / helper functions (pub(super) — used by config/mod.rs) ─

pub(crate) fn default_retention_days() -> u32 {
    30
}

pub(crate) fn default_max_storage_mb() -> u64 {
    500
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}

fn default_integrity_enabled() -> bool {
    true
}

fn default_require_signed_policy_bundle() -> bool {
    true
}

fn default_notification_enabled() -> bool {
    true
}

fn default_idle_notification() -> bool {
    true
}

fn default_idle_notification_mins() -> u32 {
    30
}

fn default_long_session_notification() -> bool {
    true
}

fn default_long_session_mins() -> u32 {
    60
}

fn default_high_usage_notification() -> bool {
    false
}

fn default_high_usage_threshold() -> u32 {
    90
}

fn default_update_enabled() -> bool {
    true
}

fn default_repo_owner() -> String {
    "pseudotop".to_string()
}

fn default_repo_name() -> String {
    "maekon-client".to_string()
}

fn default_check_interval_hours() -> u32 {
    24
}

fn default_update_require_signature() -> bool {
    true
}

fn default_update_signature_public_key() -> String {
    // D9 (Phase 4): TRUSTED_PUBLIC_KEYS (src-tauri/src/updater/trusted_keys.rs)
    // is the authoritative trust source for update-installer signatures.
    // This field is now an optional user override (e.g. dev self-signing);
    // default empty means "no override".
    //
    // Secondary consumer: src-tauri/src/integrity_guard.rs uses this field
    // as a fallback when `integrity.policy_public_key` is None. With the
    // empty default, that fallback is inert (decode of empty base64 produces
    // empty bytes, which fails the 32-byte shape check in verify_signed_policy_bundle).
    // The security baseline docs (docs/security/standalone-integrity-baseline.{md,ko.md})
    // reflect this — rely on `integrity.policy_public_key` explicitly.
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_update_signature_public_key_is_empty() {
        assert_eq!(default_update_signature_public_key(), "");
    }

    #[test]
    fn integrity_config_default_requires_signed_policy_bundle() {
        // Fail-closed baseline: the struct Default must keep the gate enabled.
        assert!(IntegrityConfig::default().require_signed_policy_bundle);
    }

    #[test]
    fn integrity_config_serde_default_matches_struct_default() {
        // A config that omits `require_signed_policy_bundle` must deserialize to
        // the same `true` as the struct Default. A plain `#[serde(default)]`
        // would silently yield `false`, opening the fail-closed gate via an
        // incomplete config file.
        let parsed: IntegrityConfig =
            serde_json::from_str("{}").expect("empty IntegrityConfig JSON must deserialize");
        assert_eq!(
            parsed.require_signed_policy_bundle,
            IntegrityConfig::default().require_signed_policy_bundle,
            "serde default must agree with the struct Default (fail-closed)"
        );
        assert!(parsed.require_signed_policy_bundle);
    }

    #[test]
    fn validate_integrity_policy_passes_with_default_config() {
        // Guard: prevents regressing to a hardcoded default that might
        // conflict with validation (e.g., a future non-base64 placeholder).
        // validate_integrity_policy returns Result<(), CoreError>; the unit
        // value carries no information, so Ok(()) is the complete contract.
        UpdateConfig::default()
            .validate_integrity_policy()
            .expect("default UpdateConfig must validate");
    }

    #[test]
    fn default_update_config_has_empty_signature_public_key() {
        // I-4 invariant: the per-config default MUST NOT be a hardcoded
        // copy of TRUSTED_PUBLIC_KEYS[0]. This test catches a silent revert
        // to any non-empty default (including a rotated-key hardcoded copy)
        // that would re-introduce the "user-configured override" false
        // positive during key rotation.
        assert_eq!(UpdateConfig::default().signature_public_key, "");
    }

    // #4836: update ceiling (`max_allowed_version`) validation.

    fn signed_update_config() -> UpdateConfig {
        // A minimal valid enabled config (signature verification on, empty
        // override key) so validation reaches the version-window checks.
        UpdateConfig {
            enabled: true,
            require_signature_verification: true,
            signature_public_key: String::new(),
            ..UpdateConfig::default()
        }
    }

    #[test]
    fn validate_accepts_well_formed_version_window() {
        let mut config = signed_update_config();
        config.min_allowed_version = Some("1.0.0".to_string());
        config.max_allowed_version = Some("2.5.0".to_string());
        // validate_integrity_policy returns Result<(), CoreError>; Ok(()) is the
        // full contract — the unit value has no further properties to assert.
        config
            .validate_integrity_policy()
            .expect("a valid [min, max] version window must pass validation");
    }

    #[test]
    fn validate_rejects_malformed_max_allowed_version() {
        let mut config = signed_update_config();
        config.max_allowed_version = Some("not-semver".to_string());
        let err = config
            .validate_integrity_policy()
            .expect_err("malformed ceiling must be rejected");
        assert!(
            format!("{err}").contains("max_allowed_version"),
            "error must name the offending field, got: {err}"
        );
    }

    #[test]
    fn validate_rejects_ceiling_below_floor() {
        let mut config = signed_update_config();
        config.min_allowed_version = Some("3.0.0".to_string());
        config.max_allowed_version = Some("2.0.0".to_string());
        let err = config
            .validate_integrity_policy()
            .expect_err("an empty window (ceiling < floor) must be rejected");
        assert!(
            format!("{err}").contains("must be >="),
            "error must explain the window inversion, got: {err}"
        );
    }

    #[test]
    fn validate_allows_ceiling_equal_to_floor() {
        // A single-version pin (min == max) is a valid (degenerate) window.
        let mut config = signed_update_config();
        config.min_allowed_version = Some("2.0.0".to_string());
        config.max_allowed_version = Some("2.0.0".to_string());
        // Contract: ceiling == floor is allowed (degenerate window: pins the fleet to
        // exactly one version). The ">= floor" check must treat equality as valid.
        config
            .validate_integrity_policy()
            .expect("min_allowed_version == max_allowed_version must be accepted as a valid single-version pin");
        // Confirm the pinned values survived the validation call.
        assert_eq!(config.min_allowed_version.as_deref(), Some("2.0.0"));
        assert_eq!(config.max_allowed_version.as_deref(), Some("2.0.0"));
    }

    #[test]
    fn default_update_config_has_no_version_window() {
        let config = UpdateConfig::default();
        assert!(config.min_allowed_version.is_none());
        assert!(config.max_allowed_version.is_none());
    }
}
