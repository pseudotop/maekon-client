use chrono::{DateTime, Utc};
use oneshim_core::models::tiered_memory::TriggerInput;
use std::collections::VecDeque;

/// Ring-buffer for timestamped trigger events within the current segment.
///
/// When the buffer reaches capacity it evicts the oldest 20% to make room.
/// This ensures recent history is always available for segment analysis
/// without unbounded memory growth.
pub struct SegmentBuffer {
    events: VecDeque<(DateTime<Utc>, TriggerInput)>,
    capacity: usize,
    segment_start: Option<DateTime<Utc>>,
}

impl SegmentBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity: capacity.max(1),
            segment_start: None,
        }
    }

    /// Push an event. If the buffer is at capacity, evict the oldest 20%.
    pub fn push(&mut self, ts: DateTime<Utc>, input: TriggerInput) {
        if self.events.len() >= self.capacity {
            let evict_count = (self.capacity / 5).max(1);
            self.events.drain(..evict_count);
        }
        self.events.push_back((ts, input));
    }

    /// Number of events in the buffer.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Timestamp of the current segment start, if a segment is open.
    pub fn start_time(&self) -> Option<DateTime<Utc>> {
        self.segment_start
    }

    /// Drain all events and return them as a Vec. Clears segment_start.
    pub fn drain_all(&mut self) -> Vec<(DateTime<Utc>, TriggerInput)> {
        self.segment_start = None;
        self.events.drain(..).collect()
    }

    /// Mark the start of a new segment.
    pub fn start_segment(&mut self, ts: DateTime<Utc>) {
        self.segment_start = Some(ts);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_input(name: &str) -> TriggerInput {
        TriggerInput::AppPoll {
            app_name: name.to_string(),
        }
    }

    #[test]
    fn segment_lifecycle() {
        let mut buf = SegmentBuffer::new(100);
        assert!(buf.start_time().is_none());

        let now = Utc::now();
        buf.start_segment(now);
        assert_eq!(buf.start_time(), Some(now));

        buf.push(now, make_input("A"));
        buf.push(now + Duration::seconds(1), make_input("B"));
        assert_eq!(buf.len(), 2);

        let drained = buf.drain_all();
        assert_eq!(drained.len(), 2);
        assert!(buf.is_empty());
        assert!(buf.start_time().is_none());
    }

    #[test]
    fn capacity_eviction() {
        let mut buf = SegmentBuffer::new(10);
        let base = Utc::now();

        // Fill to capacity
        for i in 0..10 {
            buf.push(base + Duration::seconds(i), make_input(&format!("App{i}")));
        }
        assert_eq!(buf.len(), 10);

        // Push one more — should evict oldest 20% (2 items), then add 1
        buf.push(base + Duration::seconds(10), make_input("AppNew"));
        assert_eq!(buf.len(), 9); // 10 - 2 + 1 = 9
    }

    #[test]
    fn empty_state() {
        let buf = SegmentBuffer::new(50);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert!(buf.start_time().is_none());
    }

    #[test]
    fn drain_empty_buffer() {
        let mut buf = SegmentBuffer::new(50);
        let drained = buf.drain_all();
        assert!(drained.is_empty());
    }

    #[test]
    fn min_capacity_one() {
        let mut buf = SegmentBuffer::new(0); // should be clamped to 1
        buf.push(Utc::now(), make_input("A"));
        assert_eq!(buf.len(), 1);
        // Push another — evicts 1 (max(1/5, 1) = 1), then adds 1
        buf.push(Utc::now(), make_input("B"));
        assert_eq!(buf.len(), 1);
    }
}
