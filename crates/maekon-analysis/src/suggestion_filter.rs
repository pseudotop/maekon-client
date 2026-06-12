//! Regime-aware suggestion filtering.
//!
//! Filters suggestions based on the current activity regime so users in
//! deep-focus mode are not disturbed by low-priority items.

use maekon_core::models::suggestion::{Priority, Suggestion};
use maekon_core::models::tiered_memory::Regime;

/// Filter suggestions in-place based on the current activity regime.
///
/// - **Deep Focus**: retain only High and Critical suggestions.
/// - **Communication / Research / Mixed / unknown**: no filtering.
/// - **No regime**: no filtering (fallback to defaults).
pub fn filter_by_regime(suggestions: &mut Vec<Suggestion>, regime: Option<&Regime>) {
    let Some(regime) = regime else { return };

    // #4818 review: check BOTH the user-facing name AND the auto_label — a regime
    // whose `name` was set to something else (e.g. "Deep Work") must still gate if
    // its `auto_label` classifies it as Deep Focus. Keying off `name` alone would
    // silently no-op the gate for any named/renamed focus regime.
    let in_deep_focus = regime
        .name
        .as_deref()
        .is_some_and(|name| name.starts_with("Deep Focus"))
        || regime.auto_label.starts_with("Deep Focus");

    if in_deep_focus {
        suggestions.retain(|s| s.priority >= Priority::High);
    }
    // Communication, Research, Mixed: no filtering
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::id_generation::generate_id;
    use maekon_core::models::suggestion::{SuggestionSource, SuggestionType};
    use maekon_core::models::tiered_memory::{RegimeFeatures, RegimeStatus, TriggerParams};

    fn make_suggestion(priority: Priority) -> Suggestion {
        Suggestion {
            suggestion_id: generate_id("sug"),
            suggestion_type: SuggestionType::ProductivityTip,
            content: "Test suggestion".to_string(),
            priority,
            confidence_score: 0.8,
            relevance_score: 0.7,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: SuggestionSource::RuleBased,
            reasoning: None,
            context_scope: None,
        }
    }

    fn make_regime(label: &str) -> Regime {
        Regime {
            regime_id: "r-test".to_string(),
            name: None,
            auto_label: label.to_string(),
            centroid: RegimeFeatures::default(),
            optimal_params: TriggerParams::default(),
            sample_count: 100,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status: RegimeStatus::Active,
        }
    }

    #[test]
    fn deep_focus_filters_low_priority() {
        let regime = make_regime("Deep Focus (VSCode)");
        let mut suggestions = vec![
            make_suggestion(Priority::Low),
            make_suggestion(Priority::Medium),
            make_suggestion(Priority::High),
            make_suggestion(Priority::Critical),
        ];

        filter_by_regime(&mut suggestions, Some(&regime));

        assert_eq!(suggestions.len(), 2);
        assert!(suggestions.iter().all(|s| s.priority >= Priority::High));
    }

    #[test]
    fn deep_focus_gates_even_when_name_overrides_auto_label() {
        // #4818 review: a regime whose `name` is NOT "Deep Focus*" but whose
        // `auto_label` IS must still gate — keying off `name` alone would no-op.
        let mut regime = make_regime("Deep Focus (VSCode)");
        regime.name = Some("Deep Work".to_string());
        let mut suggestions = vec![
            make_suggestion(Priority::Low),
            make_suggestion(Priority::High),
        ];

        filter_by_regime(&mut suggestions, Some(&regime));

        assert_eq!(suggestions.len(), 1);
        assert!(suggestions.iter().all(|s| s.priority >= Priority::High));
    }

    #[test]
    fn communication_keeps_all() {
        let regime = make_regime("Communication (Slack)");
        let mut suggestions = vec![
            make_suggestion(Priority::Low),
            make_suggestion(Priority::Medium),
            make_suggestion(Priority::High),
        ];

        filter_by_regime(&mut suggestions, Some(&regime));

        assert_eq!(suggestions.len(), 3);
    }

    #[test]
    fn no_regime_keeps_all() {
        let mut suggestions = vec![
            make_suggestion(Priority::Low),
            make_suggestion(Priority::Medium),
        ];

        filter_by_regime(&mut suggestions, None);

        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn research_keeps_all() {
        let regime = make_regime("Research (Chrome)");
        let mut suggestions = vec![
            make_suggestion(Priority::Low),
            make_suggestion(Priority::High),
        ];

        filter_by_regime(&mut suggestions, Some(&regime));

        assert_eq!(suggestions.len(), 2);
    }

    #[test]
    fn mixed_keeps_all() {
        let regime = make_regime("Mixed (varied)");
        let mut suggestions = vec![
            make_suggestion(Priority::Low),
            make_suggestion(Priority::Medium),
            make_suggestion(Priority::Critical),
        ];

        filter_by_regime(&mut suggestions, Some(&regime));

        assert_eq!(suggestions.len(), 3);
    }

    #[test]
    fn deep_focus_all_low_empties() {
        let regime = make_regime("Deep Focus (IntelliJ)");
        let mut suggestions = vec![
            make_suggestion(Priority::Low),
            make_suggestion(Priority::Low),
            make_suggestion(Priority::Medium),
        ];

        filter_by_regime(&mut suggestions, Some(&regime));

        assert!(suggestions.is_empty());
    }
}
