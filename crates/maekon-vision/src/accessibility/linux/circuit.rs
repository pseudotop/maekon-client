//! Circuit breaker for AT-SPI2 connection failures.
//!
//! Mirrors the same pattern used by the macOS and Windows extractors, and
//! delegates to the shared `maekon_core::circuit_breaker::CircuitBreaker`
//! (#7720 E6 consolidation). This module previously hand-rolled its own
//! `AtomicU32` state machine, which had drifted to a version *missing* the
//! `compare_exchange` retry-slot claim (#6007 finding 17) that the shared
//! struct carries — without it, two concurrent callers that both observe the
//! counter at the same retry-interval boundary would both pass the gate and
//! both issue an AT-SPI2 call.
//!
//! After `CIRCUIT_BREAKER_THRESHOLD` consecutive failures the circuit opens,
//! and retries are attempted every `CIRCUIT_BREAKER_RETRY_INTERVAL` ticks.

#[cfg(feature = "linux-atspi")]
use maekon_core::circuit_breaker::CircuitBreaker;
#[cfg(feature = "linux-atspi")]
use tracing::warn;

#[cfg(feature = "linux-atspi")]
pub(super) const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
#[cfg(feature = "linux-atspi")]
/// Retry every 10 ticks (~30 s at 3 s poll) after circuit opens.
pub(super) const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 10;

#[cfg(feature = "linux-atspi")]
static BREAKER: CircuitBreaker =
    CircuitBreaker::new(CIRCUIT_BREAKER_THRESHOLD, CIRCUIT_BREAKER_RETRY_INTERVAL);

/// Returns `true` if the circuit allows an AT-SPI2 call to proceed.
///
/// When the failure count reaches `CIRCUIT_BREAKER_THRESHOLD`, further calls
/// are blocked and the counter is incremented so that one retry attempt is
/// allowed every `CIRCUIT_BREAKER_RETRY_INTERVAL` ticks.
#[cfg(feature = "linux-atspi")]
pub(super) fn circuit_allows() -> bool {
    BREAKER.should_proceed()
}

#[cfg(feature = "linux-atspi")]
pub(super) fn record_success() {
    BREAKER.record_success();
}

#[cfg(feature = "linux-atspi")]
pub(super) fn record_failure() {
    BREAKER.record_failure();
    if BREAKER.failure_count() == CIRCUIT_BREAKER_THRESHOLD {
        warn!(
            "LinuxAccessibility: circuit breaker tripped after {CIRCUIT_BREAKER_THRESHOLD} consecutive failures"
        );
    }
}

/// Check if the AT-SPI2 D-Bus service is reachable by inspecting environment
/// variables set by the desktop session manager.
pub(super) fn check_atspi_available() -> bool {
    #[cfg(feature = "linux-atspi")]
    {
        std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
            || std::env::var("ATSPI_BUS_ADDRESS").is_ok()
    }
    #[cfg(not(feature = "linux-atspi"))]
    {
        // The AT-SPI extractor is compiled out, so extraction is hard-stubbed to
        // empty. Report the capability as UNAVAILABLE rather than claiming it is
        // ready (review4 V20) — a `true` here made desktop_permissions surface
        // "linux_atspi_ready" (green) while every extraction silently returned
        // nothing. has_permission()=false → the permission UI shows needs-attention
        // and the capture path short-circuits honestly.
        false
    }
}
