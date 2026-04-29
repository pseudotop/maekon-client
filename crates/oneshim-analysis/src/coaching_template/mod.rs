mod templates;

use oneshim_core::config::CoachingTone;
use oneshim_core::models::coaching::{trigger_type_name, CoachingProfile, TriggerType};
pub use oneshim_core::models::coaching_template::CoachingTemplate;
use std::collections::HashMap;

/// Replace `{key}` placeholders with values from the variables map.
fn substitute(template: &str, vars: &HashMap<String, String>) -> String {
    let mut result = template.to_string();
    for (key, value) in vars {
        result = result.replace(&format!("{{{}}}", key), value);
    }
    result
}

/// Registry holding 50+ templates. Selects by profile + trigger + tone
/// with a 3-tier fallback: exact match -> profile+trigger -> profile.
pub struct CoachingTemplateRegistry {
    templates: Vec<CoachingTemplate>,
}

impl CoachingTemplateRegistry {
    pub fn new() -> Self {
        Self {
            templates: templates::TEMPLATES.to_vec(),
        }
    }

    /// Select a template matching the profile, trigger, tone, and locale,
    /// then substitute variables.
    ///
    /// Locale filtering: prefer templates matching `locale`, fall back to
    /// "en" if no match exists for the requested locale.
    ///
    /// Fallback tiers (within locale-filtered set):
    /// 1. Exact match: profile + trigger + tone
    /// 2. Profile + trigger (any tone)
    /// 3. First template for this profile
    pub fn select(
        &self,
        profile: &CoachingProfile,
        trigger: &TriggerType,
        tone: &CoachingTone,
        locale: &str,
        variables: &HashMap<String, String>,
    ) -> String {
        let trigger_name = trigger_type_name(trigger);

        // Filter by locale, fall back to "en" if no match
        let locale_filtered: Vec<_> = self
            .templates
            .iter()
            .filter(|t| t.locale == locale)
            .collect();
        let candidates: Vec<_> = if locale_filtered.is_empty() {
            self.templates.iter().filter(|t| t.locale == "en").collect()
        } else {
            locale_filtered
        };

        // Best match: profile + trigger + tone
        let fallback = &self.templates[0];
        let template = candidates
            .iter()
            .find(|t| t.profile == *profile && t.trigger_type == trigger_name && t.tone == *tone)
            // Fallback: profile + trigger (any tone)
            .or_else(|| {
                candidates
                    .iter()
                    .find(|t| t.profile == *profile && t.trigger_type == trigger_name)
            })
            // Ultimate fallback: first template for this profile
            .or_else(|| candidates.iter().find(|t| t.profile == *profile))
            // Should never happen — we have 50+ templates
            .unwrap_or(&fallback);

        substitute(template.text, variables)
    }

    /// Total number of templates (for metrics/testing).
    pub fn template_count(&self) -> usize {
        self.templates.len()
    }

    /// Return a reference to all templates (for playbook listing).
    pub fn all_templates(&self) -> &[CoachingTemplate] {
        &self.templates
    }
}

impl Default for CoachingTemplateRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_at_least_50_templates() {
        let registry = CoachingTemplateRegistry::new();
        assert!(
            registry.template_count() >= 50,
            "expected >= 50 templates, got {}",
            registry.template_count()
        );
    }

    #[test]
    fn select_exact_match() {
        let registry = CoachingTemplateRegistry::new();
        let mut vars = HashMap::new();
        vars.insert("regime".to_string(), "Deep Work".to_string());
        vars.insert("context_switches".to_string(), "5".to_string());

        let result = registry.select(
            &CoachingProfile::FocusGuard,
            &TriggerType::RegimeTransition {
                from_regime: Some("Deep Work".to_string()),
                to_regime: Some("Communication".to_string()),
            },
            &CoachingTone::Direct,
            "en",
            &vars,
        );

        assert!(
            result.contains("Deep Work"),
            "template should substitute {{regime}}: got '{}'",
            result
        );
        assert!(
            result.contains("5"),
            "template should substitute {{context_switches}}: got '{}'",
            result
        );
        // No unresolved placeholders for the variables we provided
        assert!(
            !result.contains("{regime}"),
            "{{regime}} placeholder should be resolved"
        );
    }

    #[test]
    fn select_fallback_wrong_tone() {
        let registry = CoachingTemplateRegistry::new();
        let vars = HashMap::new();

        // ContextRestore + RegimeDrift has Direct, Gentle, DataDriven.
        // Request a match that falls to the tone-agnostic tier:
        // We verify the fallback returns *something* even if exact tone isn't present
        // by requesting the same profile/trigger with all three tones and confirming each works.
        for tone in [
            CoachingTone::Direct,
            CoachingTone::Gentle,
            CoachingTone::DataDriven,
        ] {
            let result = registry.select(
                &CoachingProfile::ContextRestore,
                &TriggerType::RegimeDrift {
                    regime_label: "Coding".to_string(),
                },
                &tone,
                "en",
                &vars,
            );
            assert!(!result.is_empty(), "fallback must return non-empty text");
        }
    }

    #[test]
    fn substitute_replaces_all_placeholders() {
        let mut vars = HashMap::new();
        vars.insert("regime".to_string(), "Deep Work".to_string());
        vars.insert("duration".to_string(), "2h 15m".to_string());
        vars.insert("app_name".to_string(), "VS Code".to_string());
        vars.insert("context_switches".to_string(), "3".to_string());
        vars.insert("comparison".to_string(), "1h 30m".to_string());
        vars.insert("goal_progress".to_string(), "75".to_string());
        vars.insert("goal_minutes".to_string(), "120".to_string());
        vars.insert("remaining_minutes".to_string(), "30".to_string());
        vars.insert("previous_context".to_string(), "PR review".to_string());

        let template = "Working in {regime} for {duration} on {app_name}. Switches: {context_switches}. Avg: {comparison}. Goal: {goal_progress}% of {goal_minutes}m. Left: {remaining_minutes}m. Prev: {previous_context}.";
        let result = substitute(template, &vars);

        assert!(
            !result.contains('{'),
            "no unresolved placeholders: '{}'",
            result
        );
    }

    #[test]
    fn all_profiles_have_templates() {
        let registry = CoachingTemplateRegistry::new();
        let profiles = [
            CoachingProfile::FocusGuard,
            CoachingProfile::TimeAware,
            CoachingProfile::DeepWorkCoach,
            CoachingProfile::ContextRestore,
            CoachingProfile::GoalTracker,
        ];

        for profile in &profiles {
            let count = registry
                .templates
                .iter()
                .filter(|t| t.profile == *profile)
                .count();
            assert!(
                count >= 1,
                "profile {:?} must have at least 1 template, got {}",
                profile,
                count
            );
        }
    }

    #[test]
    fn select_falls_back_to_en_for_unknown_locale() {
        let registry = CoachingTemplateRegistry::new();
        let vars = HashMap::new();
        // "ja" has no templates, should fall back to English
        let text = registry.select(
            &CoachingProfile::FocusGuard,
            &TriggerType::RegimeTransition {
                from_regime: None,
                to_regime: None,
            },
            &CoachingTone::Direct,
            "ja",
            &vars,
        );
        assert!(
            !text.is_empty(),
            "locale fallback must return non-empty text"
        );
        // Verify it returned English text (not Korean)
        assert!(
            !text.contains('\u{AC00}'),
            "fallback for 'ja' should return English, not Korean"
        );
    }

    #[test]
    fn select_returns_korean_for_ko_locale() {
        let registry = CoachingTemplateRegistry::new();
        let mut vars = HashMap::new();
        vars.insert("regime".to_string(), "Deep Work".to_string());
        vars.insert("context_switches".to_string(), "5".to_string());

        let text = registry.select(
            &CoachingProfile::FocusGuard,
            &TriggerType::RegimeTransition {
                from_regime: None,
                to_regime: None,
            },
            &CoachingTone::Direct,
            "ko",
            &vars,
        );
        assert!(!text.is_empty(), "ko locale must return non-empty text");
        // Korean text should contain Korean characters (Hangul block U+AC00..U+D7AF)
        assert!(
            text.chars().any(|c| ('\u{AC00}'..='\u{D7AF}').contains(&c)),
            "ko locale should return Korean text, got: '{}'",
            text
        );
        // Variables should still be substituted
        assert!(
            text.contains("Deep Work"),
            "Korean template should substitute {{regime}}: got '{}'",
            text
        );
    }

    #[test]
    fn registry_has_54_korean_templates() {
        let registry = CoachingTemplateRegistry::new();
        let ko_count = registry
            .templates
            .iter()
            .filter(|t| t.locale == "ko")
            .count();
        assert_eq!(
            ko_count, 54,
            "expected 54 Korean templates, got {}",
            ko_count
        );
    }

    #[test]
    fn all_templates_have_locale_field() {
        let registry = CoachingTemplateRegistry::new();
        for t in &registry.templates {
            assert!(
                !t.locale.is_empty(),
                "template for {:?}/{} must have a locale",
                t.profile,
                t.trigger_type
            );
        }
    }

    #[test]
    fn all_en_profiles_have_ko_counterparts() {
        let registry = CoachingTemplateRegistry::new();
        let profiles = [
            CoachingProfile::FocusGuard,
            CoachingProfile::TimeAware,
            CoachingProfile::DeepWorkCoach,
            CoachingProfile::ContextRestore,
            CoachingProfile::GoalTracker,
        ];

        for profile in &profiles {
            let en_count = registry
                .templates
                .iter()
                .filter(|t| t.profile == *profile && t.locale == "en")
                .count();
            let ko_count = registry
                .templates
                .iter()
                .filter(|t| t.profile == *profile && t.locale == "ko")
                .count();
            assert_eq!(
                en_count, ko_count,
                "profile {:?}: en has {} templates but ko has {}",
                profile, en_count, ko_count
            );
        }
    }
}
