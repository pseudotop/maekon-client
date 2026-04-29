use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

use oneshim_core::config::{AppConfig, SyncTransportKind};
use oneshim_core::consent::ConsentManager;
use oneshim_core::error::CoreError;

use crate::sync_engine::SyncEngine;

/// Result of the sync engine setup.
pub(super) struct SyncResult {
    /// Populated when sync is enabled, passphrase is set, and transport creation succeeds.
    pub sync_engine: Option<Arc<SyncEngine>>,
}

/// Build the cross-device sync engine.
///
/// Requires `ONESHIM_SYNC_PASSPHRASE` env var, device identity from SQLite, and a valid transport.
pub(super) async fn build_sync_engine(
    config: &AppConfig,
    data_dir: &Path,
    sqlite_storage_concrete: &Arc<oneshim_storage::sqlite::SqliteStorage>,
    consent_manager: Option<Arc<ConsentManager>>,
) -> SyncResult {
    if !config.sync.enabled {
        return SyncResult { sync_engine: None };
    }

    let passphrase = std::env::var("ONESHIM_SYNC_PASSPHRASE").unwrap_or_default();
    if passphrase.is_empty() {
        warn!("sync enabled but ONESHIM_SYNC_PASSPHRASE not set; sync disabled");
        return SyncResult { sync_engine: None };
    }

    let (device_id, device_name) =
        match sqlite_storage_concrete.ensure_device_identity(&config.sync.device_name) {
            Ok(pair) => pair,
            Err(e) => {
                warn!("Failed to get device identity for sync: {e}");
                return SyncResult { sync_engine: None };
            }
        };

    let extractor = Arc::new(oneshim_storage::sync_extractor::SqliteSyncExtractor::new(
        sqlite_storage_concrete.connection_arc(),
        device_id.clone(),
        device_name.clone(),
        config.sync.clone(),
    ));
    let merger = Arc::new(oneshim_storage::sync_merger::SqliteSyncMerger::new(
        sqlite_storage_concrete.connection_arc(),
        device_id.clone(),
    ));

    let transport_result: Result<
        Arc<dyn oneshim_core::ports::sync_transport::SyncTransport>,
        CoreError,
    > = match config.sync.transport {
        SyncTransportKind::File => match &config.sync.sync_folder {
            Some(folder) => oneshim_storage::file_transport::FileSyncTransport::new(
                std::path::PathBuf::from(folder),
                device_id.clone(),
                passphrase.clone(),
            )
            .map_err(Into::into)
            .map(|t| Arc::new(t) as Arc<dyn oneshim_core::ports::sync_transport::SyncTransport>),
            None => {
                warn!("sync transport=file but sync_folder not configured");
                // Iter-108: required-when = Missing (iter-89/99 pattern).
                Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Missing,
                    message: "sync_folder required for file transport".into(),
                })
            }
        },
        SyncTransportKind::Remote => match &config.sync.remote_endpoint {
            Some(endpoint) => {
                // Retrieve auth credential from OS keychain
                let credential = keyring::Entry::new("oneshim", "sync_remote_token")
                    .and_then(|entry| entry.get_password())
                    .unwrap_or_default();
                if credential.is_empty() {
                    warn!("sync transport=remote but no credential in keychain (key: oneshim/sync_remote_token)");
                }
                oneshim_network::sync::RemoteSyncTransport::new(
                    endpoint.clone(),
                    device_id.clone(),
                    passphrase.clone(),
                    config.sync.remote_auth.clone(),
                    credential,
                )
                .map(|t| Arc::new(t) as Arc<dyn oneshim_core::ports::sync_transport::SyncTransport>)
            }
            None => {
                warn!("sync transport=remote but remote_endpoint not configured");
                // Iter-108: required-when = Missing.
                Err(CoreError::Config {
                    code: oneshim_core::error_codes::ConfigCode::Missing,
                    message: "remote_endpoint required for remote transport".into(),
                })
            }
        },
        SyncTransportKind::Lan => {
            #[cfg(feature = "lan-sync")]
            {
                match oneshim_network::sync::lan_tls::load_or_generate_cert(data_dir, &device_id) {
                    Ok((cert_pem, key_pem, fingerprint)) => {
                        // Use block_on to await the async start in sync context
                        match tokio::runtime::Handle::current().block_on(
                            oneshim_network::sync::LanSyncTransport::start(
                                device_id.clone(),
                                device_name.clone(),
                                passphrase.clone(),
                                cert_pem,
                                key_pem,
                                fingerprint,
                                config.sync.lan_port,
                                config.sync.lan_advertise,
                            ),
                        ) {
                            Ok(t) => Ok(Arc::new(t)
                                as Arc<dyn oneshim_core::ports::sync_transport::SyncTransport>),
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(e),
                }
            }
            #[cfg(not(feature = "lan-sync"))]
            {
                let _ = data_dir; // suppress unused warning
                warn!("LAN sync requires 'lan-sync' feature; sync disabled");
                // Iter-108: feature not compiled = service unavailable in
                // this build (user can install a different build with the
                // feature, but this runtime can't serve LAN sync). Wire
                // code `service.unavailable` matches the semantic.
                Err(CoreError::ServiceUnavailable {
                    code: oneshim_core::error_codes::ServiceCode::Unavailable,
                    message: "lan-sync feature not enabled in this build".into(),
                })
            }
        }
    };

    match transport_result {
        Ok(transport) => {
            // Reuse the application-wide ConsentManager instead
            // of constructing a separate instance from the file
            // path. This ensures the SyncEngine sees the same
            // in-memory consent state as the rest of the runtime.
            let sync_engine = Arc::new(
                SyncEngine::new(
                    extractor,
                    merger,
                    transport,
                    consent_manager,
                    device_id,
                    device_name,
                )
                .await,
            );
            info!(
                transport = ?config.sync.transport,
                "Cross-device sync engine initialized"
            );
            SyncResult {
                sync_engine: Some(sync_engine),
            }
        }
        Err(e) => {
            warn!("Failed to create sync transport: {e}");
            SyncResult { sync_engine: None }
        }
    }
}
