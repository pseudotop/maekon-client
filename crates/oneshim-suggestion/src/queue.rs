use oneshim_core::models::suggestion::Suggestion;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::collections::HashSet;

#[derive(Debug, Clone)]
struct PrioritizedSuggestion {
    suggestion: Suggestion,
}

impl PartialEq for PrioritizedSuggestion {
    fn eq(&self, other: &Self) -> bool {
        self.suggestion.suggestion_id == other.suggestion.suggestion_id
    }
}

impl Eq for PrioritizedSuggestion {}

impl PartialOrd for PrioritizedSuggestion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrioritizedSuggestion {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .suggestion
            .priority
            .cmp(&self.suggestion.priority)
            .then_with(|| other.suggestion.created_at.cmp(&self.suggestion.created_at))
            .then_with(|| {
                self.suggestion
                    .suggestion_id
                    .cmp(&other.suggestion.suggestion_id)
            })
    }
}

fn content_fingerprint(suggestion: &Suggestion) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    suggestion.suggestion_type.hash(&mut hasher);
    let normalized: String = suggestion
        .content
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let truncated = &normalized[..normalized.len().min(200)];
    truncated.hash(&mut hasher);
    hasher.finish()
}

pub struct SuggestionQueue {
    items: BTreeSet<PrioritizedSuggestion>,
    fingerprints: HashSet<u64>,
    max_size: usize,
}

impl SuggestionQueue {
    pub fn new(max_size: usize) -> Self {
        Self {
            items: BTreeSet::new(),
            fingerprints: HashSet::new(),
            max_size,
        }
    }

    pub fn push(&mut self, suggestion: Suggestion) -> bool {
        let fp = content_fingerprint(&suggestion);
        if self.fingerprints.contains(&fp) {
            tracing::debug!(
                rejected_id = %suggestion.suggestion_id,
                "duplicate content fingerprint — rejected"
            );
            return false;
        }

        let item = PrioritizedSuggestion { suggestion };

        if self.items.len() >= self.max_size {
            if let Some(last) = self.items.iter().next_back() {
                if item < *last {
                    let last_clone = last.clone();
                    self.items.remove(&last_clone);
                    self.fingerprints
                        .remove(&content_fingerprint(&last_clone.suggestion));
                    tracing::warn!(
                        evicted_id = %last_clone.suggestion.suggestion_id,
                        evicted_priority = ?last_clone.suggestion.priority,
                        new_id = %item.suggestion.suggestion_id,
                        new_priority = ?item.suggestion.priority,
                        queue_size = self.max_size,
                        "suggestion queue full — evicted lower-priority item"
                    );
                } else {
                    tracing::warn!(
                        rejected_id = %item.suggestion.suggestion_id,
                        rejected_priority = ?item.suggestion.priority,
                        queue_size = self.max_size,
                        "suggestion queue full — rejected (priority too low)"
                    );
                    return false;
                }
            }
        }

        self.fingerprints.insert(fp);
        self.items.insert(item)
    }

    pub fn pop(&mut self) -> Option<Suggestion> {
        let first = self.items.iter().next()?.clone();
        self.items.remove(&first);
        self.fingerprints
            .remove(&content_fingerprint(&first.suggestion));
        Some(first.suggestion)
    }

    pub fn peek(&self) -> Option<&Suggestion> {
        self.items.iter().next().map(|p| &p.suggestion)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Suggestion> {
        self.items.iter().map(|p| &p.suggestion)
    }

    /// Remove a suggestion by its ID. Returns the removed Suggestion if found.
    pub fn remove_by_id(&mut self, suggestion_id: &str) -> Option<Suggestion> {
        let item = self
            .items
            .iter()
            .find(|ps| ps.suggestion.suggestion_id == suggestion_id)
            .cloned();
        if let Some(ref found) = item {
            self.items.remove(found);
            self.fingerprints
                .remove(&content_fingerprint(&found.suggestion));
            Some(found.suggestion.clone())
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.fingerprints.clear();
    }

    pub fn remove_expired(&mut self) -> usize {
        let now = chrono::Utc::now();
        let expired_fps: Vec<u64> = self
            .items
            .iter()
            .filter(|p| {
                p.suggestion
                    .expires_at
                    .is_some_and(|expires| expires <= now)
            })
            .map(|p| content_fingerprint(&p.suggestion))
            .collect();
        for fp in &expired_fps {
            self.fingerprints.remove(fp);
        }
        let before = self.items.len();
        self.items.retain(|p| {
            p.suggestion
                .expires_at
                .map_or(true, |expires| expires > now)
        });
        before - self.items.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::models::suggestion::{Priority, SuggestionType};

    fn make_suggestion(id: &str, priority: Priority) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: format!("suggestion {id}"),
            priority,
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
    fn priority_ordering() {
        let mut queue = SuggestionQueue::new(50);
        queue.push(make_suggestion("low", Priority::Low));
        queue.push(make_suggestion("critical", Priority::Critical));
        queue.push(make_suggestion("medium", Priority::Medium));
        queue.push(make_suggestion("high", Priority::High));

        assert_eq!(queue.pop().unwrap().suggestion_id, "critical");
        assert_eq!(queue.pop().unwrap().suggestion_id, "high");
        assert_eq!(queue.pop().unwrap().suggestion_id, "medium");
        assert_eq!(queue.pop().unwrap().suggestion_id, "low");
    }

    #[test]
    fn max_size_enforcement() {
        let mut queue = SuggestionQueue::new(2);
        queue.push(make_suggestion("1", Priority::Low));
        queue.push(make_suggestion("2", Priority::Medium));
        assert_eq!(queue.len(), 2);

        queue.push(make_suggestion("3", Priority::High));
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.peek().unwrap().suggestion_id, "3");
    }

    #[test]
    fn empty_queue() {
        let mut queue = SuggestionQueue::new(50);
        assert!(queue.is_empty());
        assert!(queue.pop().is_none());
        assert!(queue.peek().is_none());
    }

    #[test]
    fn clear_queue() {
        let mut queue = SuggestionQueue::new(50);
        queue.push(make_suggestion("1", Priority::High));
        queue.push(make_suggestion("2", Priority::Medium));
        assert_eq!(queue.len(), 2);
        queue.clear();
        assert!(queue.is_empty());
    }

    #[test]
    fn remove_by_id_returns_and_removes() {
        let mut queue = SuggestionQueue::new(50);
        let s = make_suggestion("s1", Priority::High);
        queue.push(s);
        assert_eq!(queue.len(), 1);
        let removed = queue.remove_by_id("s1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().suggestion_id, "s1");
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn remove_by_id_returns_none_if_not_found() {
        let mut queue = SuggestionQueue::new(50);
        assert!(queue.remove_by_id("nonexistent").is_none());
    }

    #[test]
    fn remove_expired() {
        let mut queue = SuggestionQueue::new(50);

        let mut expired = make_suggestion("expired", Priority::High);
        expired.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        queue.push(expired);

        let valid = make_suggestion("valid", Priority::Medium);
        queue.push(valid);

        let removed = queue.remove_expired();
        assert_eq!(removed, 1);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.peek().unwrap().suggestion_id, "valid");
    }

    #[test]
    fn duplicate_content_rejected() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let mut s2 = make_suggestion("s2", Priority::Critical);
        s2.content = s1.content.clone();
        s2.suggestion_type = s1.suggestion_type.clone();
        assert!(queue.push(s1));
        assert!(!queue.push(s2));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn different_content_accepted() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = "different content".to_string();
        assert!(queue.push(s1));
        assert!(queue.push(s2));
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn fingerprint_removed_on_pop() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.pop();
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }

    #[test]
    fn fingerprint_removed_on_remove_by_id() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.remove_by_id("s1");
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }

    #[test]
    fn fingerprint_cleared_on_clear() {
        let mut queue = SuggestionQueue::new(50);
        let s1 = make_suggestion("s1", Priority::High);
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.clear();
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }

    #[test]
    fn fingerprint_removed_on_expired() {
        let mut queue = SuggestionQueue::new(50);
        let mut s1 = make_suggestion("s1", Priority::High);
        s1.expires_at = Some(Utc::now() - chrono::Duration::hours(1));
        let content = s1.content.clone();
        let stype = s1.suggestion_type.clone();
        queue.push(s1);
        queue.remove_expired();
        // Same content can re-enter after expiry removes the fingerprint
        let mut s2 = make_suggestion("s2", Priority::High);
        s2.content = content;
        s2.suggestion_type = stype;
        assert!(queue.push(s2));
    }
}
