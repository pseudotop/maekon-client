//! #8044: capture-history re-authentication runtime — platform biometric
//! verifier + PIN fallback + shared gate.
//!
//! - [`verifier`] — platform OS biometric/system re-authentication adapter
//!   (`ReauthVerifierPort`).
//! - [`pin`] — Argon2id PIN fallback hash storage/verification.
//! - [`ReauthRuntimeState`] — Tauri managed state: the `Arc<CaptureReauthGate>`
//!   **shared** with the web middleware + the platform verifier. The re-auth
//!   command opens (`record_success`) and locks (`lock`) the gate through
//!   this state.

pub(crate) mod pin;
pub(crate) mod verifier;

use std::sync::Arc;

use maekon_core::reauth::{CaptureReauthGate, ReauthVerifierPort};

/// Capture-history re-authentication Tauri managed state.
///
/// `gate` is the **same `Arc`** `web_server_runtime` injects as the web
/// `AppState.auth.reauth_gate` — when a command opens it here via
/// `record_success()`, the web middleware sees it immediately. `verifier` is
/// the platform biometric adapter.
#[derive(Clone)]
pub struct ReauthRuntimeState {
    pub gate: Arc<CaptureReauthGate>,
    pub verifier: Arc<dyn ReauthVerifierPort>,
}

impl ReauthRuntimeState {
    #[must_use]
    pub fn new(gate: Arc<CaptureReauthGate>, verifier: Arc<dyn ReauthVerifierPort>) -> Self {
        Self { gate, verifier }
    }
}
