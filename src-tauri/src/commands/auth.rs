//! Authentication-related Tauri commands.
//!
//! OOS-TBD-N15-UI-EXPOSURE (2026-05-05): `logout_all_sessions` IPC entry point —
//! the Tauri command invoked by the UI "Sign out of all devices" button. It
//! calls server `DELETE /api/v1/auth/tokens/all` and clears local TokenManager
//! state.
//!
//! State pattern: `TokenManagerState(Option<Arc<TokenManager>>)` is registered
//! separately. When `cfg(feature = "server")` is disabled (offline / demo),
//! this remains `None` and the command returns `IpcError` immediately.

use std::sync::Arc;

use tauri::{command, State};

#[cfg(feature = "server")]
use maekon_network::auth::TokenManager;

/// Placeholder type when the `server` feature is disabled. Tauri State must
/// always be registered (`main.rs` manages `TokenManagerState(None)`), so this
/// stub keeps non-server builds compatible.
#[cfg(not(feature = "server"))]
pub struct TokenManager;

use crate::ipc_error::IpcError;

/// Tauri-managed wrapper around the optional `TokenManager`.
///
/// `None` means either:
/// - `cfg(feature = "server")` is disabled (offline / demo)
/// - server bootstrap failed before a token manager was created
///
/// In both cases, the Tauri command returns `IpcError` when invoked.
pub struct TokenManagerState(pub Option<Arc<TokenManager>>);

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
        match state.0.as_ref() {
            Some(tm) => tm.logout_all_sessions().await.map_err(IpcError::from),
            None => Err(IpcError::new(
                "auth.token_manager_unavailable",
                "TokenManager not initialized — server bootstrap likely failed",
            )),
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = state.0.as_ref();
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
    fn token_manager_state_none_constructs() {
        let state = TokenManagerState(None);
        assert!(state.0.is_none());
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
