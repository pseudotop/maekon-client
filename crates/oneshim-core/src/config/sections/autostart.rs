//! Autostart-related configuration.
//!
//! Per Phase 1 review I4: removed `enabled` cache field. OS state is sole source
//! of truth (via `src-tauri/src/autostart.rs`). This struct stores ONLY
//! onboarding-related state.
//!
//! See spec: docs/superpowers/specs/2026-04-25-phase9-pr-b-autostart-ipc-foundation-design.md §5.3

use serde::{Deserialize, Serialize};

/// Per-user autostart configuration.
///
/// IMPORTANT: Does NOT store the autostart enabled/disabled state. That state
/// lives in OS-native locations. Use `autostart::is_autostart_enabled()` to query.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
}
