use crate::assembler::PiiFilter;
use crate::prompts::{FewShotExample, FewShotOutcome};
use oneshim_core::models::suggestion::SuggestionHistoryEntry;

/// Selects representative few-shot examples from suggestion history for prompt construction.
pub struct FewShotSelector {
    max_examples: usize,
    pii_filter: Option<PiiFilter>,
}

impl FewShotSelector {
    pub fn new(max_examples: usize) -> Self {
        Self {
            max_examples,
            pii_filter: None,
        }
    }

    /// Create a selector with a PII filter applied to context_window text in few-shot examples.
    pub fn with_pii_filter(max_examples: usize, pii_filter: PiiFilter) -> Self {
        Self {
            max_examples,
            pii_filter: Some(pii_filter),
        }
    }

    /// Select best examples from history. Returns empty Vec if no feedback exists.
    ///
    /// Prefers regime-matched entries when at least 2 candidates match the current regime.
    /// Falls back to the full history otherwise.
    pub fn select(
        &self,
        history: &[SuggestionHistoryEntry],
        current_regime: Option<&str>,
    ) -> Vec<FewShotExample> {
        if history.is_empty() {
            return vec![];
        }

        // Soft regime filter: use regime-matched subset only when there are enough entries.
        let regime_matches: Vec<_> = history
            .iter()
            .filter(|h| current_regime.is_none() || h.regime_label.as_deref() == current_regime)
            .collect();
        let candidates: Vec<&SuggestionHistoryEntry> = if regime_matches.len() >= 2 {
            regime_matches
        } else {
            history.iter().collect()
        };

        let accepted: Vec<_> = candidates
            .iter()
            .filter(|h| h.feedback_type == "accepted")
            .collect();
        let rejected: Vec<_> = candidates
            .iter()
            .filter(|h| h.feedback_type == "rejected")
            .collect();

        let mut selected = Vec::new();

        // Always prefer at least one accepted example first.
        if let Some(entry) = accepted.first() {
            selected.push(self.to_example(entry, FewShotOutcome::Accepted));
        }

        // Add one rejected example if budget remains.
        if selected.len() < self.max_examples {
            if let Some(entry) = rejected.first() {
                selected.push(self.to_example(entry, FewShotOutcome::Rejected));
            }
        }

        // Fill remaining budget with additional accepted examples.
        for entry in accepted.iter().skip(1) {
            if selected.len() >= self.max_examples {
                break;
            }
            selected.push(self.to_example(entry, FewShotOutcome::Accepted));
        }

        selected
    }

    fn to_example(
        &self,
        entry: &SuggestionHistoryEntry,
        outcome: FewShotOutcome,
    ) -> FewShotExample {
        let window = match &self.pii_filter {
            Some(filter) => filter(&entry.context_window),
            None => entry.context_window.clone(),
        };
        let context_summary = if entry.context_app.is_empty() {
            "Unknown context".to_string()
        } else {
            format!("{} — {}", entry.context_app, window)
        };
        FewShotExample {
            context_summary,
            suggestion_content: entry.content.clone(),
            suggestion_type: entry.suggestion_type.clone(),
            outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_entry(
        feedback_type: &str,
        regime_label: Option<&str>,
        context_app: &str,
        content: &str,
    ) -> SuggestionHistoryEntry {
        SuggestionHistoryEntry {
            suggestion_id: uuid::Uuid::new_v4().to_string(),
            suggestion_type: "ProductivityTip".to_string(),
            content: content.to_string(),
            confidence: 0.8,
            feedback_type: feedback_type.to_string(),
            regime_label: regime_label.map(str::to_string),
            context_app: context_app.to_string(),
            context_window: "Window".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn empty_history_returns_empty() {
        let selector = FewShotSelector::new(3);
        let result = selector.select(&[], None);
        assert!(result.is_empty());
    }

    #[test]
    fn selects_accepted_and_rejected() {
        let history = vec![
            make_entry("accepted", None, "VSCode", "Take a break"),
            make_entry("rejected", None, "Slack", "Ignore notifications"),
            make_entry("accepted", None, "Terminal", "Commit work"),
        ];
        let selector = FewShotSelector::new(3);
        let result = selector.select(&history, None);

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].outcome, FewShotOutcome::Accepted);
        assert_eq!(result[1].outcome, FewShotOutcome::Rejected);
        assert_eq!(result[2].outcome, FewShotOutcome::Accepted);
    }

    #[test]
    fn regime_filter_prefers_matching() {
        let history = vec![
            make_entry("accepted", Some("deep_focus"), "VSCode", "Focus tip"),
            make_entry(
                "accepted",
                Some("deep_focus"),
                "VSCode",
                "Another focus tip",
            ),
            make_entry("rejected", None, "Slack", "Generic rejected"),
        ];
        let selector = FewShotSelector::new(3);
        // 2 regime matches → uses regime-filtered candidates (no "Generic rejected")
        let result = selector.select(&history, Some("deep_focus"));

        assert!(!result.is_empty());
        let regime_contents = ["Focus tip", "Another focus tip"];
        assert!(result
            .iter()
            .all(|e| regime_contents.contains(&e.suggestion_content.as_str())));
        // Rejected from a different regime should not appear
        assert!(!result
            .iter()
            .any(|e| e.suggestion_content == "Generic rejected"));
    }

    #[test]
    fn regime_filter_relaxes_when_insufficient() {
        let history = vec![
            make_entry("accepted", Some("deep_focus"), "VSCode", "Focus tip"),
            make_entry("accepted", None, "Slack", "Generic accepted"),
            make_entry("rejected", None, "Chrome", "Generic rejected"),
        ];
        let selector = FewShotSelector::new(3);
        // Only 1 regime match → falls back to all history (3 candidates)
        let result = selector.select(&history, Some("deep_focus"));
        assert!(result.len() >= 2);
    }

    #[test]
    fn accepted_only_works() {
        let history = vec![
            make_entry("accepted", None, "VSCode", "Good tip A"),
            make_entry("accepted", None, "VSCode", "Good tip B"),
        ];
        let selector = FewShotSelector::new(3);
        let result = selector.select(&history, None);
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.outcome == FewShotOutcome::Accepted));
    }

    #[test]
    fn pii_filter_applied_to_context_window() {
        let history = vec![make_entry("accepted", None, "Chrome", "Take a break")];
        // Mutate the context_window to contain PII
        let mut history = history;
        history[0].context_window = "Inbox - user@example.com".to_string();

        let filter: Box<dyn Fn(&str) -> String + Send + Sync> =
            Box::new(|text: &str| text.replace("user@example.com", "[EMAIL]"));
        let selector = FewShotSelector::with_pii_filter(3, filter);
        let result = selector.select(&history, None);

        assert_eq!(result.len(), 1);
        assert!(
            result[0].context_summary.contains("[EMAIL]"),
            "PII should be filtered: {}",
            result[0].context_summary
        );
        assert!(
            !result[0].context_summary.contains("user@example.com"),
            "Raw PII should not appear"
        );
    }

    #[test]
    fn limit_respected() {
        let history = vec![
            make_entry("accepted", None, "VSCode", "Tip A"),
            make_entry("rejected", None, "Slack", "Tip B"),
            make_entry("accepted", None, "Terminal", "Tip C"),
        ];
        let selector = FewShotSelector::new(1);
        let result = selector.select(&history, None);
        assert_eq!(result.len(), 1);
    }
}
