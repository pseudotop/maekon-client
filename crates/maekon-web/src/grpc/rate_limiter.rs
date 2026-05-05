//! D13-v2b PR-B3 — per-event-type token bucket rate limiter. Protects
//! subscribers from bursty event_tx floods when one publisher misbehaves.
//!
//! Decoupled from LoadPolicy's interval enforcement (that applies to
//! SubscribeMetrics). This limiter is event-rate-based: "at most N events
//! of type T per second per stream".
//!
//! Single-threaded access via `&mut self`; caller (subscribe_events handler)
//! owns one instance per stream.

use std::collections::HashMap;
use std::time::Instant;

/// Default tokens-per-second per event type. Overridable via
/// ServerLoadHint::suggested_event_rate_limit (future wiring; v2c).
pub const DEFAULT_TOKENS_PER_SEC: u32 = 10;
pub const BURST_CAPACITY: u32 = 20;

/// Maximum distinct event_type buckets tracked. Realistic set is 3
/// (frame, idle, ai_runtime_status). Cap prevents unbounded HashMap
/// growth if a caller bug passes arbitrary strings.
const MAX_BUCKETS: usize = 8;

#[derive(Debug)]
struct Bucket {
    tokens: f64,
    last_refill: Instant,
    capacity: f64,
    refill_per_sec: f64,
}

impl Bucket {
    fn new(capacity: u32, refill_per_sec: u32) -> Self {
        Self {
            tokens: capacity as f64,
            last_refill: Instant::now(),
            capacity: capacity as f64,
            refill_per_sec: refill_per_sec as f64,
        }
    }

    fn try_consume(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.refill_per_sec).min(self.capacity);
        self.last_refill = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

pub struct EventRateLimiter {
    buckets: HashMap<String, Bucket>,
    default_capacity: u32,
    default_refill: u32,
}

impl EventRateLimiter {
    pub fn new() -> Self {
        Self::with_rate(BURST_CAPACITY, DEFAULT_TOKENS_PER_SEC)
    }

    pub fn with_rate(capacity: u32, refill_per_sec: u32) -> Self {
        Self {
            buckets: HashMap::new(),
            default_capacity: capacity,
            default_refill: refill_per_sec,
        }
    }

    /// Returns `true` if the event is allowed; `false` if rate-limited.
    pub fn try_admit(&mut self, event_type: &str) -> bool {
        // Fast path: key exists.
        if let Some(bucket) = self.buckets.get_mut(event_type) {
            return bucket.try_consume();
        }
        // New key: check cap (CLAUDE.md bounded-collections guardrail).
        if self.buckets.len() >= MAX_BUCKETS {
            // At cap — admit conservatively (no bucket tracking). Caller
            // records as drop if needed. Defensive: better to admit-unmetered
            // than to silently reject new event types.
            return true;
        }
        let mut bucket = Bucket::new(self.default_capacity, self.default_refill);
        let admitted = bucket.try_consume();
        self.buckets.insert(event_type.to_string(), bucket);
        admitted
    }
}

impl Default for EventRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
#[allow(dead_code)]
impl EventRateLimiter {
    /// Test-only: shift `last_refill` of a specific bucket to a past Instant
    /// so tests can assert refill behavior without sleeping.
    pub(super) fn set_last_refill_for_test(&mut self, event_type: &str, t: Instant) {
        if let Some(bucket) = self.buckets.get_mut(event_type) {
            bucket.last_refill = t;
        }
    }

    /// Test-only: count of distinct buckets (lets tests assert the
    /// MAX_BUCKETS cap without reaching into the private field).
    pub(super) fn bucket_count_for_test(&self) -> usize {
        self.buckets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn admits_below_burst_capacity() {
        let mut rl = EventRateLimiter::new();
        for _ in 0..BURST_CAPACITY {
            assert!(rl.try_admit("frame"));
        }
    }

    #[test]
    fn rejects_beyond_burst_capacity() {
        let mut rl = EventRateLimiter::new();
        for _ in 0..BURST_CAPACITY {
            rl.try_admit("frame");
        }
        assert!(
            !rl.try_admit("frame"),
            "21st admit in burst window should fail"
        );
    }

    #[test]
    fn refills_over_time() {
        let mut rl = EventRateLimiter::with_rate(5, 50);
        // Burn the initial 5-token burst.
        for _ in 0..5 {
            assert!(rl.try_admit("frame"));
        }
        assert!(!rl.try_admit("frame"), "burst exhausted");
        // Simulate 150ms elapsed by pushing last_refill backward.
        // At 50 tokens/sec, 150ms refills 7.5 tokens — well above the 1.0 threshold.
        let past = Instant::now() - Duration::from_millis(150);
        rl.set_last_refill_for_test("frame", past);
        assert!(rl.try_admit("frame"), "refill should permit after interval");
    }

    #[test]
    fn per_type_isolation() {
        let mut rl = EventRateLimiter::new();
        for _ in 0..BURST_CAPACITY {
            rl.try_admit("frame");
        }
        assert!(!rl.try_admit("frame"), "frame bucket empty");
        assert!(rl.try_admit("idle"), "idle bucket fresh — must admit");
    }

    #[test]
    fn custom_rate_honored() {
        let mut rl = EventRateLimiter::with_rate(5, 5);
        for _ in 0..5 {
            assert!(rl.try_admit("frame"));
        }
        assert!(
            !rl.try_admit("frame"),
            "6th admit within burst window fails"
        );
    }

    #[test]
    fn bucket_cap_admits_overflow_unmetered() {
        // tight: 1 token burst — fills MAX_BUCKETS distinct keys.
        let mut rl = EventRateLimiter::with_rate(1, 1);
        for i in 0..MAX_BUCKETS {
            assert!(rl.try_admit(&format!("type_{i}")));
        }
        // Overflow: new type beyond MAX_BUCKETS should still admit (unmetered).
        assert!(rl.try_admit("overflow_type"));
        // Bucket count should NOT have grown.
        assert!(rl.bucket_count_for_test() <= MAX_BUCKETS);
    }
}
