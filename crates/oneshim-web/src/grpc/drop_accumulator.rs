//! D13-v2b PR-B3 — accumulates dropped event counts (rate limiter rejections,
//! channel lag) and surfaces them as `DroppedEventsSignal` on a throttled
//! cadence. Per-stream state; reset after each emission.
//!
//! Invariants:
//! - Emission interval: 1 second (configurable via const).
//! - Counts are per event_type String (Frame/Idle/AiRuntimeStatus).
//! - `since` timestamp rolls forward on each emission.
//!
//! Reason code: supplied at construction (`DropAccumulator::new(reason)`).
//!
//! D13-v2c splits per-reason accumulation — handlers use one instance per
//! reason (e.g., `rate_limit`, `channel_lag`), each with independent
//! throttle/emit state. The proto `DroppedEventsSignal.reason` echoes the
//! construction-time value verbatim. `by_type[].event_type` remains the
//! per-event-type bucket; for reasons where the dropped event's type is
//! unknown (e.g., `broadcast::RecvError::Lagged`), callers should record
//! drops under the `"unknown"` sentinel (OTel convention).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use crate::proto::dashboard::v1::{dropped_events_signal::TypeCount, DroppedEventsSignal};

/// Emit a DroppedEventsSignal at most this often, even if drops
/// keep accumulating.
pub const DROP_EMIT_INTERVAL: Duration = Duration::from_secs(1);

/// Cap on distinct event_type keys tracked. Realistic keys are
/// `frame`, `idle`, `ai_runtime_status` (proto event types) plus
/// `channel_lag` as a mechanism-label injected when
/// `broadcast::RecvError::Lagged` fires — not a proto event type.
/// Overflow folds to the `"other"` sentinel, so the effective max
/// bucket count is `MAX_DROP_TYPES + 1`. Cap prevents unbounded
/// HashMap growth if a caller bug passes arbitrary strings.
const MAX_DROP_TYPES: usize = 8;

pub struct DropAccumulator {
    counts_by_type: HashMap<String, u64>,
    since: DateTime<Utc>,
    last_emit_at: Option<Instant>,
    reason: &'static str, // "rate_limit" for MVP; multi-reason in v2c
}

impl DropAccumulator {
    pub fn new(reason: &'static str) -> Self {
        Self {
            counts_by_type: HashMap::new(),
            since: Utc::now(),
            last_emit_at: None,
            reason,
        }
    }

    pub fn record_drop(&mut self, event_type: &str) {
        if let Some(existing) = self.counts_by_type.get_mut(event_type) {
            *existing += 1;
            return;
        }
        // Key is new: check cap before inserting.
        if self.counts_by_type.len() < MAX_DROP_TYPES {
            self.counts_by_type.insert(event_type.to_string(), 1);
        } else {
            // At cap — fold into "other" sentinel so counts aren't lost.
            *self.counts_by_type.entry("other".to_string()).or_insert(0) += 1;
        }
    }

    /// Emit a DroppedEventsSignal iff (a) there are accumulated drops AND
    /// (b) the throttle interval has elapsed. Caller wraps the returned
    /// signal into SubscribeEventsResponse::Payload::Dropped.
    ///
    /// On emission, `counts_by_type` is cleared and `since` rolls forward
    /// to the previous `until`. No mutation when returning None.
    pub fn maybe_emit(&mut self) -> Option<DroppedEventsSignal> {
        if self.counts_by_type.is_empty() {
            return None;
        }
        let now = Instant::now();
        let throttle_ok = match self.last_emit_at {
            None => true,
            Some(t) => now.duration_since(t) >= DROP_EMIT_INTERVAL,
        };
        if !throttle_ok {
            return None;
        }

        let dropped_count: u64 = self.counts_by_type.values().sum();
        let by_type: Vec<TypeCount> = self
            .counts_by_type
            .iter()
            .map(|(k, v)| TypeCount {
                event_type: k.clone(),
                count: *v,
            })
            .collect();

        let until = Utc::now();
        let signal = DroppedEventsSignal {
            dropped_count,
            since: Some(super::to_proto_ts(self.since)),
            until: Some(super::to_proto_ts(until)),
            reason: self.reason.to_string(),
            by_type,
        };

        // Reset state post-emission.
        self.counts_by_type.clear();
        self.since = until;
        self.last_emit_at = Some(now);

        Some(signal)
    }
}

impl Default for DropAccumulator {
    /// Default reason is `"rate_limit"` — matches the PR-B3 MVP behavior for
    /// backward compat with any future callers that don't specify a reason.
    /// Prefer the explicit `new(reason)` for new callers.
    fn default() -> Self {
        Self::new("rate_limit")
    }
}

#[cfg(any(test, feature = "test-support"))]
#[allow(dead_code)]
impl DropAccumulator {
    /// Test-only: set last_emit_at to a past Instant to make the throttle
    /// interval test deterministic without sleeping.
    pub(super) fn set_last_emit_at_for_test(&mut self, t: Instant) {
        self.last_emit_at = Some(t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_accumulator_maybe_emit_returns_none() {
        let mut acc = DropAccumulator::new("rate_limit");
        assert!(acc.maybe_emit().is_none(), "no drops recorded → None");
    }

    #[test]
    fn single_drop_emits_on_first_call() {
        let mut acc = DropAccumulator::new("rate_limit");
        acc.record_drop("frame");
        let sig = acc.maybe_emit().expect("first emission should fire");
        assert_eq!(sig.dropped_count, 1);
        assert_eq!(sig.reason, "rate_limit");
        assert_eq!(sig.by_type.len(), 1);
        assert_eq!(sig.by_type[0].event_type, "frame");
        assert_eq!(sig.by_type[0].count, 1);
        // State reset
        assert!(acc.counts_by_type.is_empty());
    }

    #[test]
    fn throttle_blocks_rapid_emit() {
        let mut acc = DropAccumulator::new("rate_limit");
        acc.record_drop("frame");
        assert!(acc.maybe_emit().is_some(), "first emit");
        acc.record_drop("frame");
        assert!(
            acc.maybe_emit().is_none(),
            "second emit within throttle window should be None"
        );
    }

    #[test]
    fn throttle_permits_emit_after_interval() {
        let mut acc = DropAccumulator::new("rate_limit");
        acc.record_drop("frame");
        let first = acc.maybe_emit().expect("first emission");
        let first_until_micros = first.until.as_ref().map(|t| (t.seconds, t.nanos));

        // Fake that the throttle interval fully elapsed by pushing last_emit_at backward.
        let past = Instant::now() - (DROP_EMIT_INTERVAL + Duration::from_millis(50));
        acc.set_last_emit_at_for_test(past);
        acc.record_drop("idle");

        let second = acc.maybe_emit().expect("second emission after interval");
        let second_since_micros = second.since.as_ref().map(|t| (t.seconds, t.nanos));

        // since of second emission should equal until of first emission (rollover).
        assert_eq!(first_until_micros, second_since_micros);
        assert_eq!(second.dropped_count, 1);
        assert_eq!(second.by_type[0].event_type, "idle");
    }

    #[test]
    fn record_drop_saturates_at_max_types() {
        let mut acc = DropAccumulator::new("rate_limit");
        // Fill to cap with distinct types.
        for i in 0..MAX_DROP_TYPES {
            acc.record_drop(&format!("type_{i}"));
        }
        assert_eq!(acc.counts_by_type.len(), MAX_DROP_TYPES);
        // Exceed cap with a new key — should fold into "other".
        acc.record_drop("overflow_type");
        assert_eq!(acc.counts_by_type.len(), MAX_DROP_TYPES + 1); // +1 for "other"
        assert_eq!(acc.counts_by_type.get("other"), Some(&1));
        assert!(!acc.counts_by_type.contains_key("overflow_type"));
    }
}
