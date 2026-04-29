use oneshim_core::models::suggestion::{FeedbackType, Suggestion};
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub suggestion: Suggestion,
    pub feedback: Option<FeedbackType>,
}

pub struct SuggestionHistory {
    entries: VecDeque<HistoryEntry>,
    max_size: usize,
}

impl SuggestionHistory {
    pub fn new(max_size: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            max_size,
        }
    }

    pub fn add(&mut self, suggestion: Suggestion) {
        if self.entries.len() >= self.max_size {
            self.entries.pop_front();
        }
        self.entries.push_back(HistoryEntry {
            suggestion,
            feedback: None,
        });
    }

    pub fn record_feedback(&mut self, suggestion_id: &str, feedback: FeedbackType) -> bool {
        for entry in self.entries.iter_mut().rev() {
            if entry.suggestion.suggestion_id == suggestion_id {
                entry.feedback = Some(feedback);
                return true;
            }
        }
        false
    }

    pub fn recent(&self, limit: usize) -> Vec<&HistoryEntry> {
        self.entries.iter().rev().take(limit).collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn stats(&self) -> HistoryStats {
        let mut accepted = 0u32;
        let mut rejected = 0u32;
        let mut deferred = 0u32;
        let mut pending = 0u32;

        // Track counts per suggestion type
        let mut type_counts: HashMap<String, u32> = HashMap::new();
        // Track (total, accepted) per source
        let mut source_totals: HashMap<String, u32> = HashMap::new();
        let mut source_accepted: HashMap<String, u32> = HashMap::new();

        for entry in &self.entries {
            match &entry.feedback {
                Some(FeedbackType::Accepted) => accepted += 1,
                Some(FeedbackType::Rejected) => rejected += 1,
                Some(FeedbackType::Deferred) => deferred += 1,
                None => pending += 1,
            }

            let type_key = format!("{:?}", entry.suggestion.suggestion_type);
            *type_counts.entry(type_key).or_insert(0) += 1;

            let source_key = format!("{:?}", entry.suggestion.source);
            *source_totals.entry(source_key.clone()).or_insert(0) += 1;
            if matches!(&entry.feedback, Some(FeedbackType::Accepted)) {
                *source_accepted.entry(source_key).or_insert(0) += 1;
            }
        }

        // Sort type counts descending by count
        let mut by_type: Vec<(String, u32)> = type_counts.into_iter().collect();
        by_type.sort_by_key(|e| std::cmp::Reverse(e.1));

        // Build source stats with acceptance rate
        let mut by_source: Vec<(String, u32, f64)> = source_totals
            .into_iter()
            .map(|(source, total)| {
                let acc = *source_accepted.get(&source).unwrap_or(&0);
                let rate = if total > 0 {
                    (acc as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                (source, total, (rate * 10.0).round() / 10.0)
            })
            .collect();
        by_source.sort_by_key(|e| std::cmp::Reverse(e.1));

        HistoryStats {
            total: self.entries.len() as u32,
            accepted,
            rejected,
            deferred,
            pending,
            by_type,
            by_source,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryStats {
    pub total: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub deferred: u32,
    pub pending: u32,
    /// (type_name, count) sorted descending by count
    pub by_type: Vec<(String, u32)>,
    /// (source_name, total, acceptance_rate_percent) sorted descending by total
    pub by_source: Vec<(String, u32, f64)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::models::suggestion::{Priority, SuggestionType};

    fn make_suggestion(id: &str) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: format!("suggestion {id}"),
            priority: Priority::Medium,
            confidence_score: 0.9,
            relevance_score: 0.8,
            is_actionable: true,
            created_at: Utc::now(),
            expires_at: None,
            source: Default::default(),
            reasoning: None,
        }
    }

    #[test]
    fn add_and_recent() {
        let mut history = SuggestionHistory::new(100);
        history.add(make_suggestion("1"));
        history.add(make_suggestion("2"));
        history.add(make_suggestion("3"));

        let recent = history.recent(2);
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].suggestion.suggestion_id, "3");
        assert_eq!(recent[1].suggestion.suggestion_id, "2");
    }

    #[test]
    fn max_size_eviction() {
        let mut history = SuggestionHistory::new(2);
        history.add(make_suggestion("1"));
        history.add(make_suggestion("2"));
        history.add(make_suggestion("3"));

        assert_eq!(history.len(), 2);
        let recent = history.recent(10);
        assert_eq!(recent[0].suggestion.suggestion_id, "3");
        assert_eq!(recent[1].suggestion.suggestion_id, "2");
    }

    #[test]
    fn record_feedback() {
        let mut history = SuggestionHistory::new(100);
        history.add(make_suggestion("1"));
        history.add(make_suggestion("2"));

        assert!(history.record_feedback("1", FeedbackType::Accepted));
        assert!(history.record_feedback("2", FeedbackType::Rejected));
        assert!(!history.record_feedback("999", FeedbackType::Deferred));

        let stats = history.stats();
        assert_eq!(stats.accepted, 1);
        assert_eq!(stats.rejected, 1);
        assert_eq!(stats.pending, 0);
    }

    #[test]
    fn stats() {
        let mut history = SuggestionHistory::new(100);
        history.add(make_suggestion("1"));
        history.add(make_suggestion("2"));
        history.add(make_suggestion("3"));

        history.record_feedback("1", FeedbackType::Accepted);

        let stats = history.stats();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.accepted, 1);
        assert_eq!(stats.pending, 2);
    }
}
