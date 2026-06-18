use chrono::{DateTime, Duration, Utc};
use maekon_core::models::suggestion::Suggestion;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct DeferredEntry {
    pub suggestion: Suggestion,
    pub deferred_at: DateTime<Utc>,
    pub resurface_at: DateTime<Utc>,
}

pub struct DeferredManager {
    items: VecDeque<DeferredEntry>,
    max_size: usize,
}

impl DeferredManager {
    pub fn new(max_size: usize) -> Self {
        Self {
            items: VecDeque::new(),
            max_size,
        }
    }

    /// Defer (snooze) a suggestion. Returns `false` if the deferred set is full
    /// and the incoming snooze would resurface later than every entry already
    /// kept (so rejecting it preserves the sooner snoozes).
    pub fn defer(&mut self, suggestion: Suggestion, duration: Duration) -> bool {
        let now = Utc::now();
        let resurface_at = now + duration;
        if self.items.len() >= self.max_size {
            // Evict by FARTHEST resurface_at (least urgent), not insertion order
            // (review4): pop_front() dropped the oldest-inserted entry, which is
            // frequently the SOONEST to resurface — silently losing a snooze the
            // user expected back soon while keeping later ones. Keep the soonest
            // `max_size` entries instead, and make the outcome observable (the
            // caller previously ignored a bare unconditional `true`).
            let farthest = self
                .items
                .iter()
                .enumerate()
                .max_by_key(|(_, e)| e.resurface_at)
                .map(|(idx, e)| (idx, e.resurface_at));
            match farthest {
                Some((idx, far_at)) if far_at > resurface_at => {
                    if let Some(evicted) = self.items.remove(idx) {
                        tracing::warn!(
                            evicted_id = %evicted.suggestion.suggestion_id,
                            max_size = self.max_size,
                            "deferred set full — evicted farthest-resurface snooze"
                        );
                    }
                }
                _ => {
                    tracing::warn!(
                        rejected_id = %suggestion.suggestion_id,
                        max_size = self.max_size,
                        "deferred set full — rejected (would resurface later than all kept entries)"
                    );
                    return false;
                }
            }
        }
        self.items.push_back(DeferredEntry {
            suggestion,
            deferred_at: now,
            resurface_at,
        });
        true
    }

    pub fn collect_resurfaced(&mut self) -> Vec<Suggestion> {
        let now = Utc::now();
        let mut resurfaced = Vec::new();
        self.items.retain(|entry| {
            if entry.resurface_at <= now {
                resurfaced.push(entry.suggestion.clone());
                false
            } else {
                true
            }
        });
        resurfaced
    }

    pub fn pending_count(&self) -> usize {
        self.items.len()
    }

    pub fn list_deferred(&self) -> Vec<&DeferredEntry> {
        self.items.iter().collect()
    }

    pub fn cancel(&mut self, suggestion_id: &str) -> Option<Suggestion> {
        let pos = self
            .items
            .iter()
            .position(|e| e.suggestion.suggestion_id == suggestion_id)?;
        self.items.remove(pos).map(|e| e.suggestion)
    }

    /// Bulk-restore deferred entries from storage. Items past their resurface
    /// time are returned for immediate re-queue; the rest are inserted.
    pub fn restore(
        &mut self,
        entries: Vec<(Suggestion, DateTime<Utc>, DateTime<Utc>)>,
    ) -> Vec<Suggestion> {
        let now = Utc::now();
        let mut already_due = Vec::new();
        for (suggestion, deferred_at, resurface_at) in entries {
            if resurface_at <= now {
                already_due.push(suggestion);
            } else if self.items.len() < self.max_size {
                self.items.push_back(DeferredEntry {
                    suggestion,
                    deferred_at,
                    resurface_at,
                });
            }
        }
        already_due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::suggestion::{Priority, SuggestionSource, SuggestionType};

    fn make_suggestion(id: &str) -> Suggestion {
        Suggestion {
            suggestion_id: id.to_string(),
            suggestion_type: SuggestionType::ProductivityTip,
            content: format!("tip {id}"),
            priority: Priority::Medium,
            confidence_score: 0.8,
            relevance_score: 0.7,
            source: SuggestionSource::LlmServer,
            is_actionable: true,
            reasoning: None,
            created_at: Utc::now(),
            expires_at: None,
            context_scope: None,
        }
    }

    #[test]
    fn defer_and_collect_after_duration() {
        let mut mgr = DeferredManager::new(50);
        let s = make_suggestion("s1");
        assert!(mgr.defer(s, Duration::zero()));
        assert_eq!(mgr.pending_count(), 1);
        let resurfaced = mgr.collect_resurfaced();
        assert_eq!(resurfaced.len(), 1);
        assert_eq!(resurfaced[0].suggestion_id, "s1");
        assert_eq!(mgr.pending_count(), 0);
    }

    #[test]
    fn collect_skips_future_items() {
        let mut mgr = DeferredManager::new(50);
        let s = make_suggestion("s1");
        assert!(mgr.defer(s, Duration::hours(2)));
        let resurfaced = mgr.collect_resurfaced();
        assert!(resurfaced.is_empty());
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn max_size_eviction_drops_farthest_resurface() {
        // review4: eviction must drop the FARTHEST-resurface entry (least urgent),
        // not the oldest-inserted, so sooner snoozes are preserved.
        let mut mgr = DeferredManager::new(2);
        assert!(mgr.defer(make_suggestion("s1"), Duration::hours(5))); // farthest
        assert!(mgr.defer(make_suggestion("s2"), Duration::hours(1))); // soonest
                                                                       // s3 (2h) is sooner than the farthest kept entry (s1, 5h) → evict s1, keep
                                                                       // the two sooner snoozes (s2, s3).
        assert!(mgr.defer(make_suggestion("s3"), Duration::hours(2)));
        assert_eq!(mgr.pending_count(), 2);
        let ids: Vec<_> = mgr
            .list_deferred()
            .iter()
            .map(|e| e.suggestion.suggestion_id.as_str())
            .collect();
        assert!(
            !ids.contains(&"s1"),
            "farthest-resurface entry must be evicted"
        );
        assert!(ids.contains(&"s2"));
        assert!(ids.contains(&"s3"));
    }

    #[test]
    fn defer_rejects_when_full_and_new_is_farthest() {
        // When the incoming snooze would resurface later than every kept entry,
        // reject it (return false) rather than displace a sooner snooze.
        let mut mgr = DeferredManager::new(2);
        assert!(mgr.defer(make_suggestion("s1"), Duration::hours(1)));
        assert!(mgr.defer(make_suggestion("s2"), Duration::hours(2)));
        assert!(!mgr.defer(make_suggestion("s3"), Duration::hours(9)));
        assert_eq!(mgr.pending_count(), 2);
        let ids: Vec<_> = mgr
            .list_deferred()
            .iter()
            .map(|e| e.suggestion.suggestion_id.as_str())
            .collect();
        assert!(ids.contains(&"s1") && ids.contains(&"s2"));
        assert!(!ids.contains(&"s3"));
    }

    #[test]
    fn cancel_removes_and_returns() {
        let mut mgr = DeferredManager::new(50);
        mgr.defer(make_suggestion("s1"), Duration::hours(1));
        mgr.defer(make_suggestion("s2"), Duration::hours(1));
        let cancelled = mgr.cancel("s1");
        assert!(cancelled.is_some());
        assert_eq!(cancelled.unwrap().suggestion_id, "s1");
        assert_eq!(mgr.pending_count(), 1);
    }

    #[test]
    fn cancel_nonexistent_returns_none() {
        let mut mgr = DeferredManager::new(50);
        assert!(mgr.cancel("nope").is_none());
    }

    #[test]
    fn test_restore_future_items() {
        let mut mgr = DeferredManager::new(50);
        let now = Utc::now();
        let entries = vec![
            (make_suggestion("r1"), now, now + Duration::hours(1)),
            (make_suggestion("r2"), now, now + Duration::hours(2)),
        ];
        let due = mgr.restore(entries);
        assert!(due.is_empty());
        assert_eq!(mgr.pending_count(), 2);
    }

    #[test]
    fn test_restore_past_items() {
        let mut mgr = DeferredManager::new(50);
        let now = Utc::now();
        let entries = vec![
            (
                make_suggestion("r1"),
                now - Duration::hours(2),
                now - Duration::hours(1),
            ),
            (
                make_suggestion("r2"),
                now - Duration::hours(3),
                now - Duration::minutes(1),
            ),
        ];
        let due = mgr.restore(entries);
        assert_eq!(due.len(), 2);
        assert_eq!(mgr.pending_count(), 0);
    }

    #[test]
    fn test_restore_max_size() {
        let mut mgr = DeferredManager::new(2);
        let now = Utc::now();
        let entries = vec![
            (make_suggestion("r1"), now, now + Duration::hours(1)),
            (make_suggestion("r2"), now, now + Duration::hours(2)),
            (make_suggestion("r3"), now, now + Duration::hours(3)),
        ];
        let due = mgr.restore(entries);
        assert!(due.is_empty());
        assert_eq!(mgr.pending_count(), 2);
    }
}
