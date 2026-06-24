// Privacy/isolation config — PII filter level, automation sandbox, excluded-app list
use super::super::enums::{ConfirmationRequirement, PiiFilterLevel, SandboxProfile};
use serde::{Deserialize, Serialize};

/// Safe-by-default confidence floor for LLM-planned automation intents.
///
/// `0.0` remains a supported explicit opt-out value, but fresh installs and missing
/// config fields should reject low-confidence interpretations before auto-execution.
pub const DEFAULT_MIN_LLM_CONFIDENCE: f64 = 0.65;

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
    /// #6333 A10: minimum LLM self-reported interpretation confidence (0.0-1.0)
    /// required to auto-execute an LLM-planned intent. Below this floor the plan is
    /// rejected rather than executed, so a hallucinated/prompt-injected low-confidence
    /// interpretation cannot auto-run. Set 0.0 explicitly to disable this gate.
    #[serde(default = "default_min_llm_confidence")]
    pub min_llm_confidence: f64,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: SandboxConfig::default(),
            custom_presets: Vec::new(),
            confirmation_policy: default_confirmation_policy(),
            min_llm_confidence: default_min_llm_confidence(),
        }
    }
}

/// D2-② sign-off default for the intent-hint confirmation knob.
fn default_confirmation_policy() -> ConfirmationRequirement {
    ConfirmationRequirement::Auto
}

/// #6333 A10 / E42.2: safe default for LLM-planned automation intents.
fn default_min_llm_confidence() -> f64 {
    DEFAULT_MIN_LLM_CONFIDENCE
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

    #[test]
    fn automation_config_default_min_llm_confidence_is_safe_floor() {
        assert!(
            AutomationConfig::default().min_llm_confidence >= 0.6,
            "fresh installs must reject low-confidence LLM-planned intents by default"
        );
    }

    #[test]
    fn automation_config_serde_default_min_llm_confidence_is_safe_floor() {
        let restored: AutomationConfig = serde_json::from_str("{}").expect("must deserialise");
        assert!(
            restored.min_llm_confidence >= 0.6,
            "missing config field must deserialize to the safe LLM confidence floor"
        );
    }
}
