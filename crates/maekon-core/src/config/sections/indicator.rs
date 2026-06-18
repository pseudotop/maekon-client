use serde::{Deserialize, Serialize};

/// Tracking overlay indicator preferences — border highlight and floating panel.
///
/// Struct-level `#[serde(default)]`: a partial `indicator` section (object present
/// but missing one or more fields) deserializes by falling back to the manual
/// `Default` impl for the absent fields, instead of failing the whole-config load
/// (which would trigger a full privacy-reset fallback). See finding
/// `indicator-autostart-serde`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndicatorConfig {
    pub show_border: bool,
    pub show_panel: bool,
    pub border_opacity: f32,
}

impl Default for IndicatorConfig {
    fn default() -> Self {
        Self {
            show_border: true,
            show_panel: true,
            border_opacity: 0.6,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn partial_section_deserializes_with_defaults_for_missing_fields() {
        // Only `show_border` is provided; the remaining fields must fall back
        // to the manual Default impl instead of failing deserialization.
        let parsed: IndicatorConfig = serde_json::from_str(r#"{"show_border":false}"#)
            .expect("partial indicator section must deserialize");

        let defaults = IndicatorConfig::default();
        assert!(!parsed.show_border, "explicitly-set field is honored");
        assert_eq!(parsed.show_panel, defaults.show_panel);
        assert_eq!(parsed.border_opacity, defaults.border_opacity);
    }

    #[test]
    fn empty_section_deserializes_to_full_default() {
        let parsed: IndicatorConfig =
            serde_json::from_str(r#"{}"#).expect("empty indicator section must deserialize");
        let defaults = IndicatorConfig::default();
        assert_eq!(parsed.show_border, defaults.show_border);
        assert_eq!(parsed.show_panel, defaults.show_panel);
        assert_eq!(parsed.border_opacity, defaults.border_opacity);
    }

    #[test]
    fn partial_indicator_section_in_app_config_does_not_fail_whole_load() {
        // Regression: a partial `indicator` section embedded in an otherwise-valid
        // config file must NOT fail the WHOLE-config load (which would trigger a
        // full privacy-reset fallback). See finding `indicator-autostart-serde`.
        //
        // Start from a full default config, then drop two of the three indicator
        // fields, keeping only an explicitly-set `show_border` — this is exactly
        // the "object present, fields missing" partial-section shape.
        let mut value =
            serde_json::to_value(AppConfig::default_config()).expect("default config serializes");
        value["indicator"] = serde_json::json!({ "show_border": false });

        let config: AppConfig =
            serde_json::from_value(value).expect("partial indicator section must not fail load");

        let defaults = IndicatorConfig::default();
        assert!(
            !config.indicator.show_border,
            "explicitly-set field honored"
        );
        assert_eq!(config.indicator.show_panel, defaults.show_panel);
        assert_eq!(config.indicator.border_opacity, defaults.border_opacity);
    }
}
