//! Phase 3b LAN peer-to-peer sync transport.
//!
//! Provides mDNS-based peer discovery and encrypted HTTPS push/pull
//! changeset synchronization between devices on the same local network.
//! Wired into the main sync pipeline via `sync_setup.rs` when the user
//! selects `SyncTransportKind::Lan`.
//!
//! Requires the `lan-sync` feature flag (`cargo build --features lan-sync`).
//! When compiled without this feature, the `SyncTransportKind::Lan` arm in
//! `sync_setup` returns an error and sync is disabled.

mod auth;
mod operations;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use tracing::{debug, info};

use maekon_core::error::CoreError;
use maekon_core::models::sync::{ChangeSet, PeerInfo};
use maekon_core::ports::change_extractor::ChangeExtractor;
use maekon_core::ports::change_merger::ChangeMerger;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::lan_pin_store::LanPinStorePort;
use maekon_core::ports::sync_transport::SyncTransport;
use maekon_core::sync::Hlc;

use super::lan_discovery::{LanDiscovery, LanPeerInfo};
use super::lan_server::LanPeerServer;
use super::sync_crypto;

use auth::TokenCache;

const PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[allow(dead_code)]
pub struct LanSyncTransport {
    discovery: parking_lot::Mutex<LanDiscovery>,
    server: parking_lot::Mutex<LanPeerServer>,
    local_device_id: String,
    local_device_name: String,
    passphrase: String,
    /// Verified peers (device_id -> LanPeerInfo).
    verified_peers: Arc<RwLock<HashMap<String, LanPeerInfo>>>,
    /// Cached session tokens per peer.
    token_cache: TokenCache,
    pin_store: Arc<dyn LanPinStorePort>,
    /// Per-peer reqwest client (TLS verifier baked in), keyed by device_id.
    peer_clients: Arc<RwLock<HashMap<String, reqwest::Client>>>,
    /// Process-wide rustls crypto provider (installed once at start).
    crypto_provider: Arc<rustls::crypto::CryptoProvider>,
}

impl LanSyncTransport {
    /// Create and start the LAN sync transport.
    ///
    /// This initializes discovery, starts the peer server, and registers mDNS.
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        device_id: String,
        device_name: String,
        passphrase: String,
        cert_pem: Vec<u8>,
        key_pem: Vec<u8>,
        fingerprint: String,
        lan_port: u16,
        lan_advertise: bool,
        pin_store: Arc<dyn LanPinStorePort>,
        extractor: Option<Arc<dyn ChangeExtractor>>,
        merger: Option<Arc<dyn ChangeMerger>>,
        consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    ) -> Result<Self, CoreError> {
        let mut discovery = LanDiscovery::new(
            device_id.clone(),
            device_name.clone(),
            lan_port,
            fingerprint.clone(),
        );

        let mut server = LanPeerServer::new(
            device_id.clone(),
            device_name.clone(),
            passphrase.clone(),
            fingerprint,
        );
        // #5174 LAN: storage-back the server (production) before start(); `None` in
        // transport-level tests keeps the in-memory buffer path. consent_manager gates
        // the server's data exchange on cross_device_sync consent.
        server.set_storage(extractor, merger, consent_manager);

        // Start the peer server (HTTPS if cert/key valid, else HTTP fallback)
        let actual_port = server.start(&cert_pem, &key_pem, lan_port).await?;

        // Update discovery with the actual bound port, then start mDNS
        if lan_advertise {
            // Re-create discovery with the real port (important when lan_port was 0)
            discovery = LanDiscovery::new(
                device_id.clone(),
                device_name.clone(),
                actual_port,
                server.fingerprint().to_string(),
            );
            discovery.start()?;
        }

        // Install (or reuse) the process-wide rustls crypto provider. Per-peer
        // TLS clients (TOFU pinning) are built lazily from this provider.
        // `install_default` consumes an owned `CryptoProvider` (not an `Arc`),
        // so build the provider value, install a clone, and keep an `Arc` of it.
        let crypto_provider = rustls::crypto::CryptoProvider::get_default()
            .cloned()
            .unwrap_or_else(|| {
                let provider = rustls::crypto::aws_lc_rs::default_provider();
                let _ = provider.clone().install_default();
                Arc::new(provider)
            });

        info!(
            device_id = %device_id,
            port = actual_port,
            tls = server.is_tls_enabled(),
            advertise = lan_advertise,
            "LAN sync transport started"
        );

        Ok(Self {
            discovery: parking_lot::Mutex::new(discovery),
            server: parking_lot::Mutex::new(server),
            local_device_id: device_id,
            local_device_name: device_name,
            passphrase,
            verified_peers: Arc::new(RwLock::new(HashMap::new())),
            token_cache: TokenCache::new(),
            pin_store,
            peer_clients: Arc::new(RwLock::new(HashMap::new())),
            crypto_provider,
        })
    }

    /// Stop the transport (mDNS + server).
    pub fn stop(&self) {
        self.server.lock().stop();
        self.discovery.lock().stop();
        info!("LAN sync transport stopped");
    }

    /// Get the bound server port.
    pub fn server_port(&self) -> u16 {
        self.server.lock().port()
    }

    /// Enqueue a changeset for peers to pull from this device's server.
    pub fn enqueue_outbound(&self, changeset: ChangeSet) {
        self.server.lock().enqueue_outbound(changeset);
    }

    /// Drain changesets received from peers (pushed to this device's server).
    pub fn drain_received(&self) -> Vec<ChangeSet> {
        self.server.lock().drain_received()
    }

    /// Refresh the verified peers list from mDNS discovery.
    ///
    /// Merges newly discovered peers into the verified peer map.
    /// Peers that are no longer advertised via mDNS are removed,
    /// but only when discovery is actively running (to avoid pruning
    /// manually added peers when mDNS is disabled).
    pub fn refresh_peers(&self) {
        let disc = self.discovery.lock();
        let discovered = disc.peers();
        let is_running = disc.is_running();
        drop(disc);

        let mut verified = self.verified_peers.write();

        // Add newly discovered peers
        for (id, peer) in &discovered {
            if !verified.contains_key(id) {
                debug!(device_id = %id, host = %peer.host, "new LAN peer discovered");
            }
            verified.insert(id.clone(), peer.clone());
        }

        // Only prune when discovery is actively running.
        // When discovery is off (e.g., tests), keep manually added peers.
        if is_running && !discovered.is_empty() {
            let active_ids: std::collections::HashSet<&String> = discovered.keys().collect();
            verified.retain(|id, _| active_ids.contains(id));
        }
    }

    /// Return a snapshot of the current verified peers.
    fn current_peers(&self) -> HashMap<String, LanPeerInfo> {
        self.verified_peers.read().clone()
    }

    /// Build the base URL for a peer's sync server.
    fn peer_url(peer: &LanPeerInfo, path: &str) -> String {
        format!("https://{}:{}{}", peer.host, peer.port, path)
    }

    /// Build a per-peer reqwest client whose TLS handshake is verified by a
    /// `TofuVerifier`. Returns the client, the captured-fingerprint cell, and
    /// whether this was a first contact (None pin ⇒ caller upserts on success).
    async fn build_peer_client(
        &self,
        peer_id: &str,
        advertised_fp: Option<String>,
    ) -> Result<
        (
            reqwest::Client,
            std::sync::Arc<parking_lot::Mutex<Option<String>>>,
            bool,
        ),
        CoreError,
    > {
        use crate::sync::lan_tofu_verifier::TofuVerifier;

        let stored_pin = self.pin_store.get_pin(peer_id).await?;
        if matches!(&stored_pin, Some((_, true))) {
            return Err(CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: format!("LAN peer {peer_id} pin is revoked"),
            });
        }
        let first_contact = stored_pin.is_none();
        let captured = std::sync::Arc::new(parking_lot::Mutex::new(None));

        let verifier = std::sync::Arc::new(TofuVerifier {
            advertised_fp,
            stored_pin,
            captured: captured.clone(),
            provider: self.crypto_provider.clone(),
        });

        let tls_config = rustls::ClientConfig::builder_with_provider(self.crypto_provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("rustls config: {e}"),
            })?
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

        let client = reqwest::Client::builder()
            .timeout(PEER_REQUEST_TIMEOUT)
            .use_preconfigured_tls(tls_config)
            .build()
            .map_err(|e| CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!("build LAN TLS client: {e}"),
            })?;

        Ok((client, captured, first_contact))
    }

    /// Return a cached per-peer client, if present.
    ///
    /// Push and pull operations call this after `get_session_token_with_retry`
    /// (which drives `authenticate_with_peer` on a cache miss and populates
    /// `peer_clients`), so the client is expected to be present by the time
    /// it is needed.
    fn cached_peer_client(&self, peer_id: &str) -> Option<reqwest::Client> {
        self.peer_clients.read().get(peer_id).cloned()
    }
}

#[async_trait]
impl SyncTransport for LanSyncTransport {
    /// Push a changeset to all discovered LAN peers.
    ///
    /// Best-effort fanout: logs errors per peer but does not fail the
    /// overall push unless serialization/encryption fails.
    async fn push(&self, changes: &ChangeSet) -> Result<usize, CoreError> {
        self.refresh_peers();
        let peers = self.current_peers();
        if peers.is_empty() {
            debug!("no LAN peers discovered, push is a no-op");
            return Ok(0);
        }

        let json = serde_json::to_vec(changes).map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("serialize changeset: {e}"),
        })?;
        let encrypted = sync_crypto::encrypt(&self.passphrase, &json)?;

        let mut success_count = 0u32;
        let mut fail_count = 0u32;

        for (peer_id, peer) in &peers {
            match self.push_to_peer(peer_id, peer, &encrypted).await {
                Ok(true) => success_count += 1,
                Ok(false) => fail_count += 1,
                Err(_) => fail_count += 1,
            }
        }

        debug!(
            total_peers = peers.len(),
            success_count, fail_count, "LAN push completed"
        );
        // #5143: only peers that confirmed receipt count as egress. `0` here
        // (no peers, or all failed) means nothing actually left the device, so
        // the audit ledger records no `uploaded` row.
        Ok(success_count as usize)
    }

    /// Pull changesets from the first available LAN peer.
    ///
    /// Iterates discovered peers in arbitrary order and returns the first
    /// successful response containing data. Returns `None` if no peer has
    /// new data.
    async fn pull(&self, since: &Hlc) -> Result<Option<ChangeSet>, CoreError> {
        self.refresh_peers();
        let peers = self.current_peers();
        if peers.is_empty() {
            return Ok(None);
        }

        for (peer_id, peer) in &peers {
            match self.pull_from_peer(peer_id, peer, since).await {
                Ok(Some(cs)) => return Ok(Some(cs)),
                Ok(None) => continue,
                Err(e) => {
                    debug!(peer_id, error = %e, "pull from peer failed, trying next");
                    continue;
                }
            }
        }

        Ok(None)
    }

    /// Discover peers via mDNS.
    async fn discover_peers(&self) -> Result<Vec<PeerInfo>, CoreError> {
        self.refresh_peers();
        let disc_peers = self.current_peers();
        let peers: Vec<PeerInfo> = disc_peers
            .values()
            .map(|p| PeerInfo {
                device_id: p.device_id.clone(),
                device_name: p.device_name.clone(),
                last_sync_at: String::new(),
                watermark: Hlc::default(),
            })
            .collect();
        debug!(count = peers.len(), "discovered LAN peers");
        Ok(peers)
    }

    async fn forget_peer(&self, device_id: &str) -> Result<(), CoreError> {
        let removed = self.verified_peers.write().remove(device_id).is_some();
        self.token_cache.invalidate(device_id);
        self.peer_clients.write().remove(device_id);
        self.pin_store.clear_pin(device_id).await?;
        if removed {
            info!(device_id, "LAN peer forgotten (pin cleared)");
        } else {
            debug!(
                device_id,
                "forget_peer: peer not found in verified list (pin cleared)"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
