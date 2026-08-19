use serde::Serialize;
use tauri::command;

use crate::ipc_error::IpcError;
use crate::runtime_state::{ConfigRuntimeState, SyncRuntimeState};

/// Canonical "Sync not enabled" error — surfaced when commands require a live
/// sync engine but the feature is disabled or unwired. service.unavailable
/// lets the frontend surface this as "feature disabled, enable in settings".
fn sync_not_enabled() -> IpcError {
    IpcError::new("service.unavailable", "Sync not enabled")
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncAvailabilityDto {
    Ready,
    Disabled,
    Unavailable,
}

fn classify_sync_availability(enabled: bool, runtime_available: bool) -> SyncAvailabilityDto {
    match (enabled, runtime_available) {
        (false, _) => SyncAvailabilityDto::Disabled,
        (true, true) => SyncAvailabilityDto::Ready,
        (true, false) => SyncAvailabilityDto::Unavailable,
    }
}

#[derive(Serialize)]
pub struct SyncStatusDto {
    pub enabled: bool,
    pub runtime_available: bool,
    pub runtime_state: SyncAvailabilityDto,
    pub unavailable_reason: Option<String>,
    pub device_id: String,
    pub device_name: String,
    pub last_health_state: String,
    pub last_sync_at: Option<String>,
    pub last_error: Option<String>,
    /// Known peers discovered during the last discovery scan.
    pub peers: Vec<SyncPeerDto>,
}

#[derive(Serialize, Default)]
pub struct SyncResultDto {
    pub applied: usize,
    pub skipped: usize,
    pub tombstoned: usize,
    /// #8056 P3: whether the cycle attempted a push (had local changes). `false`
    /// means "nothing to push" — a clean no-op distinct from a failed delivery.
    pub push_attempted: bool,
    /// #8056 P3: number of peers that confirmed receipt of the pushed changeset.
    /// `0` with `push_attempted = true` means the local changes reached NO peer
    /// (all peers offline/failed) — the UI can surface this as a delivery
    /// warning instead of silently reporting a successful-looking empty cycle.
    pub pushed_to_peers: usize,
}

#[derive(Serialize, Clone)]
pub struct SyncPeerDto {
    pub device_id: String,
    pub device_name: String,
    pub last_sync_at: String,
}

fn peer_dto(peer: maekon_core::models::sync::PeerInfo) -> SyncPeerDto {
    SyncPeerDto {
        device_id: peer.device_id,
        device_name: peer.device_name,
        last_sync_at: peer.last_sync_at,
    }
}

fn validate_peer_id(device_id: &str) -> Result<&str, IpcError> {
    let trimmed = device_id.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || !trimmed.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
    {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "Invalid sync peer identifier",
        ));
    }
    Ok(trimmed)
}

#[command]
pub async fn get_sync_status(
    state: tauri::State<'_, SyncRuntimeState>,
    config_state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<SyncStatusDto, IpcError> {
    // config.sync.enabled is the authoritative master switch regardless of
    // whether the engine is currently wired up.
    let config_enabled = config_state.config_manager().get().sync.enabled;

    match state.engine() {
        Some(engine) => {
            let (sync_at, error) = engine.health_status();
            // Attempt a lightweight peer discovery to populate the status; ignore
            // errors so that a discovery failure does not fail the status query.
            let peers = engine
                .discover_peers()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(peer_dto)
                .collect();

            Ok(SyncStatusDto {
                enabled: config_enabled,
                runtime_available: true,
                runtime_state: classify_sync_availability(config_enabled, true),
                unavailable_reason: None,
                device_id: engine.device_id().to_string(),
                device_name: engine.device_name().to_string(),
                last_health_state: engine.health_state().to_string(),
                last_sync_at: sync_at,
                last_error: error,
                peers,
            })
        }
        None => Ok(SyncStatusDto {
            enabled: config_enabled,
            runtime_available: false,
            runtime_state: classify_sync_availability(config_enabled, false),
            unavailable_reason: config_enabled.then(|| {
                "Sync is enabled, but the runtime engine is not available yet".to_string()
            }),
            device_id: String::new(),
            device_name: String::new(),
            last_health_state: "unavailable".to_string(),
            last_sync_at: None,
            last_error: None,
            peers: Vec::new(),
        }),
    }
}

#[command]
pub async fn trigger_sync_cycle(
    state: tauri::State<'_, SyncRuntimeState>,
) -> Result<SyncResultDto, IpcError> {
    let engine = state.engine().ok_or_else(sync_not_enabled)?;

    match engine.run_cycle().await {
        Ok(Some(result)) => Ok(SyncResultDto {
            applied: result.applied,
            skipped: result.skipped_lww + result.skipped_dup,
            tombstoned: result.tombstoned,
            push_attempted: result.push_attempted,
            pushed_to_peers: result.pushed_to_peers,
        }),
        // A `None` result means neither pull nor push happened this cycle — a
        // genuine no-op. `SyncResultDto::default()` reports push_attempted=false.
        Ok(None) => Ok(SyncResultDto::default()),
        Err(e) => Err(IpcError::from(e)),
    }
}

#[command]
pub async fn discover_sync_peers(
    state: tauri::State<'_, SyncRuntimeState>,
) -> Result<Vec<SyncPeerDto>, IpcError> {
    let engine = state.engine().ok_or_else(sync_not_enabled)?;

    let peers = engine.discover_peers().await.map_err(IpcError::from)?;

    Ok(peers.into_iter().map(peer_dto).collect())
}

#[command]
pub async fn forget_sync_peer(
    state: tauri::State<'_, SyncRuntimeState>,
    device_id: String,
) -> Result<Vec<SyncPeerDto>, IpcError> {
    let device_id = validate_peer_id(&device_id)?;
    let engine = state.engine().ok_or_else(sync_not_enabled)?;

    engine
        .forget_peer(device_id)
        .await
        .map_err(IpcError::from)?;
    let peers = engine.discover_peers().await.map_err(IpcError::from)?;
    Ok(peers.into_iter().map(peer_dto).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_availability_distinguishes_disabled_from_unavailable() {
        assert!(matches!(
            classify_sync_availability(false, false),
            SyncAvailabilityDto::Disabled
        ));
        assert!(matches!(
            classify_sync_availability(true, false),
            SyncAvailabilityDto::Unavailable
        ));
        assert!(matches!(
            classify_sync_availability(true, true),
            SyncAvailabilityDto::Ready
        ));
    }

    #[test]
    fn sync_peer_identifier_validation_is_bounded_and_transport_safe() {
        assert_eq!(
            validate_peer_id(" qc-peer_01.example:2 ").unwrap(),
            "qc-peer_01.example:2"
        );
        assert_eq!(
            validate_peer_id("").unwrap_err().code,
            "validation.invalid_arguments"
        );
        assert_eq!(
            validate_peer_id("peer with spaces").unwrap_err().code,
            "validation.invalid_arguments"
        );
        assert_eq!(
            validate_peer_id("peer/with/path").unwrap_err().code,
            "validation.invalid_arguments"
        );
        assert_eq!(
            validate_peer_id(&"x".repeat(129)).unwrap_err().code,
            "validation.invalid_arguments"
        );
    }
}
