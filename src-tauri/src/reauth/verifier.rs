//! #8044: platform OS biometric/system re-authentication adapter
//! (`ReauthVerifierPort` implementation).
//!
//! - **macOS**: `LocalAuthentication` (LAContext). Uses
//!   `LAPolicyDeviceOwnerAuthentication` — Touch ID first, falling back to
//!   the device password on failure (so even a Mac without biometric
//!   hardware can re-authenticate via the system password). The prompt's
//!   async completion block is bridged to a sync channel + timeout.
//! - **Windows / Linux**: currently returns `Unsupported` (an explicit
//!   degrade) → the caller falls back to the app PIN. Native Windows Hello
//!   (`UserConsentVerifier`) / Linux polkit integration is a follow-up (the
//!   port contract already accommodates it).
//!
//! **Fail-safe**: on any path — missing class, error, timeout — this returns
//! `Unsupported`/`Failed`. The re-auth gate is never opened by accident; it
//! always converges to the PIN fallback.

use maekon_core::reauth::{ReauthCapabilities, ReauthOutcome, ReauthVerifierPort};

/// Platform biometric re-authentication verifier. Stateless (calls the
/// platform API fresh each time).
#[derive(Debug, Default, Clone, Copy)]
pub struct PlatformReauthVerifier;

impl PlatformReauthVerifier {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl ReauthVerifierPort for PlatformReauthVerifier {
    fn capabilities(&self) -> ReauthCapabilities {
        platform::capabilities()
    }

    async fn verify_biometric(&self, reason: &str) -> ReauthOutcome {
        let reason = reason.to_string();
        // The biometric prompt blocks waiting on user input → isolate it on
        // spawn_blocking so it doesn't starve the async worker pool (F-RR-06).
        tokio::task::spawn_blocking(move || platform::verify_biometric(&reason))
            .await
            .unwrap_or_else(|error| {
                ReauthOutcome::Failed(format!("re-auth task join failed: {error}"))
            })
    }
}

// ───────────────────────── macOS: LocalAuthentication ─────────────────────────
#[cfg(target_os = "macos")]
mod platform {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use block2::RcBlock;
    use maekon_core::reauth::{ReauthCapabilities, ReauthOutcome};
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool};
    use objc2_foundation::NSString;

    /// LAPolicyDeviceOwnerAuthenticationWithBiometrics — biometrics only.
    const LA_POLICY_BIOMETRICS: isize = 1;
    /// LAPolicyDeviceOwnerAuthentication — biometrics, falling back to the
    /// device password on failure.
    const LA_POLICY_DEVICE_OWNER_AUTH: isize = 2;
    /// LABiometryTypeFaceID.
    const LA_BIOMETRY_FACE_ID: isize = 2;
    /// Upper bound on how long to wait for the biometric prompt (generous,
    /// to account for user interaction).
    const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

    pub(super) fn capabilities() -> ReauthCapabilities {
        // SAFETY: LAContext alloc/init + canEvaluatePolicy is a thread-safe
        // query that never shows UI. The class is looked up via
        // AnyClass::get and null-checked, and each selector matches the
        // receiver's real signature. The context is released synchronously
        // within this function.
        unsafe {
            let Some(la_context_class) = AnyClass::get(c"LAContext") else {
                return ReauthCapabilities::default();
            };
            let context: *mut AnyObject = msg_send![la_context_class, alloc];
            let context: *mut AnyObject = msg_send![context, init];
            if context.is_null() {
                return ReauthCapabilities::default();
            }
            let can: Bool = msg_send![
                context,
                canEvaluatePolicy: LA_POLICY_BIOMETRICS,
                error: std::ptr::null_mut::<*mut AnyObject>()
            ];
            let available = can.as_bool();
            let kind = if available {
                let biometry_type: isize = msg_send![context, biometryType];
                Some(if biometry_type == LA_BIOMETRY_FACE_ID {
                    "Face ID".to_string()
                } else {
                    "Touch ID".to_string()
                })
            } else {
                None
            };
            let _: () = msg_send![context, release];
            ReauthCapabilities {
                biometric_available: available,
                biometric_kind: kind,
            }
        }
    }

    pub(super) fn verify_biometric(reason: &str) -> ReauthOutcome {
        // SAFETY: allocates/inits an LAContext (kept alive via leak) and
        // calls evaluatePolicy asynchronously. The reply block (RcBlock) is
        // kept alive via mem::forget because the framework invokes it
        // asynchronously after this call returns (mirrors the verified
        // desktop_permissions notification-permission pattern). Each
        // selector matches the receiver's real signature.
        unsafe {
            let Some(la_context_class) = AnyClass::get(c"LAContext") else {
                return ReauthOutcome::Unsupported;
            };
            let context: *mut AnyObject = msg_send![la_context_class, alloc];
            let context: *mut AnyObject = msg_send![context, init];
            if context.is_null() {
                return ReauthOutcome::Unsupported;
            }

            let reason_ns = NSString::from_str(reason);

            let (tx, rx) = std::sync::mpsc::sync_channel::<ReauthOutcome>(1);
            let tx = Arc::new(Mutex::new(Some(tx)));
            let sender = Arc::clone(&tx);
            let handler = RcBlock::new(move |success: Bool, error: *mut AnyObject| {
                let outcome = if success.as_bool() {
                    ReauthOutcome::Authenticated
                } else {
                    map_la_error(error)
                };
                send_once(&sender, outcome);
            });

            let _: () = msg_send![
                context,
                evaluatePolicy: LA_POLICY_DEVICE_OWNER_AUTH,
                localizedReason: &*reason_ns,
                reply: &*handler
            ];

            // evaluatePolicy invokes reply asynchronously. Keep the
            // block/context alive beyond this stack frame (leak) — the
            // prompt is a rare, user-initiated event, so the memory impact
            // is negligible.
            std::mem::forget(handler);

            rx.recv_timeout(PROMPT_TIMEOUT)
                .unwrap_or_else(|_| ReauthOutcome::Failed("biometric prompt timed out".to_string()))
        }
    }

    /// Maps an NSError code to a re-authentication outcome.
    ///
    /// LAError: userCancel(-2)/userFallback(-3)/systemCancel(-4)/appCancel(-9)
    /// are treated as cancellation; everything else is treated as a failure
    /// (fail-closed — the gate never opens).
    ///
    /// # SAFETY
    /// `error` must be a live `NSError *` handed in by the framework (or null).
    unsafe fn map_la_error(error: *mut AnyObject) -> ReauthOutcome {
        if error.is_null() {
            return ReauthOutcome::Failed("biometric authentication failed".to_string());
        }
        let code: isize = msg_send![&*error, code];
        match code {
            -2 | -3 | -4 | -9 => ReauthOutcome::Cancelled,
            _ => {
                let desc: Retained<NSString> = msg_send![&*error, localizedDescription];
                ReauthOutcome::Failed(desc.to_string())
            }
        }
    }

    /// Sends the outcome on the channel exactly once (safe even if reply is
    /// invoked twice).
    fn send_once(
        sender: &Arc<Mutex<Option<std::sync::mpsc::SyncSender<ReauthOutcome>>>>,
        value: ReauthOutcome,
    ) {
        if let Ok(mut guard) = sender.lock() {
            if let Some(sender) = guard.take() {
                let _ = sender.send(value);
            }
        }
    }
}

// ───────────────────────── Windows / Linux: PIN fallback ─────────────────────────
// An explicit degrade — no native biometric integration yet. The caller (the
// command) falls back to the app PIN. Windows Hello (UserConsentVerifier) /
// Linux polkit integration is a follow-up; the port contract
// (ReauthVerifierPort) is already designed so only this module needs
// swapping when that lands.
#[cfg(not(target_os = "macos"))]
mod platform {
    use maekon_core::reauth::{ReauthCapabilities, ReauthOutcome};

    pub(super) fn capabilities() -> ReauthCapabilities {
        ReauthCapabilities {
            biometric_available: false,
            biometric_kind: None,
        }
    }

    pub(super) fn verify_biometric(_reason: &str) -> ReauthOutcome {
        // No biometric support → signal the caller to use the PIN fallback.
        ReauthOutcome::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_capabilities_never_panics() {
        // capabilities() must safely return a snapshot on any platform/environment.
        let caps = PlatformReauthVerifier::new().capabilities();
        // If biometrics are unavailable, kind must be None (consistency).
        if !caps.biometric_available {
            assert!(caps.biometric_kind.is_none());
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn non_macos_biometric_is_unsupported() {
        use maekon_core::reauth::ReauthOutcome;
        let outcome = PlatformReauthVerifier::new()
            .verify_biometric("test reason")
            .await;
        assert_eq!(outcome, ReauthOutcome::Unsupported);
    }
}
