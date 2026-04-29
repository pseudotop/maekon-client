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

use oneshim_core::error::CoreError;
use oneshim_core::models::sync::{ChangeSet, PeerInfo};
use oneshim_core::ports::sync_transport::SyncTransport;
use oneshim_core::sync::Hlc;

use super::lan_discovery::{LanDiscovery, LanPeerInfo};
use super::lan_server::LanPeerServer;
use super::sync_crypto;

use auth::TokenCache;

const PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[allow(dead_code)]
pub struct LanSyncTransport {
    discovery: parking_lot::Mutex<LanDiscovery>,
    server: parking_lot::Mutex<LanPeerServer>,
    http_client: reqwest::Client,
    local_device_id: String,
    local_device_name: String,
    passphrase: String,
    /// Verified peers (device_id -> LanPeerInfo).
    verified_peers: Arc<RwLock<HashMap<String, LanPeerInfo>>>,
    /// Cached session tokens per peer.
    token_cache: TokenCache,
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

        // Build the HTTP client for outbound peer requests
        let http_client = reqwest::Client::builder()
            .timeout(PEER_REQUEST_TIMEOUT)
            .danger_accept_invalid_certs(true) // Self-signed certs in LAN mode
            .build()
            .map_err(|e| CoreError::Network {
                code: oneshim_core::error_codes::NetworkCode::Generic,
                message: format!("failed to build LAN HTTP client: {e}"),
            })?;

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
            http_client,
            local_device_id: device_id,
            local_device_name: device_name,
            passphrase,
            verified_peers: Arc::new(RwLock::new(HashMap::new())),
            token_cache: TokenCache::new(),
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
        // Use HTTP -- TLS is handled by the server side.
        // The reqwest client is built with danger_accept_invalid_certs(true).
        format!("http://{}:{}{}", peer.host, peer.port, path)
    }
}

#[async_trait]
impl SyncTransport for LanSyncTransport {
    /// Push a changeset to all discovered LAN peers.
    ///
    /// Best-effort fanout: logs errors per peer but does not fail the
    /// overall push unless serialization/encryption fails.
    async fn push(&self, changes: &ChangeSet) -> Result<(), CoreError> {
        self.refresh_peers();
        let peers = self.current_peers();
        if peers.is_empty() {
            debug!("no LAN peers discovered, push is a no-op");
            return Ok(());
        }

        let json = serde_json::to_vec(changes).map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
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
        Ok(())
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
        if removed {
            info!(device_id, "LAN peer forgotten");
        } else {
            debug!(device_id, "forget_peer: peer not found in verified list");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
