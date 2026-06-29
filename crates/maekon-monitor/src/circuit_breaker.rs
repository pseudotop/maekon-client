//! Subprocess-spawn circuit breaker (#6828).
//!
//! After a helper binary (e.g. `xdotool`) is missing or hangs for `threshold`
//! consecutive monitor ticks, the breaker opens and only one caller per
//! `retry_interval` boundary is allowed to retry — so a host without the binary
//! (or one where it repeatedly times out) cannot make the monitor fork a
//! subprocess on every tick. Mirrors the macOS osascript breaker
//! (`macos::circuit_breaker_should_proceed`); kept cfg-free so the state machine
//! is unit-testable on any OS.

use std::sync::atomic::{AtomicU32, Ordering};

/// Atomic consecutive-failure circuit breaker for guarding subprocess spawns.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct CircuitBreaker {
    consecutive_failures: AtomicU32,
    threshold: u32,
    retry_interval: u32,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl CircuitBreaker {
    pub(crate) const fn new(threshold: u32, retry_interval: u32) -> Self {
        Self {
            consecutive_failures: AtomicU32::new(0),
            threshold,
            retry_interval,
        }
    }

    /// Returns `true` if the caller may spawn the subprocess, `false` if it must
    /// short-circuit. While the breaker is open it only returns `true` for the
    /// single caller that atomically claims a `retry_interval` boundary slot
    /// (the `compare_exchange` prevents two concurrent callers from both
    /// proceeding); every other call increments-and-skips.
    pub(crate) fn should_proceed(&self) -> bool {
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

    /// Record that the subprocess actually executed — closes the breaker.
    pub(crate) fn record_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    /// Record that the subprocess was absent (`NotFound`) or hung (timeout) —
    /// advances the breaker toward opening.
    pub(crate) fn record_failure(&self) {
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
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
}
