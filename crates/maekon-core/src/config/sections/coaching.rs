use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::super::enums::{CoachingTone, DataLookback, OverlayMode};

/// Coaching engine configuration.
///
/// All fields use `#[serde(default)]` for backward-compatible deserialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoachingConfig {
    /// Master switch. Default: false (opt-in only).
    #[serde(default)]
    pub enabled: bool,

    /// Per-profile configuration.
    #[serde(default = "default_profile_configs")]
    pub profiles: HashMap<String, ProfileConfig>,

    /// Lookback window for historical comparisons.
    #[serde(default)]
    pub data_lookback: DataLookback,

    /// Message tone preference.
    #[serde(default)]
    pub tone: CoachingTone,

    /// Manual quiet hours (coaching suppressed during these ranges).
    #[serde(default)]
    pub quiet_hours: Vec<TimeRange>,

    /// Per-regime daily time goals (regime_label -> target minutes).
    #[serde(default)]
    pub regime_goals: HashMap<String, u32>,

    /// Locale for coaching messages ("en", "ko"). Default: "en".
    #[serde(default = "default_locale")]
    pub locale: String,

    /// Overlay display mode (Phase 2 — stored for forward compatibility).
    #[serde(default)]
    pub overlay_mode: OverlayMode,

    /// Overlay toggle hotkey (Phase 2 — stored for forward compatibility).
    #[serde(default = "default_overlay_hotkey")]
    pub overlay_hotkey: String,
}

impl Default for CoachingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            profiles: default_profile_configs(),
            data_lookback: DataLookback::default(),
            tone: CoachingTone::default(),
            quiet_hours: vec![],
            regime_goals: HashMap::new(),
            locale: default_locale(),
            overlay_mode: OverlayMode::default(),
            overlay_hotkey: default_overlay_hotkey(),
        }
    }
}

fn default_locale() -> String {
    "en".to_string()
}

fn default_overlay_hotkey() -> String {
    if cfg!(target_os = "macos") {
        "Cmd+Shift+O".to_string()
    } else {
        "Ctrl+Shift+O".to_string()
    }
}

fn default_profile_configs() -> HashMap<String, ProfileConfig> {
    let mut profiles = HashMap::new();
    for name in [
        "FocusGuard",
        "TimeAware",
        "DeepWorkCoach",
        "ContextRestore",
        "GoalTracker",
    ] {
        profiles.insert(name.to_string(), ProfileConfig::default());
    }
    profiles
}

/// Per-profile settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    /// Whether this profile is active.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Minimum seconds between alerts for this profile.
    #[serde(default = "default_min_interval_secs")]
    pub min_interval_secs: u64,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_interval_secs: 300,
        }
    }
}

fn default_true() -> bool {
    true
}
fn default_min_interval_secs() -> u64 {
    300
}

/// Time range for quiet hours.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    /// Start time in "HH:MM" format.
    pub start: String,
    /// End time in "HH:MM" format.
    pub end: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coaching_config_default_disabled() {
        let config = CoachingConfig::default();
        assert!(
            !config.enabled,
            "coaching must be opt-in (default disabled)"
        );
    }

    #[test]
    fn coaching_config_has_five_profiles() {
        let config = CoachingConfig::default();
        assert_eq!(config.profiles.len(), 5);
        assert!(config.profiles.contains_key("FocusGuard"));
        assert!(config.profiles.contains_key("TimeAware"));
        assert!(config.profiles.contains_key("DeepWorkCoach"));
        assert!(config.profiles.contains_key("ContextRestore"));
        assert!(config.profiles.contains_key("GoalTracker"));
    }

    #[test]
    fn coaching_config_serde_roundtrip() {
        let config = CoachingConfig::default();
        let json = serde_json::to_string_pretty(&config).expect("serialize");
        let restored: CoachingConfig = serde_json::from_str(&json).expect("deserialize");

        // Compare semantically (HashMap order is non-deterministic)
        assert_eq!(restored.enabled, config.enabled);
        assert_eq!(restored.profiles.len(), config.profiles.len());
        assert_eq!(restored.quiet_hours.len(), config.quiet_hours.len());
        assert_eq!(restored.regime_goals.len(), config.regime_goals.len());

        // Verify round-trip preserves serde_json::Value equality
        let val1: serde_json::Value = serde_json::from_str(&json).expect("parse original");
        let json2 = serde_json::to_string_pretty(&restored).expect("re-serialize");
        let val2: serde_json::Value = serde_json::from_str(&json2).expect("parse restored");
        assert_eq!(val1, val2, "JSON value must be identical after round-trip");
    }

    #[test]
    fn coaching_config_unknown_fields_ignored() {
        let json = r#"{
            "enabled": true,
            "tone": "Direct",
            "unknown_future_field": 42,
            "another_field": { "nested": true }
        }"#;
        let config: CoachingConfig =
            serde_json::from_str(json).expect("unknown fields must be ignored");
        assert!(config.enabled);
    }

    #[test]
    fn coaching_config_locale_defaults_to_en() {
        let config = CoachingConfig::default();
        assert_eq!(config.locale, "en", "locale must default to 'en'");
    }

    #[test]
    fn coaching_config_locale_deserializes_from_json() {
        let json = r#"{ "enabled": true, "locale": "ko" }"#;
        let config: CoachingConfig = serde_json::from_str(json).expect("locale must deserialize");
        assert_eq!(config.locale, "ko");
    }

    #[test]
    fn coaching_config_locale_defaults_when_missing_in_json() {
        let json = r#"{ "enabled": true }"#;
        let config: CoachingConfig =
            serde_json::from_str(json).expect("missing locale must use default");
        assert_eq!(config.locale, "en");
    }

    #[test]
    fn profile_config_default_values() {
        let config = ProfileConfig::default();
        assert!(config.enabled, "profiles default to enabled");
        assert_eq!(config.min_interval_secs, 300);
    }
}
