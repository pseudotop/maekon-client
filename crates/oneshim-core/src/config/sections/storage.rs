// 스토리지/무결성/알림/업데이트/텔레메트리 설정 — 데이터 생명주기 및 시스템 상태 관리
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

impl StorageConfig {
    /// Validate that storage configuration values are within acceptable bounds.
    pub fn validate_bounds(&self) -> Result<(), String> {
        if self.retention_days < 1 {
            return Err("storage.retention_days must be >= 1".to_string());
        }
        if self.max_storage_mb < 10 {
            return Err("storage.max_storage_mb must be >= 10".to_string());
        }
        Ok(())
    }
}

// ── IntegrityConfig ────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrityConfig {
    #[serde(default = "default_integrity_enabled")]
    pub enabled: bool,
    #[serde(default)]
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
    #[serde(default)]
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

fn default_telemetry_sample_rate() -> f64 {
    1.0
}

fn default_telemetry_service_name() -> String {
    "oneshim-client".to_string()
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            crash_reports: false,
            usage_analytics: false,
            performance_metrics: false,
            otlp_endpoint: None,
            sample_rate: default_telemetry_sample_rate(),
            service_name: default_telemetry_service_name(),
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

        Ok(())
    }
}

// ── Default / helper functions (pub(super) — config/mod.rs 에서 사용) ─

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
    fn validate_integrity_policy_passes_with_default_config() {
        // Guard: prevents regressing to a hardcoded default that might
        // conflict with validation (e.g., a future non-base64 placeholder).
        let config = UpdateConfig::default();
        assert!(
            config.validate_integrity_policy().is_ok(),
            "default UpdateConfig must validate: {:?}",
            config.validate_integrity_policy()
        );
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
}
