//! #8044: capture-history re-authentication Tauri IPC commands.
//!
//! When entering the captured screenshot timeline/replay views, the
//! frontend calls these commands to perform an OS biometric (Touch ID) or
//! app PIN re-auth. On success, the shared gate is opened
//! (`record_success`), and the web middleware (`require_capture_reauth`)
//! then serves capture history.
//!
//! **Fail-closed**: on failed/cancelled/unsupported authentication, the
//! gate does not open. On platforms without biometric support (Windows/
//! Linux) or missing hardware, this converges to the PIN fallback.

use maekon_core::reauth::{ReauthCapabilities, ReauthMethod, ReauthOutcome};
use serde::{Deserialize, Serialize};

use crate::ipc_error::IpcError;
use crate::reauth::pin::{hash_pin, verify_pin, REAUTH_PIN_HASH_KEY};
use crate::reauth::ReauthRuntimeState;
use crate::runtime_state::{AppState, ConfigRuntimeState};

/// Default reason shown in the OS biometric prompt (when the frontend does
/// not pass a localized string).
const DEFAULT_REAUTH_REASON: &str = "Confirm it's you to view your capture history";

/// Re-authentication status snapshot (used by the frontend to decide which
/// re-auth UI to show).
#[derive(Debug, Clone, Serialize)]
pub struct ReauthStatusResponse {
    /// Whether the re-auth gate is enabled (config value).
    pub enabled: bool,
    /// Idle expiry in seconds.
    pub idle_timeout_secs: u64,
    /// Whether a currently-valid re-auth session exists (i.e. whether
    /// viewing is allowed right now).
    pub authenticated: bool,
    /// Whether OS biometrics are currently available.
    pub biometric_available: bool,
    /// Biometric method name ("Touch ID" etc). `None` when unavailable.
    pub biometric_kind: Option<String>,
    /// Whether an app PIN fallback is enrolled.
    pub pin_enrolled: bool,
}

/// A re-authentication attempt request.
#[derive(Debug, Clone, Deserialize)]
pub struct AuthenticateRequest {
    /// Which method to attempt (biometric or PIN).
    pub method: ReauthMethod,
    /// The entered PIN, when using the PIN method.
    #[serde(default)]
    pub pin: Option<String>,
    /// Optional localized reason to show in the OS biometric prompt.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Whether a stored PIN verifier (PHC) exists — i.e. whether a PIN is enrolled.
fn pin_enrolled(state: &AppState) -> bool {
    state
        .storage
        .get_meta(REAUTH_PIN_HASH_KEY)
        .is_some_and(|value| !value.trim().is_empty())
}

/// Returns the current re-auth status + platform capabilities + PIN
/// enrollment state.
#[tauri::command]
pub async fn get_capture_reauth_status(
    reauth: tauri::State<'_, ReauthRuntimeState>,
    state: tauri::State<'_, AppState>,
) -> Result<ReauthStatusResponse, IpcError> {
    let status = reauth.gate.status();
    let ReauthCapabilities {
        biometric_available,
        biometric_kind,
    } = reauth.verifier.capabilities();
    Ok(ReauthStatusResponse {
        enabled: status.enabled,
        idle_timeout_secs: status.idle_timeout_secs,
        authenticated: status.authenticated,
        biometric_available,
        biometric_kind,
        pin_enrolled: pin_enrolled(&state),
    })
}

/// Performs re-authentication to view capture history.
///
/// - `Biometric`: shows the OS biometric prompt. Opens the gate on success.
///   Returns `Unsupported` on unsupported platforms/missing hardware (the
///   frontend then falls back to PIN).
/// - `Pin`: compares against the stored Argon2id verifier. Opens the gate on
///   success. Returns `Unsupported` if no PIN is enrolled, or `Failed` on a
///   mismatch (fail-closed).
///
/// Only an `Authenticated` outcome opens the gate via `gate.record_success()`.
#[tauri::command]
pub async fn authenticate_capture_history(
    reauth: tauri::State<'_, ReauthRuntimeState>,
    state: tauri::State<'_, AppState>,
    request: AuthenticateRequest,
) -> Result<ReauthOutcome, IpcError> {
    // Clone the Arc up front so we don't hold the State borrow across an await.
    let gate = reauth.gate.clone();

    let outcome = match request.method {
        ReauthMethod::Biometric => {
            let verifier = reauth.verifier.clone();
            let reason = request
                .reason
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map_or_else(|| DEFAULT_REAUTH_REASON.to_string(), ToString::to_string);
            verifier.verify_biometric(&reason).await
        }
        ReauthMethod::Pin => {
            let Some(pin) = request.pin.clone() else {
                return Err(IpcError::new(
                    "validation.invalid_arguments",
                    "PIN is required for PIN re-authentication",
                ));
            };
            verify_pin_against_store(&state, pin).await?
        }
    };

    if outcome.is_authenticated() {
        gate.record_success();
    }
    Ok(outcome)
}

/// Compares the entered PIN against the stored PHC (blocking Argon2
/// verification → run in spawn_blocking).
///
/// Returns `Unsupported` if no PIN is enrolled, or `Failed` on a mismatch
/// (fail-closed).
async fn verify_pin_against_store(
    state: &AppState,
    pin: String,
) -> Result<ReauthOutcome, IpcError> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || {
        let Some(stored) = storage
            .get_meta(REAUTH_PIN_HASH_KEY)
            .filter(|value| !value.trim().is_empty())
        else {
            return ReauthOutcome::Unsupported;
        };
        if verify_pin(&pin, &stored) {
            ReauthOutcome::Authenticated
        } else {
            ReauthOutcome::Failed("incorrect PIN".to_string())
        }
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("PIN verification task join failed: {join_err}"),
        )
    })
}

/// Enrolls/updates the app PIN fallback (stored as an Argon2id hash).
#[tauri::command]
pub async fn register_capture_reauth_pin(
    state: tauri::State<'_, AppState>,
    pin: String,
) -> Result<(), IpcError> {
    let storage = state.storage.clone();
    // Argon2 hashing is CPU-intensive → spawn_blocking.
    tokio::task::spawn_blocking(move || {
        let phc = hash_pin(&pin)?;
        storage
            .set_meta_checked(REAUTH_PIN_HASH_KEY, &phc)
            .map_err(IpcError::from)
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("PIN registration task join failed: {join_err}"),
        )
    })?
}

/// Removes the enrolled app PIN fallback.
#[tauri::command]
pub async fn clear_capture_reauth_pin(state: tauri::State<'_, AppState>) -> Result<(), IpcError> {
    let storage = state.storage.clone();
    tokio::task::spawn_blocking(move || storage.delete_meta_checked(REAUTH_PIN_HASH_KEY))
        .await
        .map_err(|join_err| {
            IpcError::new(
                "internal.generic",
                format!("PIN clear task join failed: {join_err}"),
            )
        })?
        .map_err(IpcError::from)
}

/// Immediately locks the re-auth session (called by the frontend on app
/// foreground re-entry / manual lock).
///
/// Forces re-authentication the next time capture history is viewed.
#[tauri::command]
pub async fn lock_capture_reauth(
    reauth: tauri::State<'_, ReauthRuntimeState>,
) -> Result<(), IpcError> {
    reauth.gate.lock();
    Ok(())
}

/// Updates the re-auth configuration (enabled / idle expiry) — persists to
/// config AND **applies to the live gate immediately**.
///
/// Saves the setting to `config.privacy.reauth`, then updates the running
/// shared gate via `update_config`. On enable, the gate starts locked (it is
/// fail-closed until `record_success`), so a gate just turned on is never
/// left open by a stale authentication. The toggle takes effect immediately
/// without waiting for a restart.
#[tauri::command]
pub async fn set_capture_reauth_config(
    config_state: tauri::State<'_, ConfigRuntimeState>,
    reauth: tauri::State<'_, ReauthRuntimeState>,
    enabled: bool,
    idle_timeout_secs: u64,
) -> Result<(), IpcError> {
    let config_manager = config_state.config_manager().clone();
    // Persist to config (blocking file write → spawn_blocking). update_with
    // only changes reauth, preserving privacy's other fields (a direct field
    // edit, not a full overwrite).
    let reauth_config = tokio::task::spawn_blocking(move || {
        let new_config = config_manager.update_with(|config| {
            config.privacy.reauth.enabled = enabled;
            config.privacy.reauth.idle_timeout_secs = idle_timeout_secs;
            Ok(())
        })?;
        Ok::<_, IpcError>(new_config.privacy.reauth)
    })
    .await
    .map_err(|join_err| {
        IpcError::new(
            "internal.generic",
            format!("reauth config task join failed: {join_err}"),
        )
    })??;

    // Apply to the live gate immediately (using the clamped idle timeout).
    reauth.gate.update_config(
        reauth_config.enabled,
        reauth_config.effective_idle_timeout(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticate_request_deserializes_method_and_pin() {
        let json = r#"{"method":"pin","pin":"1234"}"#;
        let request: AuthenticateRequest = serde_json::from_str(json).expect("deserialise");
        assert_eq!(request.method, ReauthMethod::Pin);
        assert_eq!(request.pin.as_deref(), Some("1234"));
        assert!(request.reason.is_none());
    }

    #[test]
    fn authenticate_request_biometric_without_pin() {
        // Non-ASCII reason string round-trips through JSON unchanged
        // (localized prompt reasons are arbitrary UTF-8).
        let json = r#"{"method":"biometric","reason":"열람 확인"}"#;
        let request: AuthenticateRequest = serde_json::from_str(json).expect("deserialise");
        assert_eq!(request.method, ReauthMethod::Biometric);
        assert!(request.pin.is_none());
        assert_eq!(request.reason.as_deref(), Some("열람 확인"));
    }

    #[test]
    fn status_response_serializes_expected_fields() {
        let response = ReauthStatusResponse {
            enabled: true,
            idle_timeout_secs: 300,
            authenticated: false,
            biometric_available: true,
            biometric_kind: Some("Touch ID".to_string()),
            pin_enrolled: false,
        };
        let value = serde_json::to_value(&response).expect("serialise");
        assert_eq!(value["enabled"], serde_json::json!(true));
        assert_eq!(value["idle_timeout_secs"], serde_json::json!(300));
        assert_eq!(value["authenticated"], serde_json::json!(false));
        assert_eq!(value["biometric_available"], serde_json::json!(true));
        assert_eq!(value["biometric_kind"], serde_json::json!("Touch ID"));
        assert_eq!(value["pin_enrolled"], serde_json::json!(false));
    }
}
