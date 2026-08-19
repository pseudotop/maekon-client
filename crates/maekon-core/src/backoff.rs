//! Deterministic exponential backoff curve (#7725).
//!
//! `exponential_delay` computes `base * 2^min(attempt, MAX_BACKOFF_EXPONENT)`,
//! capped at `max`. It carries NO randomness — callers that want retry jitter
//! (to avoid synchronized retry storms across many clients hitting the same
//! server) layer that on top, as `maekon-network::resilience::jittered_backoff_delay`
//! does.
//!
//! Lives in `maekon-core` (std time only, zero new dependencies) rather than in
//! `maekon-network` because at least one caller — `SyncEngine`'s erasure-event
//! retry backoff in the `maekon-app` composition root — is compiled
//! unconditionally, while `maekon-network` is an optional dependency gated
//! behind the `analysis`/`server` feature flags. A caller compiled without
//! those features could not resolve a `maekon-network` import. Putting the
//! pure math here (mirroring the existing `circuit_breaker` precedent in this
//! crate) lets every adapter — feature-gated or not — share ONE
//! implementation instead of hand-rolling per-site copies that can silently
//! drift out of sync (the original motivation for #7725: `sse_client.rs`,
//! `batch_uploader.rs`, and `suggestions.rs` had each re-implemented this
//! curve independently, so a correctness fix to one never propagated to the
//! others).
//!
//! `SyncEngine`'s `ErasureBackoff` in particular pins an EXACT (non-jittered)
//! delay in its tests (`undeliverable_erasure_backs_off_then_retries` advances
//! a paused clock by exactly 31s to cross a 30s backoff window), so it
//! deliberately calls this deterministic function directly rather than the
//! jittered variant.

use std::time::Duration;

/// Exponent ceiling — `2^10 == 1024`, more than enough headroom that in
/// practice `max` (not the exponent cap) is what bounds the delay for any
/// caller's real `attempt` range.
pub const MAX_BACKOFF_EXPONENT: u32 = 10;

/// Deterministic exponential backoff delay for the `attempt`-th retry
/// (0-based: `attempt == 0` is the first retry).
///
/// `base * 2^min(attempt, MAX_BACKOFF_EXPONENT)`, capped at `max`. Returns
/// zero when either `base` or `max` is zero (a degenerate/disabled backoff
/// configuration), so callers do not need to special-case that themselves.
pub fn exponential_delay(attempt: u32, base: Duration, max: Duration) -> Duration {
    let base_ms = base.as_millis().min(u128::from(u64::MAX)) as u64;
    let max_ms = max.as_millis().min(u128::from(u64::MAX)) as u64;
    if base_ms == 0 || max_ms == 0 {
        return Duration::from_millis(0);
    }

    let exp_ms = base_ms.saturating_mul(2u64.saturating_pow(attempt.min(MAX_BACKOFF_EXPONENT)));
    Duration::from_millis(exp_ms.min(max_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_per_attempt_until_capped() {
        let base = Duration::from_secs(1);
        let max = Duration::from_secs(30);
        assert_eq!(exponential_delay(0, base, max), Duration::from_secs(1));
        assert_eq!(exponential_delay(1, base, max), Duration::from_secs(2));
        assert_eq!(exponential_delay(2, base, max), Duration::from_secs(4));
        assert_eq!(exponential_delay(3, base, max), Duration::from_secs(8));
        assert_eq!(exponential_delay(4, base, max), Duration::from_secs(16));
        assert_eq!(exponential_delay(5, base, max), Duration::from_secs(30)); // capped
    }

    #[test]
    fn never_exceeds_max_for_large_attempts() {
        let base = Duration::from_secs(1);
        let max = Duration::from_secs(30);
        for attempt in [6u32, 10, 32, 63, u32::MAX] {
            assert!(exponential_delay(attempt, base, max) <= max);
        }
    }

    #[test]
    fn zero_base_or_max_collapses_to_zero() {
        assert_eq!(
            exponential_delay(5, Duration::ZERO, Duration::from_secs(30)),
            Duration::ZERO
        );
        assert_eq!(
            exponential_delay(5, Duration::from_secs(1), Duration::ZERO),
            Duration::ZERO
        );
    }

    #[test]
    fn matches_erasure_backoff_schedule() {
        // Pins the SyncEngine ErasureBackoff tuning (base=30s, max=30min) used
        // in src-tauri/src/sync_engine.rs so a change to this shared curve
        // cannot silently shift that site's schedule without a test failure
        // here too.
        let base = Duration::from_secs(30);
        let max = Duration::from_secs(1800);
        assert_eq!(exponential_delay(0, base, max), Duration::from_secs(30));
        assert_eq!(exponential_delay(1, base, max), Duration::from_secs(60));
        assert_eq!(exponential_delay(2, base, max), Duration::from_secs(120));
        assert_eq!(exponential_delay(6, base, max), Duration::from_secs(1800)); // capped
    }
}
