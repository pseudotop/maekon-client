//! Autostart-related configuration.
//!
//! Per Phase 1 review I4: removed `enabled` cache field. OS state is sole source
//! of truth (via `src-tauri/src/autostart.rs`). This struct stores ONLY
//! onboarding-related state.
//!
//! Public behavior: docs/guides/autostart.md.

use serde::{Deserialize, Serialize};

/// Per-user autostart configuration.
///
/// IMPORTANT: Does NOT store the autostart enabled/disabled state. That state
/// lives in OS-native locations. Use `autostart::is_autostart_enabled()` to query.
///
/// Struct-level `#[serde(default)]`: a partial `autostart` section (object present
/// but missing one or more fields) deserializes by falling back to the manual
/// `Default` impl for the absent fields, instead of failing the whole-config load
/// (which would trigger a full privacy-reset fallback). See finding
/// `indicator-autostart-serde`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AutostartConfig {
    /// State machine for one-time onboarding prompt.
    pub prompt_state: AutostartPromptState,

    /// Monotonic counter of completed productive sessions (≥25 min focus blocks).
    /// Incremented by scheduler in monitor.rs (NOT by frontend round-trip).
    pub productive_session_count: u32,

    /// Last observed productive session UUID — provides idempotency for
    /// counter increments. Scheduler increments only when current_session_id
    /// differs from last_session_id.
    pub last_session_id: Option<String>,
}

/// State machine for the onboarding prompt.
///
/// Transitions:
/// - Pending → Dismissed (user clicks Enable or DontAsk)
/// - Pending → Snoozed (user clicks NotNow)
/// - Snoozed → Dismissed (user clicks Enable or DontAsk on re-prompt)
/// - Snoozed → Snoozed (user clicks NotNow on re-prompt; updates remind_after)
/// - Dismissed → Dismissed (terminal — no further prompts)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AutostartPromptState {
    /// Never prompted. Show prompt when productive_session_count >= 1.
    #[default]
    Pending,

    /// "Not now" — re-prompt when productive_session_count >= remind_after_session_count.
    Snoozed { remind_after_session_count: u32 },

    /// "Don't ask again" or already enabled — never prompt.
    Dismissed,
}

impl std::fmt::Display for AutostartPromptState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => f.write_str("pending"),
            Self::Snoozed { .. } => f.write_str("snoozed"),
            Self::Dismissed => f.write_str("dismissed"),
        }
    }
}

impl Default for AutostartConfig {
    fn default() -> Self {
        Self {
            prompt_state: AutostartPromptState::Pending,
            productive_session_count: 0,
            last_session_id: None,
        }
    }
}

/// Eligibility helper — pure function. Used by scheduler to decide when to
/// emit `autostart:eligible-for-prompt` Tauri event for frontend.
pub fn should_prompt(config: &AutostartConfig) -> bool {
    match &config.prompt_state {
        AutostartPromptState::Dismissed => false,
        AutostartPromptState::Pending => config.productive_session_count >= 1,
        AutostartPromptState::Snoozed {
            remind_after_session_count,
        } => config.productive_session_count >= *remind_after_session_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_pending_with_zero_count() {
        let config = AutostartConfig::default();
        assert_eq!(config.prompt_state, AutostartPromptState::Pending);
        assert_eq!(config.productive_session_count, 0);
        assert!(config.last_session_id.is_none());
    }

    #[test]
    fn prompt_state_pending_serde_roundtrip() {
        let state = AutostartPromptState::Pending;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#"{"kind":"pending"}"#);
        let parsed: AutostartPromptState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn prompt_state_snoozed_serde_roundtrip() {
        let state = AutostartPromptState::Snoozed {
            remind_after_session_count: 6,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#"{"kind":"snoozed","remind_after_session_count":6}"#);
        let parsed: AutostartPromptState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn prompt_state_dismissed_serde_roundtrip() {
        let state = AutostartPromptState::Dismissed;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, r#"{"kind":"dismissed"}"#);
        let parsed: AutostartPromptState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn should_prompt_pending_with_zero_count_returns_false() {
        let config = AutostartConfig {
            prompt_state: AutostartPromptState::Pending,
            productive_session_count: 0,
            ..Default::default()
        };
        assert!(!should_prompt(&config));
    }

    #[test]
    fn should_prompt_pending_with_one_count_returns_true() {
        let config = AutostartConfig {
            prompt_state: AutostartPromptState::Pending,
            productive_session_count: 1,
            ..Default::default()
        };
        assert!(should_prompt(&config));
    }

    #[test]
    fn should_prompt_snoozed_below_threshold_returns_false() {
        let config = AutostartConfig {
            prompt_state: AutostartPromptState::Snoozed {
                remind_after_session_count: 5,
            },
            productive_session_count: 4,
            ..Default::default()
        };
        assert!(!should_prompt(&config));
    }

    #[test]
    fn should_prompt_snoozed_at_threshold_returns_true() {
        let config = AutostartConfig {
            prompt_state: AutostartPromptState::Snoozed {
                remind_after_session_count: 5,
            },
            productive_session_count: 5,
            ..Default::default()
        };
        assert!(should_prompt(&config));
    }

    #[test]
    fn should_prompt_dismissed_always_false_regardless_of_count() {
        let config = AutostartConfig {
            prompt_state: AutostartPromptState::Dismissed,
            productive_session_count: 1000,
            ..Default::default()
        };
        assert!(!should_prompt(&config));
    }

    #[test]
    fn migration_from_old_config_uses_default() {
        // Simulate deserialization from old config without `autostart` field
        let parsed: AutostartConfig = serde_json::from_str(r#"{}"#).unwrap_or_default();
        assert_eq!(parsed, AutostartConfig::default());
    }

    #[test]
    fn partial_section_deserializes_with_defaults_for_missing_fields() {
        // Only `productive_session_count` is provided; the remaining fields must
        // fall back to the manual Default impl instead of failing deserialization.
        let parsed: AutostartConfig = serde_json::from_str(r#"{"productive_session_count":3}"#)
            .expect("partial autostart section must deserialize");

        let defaults = AutostartConfig::default();
        assert_eq!(parsed.productive_session_count, 3, "explicit field honored");
        assert_eq!(parsed.prompt_state, defaults.prompt_state);
        assert_eq!(parsed.last_session_id, defaults.last_session_id);
    }

    #[test]
    fn partial_autostart_section_in_app_config_does_not_fail_whole_load() {
        // Regression: a partial `autostart` section embedded in an otherwise-valid
        // config file must NOT fail the WHOLE-config load (which would trigger a
        // full privacy-reset fallback). See finding `indicator-autostart-serde`.
        //
        // Start from a full default config, then drop two of the three autostart
        // fields, keeping only an explicitly-set `productive_session_count` — this
        // is exactly the "object present, fields missing" partial-section shape.
        use crate::config::AppConfig;

        let mut value =
            serde_json::to_value(AppConfig::default_config()).expect("default config serializes");
        value["autostart"] = serde_json::json!({ "productive_session_count": 3 });

        let config: AppConfig =
            serde_json::from_value(value).expect("partial autostart section must not fail load");

        let defaults = AutostartConfig::default();
        assert_eq!(
            config.autostart.productive_session_count, 3,
            "explicit field honored"
        );
        assert_eq!(config.autostart.prompt_state, defaults.prompt_state);
        assert_eq!(config.autostart.last_session_id, defaults.last_session_id);
    }

    #[test]
    fn autostart_prompt_state_display_matches_serde_kind_tag() {
        assert_eq!(AutostartPromptState::Pending.to_string(), "pending");
        assert_eq!(
            AutostartPromptState::Snoozed {
                remind_after_session_count: 3
            }
            .to_string(),
            "snoozed"
        );
        assert_eq!(AutostartPromptState::Dismissed.to_string(), "dismissed");
    }
}
