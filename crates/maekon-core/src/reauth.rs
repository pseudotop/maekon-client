//! Capture-history re-authentication gate — mitigates physical-access threats.
//!
//! Anyone with physical access to an unlocked device can open the app and view
//! the entire captured screenshot timeline with no additional authentication.
//! This module provides a **session gate** that requires OS biometrics (Touch
//! ID / Windows Hello) or an app PIN re-auth before entering the capture
//! history / replay / timeline views. It is a "view" gate distinct from
//! `require_local_auth` (the session token) — the token protects against other
//! local processes, whereas this gate protects against a physical accessor on
//! an already-authenticated session (the same goal as Microsoft Recall's
//! post-backlash fix).
//!
//! # Fail-closed
//! When the gate is `enabled` and there is no valid re-auth, viewing is
//! denied. A failed/cancelled auth attempt never opens the gate. When the
//! feature is disabled (`enabled=false`) the gate always passes (no-op) — an
//! explicit user opt-out.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Re-authentication method.
///
/// Tells the backend which method the frontend attempted. On platforms with
/// no biometric support (or no available hardware) the frontend falls back
/// to `Pin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReauthMethod {
    /// OS biometric/system authentication (Touch ID, Windows Hello, watch, login password).
    Biometric,
    /// App-level PIN (enrolled in Settings). The baseline for platforms without biometrics.
    Pin,
}

/// Result of a re-authentication attempt.
///
/// Only `Authenticated` opens the gate. Every other variant (cancelled /
/// failed / unsupported) is fail-closed — viewing stays denied. The frontend
/// uses this to decide the prompt state (retry / PIN fallback / error).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome", content = "detail")]
pub enum ReauthOutcome {
    /// Authentication succeeded — opens the gate.
    Authenticated,
    /// The user cancelled the prompt.
    Cancelled,
    /// Authentication failed (wrong biometric/PIN, system error, etc.).
    Failed(String),
    /// This method is unavailable on this platform/configuration (e.g. no
    /// biometric hardware). The frontend should fall back to PIN.
    Unsupported,
}

impl ReauthOutcome {
    /// Whether this outcome opens the gate.
    #[must_use]
    pub const fn is_authenticated(&self) -> bool {
        matches!(self, Self::Authenticated)
    }
}

/// Snapshot of platform re-authentication capabilities.
///
/// Used by the frontend to decide which re-auth UI to show (biometric button
/// vs. PIN input).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReauthCapabilities {
    /// Whether OS biometric/system authentication is currently available.
    pub biometric_available: bool,
    /// Human-readable name of the available biometric method ("Touch ID",
    /// "Windows Hello"). `None` when `biometric_available=false`.
    pub biometric_kind: Option<String>,
}

/// Port that performs OS biometric/system re-authentication.
///
/// Implemented by platform-specific adapters (`#[cfg(target_os)]`):
/// - macOS: `LocalAuthentication` (LAContext, Touch ID/watch/password)
/// - Windows: Windows Hello (`UserConsentVerifier`)
/// - Linux: no standard biometric API → returns `Unsupported` (PIN fallback)
///
/// PIN verification is handled by a separate secret-store-backed path, not
/// through this port (the storage mechanism differs from biometrics).
#[async_trait::async_trait]
pub trait ReauthVerifierPort: Send + Sync {
    /// Returns the current platform's biometric re-authentication
    /// capabilities (synchronous — hardware probe only).
    fn capabilities(&self) -> ReauthCapabilities;

    /// Shows the OS biometric/system re-authentication prompt and returns
    /// the outcome.
    ///
    /// `reason` is the reason string shown in the prompt (e.g. "View capture
    /// history"). Platforms without biometric support return `Unsupported`
    /// (the caller then falls back to PIN).
    async fn verify_biometric(&self, reason: &str) -> ReauthOutcome;
}

/// Snapshot of the re-authentication session gate's current state (for
/// frontend/command responses).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReauthGateStatus {
    /// Whether the re-auth gate is enabled (config value). `false` means
    /// viewing is always allowed.
    pub enabled: bool,
    /// Idle expiry in seconds. Re-auth is required again once this much time
    /// has elapsed since the last successful authentication.
    pub idle_timeout_secs: u64,
    /// Whether a currently-valid re-auth session exists (i.e. whether
    /// viewing is allowed right now).
    pub authenticated: bool,
}

/// Pure idle-window check.
///
/// Returns true when the elapsed time from the last authentication time
/// `last` to `now` is within `idle_timeout`. When `now < last` (clock
/// regression / tests), `saturating_duration_since` yields 0, so this safely
/// returns true. `now` is an explicit parameter so this stays unit-testable.
fn within_idle_window(last: Option<Instant>, now: Instant, idle_timeout: Duration) -> bool {
    last.is_some_and(|last| now.saturating_duration_since(last) <= idle_timeout)
}

#[derive(Debug)]
struct GateState {
    enabled: bool,
    idle_timeout: Duration,
    /// Timestamp of the last successful re-authentication. `None` means
    /// unauthenticated (locked).
    last_authenticated: Option<Instant>,
}

/// Capture-history viewing re-authentication session gate.
///
/// The web middleware (`require_capture_reauth`) reads it via `is_satisfied()`,
/// and the Tauri re-auth command writes it via `record_success()`. Both
/// crates share the **same `Arc`**. Interior mutability (Mutex) lets state be
/// updated through `&self` alone.
#[derive(Debug)]
pub struct CaptureReauthGate {
    inner: Mutex<GateState>,
}

impl CaptureReauthGate {
    /// Creates a gate with the given configuration.
    #[must_use]
    pub fn new(enabled: bool, idle_timeout: Duration) -> Self {
        Self {
            inner: Mutex::new(GateState {
                enabled,
                idle_timeout,
                last_authenticated: None,
            }),
        }
    }

    /// A disabled (always-pass) gate. The test/unwired default — not
    /// fail-open, but an explicit expression of "feature off" (only an
    /// enabled gate ever blocks viewing).
    #[must_use]
    pub fn disabled() -> Self {
        Self::new(false, Duration::from_secs(0))
    }

    /// Whether viewing is currently allowed (the middleware's entry check).
    ///
    /// - Always true when disabled (no-op).
    /// - When enabled, true only if there is a valid re-auth session (within
    ///   the idle window).
    #[must_use]
    pub fn is_satisfied(&self) -> bool {
        let guard = self.inner.lock();
        if !guard.enabled {
            return true;
        }
        within_idle_window(guard.last_authenticated, Instant::now(), guard.idle_timeout)
    }

    /// Records a successful re-authentication (resets the idle timer).
    pub fn record_success(&self) {
        self.inner.lock().last_authenticated = Some(Instant::now());
    }

    /// Re-locks — invalidates any valid re-auth session (app foreground
    /// re-entry / manual lock).
    pub fn lock(&self) {
        self.inner.lock().last_authenticated = None;
    }

    /// Updates the configuration (enabled / idle timeout).
    ///
    /// If the gate just transitioned to enabled, the session is invalidated
    /// so it starts locked — preventing a just-enabled gate from being open
    /// because of a stale (pre-existing) authentication. If only the idle
    /// timeout changes, the existing session is preserved.
    pub fn update_config(&self, enabled: bool, idle_timeout: Duration) {
        let mut guard = self.inner.lock();
        let was_enabled = guard.enabled;
        guard.enabled = enabled;
        guard.idle_timeout = idle_timeout;
        if enabled && !was_enabled {
            guard.last_authenticated = None;
        }
    }

    /// Returns a snapshot of the current state (for frontend/command
    /// responses).
    #[must_use]
    pub fn status(&self) -> ReauthGateStatus {
        let guard = self.inner.lock();
        let authenticated = if guard.enabled {
            within_idle_window(guard.last_authenticated, Instant::now(), guard.idle_timeout)
        } else {
            true
        };
        ReauthGateStatus {
            enabled: guard.enabled,
            idle_timeout_secs: guard.idle_timeout.as_secs(),
            authenticated,
        }
    }
}

impl Default for CaptureReauthGate {
    /// Defaults to a disabled gate — until wired, it never blocks viewing.
    /// The production composition root injects an enabled gate from config.
    fn default() -> Self {
        Self::disabled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_window_none_last_is_never_authenticated() {
        let now = Instant::now();
        assert!(!within_idle_window(None, now, Duration::from_secs(300)));
    }

    #[test]
    fn within_window_recent_auth_is_valid() {
        let base = Instant::now();
        let now = base + Duration::from_secs(60);
        assert!(within_idle_window(
            Some(base),
            now,
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn within_window_expired_auth_is_invalid() {
        let base = Instant::now();
        let now = base + Duration::from_secs(301);
        assert!(!within_idle_window(
            Some(base),
            now,
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn within_window_boundary_is_inclusive() {
        let base = Instant::now();
        let now = base + Duration::from_secs(300);
        assert!(within_idle_window(
            Some(base),
            now,
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn within_window_clock_regression_is_saturating() {
        // now < last (clock regression): saturates to 0 elapsed → valid.
        let base = Instant::now();
        let now = base;
        assert!(within_idle_window(
            Some(base + Duration::from_secs(10)),
            now,
            Duration::from_secs(300)
        ));
    }

    #[test]
    fn disabled_gate_always_satisfied() {
        let gate = CaptureReauthGate::disabled();
        // Passes even with no auth record, because it's disabled.
        assert!(gate.is_satisfied());
        let status = gate.status();
        assert!(!status.enabled);
        assert!(status.authenticated);
    }

    #[test]
    fn enabled_gate_starts_locked() {
        let gate = CaptureReauthGate::new(true, Duration::from_secs(300));
        // An enabled gate is fail-closed until re-authenticated.
        assert!(!gate.is_satisfied());
    }

    #[test]
    fn enabled_gate_opens_after_record_success() {
        let gate = CaptureReauthGate::new(true, Duration::from_secs(300));
        gate.record_success();
        assert!(gate.is_satisfied());
        let status = gate.status();
        assert!(status.enabled);
        assert!(status.authenticated);
        assert_eq!(status.idle_timeout_secs, 300);
    }

    #[test]
    fn lock_reverts_to_locked() {
        let gate = CaptureReauthGate::new(true, Duration::from_secs(300));
        gate.record_success();
        assert!(gate.is_satisfied());
        gate.lock();
        // Fail-closed again after re-locking.
        assert!(!gate.is_satisfied());
    }

    #[test]
    fn zero_idle_timeout_expires_immediately() {
        // A 0-second idle timeout means re-auth on every request (the
        // strictest setting). Even immediately after record, Instant::now()
        // has advanced by at least a few nanoseconds, so it may already
        // count as expired.
        let gate = CaptureReauthGate::new(true, Duration::from_secs(0));
        gate.record_success();
        // A 0-second window expires the instant any time elapses —
        // is_satisfied should safely be false.
        // (Boundary: exactly 0 elapsed is inclusive, but `now` is always
        // strictly after the record timestamp.)
        // This test documents that a very strict setting converges to
        // fail-closed.
        // Since nanosecond-scale elapsed time is always > 0, this is false.
        assert!(!gate.is_satisfied());
    }

    #[test]
    fn update_config_enable_starts_locked_even_with_prior_success() {
        // If record_success happened while disabled and the gate is then
        // enabled, it must not be open just because of a prior
        // authentication (it must re-lock).
        let gate = CaptureReauthGate::new(false, Duration::from_secs(300));
        gate.record_success();
        assert!(gate.is_satisfied()); // passes because it's disabled
        gate.update_config(true, Duration::from_secs(300));
        assert!(!gate.is_satisfied()); // re-locks on enable
    }

    #[test]
    fn update_config_idle_only_keeps_session() {
        let gate = CaptureReauthGate::new(true, Duration::from_secs(300));
        gate.record_success();
        assert!(gate.is_satisfied());
        // Stays enabled, only the idle timeout changes → session is kept.
        gate.update_config(true, Duration::from_secs(600));
        assert!(gate.is_satisfied());
    }

    #[test]
    fn update_config_disable_opens_gate() {
        let gate = CaptureReauthGate::new(true, Duration::from_secs(300));
        assert!(!gate.is_satisfied());
        gate.update_config(false, Duration::from_secs(300));
        assert!(gate.is_satisfied());
    }

    #[test]
    fn outcome_only_authenticated_opens() {
        assert!(ReauthOutcome::Authenticated.is_authenticated());
        assert!(!ReauthOutcome::Cancelled.is_authenticated());
        assert!(!ReauthOutcome::Failed("x".into()).is_authenticated());
        assert!(!ReauthOutcome::Unsupported.is_authenticated());
    }

    #[test]
    fn outcome_serde_round_trip_tagged() {
        let json = serde_json::to_string(&ReauthOutcome::Authenticated).unwrap();
        assert!(json.contains("authenticated"));
        let failed = serde_json::to_string(&ReauthOutcome::Failed("bad pin".into())).unwrap();
        let back: ReauthOutcome = serde_json::from_str(&failed).unwrap();
        assert_eq!(back, ReauthOutcome::Failed("bad pin".into()));
    }

    #[test]
    fn method_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&ReauthMethod::Biometric).unwrap(),
            "\"biometric\""
        );
        assert_eq!(
            serde_json::to_string(&ReauthMethod::Pin).unwrap(),
            "\"pin\""
        );
    }
}
