//! Authentication-related Tauri commands.
//!
//! OOS-TBD-N15-UI-EXPOSURE (2026-05-05): `logout_all_sessions` IPC entry point —
//! the Tauri command invoked by the UI "Sign out of all devices" button. It
//! calls server `DELETE /api/v1/auth/tokens/all` and clears local TokenManager
//! state.
//!
//! State pattern: `TokenManagerState` holds an interior-mutable slot
//! (`Mutex<Option<Arc<TokenManager>>>`) that is registered ONCE at build time
//! in `main.rs` and POPULATED at setup time once server bootstrap creates the
//! token manager. When `cfg(feature = "server")` is disabled (offline / demo)
//! or bootstrap never populates it, the slot stays `None` and the command
//! returns `IpcError` immediately.
//!
//! Why a slot instead of re-`manage()`-ing? Tauri's `Manager::manage()` does
//! NOT overwrite an already-managed type — it returns `false` and the value is
//! discarded. Tauri's own docs recommend wrapping the state in a `Mutex` and
//! using `Option` (rather than the deprecated/unsafe `unmanage()`) precisely so
//! the managed slot can be populated after the builder registers it once.

use std::sync::{Arc, Mutex};

use tauri::{command, State};

#[cfg(feature = "server")]
use maekon_network::auth::TokenManager;

/// Placeholder type when the `server` feature is disabled. Tauri State must
/// always be registered (`main.rs` manages a `TokenManagerState`), so this stub
/// keeps non-server builds compatible.
#[cfg(not(feature = "server"))]
pub struct TokenManager;

use crate::ipc_error::IpcError;

/// Tauri-managed wrapper around the optional `TokenManager`.
///
/// Registered exactly once at build time (`main.rs`) with an empty slot, then
/// populated at setup time once a real `TokenManager` exists. The inner slot is
/// `None` when:
/// - `cfg(feature = "server")` is disabled (offline / demo)
/// - server bootstrap failed (or never ran) before a token manager was created
///
/// In all of those cases, the Tauri command returns `IpcError` when invoked.
pub struct TokenManagerState(Mutex<Option<Arc<TokenManager>>>);

impl TokenManagerState {
    /// Build-time constructor: an empty slot registered once on the Tauri
    /// builder. Setup-time wiring later calls [`TokenManagerState::set`].
    #[must_use]
    pub fn empty() -> Self {
        Self(Mutex::new(None))
    }

    /// Populate the slot after server bootstrap. Replaces any previous value.
    ///
    /// Called from `app_runtime_launch` setup wiring. Because the state is
    /// registered ONCE at build time, this slot write is what actually makes
    /// the real `TokenManager` reachable from `logout_all_sessions` — a second
    /// `Manager::manage()` would be a silent no-op.
    ///
    /// Only the `server` build ever populates the slot; without the `server`
    /// feature there is no `TokenManager` to write and the slot stays empty.
    #[cfg(feature = "server")]
    pub fn set(&self, manager: Arc<TokenManager>) {
        // A poisoned lock here only means a prior holder panicked while the
        // (brief, non-await) critical section was held; recover the guard so
        // bootstrap still wires the manager rather than panicking the setup.
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(manager);
    }

    /// Read the current manager, cloning the `Arc` out of the lock so the guard
    /// is never held across an `.await`.
    #[must_use]
    pub fn get(&self) -> Option<Arc<TokenManager>> {
        let slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        slot.clone()
    }
}

/// Sign out of all devices.
///
/// OOS-TBD-N15-UI-EXPOSURE (2026-05-05): calls
/// `TokenManager.logout_all_sessions()`, which invokes server
/// `DELETE /api/v1/auth/tokens/all`, revokes all device tokens/sessions, and
/// clears local state.
///
/// This includes the current device, so the user must sign in again.
///
/// # Errors
///
/// - `cfg(feature = "server")` disabled: `IpcError`
/// - local state unavailable: `IpcError`
/// - other `CoreError` values: converted to `IpcError`
#[command]
pub async fn logout_all_sessions(state: State<'_, TokenManagerState>) -> Result<(), IpcError> {
    #[cfg(feature = "server")]
    {
        // Clone the Arc out of the slot first; never hold the lock across the
        // `.await` below.
        match state.get() {
            Some(tm) => tm.logout_all_sessions().await.map_err(IpcError::from),
            None => Err(IpcError::new(
                "auth.token_manager_unavailable",
                "TokenManager not initialized — server bootstrap likely failed",
            )),
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = state.get();
        Err(IpcError::new(
            "auth.feature_disabled",
            "logout_all_sessions: server feature disabled in this build",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_manager_state_empty_resolves_to_none() {
        let state = TokenManagerState::empty();
        assert!(state.get().is_none());
    }

    // Regression (2nd-pass #22): the build-time slot is registered ONCE and
    // populated later via `set()`. This mirrors the real wiring — `main.rs`
    // registers `TokenManagerState::empty()`, then setup calls `set(..)` — and
    // asserts the slot resolves to `Some` afterwards. Before the slot fix, the
    // setup-time `Manager::manage(Some(..))` was a silent no-op, so the command
    // always saw `None` and `DELETE /api/v1/auth/tokens/all` never ran.
    #[cfg(feature = "server")]
    #[test]
    #[allow(deprecated)] // TokenManager::new used intentionally in tests
    fn token_manager_state_set_populates_slot() {
        let state = TokenManagerState::empty();
        assert!(state.get().is_none(), "slot must start empty");

        let tm = Arc::new(maekon_network::auth::TokenManager::new(
            "http://127.0.0.1:19999",
        ));
        state.set(tm.clone());

        let resolved = state
            .get()
            .expect("slot must resolve to Some after set() — the bootstrap wiring path");
        assert!(
            Arc::ptr_eq(&resolved, &tm),
            "the populated slot must hand back the exact bootstrap TokenManager"
        );
    }

    // F1 (P1): Security IPC test — `Some(tm)` path in `logout_all_sessions`.
    //
    // The Tauri command takes `State<'_, TokenManagerState>` so it cannot be
    // called directly in a unit test without a running Tauri app. Instead we
    // exercise the *identical* logic by calling `TokenManager::logout_all_sessions`
    // directly and asserting the `IpcError` mapping:
    //
    //   Some(tm) => tm.logout_all_sessions().await.map_err(IpcError::from)
    //
    // This is the security-critical path: a live `TokenManager` invokes
    // `DELETE /api/v1/auth/tokens/all`, then unconditionally clears local state.
    // The server call is fire-and-forget; `logout_all_sessions` always returns
    // `Ok(())` regardless of server reachability (same semantics as `logout`).
    #[cfg(feature = "server")]
    #[tokio::test]
    #[allow(deprecated)] // TokenManager::new used intentionally in tests
    async fn logout_all_sessions_some_tm_path_always_succeeds() {
        // Construct a TokenManager pointing to a deliberately unreachable URL.
        // The server call will fail silently (warn-logged); local state is cleared.
        let tm = maekon_network::auth::TokenManager::new("http://127.0.0.1:19999");
        let tm_arc = Arc::new(tm);

        // This mirrors the `Some(tm) => tm.logout_all_sessions().await.map_err(IpcError::from)`
        // branch in the Tauri command, exercising the security-critical code path.
        let result: Result<(), IpcError> =
            tm_arc.logout_all_sessions().await.map_err(IpcError::from);

        // logout_all_sessions unconditionally returns Ok(()) — server failures are
        // warn-logged and ignored; local token state is cleared in all cases.
        result.expect(
            "logout_all_sessions Some(tm) path must succeed regardless of server reachability",
        );
    }

    // F1 (P1): IpcError wire-code propagation from CoreError through the TokenManager.
    //
    // Verifies that CoreError → IpcError conversion preserves the wire code so the
    // frontend receives a structured `{"code": "...", "message": "..."}` payload
    // instead of a raw string. This is the conversion that `map_err(IpcError::from)`
    // inside the Some(tm) branch would perform if logout_all_sessions could fail.
    #[cfg(feature = "server")]
    #[test]
    fn logout_all_sessions_ipc_error_conversion_preserves_wire_code() {
        let core = maekon_core::error::CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "forced test error for IpcError mapping verification".into(),
        };
        let ipc: IpcError = IpcError::from(core);
        assert_eq!(
            ipc.code, "auth.failed",
            "CoreError::Auth → IpcError must preserve 'auth.failed' wire code"
        );
        assert!(
            ipc.message.contains("auth.failed"),
            "IpcError message must embed the wire code per ADR-019 Display convention"
        );
    }
}
