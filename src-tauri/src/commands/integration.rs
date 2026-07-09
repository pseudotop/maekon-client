// #7600: integration_auth_status, integration_start_device_authorization,
// integration_poll_device_authorization, integration_cancel_device_authorization,
// and integration_reset_auth_state were removed as dead IPC duplicates — the
// React frontend drives the device-auth flow via the embedded HTTP API
// (GET/POST /integration/auth/*) instead. The OAuth-flow commands below remain
// live IPC callers.

use tauri::command;

use maekon_core::ports::oauth::{OAuthConnectionStatus, OAuthFlowHandle, OAuthFlowStatus};

use crate::ipc_error::IpcError;
use crate::runtime_state::{OAuthCoordinatorState, OAuthState};

// ── OAuth IPC commands ──────────────────────────────────────

fn require_oauth(
    state: &OAuthState,
) -> Result<std::sync::Arc<dyn maekon_core::ports::oauth::OAuthPort>, IpcError> {
    state.0.clone().ok_or_else(|| {
        IpcError::new(
            "service.unavailable",
            "OAuth is not available (OS keychain unavailable or feature disabled)",
        )
    })
}

/// Start the OAuth authentication flow — returns the auth_url to the frontend.
#[command]
pub async fn oauth_start_flow(
    provider_id: String,
    oauth: tauri::State<'_, OAuthState>,
) -> Result<OAuthFlowHandle, IpcError> {
    let port = require_oauth(&oauth)?;
    port.start_flow(&provider_id).await.map_err(IpcError::from)
}

/// Query OAuth flow status — for frontend polling.
///
/// When the flow completes successfully, the coordinator's backoff state
/// is reset so background refresh resumes immediately.
#[command]
pub async fn oauth_flow_status(
    flow_id: String,
    oauth: tauri::State<'_, OAuthState>,
    coordinator: tauri::State<'_, OAuthCoordinatorState>,
) -> Result<OAuthFlowStatus, IpcError> {
    let port = require_oauth(&oauth)?;
    let status = port.flow_status(&flow_id).await.map_err(IpcError::from)?;

    // Reset coordinator backoff after successful re-authentication so the
    // background refresh loop resumes normal operation immediately.
    #[cfg(feature = "analysis")]
    if matches!(status, OAuthFlowStatus::Completed) {
        if let Some(ref coord) = coordinator.0 {
            coord.reset().await;
        }
    }
    let _ = &coordinator; // suppress unused-variable warning when analysis feature is off

    Ok(status)
}

/// Cancel the OAuth flow.
#[command]
pub async fn oauth_cancel_flow(
    flow_id: String,
    oauth: tauri::State<'_, OAuthState>,
) -> Result<(), IpcError> {
    let port = require_oauth(&oauth)?;
    port.cancel_flow(&flow_id).await.map_err(IpcError::from)
}

/// Disconnect OAuth — deletes stored credentials.
#[command]
pub async fn oauth_revoke(
    provider_id: String,
    oauth: tauri::State<'_, OAuthState>,
) -> Result<(), IpcError> {
    let port = require_oauth(&oauth)?;
    port.revoke(&provider_id).await.map_err(IpcError::from)
}

/// Query OAuth connection status.
#[command]
pub async fn oauth_connection_status(
    provider_id: String,
    oauth: tauri::State<'_, OAuthState>,
) -> Result<OAuthConnectionStatus, IpcError> {
    let port = require_oauth(&oauth)?;
    port.connection_status(&provider_id)
        .await
        .map_err(IpcError::from)
}
