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
/// - server call failure: ignored after local state is cleared
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

    // OOS-TBD-N15-UI-EXPOSURE: Integration tests with a real TokenManager and
    // mockito server belong in a follow-up because Tauri State injection needs
    // a fuller fixture. TokenManager.logout_all_sessions has direct coverage in
    // maekon-network/src/auth.rs.
}
