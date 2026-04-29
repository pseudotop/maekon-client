//! Regime-aware suggestion filtering.
//!
//! Filters suggestions based on the current activity regime so users in
//! deep-focus mode are not disturbed by low-priority items.

use oneshim_core::models::suggestion::{Priority, Suggestion};
use oneshim_core::models::tiered_memory::Regime;

/// Filter suggestions in-place based on the current activity regime.
///
/// - **Deep Focus**: retain only High and Critical suggestions.
/// - **Communication / Research / Mixed / unknown**: no filtering.
/// - **No regime**: no filtering (fallback to defaults).
pub fn filter_by_regime(suggestions: &mut Vec<Suggestion>, regime: Option<&Regime>) {
    let Some(regime) = regime else { return };

    let label = regime.name.as_deref().unwrap_or(&regime.auto_label);

    if label.starts_with("Deep Focus") {
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
    use oneshim_core::models::suggestion::{SuggestionSource, SuggestionType};
    use oneshim_core::models::tiered_memory::{RegimeFeatures, RegimeStatus, TriggerParams};

    fn make_suggestion(priority: Priority) -> Suggestion {
        Suggestion {
            suggestion_id: uuid::Uuid::new_v4().to_string(),
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
