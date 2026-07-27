//! Debug-only synthetic sync peer transport for isolated recovery QC.
//!
//! The transport never opens a socket and persists only one boolean under the
//! dedicated profile. It exists so peer discovery and forget persistence can
//! be exercised through the real SyncEngine and Tauri/UI boundaries without
//! mutating a real peer, keychain trust entry, or host network.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::sync::{ChangeSet, PeerInfo};
use maekon_core::ports::sync_transport::SyncTransport;
use maekon_core::sync::Hlc;
use serde::{Deserialize, Serialize};

const DEBUG_GATE_ENV: &str = "MAEKON_DEBUG_QC_FIXTURE_CLI";
const ISOLATED_GATE_ENV: &str = "MAEKON_TC_ISOLATED_PROFILE";
const FLAVOR_ENV: &str = "MAEKON_APP_FLAVOR";
const SYNC_PEER_GATE_ENV: &str = "MAEKON_DEBUG_QC_SYNC_PEER_FIXTURE";
const STATE_FILE: &str = "qc-sync-peer-state.json";
const PEER_ID: &str = "qc-peer-cj-05-04";
const PEER_NAME: &str = "Synthetic recovery peer";

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize)]
struct FixtureState {
    forgotten: bool,
}

pub(crate) struct QcSyncPeerTransport {
    state_path: PathBuf,
}

pub(crate) fn fixture_enabled() -> bool {
    fixture_enabled_from_values(
        std::env::var(DEBUG_GATE_ENV).ok().as_deref(),
        std::env::var(ISOLATED_GATE_ENV).ok().as_deref(),
        std::env::var(FLAVOR_ENV).ok().as_deref(),
        std::env::var(SYNC_PEER_GATE_ENV).ok().as_deref(),
    )
}

pub(crate) fn transport_from_env(
    data_dir: &Path,
) -> Result<Option<Arc<dyn SyncTransport>>, CoreError> {
    if !fixture_enabled() {
        return Ok(None);
    }

    let state_path = data_dir.join(STATE_FILE);
    if !state_path.exists() {
        write_state(&state_path, FixtureState::default())?;
    }
    Ok(Some(Arc::new(QcSyncPeerTransport { state_path })))
}

pub(crate) fn prepare_fixture(data_dir: &Path) -> Result<(), CoreError> {
    if !fixture_enabled() {
        return Err(internal_error(
            "isolated QC sync-peer fixture gates are incomplete",
        ));
    }
    write_state(&data_dir.join(STATE_FILE), FixtureState::default())
}

fn fixture_enabled_from_values(
    debug_gate: Option<&str>,
    isolated_gate: Option<&str>,
    flavor: Option<&str>,
    sync_peer_gate: Option<&str>,
) -> bool {
    debug_gate == Some("1")
        && isolated_gate == Some("1")
        && sync_peer_gate == Some("1")
        && flavor.is_some_and(is_isolated_flavor)
}

fn is_isolated_flavor(flavor: &str) -> bool {
    let trimmed = flavor.trim();
    (trimmed.starts_with("qc-") || trimmed.starts_with("tc-"))
        && trimmed.len() > 3
        && trimmed
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn read_state(path: &Path) -> Result<FixtureState, CoreError> {
    if !path.exists() {
        return Ok(FixtureState::default());
    }
    let bytes = std::fs::read(path)
        .map_err(|error| internal_error(format!("read QC sync-peer state: {error}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| internal_error(format!("parse QC sync-peer state: {error}")))
}

fn write_state(path: &Path, state: FixtureState) -> Result<(), CoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| internal_error("QC sync-peer state has no parent directory"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| internal_error(format!("create QC sync-peer state directory: {error}")))?;
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(&state)
        .map_err(|error| internal_error(format!("serialize QC sync-peer state: {error}")))?;
    std::fs::write(&temporary, bytes)
        .map_err(|error| internal_error(format!("write QC sync-peer state: {error}")))?;
    // Windows rename does not replace an existing destination. This fixture
    // file contains only a synthetic boolean, so remove the old isolated
    // state before committing the prepared replacement.
    if path.exists() {
        std::fs::remove_file(path)
            .map_err(|error| internal_error(format!("replace QC sync-peer state: {error}")))?;
    }
    std::fs::rename(&temporary, path)
        .map_err(|error| internal_error(format!("commit QC sync-peer state: {error}")))
}

fn internal_error(message: impl Into<String>) -> CoreError {
    CoreError::Internal {
        code: InternalCode::Generic,
        message: message.into(),
    }
}

#[async_trait]
impl SyncTransport for QcSyncPeerTransport {
    async fn push(&self, _changes: &ChangeSet) -> Result<usize, CoreError> {
        Ok(0)
    }

    async fn pull(&self, _since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
        Ok(None)
    }

    async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
        let state_path = self.state_path.clone();
        let state = tokio::task::spawn_blocking(move || read_state(&state_path))
            .await
            .map_err(|error| internal_error(format!("join QC sync-peer read: {error}")))??;
        if state.forgotten {
            return Ok(Vec::new());
        }
        Ok(vec![PeerInfo {
            device_id: PEER_ID.to_string(),
            device_name: PEER_NAME.to_string(),
            last_sync_at: "2026-07-19T00:00:00Z".to_string(),
            watermark: Hlc {
                wall_ms: 1_784_419_200_000,
                counter: 1,
                device_id: PEER_ID.to_string(),
            },
        }])
    }

    async fn forget_peer(&self, device_id: &str) -> Result<(), CoreError> {
        if device_id != PEER_ID {
            return Ok(());
        }
        let state_path = self.state_path.clone();
        tokio::task::spawn_blocking(move || {
            write_state(&state_path, FixtureState { forgotten: true })
        })
        .await
        .map_err(|error| internal_error(format!("join QC sync-peer write: {error}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_requires_every_gate_and_isolated_flavor() {
        let valid = [Some("1"), Some("1"), Some("qc-sync-peer"), Some("1")];
        for missing in 0..valid.len() {
            let mut values = valid;
            values[missing] = None;
            assert!(!fixture_enabled_from_values(
                values[0], values[1], values[2], values[3]
            ));
        }
        assert!(!fixture_enabled_from_values(
            Some("1"),
            Some("1"),
            Some("production"),
            Some("1")
        ));
    }

    #[tokio::test]
    async fn forgotten_peer_stays_absent_after_transport_recreation() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join(STATE_FILE);
        write_state(&state_path, FixtureState::default()).unwrap();

        let first = QcSyncPeerTransport {
            state_path: state_path.clone(),
        };
        assert_eq!(first.discover_peers().await.unwrap().len(), 1);
        first.forget_peer(PEER_ID).await.unwrap();

        let restarted = QcSyncPeerTransport { state_path };
        assert!(restarted.discover_peers().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn unknown_peer_forget_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let state_path = directory.path().join(STATE_FILE);
        write_state(&state_path, FixtureState::default()).unwrap();
        let transport = QcSyncPeerTransport { state_path };

        transport.forget_peer("unknown-peer").await.unwrap();
        assert_eq!(transport.discover_peers().await.unwrap().len(), 1);
    }
}
