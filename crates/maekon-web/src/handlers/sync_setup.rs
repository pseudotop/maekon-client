//! `GET`/`POST /api/sync/setup` — enable cross-device sync from the GUI (#8056 P2-2).
//!
//! Before this route, the only way to give the sync engine a passphrase was the
//! `MAEKON_SYNC_PASSPHRASE` environment variable, which a GUI app launched from
//! Finder / the Start menu never inherits — so `config.sync.enabled = true`
//! silently produced a disabled engine. This route lets the Settings → Sync UI
//! store the passphrase in the OS keychain (read back at startup by
//! `agent_runtime::sync_setup::resolve_sync_passphrase`) and flip the config
//! master switch, all without a shell env var or hand-editing the config file.
//!
//! Enabling takes effect on the next app start (the engine is built once during
//! startup), so the response carries `restart_required`.

use axum::{extract::State, Json};
use maekon_core::config::{
    SyncTransportKind, MIN_SYNC_PASSPHRASE_LEN, SYNC_PASSPHRASE_KEYCHAIN_ACCOUNT,
    SYNC_PASSPHRASE_KEYCHAIN_SERVICE,
};

use crate::error::ApiError;
use crate::services::web_contexts::ConfigWebContext;
use maekon_api_contracts::sync_setup::{SyncSetupRequest, SyncSetupStatus};

fn parse_transport(raw: &str) -> Result<SyncTransportKind, ApiError> {
    match raw {
        "file" => Ok(SyncTransportKind::File),
        "lan" => Ok(SyncTransportKind::Lan),
        "remote" => Ok(SyncTransportKind::Remote),
        other => Err(ApiError::BadRequest(format!(
            "unknown sync transport '{other}' (expected file, lan, or remote)"
        ))),
    }
}

/// Write the passphrase into the OS keychain off the reactor (keychain access is
/// blocking OS I/O).
async fn keychain_store_passphrase(passphrase: String) -> Result<(), ApiError> {
    tokio::task::spawn_blocking(move || {
        let entry = keyring::Entry::new(
            SYNC_PASSPHRASE_KEYCHAIN_SERVICE,
            SYNC_PASSPHRASE_KEYCHAIN_ACCOUNT,
        )
        .map_err(|e| format!("open keychain entry: {e}"))?;
        entry
            .set_password(&passphrase)
            .map_err(|e| format!("write keychain entry: {e}"))
    })
    .await
    .map_err(|e| ApiError::Internal(format!("keychain task join failed: {e}")))?
    .map_err(ApiError::Internal)
}

/// Whether a non-empty passphrase is present in the keychain (off the reactor).
async fn keychain_passphrase_present() -> bool {
    tokio::task::spawn_blocking(|| {
        keyring::Entry::new(
            SYNC_PASSPHRASE_KEYCHAIN_SERVICE,
            SYNC_PASSPHRASE_KEYCHAIN_ACCOUNT,
        )
        .and_then(|entry| entry.get_password())
        .map(|value| !value.is_empty())
        .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn current_status(
    context: &ConfigWebContext,
    restart_required: bool,
    passphrase_set: bool,
) -> Result<SyncSetupStatus, ApiError> {
    let cm = context
        .config_manager
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Config manager not available".to_string()))?;
    let cfg = cm.get();
    Ok(SyncSetupStatus {
        enabled: cfg.sync.enabled,
        transport: cfg.sync.transport.to_string(),
        passphrase_set,
        restart_required,
    })
}

/// `GET /api/sync/setup` — report enable state, transport, and whether a
/// passphrase is stored (never the value).
pub async fn get_sync_setup(
    State(context): State<ConfigWebContext>,
) -> Result<Json<SyncSetupStatus>, ApiError> {
    let passphrase_set = keychain_passphrase_present().await;
    Ok(Json(current_status(&context, false, passphrase_set)?))
}

/// `POST /api/sync/setup` — store a passphrase (optional) + set the enable state
/// and transport. Activation happens on next app start.
pub async fn update_sync_setup(
    State(context): State<ConfigWebContext>,
    Json(request): Json<SyncSetupRequest>,
) -> Result<Json<SyncSetupStatus>, ApiError> {
    let cm = context
        .config_manager
        .as_ref()
        .ok_or_else(|| ApiError::ServiceUnavailable("Config manager not available".to_string()))?;

    // Parse the transport (if any) BEFORE any side effect so a bad value fails
    // the whole request cleanly.
    let transport = match request.transport.as_deref() {
        Some(raw) => Some(parse_transport(raw)?),
        None => None,
    };

    // Enabling sync without a usable passphrase would just reproduce the silent-
    // disable bug, so require one to be present (either supplied now or already
    // in the keychain).
    if let Some(passphrase) = request.passphrase.as_deref() {
        if passphrase.chars().count() < MIN_SYNC_PASSPHRASE_LEN {
            return Err(ApiError::BadRequest(format!(
                "sync passphrase must be at least {MIN_SYNC_PASSPHRASE_LEN} characters"
            )));
        }
        keychain_store_passphrase(passphrase.to_string()).await?;
    } else if request.enabled && !keychain_passphrase_present().await {
        return Err(ApiError::BadRequest(
            "cannot enable sync without a passphrase — provide one in this request".to_string(),
        ));
    }

    cm.update_with(|cfg| {
        cfg.sync.enabled = request.enabled;
        if let Some(kind) = transport.clone() {
            cfg.sync.transport = kind;
        }
        Ok(())
    })
    .map_err(ApiError::from)?;

    // A newly-enabled engine only comes up on the next start.
    let passphrase_set = keychain_passphrase_present().await;
    Ok(Json(current_status(
        &context,
        request.enabled,
        passphrase_set,
    )?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_transport_maps_known_kinds() {
        assert_eq!(parse_transport("file").unwrap(), SyncTransportKind::File);
        assert_eq!(parse_transport("lan").unwrap(), SyncTransportKind::Lan);
        assert_eq!(
            parse_transport("remote").unwrap(),
            SyncTransportKind::Remote
        );
    }

    #[test]
    fn parse_transport_rejects_unknown() {
        let err = parse_transport("carrier-pigeon").unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn status_serializes_without_passphrase_value() {
        let status = SyncSetupStatus {
            enabled: true,
            transport: "lan".to_string(),
            passphrase_set: true,
            restart_required: true,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"passphrase_set\":true"));
        // The value is never part of the wire contract.
        assert!(!json.contains("passphrase\":\""));
    }
}
