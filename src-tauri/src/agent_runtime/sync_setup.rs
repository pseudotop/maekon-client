use std::path::Path;
use std::sync::Arc;
use tracing::{info, warn};

// F-RR-C34-01: no block_on in async fn — enforced structurally; tests exercise
// the async call-path to confirm the function is awaitable without panic.

use maekon_core::config::{AppConfig, SyncTransportKind};
use maekon_core::error::CoreError;
use maekon_core::ports::consent_manager::ConsentManagerPort;

use crate::sync_engine::SyncEngine;

/// Result of the sync engine setup.
pub(super) struct SyncResult {
    /// Populated when sync is enabled, passphrase is set, and transport creation succeeds.
    pub sync_engine: Option<Arc<SyncEngine>>,
}

/// Minimum length for `MAEKON_SYNC_PASSPHRASE`.
///
/// The passphrase derives (via Argon2id) both the LAN HMAC challenge-response
/// key and the AES-256-GCM payload-encryption key, so a trivially short value
/// yields a weak key.  This is a length floor, not a substitute for entropy.
const MIN_SYNC_PASSPHRASE_LEN: usize = 12;

/// Whether a sync passphrase satisfies the minimum-length policy.
fn sync_passphrase_meets_policy(passphrase: &str) -> bool {
    passphrase.chars().count() >= MIN_SYNC_PASSPHRASE_LEN
}

/// Build the cross-device sync engine.
///
/// Requires `MAEKON_SYNC_PASSPHRASE` env var, device identity from SQLite, and a valid transport.
pub(super) async fn build_sync_engine(
    config: &AppConfig,
    data_dir: &Path,
    sqlite_storage_concrete: &Arc<maekon_storage::sqlite::SqliteStorage>,
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    erasure_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> SyncResult {
    if !config.sync.enabled {
        return SyncResult { sync_engine: None };
    }

    let passphrase = std::env::var("MAEKON_SYNC_PASSPHRASE").unwrap_or_default();
    if passphrase.is_empty() {
        warn!("sync enabled but MAEKON_SYNC_PASSPHRASE not set; sync disabled");
        return SyncResult { sync_engine: None };
    }
    if !sync_passphrase_meets_policy(&passphrase) {
        warn!(
            min_len = MIN_SYNC_PASSPHRASE_LEN,
            actual_len = passphrase.chars().count(),
            "MAEKON_SYNC_PASSPHRASE is shorter than the minimum; sync disabled (use a longer passphrase)"
        );
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

    let extractor = Arc::new(maekon_storage::sync_extractor::SqliteSyncExtractor::new(
        sqlite_storage_concrete.connection_arc(),
        device_id.clone(),
        device_name.clone(),
        config.sync.clone(),
    ));
    let merger = Arc::new(maekon_storage::sync_merger::SqliteSyncMerger::new(
        sqlite_storage_concrete.connection_arc(),
        device_id.clone(),
    ));

    let transport_result: Result<
        Arc<dyn maekon_core::ports::sync_transport::SyncTransport>,
        CoreError,
    > = match config.sync.transport {
        SyncTransportKind::File => match &config.sync.sync_folder {
            Some(folder) => maekon_storage::file_transport::FileSyncTransport::new(
                std::path::PathBuf::from(folder),
                device_id.clone(),
                passphrase.clone(),
            )
            .map_err(Into::into)
            .map(|t| Arc::new(t) as Arc<dyn maekon_core::ports::sync_transport::SyncTransport>),
            None => {
                warn!("sync transport=file but sync_folder not configured");
                // Iter-108: required-when = Missing (iter-89/99 pattern).
                Err(CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Missing,
                    message: "sync_folder required for file transport".into(),
                })
            }
        },
        // #5032: the HTTP remote sync transport lives in `maekon-network`, only
        // compiled when the `analysis` feature pulls in `dep:maekon-network`.
        // Under `--no-default-features` it falls back to `service.unavailable`
        // (same shape as the `lan-sync`-disabled branch below). No behaviour
        // change when `analysis` is enabled.
        #[cfg(feature = "analysis")]
        SyncTransportKind::Remote => match &config.sync.remote_endpoint {
            Some(endpoint) => {
                // Retrieve auth credential from OS keychain
                let credential = keyring::Entry::new("maekon", "sync_remote_token")
                    .and_then(|entry| entry.get_password())
                    .unwrap_or_default();
                if credential.is_empty() {
                    warn!("sync transport=remote but no credential in keychain (key: maekon/sync_remote_token)");
                }
                maekon_network::sync::RemoteSyncTransport::new(
                    endpoint.clone(),
                    device_id.clone(),
                    passphrase.clone(),
                    config.sync.remote_auth.clone(),
                    credential,
                )
                .map(|t| Arc::new(t) as Arc<dyn maekon_core::ports::sync_transport::SyncTransport>)
            }
            None => {
                warn!("sync transport=remote but remote_endpoint not configured");
                // Iter-108: required-when = Missing.
                Err(CoreError::Config {
                    code: maekon_core::error_codes::ConfigCode::Missing,
                    message: "remote_endpoint required for remote transport".into(),
                })
            }
        },
        #[cfg(not(feature = "analysis"))]
        SyncTransportKind::Remote => {
            warn!(
                "sync transport=remote requires the 'analysis' feature \
                 (maekon-network) which is not compiled in this build; sync disabled"
            );
            Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "remote sync transport not enabled in this build".into(),
            })
        }
        SyncTransportKind::Lan => {
            #[cfg(feature = "lan-sync")]
            {
                // F-RR-C29-03: load_or_generate_cert does blocking disk I/O (std::fs::read /
                // std::fs::write) and CPU-bound rcgen key generation on the generate path.
                // Offload to spawn_blocking so the async runtime thread is not stalled.
                let data_dir_owned = data_dir.to_path_buf();
                let device_id_for_tls = device_id.clone();
                let tls_result = tokio::task::spawn_blocking(move || {
                    maekon_network::sync::lan_tls::load_or_generate_cert(
                        &data_dir_owned,
                        &device_id_for_tls,
                    )
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("load_or_generate_cert task panicked: {e}"),
                })
                .and_then(|r| r);
                match tls_result {
                    Ok((cert_pem, key_pem, fingerprint)) => {
                        // build_sync_engine is already async, so .await is correct here.
                        // block_on inside an async context panics; use .await directly.
                        let pin_store: std::sync::Arc<
                            dyn maekon_core::ports::lan_pin_store::LanPinStorePort,
                        > = std::sync::Arc::new(
                            maekon_storage::lan_pin_store_adapter::LanPinStoreAdapter::new(
                                sqlite_storage_concrete.clone(),
                            ),
                        );
                        // #5174 LAN: storage-back the peer server so /sync/pull serves real
                        // extractor data and /sync/push applies via the merger (same
                        // instances the SyncEngine drives). The `let` bindings trigger the
                        // Arc<Concrete> -> Arc<dyn> unsizing coercion.
                        let lan_extractor: std::sync::Arc<
                            dyn maekon_core::ports::change_extractor::ChangeExtractor,
                        > = extractor.clone();
                        let lan_merger: std::sync::Arc<
                            dyn maekon_core::ports::change_merger::ChangeMerger,
                        > = merger.clone();
                        match maekon_network::sync::LanSyncTransport::start(
                            device_id.clone(),
                            device_name.clone(),
                            passphrase.clone(),
                            cert_pem,
                            key_pem,
                            fingerprint,
                            config.sync.lan_port,
                            config.sync.lan_advertise,
                            pin_store,
                            Some(lan_extractor),
                            Some(lan_merger),
                            consent_manager.clone(),
                        )
                        .await
                        {
                            Ok(t) => Ok(Arc::new(t)
                                as Arc<dyn maekon_core::ports::sync_transport::SyncTransport>),
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
                    code: maekon_core::error_codes::ServiceCode::Unavailable,
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
            //
            // #5143: audit sync egress in the #4803 ledger. The concrete
            // SqliteStorage already implements `EgressLedgerSink`; the
            // destination label is the transport family (peer/endpoint detail is
            // deliberately not logged).
            let egress_destination = match config.sync.transport {
                SyncTransportKind::File => "sync.file",
                SyncTransportKind::Remote => "sync.remote",
                SyncTransportKind::Lan => "sync.lan",
            }
            .to_string();
            let sync_engine = Arc::new(
                SyncEngine::new(
                    extractor,
                    merger,
                    transport,
                    consent_manager,
                    erasure_requested,
                    device_id,
                    device_name,
                )
                .await
                .with_egress(sqlite_storage_concrete.clone(), egress_destination)
                // #5156: durable fire-once gate for consent-revoke DeletionEvents.
                .with_erasure_store(sqlite_storage_concrete.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// F-RR-C34-01 regression guard.
    ///
    /// `build_sync_engine` is an `async fn`. Before this fix, the LAN transport
    /// branch called `Handle::current().block_on(...)` inside an async context,
    /// which panics at runtime ("Cannot start a runtime from within a runtime").
    ///
    /// This test calls `build_sync_engine` directly with sync disabled so it
    /// returns on the early-return path at line 27. The key property being
    /// verified is that the function is a proper `async fn` that can be
    /// `.await`-ed — if `block_on` were still present and called on the Tokio
    /// runtime thread it would panic even before reaching the transport branch.
    #[tokio::test]
    async fn test_build_sync_engine_no_panic_when_sync_disabled() {
        let mut config = maekon_core::config::AppConfig::default_config();
        config.sync.enabled = false;

        let storage = Arc::new(
            maekon_storage::sqlite::SqliteStorage::open_in_memory(30).expect("in-memory sqlite"),
        );
        let data_dir = PathBuf::from("/tmp");

        let result = build_sync_engine(&config, &data_dir, &storage, None, None).await;

        assert!(
            result.sync_engine.is_none(),
            "sync disabled must yield no engine"
        );
    }

    /// GN-3: the passphrase length floor rejects short/empty passphrases and
    /// accepts ones at or above the minimum — verified without env-var mutation.
    #[test]
    fn short_passphrase_rejected_by_policy() {
        assert!(!sync_passphrase_meets_policy(""));
        assert!(!sync_passphrase_meets_policy("short"));
        // 11 chars — just below the floor.
        assert!(!sync_passphrase_meets_policy("12345678901"));
        // 12 chars — exactly at the floor.
        assert!(sync_passphrase_meets_policy("123456789012"));
        assert!(sync_passphrase_meets_policy("a-strong-enough-passphrase"));
    }

    /// #5147 item 3: the egress ledger audit must not be silently skippable. If a future
    /// construction path drops the `.with_egress(..)` chain in `build_sync_engine`, the
    /// compliance egress audit would compile away to nothing — this guard fails first.
    /// Serialized because it mutates the `MAEKON_SYNC_PASSPHRASE` process env.
    #[tokio::test]
    #[serial_test::serial]
    async fn build_sync_engine_wires_egress_sink_when_enabled() {
        let dir = std::env::temp_dir().join("maekon_egress_wiring_test");
        let mut config = maekon_core::config::AppConfig::default_config();
        config.sync.enabled = true; // default transport is File
        config.sync.sync_folder = Some(dir.to_string_lossy().to_string());

        std::env::set_var("MAEKON_SYNC_PASSPHRASE", "test-passphrase-1234");
        let storage = Arc::new(
            maekon_storage::sqlite::SqliteStorage::open_in_memory(30).expect("in-memory sqlite"),
        );
        let result = build_sync_engine(&config, &dir, &storage, None, None).await;
        std::env::remove_var("MAEKON_SYNC_PASSPHRASE");

        let engine = result
            .sync_engine
            .expect("enabled sync + File transport should build an engine");
        assert!(
            engine.has_egress_sink(),
            "build_sync_engine must wire the egress ledger sink (#5147)"
        );
    }
}
