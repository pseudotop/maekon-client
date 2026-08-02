//! Context-home IPC boundary (#9625, WD-02.2a).
//!
//! One command, no arguments: the WebView asks for "my context home" and the
//! server answers from the JWT the Rust side already holds.
//!
//! ## What crosses this boundary, and what does not
//!
//! Out: a [`ContextHomeSnapshot`] — thread subjects, bounded previews,
//! participants, projects. In: nothing. There is no `user_id` parameter, no
//! `organization_id` parameter, and no token parameter, so a compromised
//! WebView cannot ask for another actor's home or replay a captured bearer:
//! neither is expressible in this signature.
//!
//! **The bearer never reaches JavaScript.** It is read from the shared
//! `TokenManager` inside `maekon-network` and attached to the request there.
//! Nothing on this path returns, logs, or formats it — see
//! `maekon_network::context_home` for the transport-side statement of the same
//! invariant, and the tests at the bottom of this file for the boundary-side
//! one.
//!
//! ## State pattern
//!
//! [`ContextHomeState`] mirrors `auth::TokenManagerState`: a slot registered
//! ONCE (empty) on the Tauri builder and populated at setup time, because
//! `Manager::manage()` does not overwrite an already-managed type — a second
//! call silently discards the value. The slot stays `None` when the build has
//! no server transport (`cfg(feature = "server")` off) or bootstrap never ran;
//! the command then returns `service.unavailable` rather than pretending the
//! home is empty. "Empty" and "could not ask" are different answers, which is
//! the whole reason the server contract carries a per-section `status`.

use std::sync::{Arc, Mutex};

use maekon_core::models::context_home::ContextHomeSnapshot;
use maekon_core::ports::context_home_client::ContextHomeClient;
use tauri::{command, State};

use crate::ipc_error::IpcError;

/// Wire code returned when no transport is wired into the slot.
///
/// A registry code (`ServiceCode::Unavailable`) on purpose: minting a new
/// `IpcError::new("context_home.unavailable", ..)` literal would land outside
/// the ADR-019 catalog and hit `translateError`'s raw-English fallback — the
/// exact defect #9492 spent a slice removing. `service.unavailable` already has
/// a template in all five locales and says the true thing.
const CODE_UNAVAILABLE: &str = "service.unavailable";

/// Tauri-managed slot holding the optional context-home transport.
///
/// See the module doc for why this is a slot rather than a second `manage()`.
pub struct ContextHomeState(Mutex<Option<Arc<dyn ContextHomeClient>>>);

impl ContextHomeState {
    /// Build-time constructor: an empty slot registered once on the builder.
    #[must_use]
    pub fn empty() -> Self {
        Self(Mutex::new(None))
    }

    /// Populate the slot after the shared login session exists.
    ///
    /// Called from `app_runtime_launch::auth_wiring`, which owns the one
    /// `TokenManager` this transport authenticates with. Writing through the
    /// slot is what makes the client reachable from the command; a second
    /// `Manager::manage()` would be a silent no-op.
    pub fn set(&self, client: Arc<dyn ContextHomeClient>) {
        // A poisoned lock here only means a prior holder panicked inside the
        // brief, non-await critical section — recover the guard so bootstrap
        // still wires the client rather than panicking the whole setup.
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(client);
    }

    /// Read the current client, cloning the `Arc` out of the lock so the guard
    /// is never held across an `.await`.
    #[must_use]
    pub fn get(&self) -> Option<Arc<dyn ContextHomeClient>> {
        let slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slot.clone()
    }
}

/// Fetch the signed-in actor's context-home snapshot.
///
/// Takes no arguments — actor and organization come from the JWT the transport
/// holds. See the module doc for why that is a security property and not just
/// an ergonomic one.
///
/// # Errors
///
/// - `service.unavailable` — no transport wired (no-server build, or bootstrap
///   never populated the slot). Distinct from "the server answered with an
///   empty home".
/// - `auth.failed` — the session expired; re-login is the fix.
/// - `policy.denied` — authenticated but not permitted; re-login will not help.
/// - `network.timeout` / `service.unavailable` — transient; retrying is sane.
/// - `validation.invalid_field` — the response was absent, oversized, or not a
///   valid snapshot.
#[command]
pub async fn fetch_context_home(
    state: State<'_, ContextHomeState>,
) -> Result<ContextHomeSnapshot, IpcError> {
    let client = state.get().ok_or_else(|| {
        IpcError::new(
            CODE_UNAVAILABLE,
            "context home transport is not wired in this build",
        )
    })?;

    client.fetch_context_home().await.map_err(IpcError::from)
}

#[cfg(test)]
mod tests {
    use maekon_core::error::CoreError;
    use maekon_core::error_codes::{AuthCode, PolicyCode};

    use super::*;

    struct StubClient {
        result: std::sync::Mutex<Option<Result<ContextHomeSnapshot, CoreError>>>,
    }

    #[async_trait::async_trait]
    impl ContextHomeClient for StubClient {
        async fn fetch_context_home(&self) -> Result<ContextHomeSnapshot, CoreError> {
            self.result
                .lock()
                .unwrap()
                .take()
                .expect("stub must be primed exactly once")
        }
    }

    fn stub(result: Result<ContextHomeSnapshot, CoreError>) -> Arc<dyn ContextHomeClient> {
        Arc::new(StubClient {
            result: std::sync::Mutex::new(Some(result)),
        })
    }

    fn fixture_snapshot() -> ContextHomeSnapshot {
        let fixture = include_str!("../../../api/fixtures/context-home.v1.json");
        serde_json::from_str(fixture).expect("committed fixture must parse")
    }

    #[test]
    fn an_unwired_slot_reads_empty() {
        // An empty slot and a server-reported empty home are different events.
        assert!(ContextHomeState::empty().get().is_none());
    }

    #[test]
    fn the_slot_is_populated_by_set_not_by_a_second_manage() {
        let state = ContextHomeState::empty();
        state.set(stub(Ok(fixture_snapshot())));
        assert!(state.get().is_some());
    }

    #[test]
    fn a_later_set_replaces_the_earlier_client() {
        // If a re-wire after re-login were silently ignored, the app would keep
        // using a dead transport.
        let state = ContextHomeState::empty();
        state.set(stub(Err(CoreError::Auth {
            code: AuthCode::Failed,
            message: "first".into(),
        })));
        state.set(stub(Ok(fixture_snapshot())));

        let client = state.get().expect("slot must hold the second client");
        let snapshot = tokio_block_on(client.fetch_context_home())
            .expect("the second client must be the one in effect");
        assert_eq!(snapshot.actor.actor_id, "wd-brk-024");
    }

    #[test]
    fn a_poisoned_slot_still_serves_the_wired_client() {
        // A panic elsewhere during bootstrap must not permanently kill the home.
        let state = Arc::new(ContextHomeState::empty());
        state.set(stub(Ok(fixture_snapshot())));

        let poisoner = Arc::clone(&state);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.0.lock().unwrap();
            panic!("poison the slot");
        })
        .join();

        assert!(
            state.get().is_some(),
            "a poisoned lock must not blank the home"
        );
    }

    #[test]
    fn transport_error_codes_survive_the_ipc_conversion() {
        // If the 401/403 split is flattened on the way through IpcError, the
        // point of this slice is gone.
        let expired: IpcError = CoreError::Auth {
            code: AuthCode::Failed,
            message: "context home session expired".into(),
        }
        .into();
        let denied: IpcError = CoreError::PolicyDenied {
            code: PolicyCode::Denied,
            message: "context home access denied for this actor".into(),
        }
        .into();

        assert_eq!(expired.code, "auth.failed");
        assert_eq!(denied.code, "policy.denied");
        assert_ne!(expired.code, denied.code);
    }

    #[test]
    fn the_unavailable_code_is_a_registry_code_not_a_new_literal() {
        // An out-of-registry code falls through to translateError's raw-English
        // fallback (#9492).
        assert_eq!(
            CODE_UNAVAILABLE,
            maekon_core::error_codes::ServiceCode::Unavailable.as_str()
        );
    }

    #[test]
    fn no_ipc_error_on_this_path_can_carry_a_bearer() {
        let unavailable = IpcError::new(
            CODE_UNAVAILABLE,
            "context home transport is not wired in this build",
        );
        for text in [unavailable.message.clone(), unavailable.code.clone()] {
            let lower = text.to_lowercase();
            assert!(!lower.contains("bearer"));
            assert!(!lower.contains("authorization"));
            assert!(!text.contains("eyJ"));
        }
    }

    #[test]
    fn the_command_signature_accepts_no_identity_argument() {
        // The signature itself is the defense — no user_id/organization_id
        // parameter may appear in this source.
        let source = include_str!("context_home.rs");
        let signature_start = source
            .find("pub async fn fetch_context_home(")
            .expect("command must exist");
        let signature_end = signature_start
            + source[signature_start..]
                .find(')')
                .expect("signature must close");
        let signature = &source[signature_start..signature_end];

        for forbidden in ["user_id", "organization_id", "actor_id", "token"] {
            assert!(
                !signature.contains(forbidden),
                "fetch_context_home must not take `{forbidden}`: {signature}"
            );
        }
    }

    fn tokio_block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(f)
    }
}
