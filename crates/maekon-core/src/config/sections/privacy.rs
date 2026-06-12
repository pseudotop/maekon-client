// 개인정보/격리 설정 — PII 필터 수준, 자동화 샌드박스, 제외 앱 목록
use super::super::enums::{ConfirmationRequirement, PiiFilterLevel, SandboxProfile};
use serde::{Deserialize, Serialize};

// ── PrivacyConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    #[serde(default)]
    pub excluded_apps: Vec<String>,
    #[serde(default)]
    pub excluded_app_patterns: Vec<String>,
    #[serde(default)]
    pub excluded_title_patterns: Vec<String>,
    #[serde(default = "default_true")]
    pub auto_exclude_sensitive: bool,
    #[serde(default)]
    pub pii_filter_level: PiiFilterLevel,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            excluded_apps: Vec::new(),
            excluded_app_patterns: Vec::new(),
            excluded_title_patterns: Vec::new(),
            auto_exclude_sensitive: true,
            pii_filter_level: PiiFilterLevel::Standard,
        }
    }
}

// ── SandboxConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub profile: SandboxProfile,
    #[serde(default)]
    pub allowed_read_paths: Vec<String>,
    #[serde(default)]
    pub allowed_write_paths: Vec<String>,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub max_memory_bytes: u64,
    #[serde(default)]
    pub max_cpu_time_ms: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profile: SandboxProfile::Standard,
            allowed_read_paths: Vec::new(),
            allowed_write_paths: Vec::new(),
            allow_network: false,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
        }
    }
}

// ── AutomationConfig ───────────────────────────────────────────────

/// Configuration for the automation (RPA) subsystem.
///
/// `confirmation_policy` uses [`ConfirmationRequirement`], which is the
/// canonical runtime gate enum (Auto / Confirm / Block).  The former
/// `AutomationConfirmPolicy` (AlwaysConfirm / TrustedOnly / NeverConfirm)
/// has been retired and unified here (F-RC-C24-02).  Existing config files
/// that stored `ALWAYS_CONFIRM` or `TRUSTED_ONLY` should be migrated to
/// `CONFIRM`; `NEVER_CONFIRM` maps to `AUTO`.
///
/// The knob's default is `Auto` (D2-② product sign-off: intent-hint runs
/// immediately under strict sandbox, matching the "Runs immediately under
/// strict sandbox" caption shown in the UI).  Set `confirmation_policy =
/// "CONFIRM"` to require user approval on every intent-hint execution, or
/// `"BLOCK"` to disable it.
///
/// Note: this `Auto` default is applied at the FIELD level. The enum-level
/// `ConfirmationRequirement::default()` stays `Confirm` (fail-safe) because it
/// is also the `#[serde(default)]` for `ExecutionPolicy.confirmation` — a
/// security gate that must not default open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub custom_presets: Vec<crate::models::intent::WorkflowPreset>,
    #[serde(default = "default_confirmation_policy")]
    pub confirmation_policy: ConfirmationRequirement,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: SandboxConfig::default(),
            custom_presets: Vec::new(),
            confirmation_policy: default_confirmation_policy(),
        }
    }
}

/// D2-② sign-off default for the intent-hint confirmation knob.
fn default_confirmation_policy() -> ConfirmationRequirement {
    ConfirmationRequirement::Auto
}

// ── Private default helpers ─────────────────────────────────────────

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_config_confirmation_policy_serde_round_trip() {
        // AutomationConfig.confirmation_policy must serialise/deserialise using
        // SCREAMING_SNAKE_CASE tokens (inherited from ConfirmationRequirement serde attr).
        let config = AutomationConfig {
            enabled: false,
            confirmation_policy: ConfirmationRequirement::Confirm,
            ..AutomationConfig::default()
        };
        let json = serde_json::to_string(&config).expect("must serialise");
        assert!(
            json.contains("\"CONFIRM\""),
            "confirmation_policy must serialise as SCREAMING_SNAKE_CASE; got: {json}"
        );
        let restored: AutomationConfig = serde_json::from_str(&json).expect("must deserialise");
        assert_eq!(
            restored.confirmation_policy,
            ConfirmationRequirement::Confirm
        );
    }

    #[test]
    fn automation_config_default_confirmation_policy_is_auto() {
        // Default is Auto (D2-② product sign-off: immediate-run under strict sandbox).
        // F-RC-C24-02: users opt into Confirm/Block via config.
        assert_eq!(
            AutomationConfig::default().confirmation_policy,
            ConfirmationRequirement::Auto
        );
    }
}
