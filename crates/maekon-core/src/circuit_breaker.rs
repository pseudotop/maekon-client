//! Generic tick-based consecutive-failure circuit breaker (#6828 origin; #7720
//! E6 consolidation).
//!
//! A shared primitive for guarding a fallible/expensive operation (subprocess
//! spawn, platform accessibility API call, etc.) from being retried on every
//! tick once it starts failing. After `threshold` consecutive failures the
//! breaker opens; while open, only the single caller that atomically claims a
//! `retry_interval` boundary slot is allowed to retry (via `compare_exchange`),
//! which prevents a race where two concurrent callers both observe an
//! "open, retry-eligible" counter value and both proceed.
//!
//! Lives in `maekon-core` (std atomics only, `cfg`-free, zero new dependencies)
//! so every adapter crate — `maekon-monitor` (subprocess-spawn guards) and
//! `maekon-vision` (platform accessibility API guards) — can share ONE
//! implementation instead of hand-rolling per-site copies that can silently
//! drift out of sync. Before the #7720 consolidation, three `maekon-vision`
//! copies (macOS AX, Windows UIA, Linux AT-SPI2) had drifted to a version
//! *missing* the `compare_exchange` retry-slot fix (#6007 finding 17), so two
//! concurrent racer calls at a retry boundary could both slip through and
//! both make the guarded call.

use std::sync::atomic::{AtomicU32, Ordering};

/// Atomic consecutive-failure circuit breaker for guarding a fallible/expensive
/// operation from being retried on every tick.
pub struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    threshold: u32,
    retry_interval: u32,
}

impl CircuitBreaker {
    /// `threshold`: consecutive failures required to open the breaker.
    /// `retry_interval`: once open, only every Nth tick (a multiple of this
    /// value) is eligible to attempt a retry.
    pub const fn new(threshold: u32, retry_interval: u32) -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            threshold,
            retry_interval,
        }
    }

    /// Returns `true` if the caller may proceed with the guarded operation,
    /// `false` if it must short-circuit. While the breaker is open it only
    /// returns `true` for the single caller that atomically claims a
    /// `retry_interval` boundary slot (the `compare_exchange` prevents two
    /// concurrent callers from both proceeding); every other call
    /// increments-and-skips.
    pub fn should_proceed(&self) -> bool {
        let failures = self.consecutive_failures.load(Ordering::Relaxed);
        if failures < self.threshold {
            return true;
        }
        if !failures.is_multiple_of(self.retry_interval) {
            self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        self.consecutive_failures
            .compare_exchange(failures, failures + 1, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    /// Record that the guarded operation actually executed — closes the breaker.
    pub fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Record that the guarded operation was unavailable or failed — advances
    /// the breaker toward opening.
    pub fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
    }

    /// Current consecutive-failure counter. Introspection for observability
    /// (e.g. deciding whether a call just tripped the breaker) and for tests.
    pub fn failure_count(&self) -> u32 {
        self.consecutive_failures.load(Ordering::Relaxed)
    }

    /// Force the consecutive-failure counter to an exact value, bypassing the
    /// normal increment path. For test setup (precisely positioning a breaker
    /// at a threshold/retry boundary) and operational overrides (seeding state
    /// from a persisted counter).
    pub fn set_failure_count(&self, failures: u32) {
        self.consecutive_failures.store(failures, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_breaker_always_proceeds() {
        let breaker = CircuitBreaker::new(3, 60);
        for _ in 0..10 {
            assert!(breaker.should_proceed());
        }
    }

    #[test]
    fn opens_after_threshold_failures_and_skips() {
        let breaker = CircuitBreaker::new(3, 60);
        for _ in 0..3 {
            breaker.record_failure();
        }
        // failures == 3 (== threshold), not a multiple of 60 → short-circuit.
        assert!(!breaker.should_proceed());
    }

    #[test]
    fn success_resets_the_breaker() {
        let breaker = CircuitBreaker::new(3, 60);
        for _ in 0..5 {
            breaker.record_failure();
        }
        assert!(!breaker.should_proceed());
        breaker.record_success();
        assert!(breaker.should_proceed());
    }

    #[test]
    fn retries_only_at_interval_boundary() {
        let breaker = CircuitBreaker::new(3, 60);
        for _ in 0..60 {
            breaker.record_failure();
        }
        // failures == 60 (>= threshold, multiple of 60) → the single claimer proceeds.
        assert!(breaker.should_proceed());
        // failures == 61 → not a boundary → skip until the next multiple of 60.
        assert!(!breaker.should_proceed());
    }

    #[test]
    fn failure_count_and_set_failure_count_roundtrip() {
        let breaker = CircuitBreaker::new(3, 60);
        assert_eq!(breaker.failure_count(), 0);
        breaker.set_failure_count(42);
        assert_eq!(breaker.failure_count(), 42);
        breaker.record_success();
        assert_eq!(breaker.failure_count(), 0);
    }

    /// Regression for #7720 (E6): verify that when the counter is exactly at a
    /// retry boundary, only ONE concurrent caller is allowed to proceed (the
    /// CAS winner); a caller that arrives while the counter still reads the
    /// same multiple-of-N value loses the CAS and short-circuits.
    ///
    /// Ported from the original `maekon-monitor` macOS osascript-path test
    /// (finding #6007) as the single canonical proof for this struct: before
    /// the #7720 consolidation, three `maekon-vision` copies of this exact
    /// state machine had drifted to a version missing this `compare_exchange`
    /// claim, so two callers that both read the same multiple-of-N value would
    /// BOTH pass the `is_multiple_of` gate and both proceed.
    #[test]
    fn retry_slot_claimed_only_once() {
        let breaker = CircuitBreaker::new(3, 60);
        let retry_boundary = 60;

        // First caller at the boundary claims the slot (proceeds) and advances
        // the counter exactly one past the boundary.
        breaker.set_failure_count(retry_boundary);
        assert!(
            breaker.should_proceed(),
            "first caller at the retry boundary must claim the slot and proceed"
        );
        let after_first = breaker.failure_count();
        assert_eq!(
            after_first,
            retry_boundary + 1,
            "counter must advance exactly one past the retry boundary"
        );
        assert!(
            !after_first.is_multiple_of(retry_boundary),
            "counter ({after_first}) must not be a multiple of the retry interval \
             ({retry_boundary}) after the slot is claimed"
        );

        // A caller arriving after the slot was claimed (counter past the
        // boundary) must skip rather than proceed a second time.
        assert!(
            !breaker.should_proceed(),
            "a caller arriving after the slot was claimed must short-circuit"
        );

        // Directly prove the atomic claim: two callers that both read the SAME
        // boundary value — exactly one CAS succeeds, the other loses. This is a
        // white-box check against the internal atomic (same crate/module), the
        // same technique the `should_proceed` implementation itself uses.
        breaker.set_failure_count(retry_boundary);
        let a = breaker.consecutive_failures.compare_exchange(
            retry_boundary,
            retry_boundary + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        let b = breaker.consecutive_failures.compare_exchange(
            retry_boundary,
            retry_boundary + 1,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        // The winning CAS returns Ok(old_value) — the value that was replaced.
        // The losing CAS returns Err(current_value) — what it found instead.
        assert_eq!(
            a,
            Ok(retry_boundary),
            "CAS winner must return Ok(old value) == Ok(retry_boundary)"
        );
        assert_eq!(
            b,
            Err(retry_boundary + 1),
            "CAS loser must return Err(current value) == Err(retry_boundary + 1)"
        );
    }
}
