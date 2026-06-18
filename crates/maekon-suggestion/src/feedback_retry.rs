use chrono::{DateTime, Duration, Utc};
use maekon_core::models::suggestion::FeedbackType;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct PendingFeedback {
    pub suggestion_id: String,
    pub feedback_type: FeedbackType,
    pub comment: Option<String>,
    pub attempts: u32,
    pub next_retry_at: DateTime<Utc>,
}

/// Outcome of [`FeedbackRetryQueue::retry_failed`].
pub struct RetryReschedule {
    /// The rescheduled record (bumped attempts + backoff). Re-persist this so a
    /// restart does not restore a stale attempt count (#6095).
    pub updated: PendingFeedback,
    /// An entry evicted to stay within `max_size`, if any. The caller must delete
    /// its durable SQLite row to avoid an orphaned pending-retry (review4).
    pub evicted: Option<PendingFeedback>,
}

pub struct FeedbackRetryQueue {
    items: VecDeque<PendingFeedback>,
    max_size: usize,
    max_attempts: u32,
}

/// Retry delays: 5s, 15s, 45s, 2m, 5m (3x multiplier, capped at 5m)
fn retry_delay(attempt: u32) -> Duration {
    let secs = match attempt {
        0 => 5,
        1 => 15,
        2 => 45,
        3 => 120,
        _ => 300,
    };
    Duration::seconds(secs)
}

impl FeedbackRetryQueue {
    pub fn new(max_size: usize, max_attempts: u32) -> Self {
        Self {
            items: VecDeque::new(),
            max_size,
            max_attempts,
        }
    }

    /// Remove any existing entry that shares `suggestion_id`. The in-memory
    /// queue must stay 1:1 with the SQLite `UNIQUE(suggestion_id)` constraint —
    /// otherwise a single suggestion could be sent to the server more than once.
    /// Callers always (re)compute fresh `attempts`/`next_retry_at`, so the
    /// incoming record is the authoritative one and replaces the stale entry.
    fn remove_existing(&mut self, suggestion_id: &str) {
        self.items.retain(|f| f.suggestion_id != suggestion_id);
    }

    /// Enqueue a pending feedback retry. Returns the entry evicted to stay within
    /// `max_size` (if any) so the caller can delete its now-orphaned durable
    /// SQLite row — otherwise an evicted item's row would persist as a pending
    /// retry that the in-session maintenance loop never drains, and the feedback
    /// would only be retried after a restart (review4).
    #[must_use]
    pub fn enqueue(&mut self, mut feedback: PendingFeedback) -> Option<PendingFeedback> {
        feedback.next_retry_at = Utc::now() + retry_delay(feedback.attempts);
        // Dedup by suggestion_id (mirror SQLite UNIQUE) before bounding by size.
        self.remove_existing(&feedback.suggestion_id);
        let evicted = if self.items.len() >= self.max_size {
            self.items.pop_front()
        } else {
            None
        };
        self.items.push_back(feedback);
        evicted
    }

    pub fn collect_ready(&mut self) -> Vec<PendingFeedback> {
        let now = Utc::now();
        let mut ready = Vec::new();
        self.items.retain(|f| {
            if f.next_retry_at <= now {
                ready.push(f.clone());
                false
            } else {
                true
            }
        });
        ready
    }

    /// Bump the attempt count, reschedule `next_retry_at` with backoff, and
    /// re-enqueue. Returns the rescheduled record (re-persist it; #6095) plus any
    /// entry evicted to stay within `max_size` so the caller can delete that
    /// entry's now-orphaned durable row (review4).
    pub fn retry_failed(&mut self, mut feedback: PendingFeedback) -> RetryReschedule {
        feedback.attempts += 1;
        feedback.next_retry_at = Utc::now() + retry_delay(feedback.attempts);
        // Dedup by suggestion_id (mirror SQLite UNIQUE) so a re-enqueued retry
        // never coexists with a stale copy of the same suggestion.
        self.remove_existing(&feedback.suggestion_id);
        // Reachable despite collect_ready having freed a slot: the maintenance loop
        // releases the retry_queue lock across the (possibly multi-second) network
        // retry, so a concurrent user-feedback enqueue can refill to max_size before
        // retry_failed re-acquires it. Return the evicted entry so the caller deletes
        // its durable row, mirroring enqueue (review4 re-verify).
        let evicted = if self.items.len() >= self.max_size {
            self.items.pop_front()
        } else {
            None
        };
        let updated = feedback.clone();
        self.items.push_back(feedback);
        RetryReschedule { updated, evicted }
    }

    pub fn drop_exhausted(&mut self, suggestion_id: &str) {
        self.items.retain(|f| f.suggestion_id != suggestion_id);
    }

    pub fn pending_count(&self) -> usize {
        self.items.len()
    }

    pub fn is_exhausted(&self, feedback: &PendingFeedback) -> bool {
        feedback.attempts >= self.max_attempts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pending(id: &str) -> PendingFeedback {
        PendingFeedback {
            suggestion_id: id.to_string(),
            feedback_type: FeedbackType::Accepted,
            comment: None,
            attempts: 0,
            next_retry_at: Utc::now(),
        }
    }

    #[test]
    fn enqueue_sets_retry_delay() {
        let mut q = FeedbackRetryQueue::new(100, 5);
        let before = Utc::now();
        let f = make_pending("s1");
        let _ = q.enqueue(f);
        assert_eq!(q.pending_count(), 1);
        // enqueue sets next_retry_at = now + retry_delay(0) = now + 5s
        let item = &q.items[0];
        assert!(item.next_retry_at >= before + Duration::seconds(5));
        // Not yet ready (scheduled 5s in the future)
        let ready = q.collect_ready();
        assert!(ready.is_empty());
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn collect_ready_returns_past_items() {
        let mut q = FeedbackRetryQueue::new(100, 5);
        let f = make_pending("s1");
        let _ = q.enqueue(f);
        // Manually backdate the item so it becomes ready
        q.items[0].next_retry_at = Utc::now() - Duration::seconds(1);
        let ready = q.collect_ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].suggestion_id, "s1");
        assert_eq!(q.pending_count(), 0);
    }

    #[test]
    fn collect_skips_future_items() {
        let mut q = FeedbackRetryQueue::new(100, 5);
        let mut f = make_pending("s1");
        f.next_retry_at = Utc::now() + Duration::hours(1);
        let _ = q.enqueue(f);
        let ready = q.collect_ready();
        assert!(ready.is_empty());
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn retry_failed_increments_attempt_and_reschedules() {
        let mut q = FeedbackRetryQueue::new(100, 5);
        let f = make_pending("s1");
        q.retry_failed(f);
        assert_eq!(q.pending_count(), 1);
        let items: Vec<_> = q.items.iter().collect();
        assert_eq!(items[0].attempts, 1);
        assert!(items[0].next_retry_at > Utc::now());
    }

    #[test]
    fn retry_failed_returns_rescheduled_record_for_persistence() {
        // The returned record must carry the bumped attempt count and the
        // freshly-scheduled backoff so the scheduler can re-persist it to
        // SQLite; otherwise a restart would restore a stale attempt count and
        // reset the max-attempts bound (#6095).
        let mut q = FeedbackRetryQueue::new(100, 5);
        let mut f = make_pending("s1");
        f.attempts = 2;
        let before = Utc::now();
        let updated = q.retry_failed(f).updated;
        assert_eq!(updated.suggestion_id, "s1");
        assert_eq!(updated.attempts, 3);
        // attempt 3 → 120s backoff
        assert!(updated.next_retry_at >= before + Duration::seconds(120));
        // The returned record mirrors the in-memory queue entry exactly.
        assert_eq!(q.items[0].attempts, updated.attempts);
        assert_eq!(q.items[0].next_retry_at, updated.next_retry_at);
    }

    #[test]
    fn enqueue_dedups_by_suggestion_id() {
        // The in-memory queue mirrors the SQLite UNIQUE(suggestion_id) constraint:
        // enqueuing the same suggestion_id twice must yield a single entry, and
        // the surviving entry carries the fresher retry metadata.
        let mut q = FeedbackRetryQueue::new(100, 5);
        let mut first = make_pending("s1");
        first.attempts = 1;
        let _ = q.enqueue(first);
        let mut second = make_pending("s1");
        second.attempts = 3;
        let _ = q.enqueue(second);
        assert_eq!(q.pending_count(), 1);
        // The later enqueue wins (fresher attempts/backoff).
        assert_eq!(q.items[0].attempts, 3);
    }

    #[test]
    fn retry_failed_dedups_by_suggestion_id() {
        // A retry re-enqueue must not coexist with a stale copy of the same id.
        let mut q = FeedbackRetryQueue::new(100, 5);
        let _ = q.enqueue(make_pending("s1"));
        let f = make_pending("s1");
        q.retry_failed(f);
        assert_eq!(q.pending_count(), 1);
        assert_eq!(q.items[0].attempts, 1);
    }

    #[test]
    fn max_size_evicts_oldest() {
        let mut q = FeedbackRetryQueue::new(2, 5);
        let _ = q.enqueue(make_pending("s1"));
        let _ = q.enqueue(make_pending("s2"));
        let _ = q.enqueue(make_pending("s3"));
        assert_eq!(q.pending_count(), 2);
        let ids: Vec<_> = q.items.iter().map(|f| f.suggestion_id.as_str()).collect();
        assert!(!ids.contains(&"s1"));
    }

    #[test]
    fn is_exhausted_after_max_attempts() {
        let q = FeedbackRetryQueue::new(100, 5);
        let mut f = make_pending("s1");
        f.attempts = 5;
        assert!(q.is_exhausted(&f));
    }

    #[test]
    fn retry_delay_schedule() {
        assert_eq!(retry_delay(0), Duration::seconds(5));
        assert_eq!(retry_delay(1), Duration::seconds(15));
        assert_eq!(retry_delay(2), Duration::seconds(45));
        assert_eq!(retry_delay(3), Duration::seconds(120));
        assert_eq!(retry_delay(4), Duration::seconds(300));
        assert_eq!(retry_delay(10), Duration::seconds(300)); // capped
    }
}
