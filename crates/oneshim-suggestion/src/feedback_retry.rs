use chrono::{DateTime, Duration, Utc};
use oneshim_core::models::suggestion::FeedbackType;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct PendingFeedback {
    pub suggestion_id: String,
    pub feedback_type: FeedbackType,
    pub comment: Option<String>,
    pub attempts: u32,
    pub next_retry_at: DateTime<Utc>,
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

    pub fn enqueue(&mut self, mut feedback: PendingFeedback) {
        feedback.next_retry_at = Utc::now() + retry_delay(feedback.attempts);
        if self.items.len() >= self.max_size {
            self.items.pop_front();
        }
        self.items.push_back(feedback);
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

    pub fn retry_failed(&mut self, mut feedback: PendingFeedback) {
        feedback.attempts += 1;
        feedback.next_retry_at = Utc::now() + retry_delay(feedback.attempts);
        if self.items.len() >= self.max_size {
            self.items.pop_front();
        }
        self.items.push_back(feedback);
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
        q.enqueue(f);
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
        q.enqueue(f);
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
        q.enqueue(f);
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
    fn max_size_evicts_oldest() {
        let mut q = FeedbackRetryQueue::new(2, 5);
        q.enqueue(make_pending("s1"));
        q.enqueue(make_pending("s2"));
        q.enqueue(make_pending("s3"));
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
