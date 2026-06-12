//! Circuit breaker for AT-SPI2 connection failures.
//!
//! Mirrors the same pattern used by macOS and Windows extractors.
//! After `CIRCUIT_BREAKER_THRESHOLD` consecutive failures the circuit opens,
//! and retries are attempted every `CIRCUIT_BREAKER_RETRY_INTERVAL` ticks.

#[cfg(feature = "linux-atspi")]
use std::sync::atomic::{AtomicU32, Ordering};
#[cfg(feature = "linux-atspi")]
use tracing::warn;

#[cfg(feature = "linux-atspi")]
/// Consecutive AT-SPI2 failures before the circuit breaker opens.
pub(super) static CONSECUTIVE_FAILURES: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "linux-atspi")]
pub(super) const CIRCUIT_BREAKER_THRESHOLD: u32 = 3;
#[cfg(feature = "linux-atspi")]
/// Retry every 10 ticks (~30 s at 3 s poll) after circuit opens.
pub(super) const CIRCUIT_BREAKER_RETRY_INTERVAL: u32 = 10;

/// Returns `true` if the circuit allows an AT-SPI2 call to proceed.
///
/// When the failure count reaches `CIRCUIT_BREAKER_THRESHOLD`, further calls
/// are blocked and the counter is incremented so that one retry attempt is
/// allowed every `CIRCUIT_BREAKER_RETRY_INTERVAL` ticks.
#[cfg(feature = "linux-atspi")]
pub(super) fn circuit_allows() -> bool {
    let failures = CONSECUTIVE_FAILURES.load(Ordering::Relaxed);
    if failures >= CIRCUIT_BREAKER_THRESHOLD {
        if !failures.is_multiple_of(CIRCUIT_BREAKER_RETRY_INTERVAL) {
            CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        warn!(
            "LinuxAccessibility: circuit breaker retry after {} skipped",
            failures - CIRCUIT_BREAKER_THRESHOLD
        );
    }
    true
}

#[cfg(feature = "linux-atspi")]
pub(super) fn record_success() {
    CONSECUTIVE_FAILURES.store(0, Ordering::Relaxed);
}

#[cfg(feature = "linux-atspi")]
pub(super) fn record_failure() {
    let prev = CONSECUTIVE_FAILURES.fetch_add(1, Ordering::Relaxed);
    if prev + 1 == CIRCUIT_BREAKER_THRESHOLD {
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
        true // Stub mode: claim available
    }
}
