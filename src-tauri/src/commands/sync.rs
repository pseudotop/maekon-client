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
}

#[derive(Serialize, Clone)]
pub struct SyncPeerDto {
    pub device_id: String,
    pub device_name: String,
    pub last_sync_at: String,
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
                .map(|p| SyncPeerDto {
                    device_id: p.device_id,
                    device_name: p.device_name,
                    last_sync_at: p.last_sync_at,
                })
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
        }),
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

    Ok(peers
        .into_iter()
        .map(|p| SyncPeerDto {
            device_id: p.device_id,
            device_name: p.device_name,
            last_sync_at: p.last_sync_at,
        })
        .collect())
}

// #7683 F2: set_sync_enabled and forget_peer were removed as residual dead
// IPCs. SyncTab.tsx only ever calls get_sync_status / discover_sync_peers /
// trigger_sync_cycle — there is no "forget device" button, and enabling sync
// is documented as a manual `sync.enabled = true` config-file edit (the
// "not enabled" guidance panel), never a UI toggle. Zero callers anywhere in
// crates/maekon-web/frontend/src for either command.

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
}
