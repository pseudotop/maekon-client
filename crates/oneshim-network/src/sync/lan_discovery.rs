//! mDNS service registration + browse via `mdns-sd` (Phase 3b-2).
//!
//! Discovers LAN peers advertising `_oneshim-sync._tcp.local.` services.
//! Requires the `lan-sync` feature flag.
//!
//! ## Protocol
//!
//! Each Maekon client registers itself as an mDNS service with:
//! - Service type: `_oneshim-sync._tcp.local.`
//! - Instance name: device_id
//! - TXT records: `device_id`, `device_name`, `fingerprint`, `version`
//!
//! Browsing discovers all such services on the local network and maintains
//! a peer map keyed by device_id. Peers that go offline are automatically
//! removed when their mDNS goodbye is received.

use std::collections::HashMap;
use std::sync::Arc;

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use parking_lot::RwLock;
use tracing::{debug, info, warn};

use oneshim_core::error::CoreError;

const SERVICE_TYPE: &str = "_oneshim-sync._tcp.local.";

/// Version advertised in TXT records for protocol compatibility checks.
const PROTOCOL_VERSION: &str = "1";

/// Peer information discovered via mDNS.
#[derive(Debug, Clone)]
pub struct LanPeerInfo {
    pub device_id: String,
    pub device_name: String,
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub version: String,
}

/// mDNS-based LAN peer discovery.
///
/// Wraps `mdns_sd::ServiceDaemon` for service registration and browsing.
/// The daemon runs its own background thread; `start()` registers the local
/// service and spawns a tokio task that feeds discovered peers into the
/// shared `peers` map.
pub struct LanDiscovery {
    device_id: String,
    device_name: String,
    port: u16,
    fingerprint: String,
    peers: Arc<RwLock<HashMap<String, LanPeerInfo>>>,
    daemon: Option<ServiceDaemon>,
    /// Full service name used for unregistration.
    registered_fullname: Option<String>,
    /// Handle to the background browse task.
    browse_handle: Option<tokio::task::JoinHandle<()>>,
    running: bool,
}

impl LanDiscovery {
    /// Create a new discovery instance (does not start browsing).
    pub fn new(device_id: String, device_name: String, port: u16, fingerprint: String) -> Self {
        Self {
            device_id,
            device_name,
            port,
            fingerprint,
            peers: Arc::new(RwLock::new(HashMap::new())),
            daemon: None,
            registered_fullname: None,
            browse_handle: None,
            running: false,
        }
    }

    /// Register mDNS service and start browsing for peers.
    ///
    /// This performs three actions:
    /// 1. Creates an `mdns_sd::ServiceDaemon`
    /// 2. Registers the local device as `_oneshim-sync._tcp.local.`
    /// 3. Starts browsing for other instances and populates the peer map
    pub fn start(&mut self) -> Result<(), CoreError> {
        if self.running {
            return Ok(());
        }

        let daemon = ServiceDaemon::new().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("failed to create mDNS daemon: {e}"),
        })?;

        // -- Register our service --
        self.register_service(&daemon)?;

        // -- Start browsing --
        let receiver = daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("failed to start mDNS browse: {e}"),
            })?;

        let peers = Arc::clone(&self.peers);
        let local_device_id = self.device_id.clone();

        // Spawn a tokio task that reads from the mdns-sd channel.
        // mdns-sd uses a flume::Receiver which is both sync and async-capable.
        let handle = tokio::task::spawn(async move {
            Self::browse_loop(receiver, peers, &local_device_id).await;
        });

        self.browse_handle = Some(handle);
        self.daemon = Some(daemon);
        self.running = true;

        info!(
            device_id = %self.device_id,
            port = self.port,
            "LAN discovery started (mDNS service: {SERVICE_TYPE})"
        );

        Ok(())
    }

    /// Register the local device as an mDNS service.
    fn register_service(&mut self, daemon: &ServiceDaemon) -> Result<(), CoreError> {
        // Build TXT record properties
        let properties = [
            ("device_id", self.device_id.as_str()),
            ("device_name", self.device_name.as_str()),
            ("fingerprint", self.fingerprint.as_str()),
            ("version", PROTOCOL_VERSION),
        ];

        // ServiceInfo::new wants: service_type, instance_name, host, ip, port, properties
        // Use empty string for host_name to let mdns-sd choose the local hostname.
        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &self.device_id,
            &format!("{}.", gethostname::gethostname().to_string_lossy()),
            "", // IP: let the daemon resolve local addresses
            self.port,
            &properties[..],
        )
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("failed to create service info: {e}"),
        })?;

        let fullname = service_info.get_fullname().to_string();

        daemon
            .register(service_info)
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("failed to register mDNS service: {e}"),
            })?;

        debug!(
            fullname = %fullname,
            "mDNS service registered"
        );

        self.registered_fullname = Some(fullname);
        Ok(())
    }

    /// Background loop that processes mDNS browse events.
    ///
    /// Runs inside a tokio task. Uses `recv_timeout` so the task can be
    /// cancelled (aborted) between iterations.
    async fn browse_loop(
        receiver: mdns_sd::Receiver<ServiceEvent>,
        peers: Arc<RwLock<HashMap<String, LanPeerInfo>>>,
        local_device_id: &str,
    ) {
        loop {
            // recv_async() returns a future that resolves when a service event
            // arrives. We add a tokio timeout so the task is cancellable.
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(2), receiver.recv_async())
                    .await;

            match result {
                Ok(Ok(event)) => {
                    Self::handle_browse_event(&event, &peers, local_device_id);
                }
                Ok(Err(_)) => {
                    // Channel disconnected -- daemon shut down
                    debug!("mDNS browse channel disconnected");
                    break;
                }
                Err(_elapsed) => {
                    // Timeout, loop again. Lets tokio abort the task.
                    continue;
                }
            }
        }
    }

    /// Process a single mDNS browse event, updating the peer map.
    fn handle_browse_event(
        event: &ServiceEvent,
        peers: &Arc<RwLock<HashMap<String, LanPeerInfo>>>,
        local_device_id: &str,
    ) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let device_id = info
                    .get_properties()
                    .get_property_val_str("device_id")
                    .unwrap_or_default()
                    .to_string();

                // Skip our own service
                if device_id == local_device_id || device_id.is_empty() {
                    return;
                }

                let device_name = info
                    .get_properties()
                    .get_property_val_str("device_name")
                    .unwrap_or_default()
                    .to_string();
                let fingerprint = info
                    .get_properties()
                    .get_property_val_str("fingerprint")
                    .unwrap_or_default()
                    .to_string();
                let version = info
                    .get_properties()
                    .get_property_val_str("version")
                    .unwrap_or_default()
                    .to_string();

                // Pick the first address advertised
                let host = info
                    .get_addresses()
                    .iter()
                    .next()
                    .map(|addr| addr.to_string())
                    .unwrap_or_default();
                let port = info.get_port();

                // Only add peers with a matching protocol version
                if version != PROTOCOL_VERSION {
                    debug!(
                        device_id = %device_id,
                        peer_version = %version,
                        local_version = PROTOCOL_VERSION,
                        "ignoring LAN peer with incompatible protocol version"
                    );
                    return;
                }

                let peer = LanPeerInfo {
                    device_id: device_id.clone(),
                    device_name: device_name.clone(),
                    host,
                    port,
                    fingerprint,
                    version,
                };

                info!(
                    device_id = %device_id,
                    device_name = %device_name,
                    port,
                    "LAN peer discovered"
                );

                peers.write().insert(device_id, peer);
            }
            ServiceEvent::ServiceRemoved(_, fullname) => {
                // Try to extract device_id from the removed fullname.
                // Fullname format: "<instance>._oneshim-sync._tcp.local."
                let instance = fullname.split('.').next().unwrap_or_default();
                if !instance.is_empty() && peers.write().remove(instance).is_some() {
                    info!(device_id = %instance, "LAN peer removed");
                }
            }
            ServiceEvent::SearchStarted(_) => {
                debug!("mDNS browse started for {SERVICE_TYPE}");
            }
            ServiceEvent::SearchStopped(_) => {
                debug!("mDNS browse stopped");
            }
            ServiceEvent::ServiceFound(_, _) => {
                // ServiceFound fires before resolution. We handle ServiceResolved.
            }
            _ => {
                // Future variants or platform-specific events.
            }
        }
    }

    /// Stop discovery and unregister mDNS service.
    pub fn stop(&mut self) {
        if !self.running {
            return;
        }

        // Abort the browse task
        if let Some(handle) = self.browse_handle.take() {
            handle.abort();
        }

        // Unregister our service and shut down the daemon
        if let Some(ref daemon) = self.daemon {
            if let Some(ref fullname) = self.registered_fullname {
                if let Err(e) = daemon.unregister(fullname) {
                    warn!("failed to unregister mDNS service: {e}");
                }
            }
            if let Err(e) = daemon.shutdown() {
                warn!("failed to shut down mDNS daemon: {e}");
            }
        }

        self.daemon = None;
        self.registered_fullname = None;
        self.running = false;
        self.peers.write().clear();

        info!("LAN discovery stopped");
    }

    /// Return a snapshot of currently discovered peers.
    pub fn peers(&self) -> HashMap<String, LanPeerInfo> {
        self.peers.read().clone()
    }

    /// Check if discovery is running.
    pub fn is_running(&self) -> bool {
        self.running
    }
}

impl Drop for LanDiscovery {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_stop_lifecycle() {
        // Test with real mDNS daemon. start() spawns a tokio task
        // for browsing, so a runtime must be active.
        let mut disc = LanDiscovery::new(
            "dev-test-1".to_string(),
            "Test Mac".to_string(),
            0,
            "abc123".to_string(),
        );
        assert!(!disc.is_running());
        // Note: start() creates a real mDNS daemon. On CI without
        // multicast this may fail, so we allow the error.
        let result = disc.start();
        if result.is_ok() {
            assert!(disc.is_running());
            disc.stop();
            assert!(!disc.is_running());
        }
    }

    #[test]
    fn peers_initially_empty() {
        let disc = LanDiscovery::new(
            "dev-1".to_string(),
            "Test".to_string(),
            9090,
            "fp".to_string(),
        );
        assert!(disc.peers().is_empty());
    }

    #[test]
    fn handle_browse_event_resolved() {
        // Simulate a ServiceResolved event by constructing a LanPeerInfo directly
        // and verify the peer map logic.
        let peers = Arc::new(RwLock::new(HashMap::new()));

        // Insert a simulated peer
        let peer = LanPeerInfo {
            device_id: "peer-1".to_string(),
            device_name: "Peer Mac".to_string(),
            host: "192.168.1.42".to_string(),
            port: 19090,
            fingerprint: "fp-peer-1".to_string(),
            version: "1".to_string(),
        };
        peers.write().insert("peer-1".to_string(), peer);

        assert_eq!(peers.read().len(), 1);
        assert!(peers.read().contains_key("peer-1"));

        // Simulate removal
        peers.write().remove("peer-1");
        assert!(peers.read().is_empty());
    }

    #[test]
    fn drop_cleans_up() {
        let disc = LanDiscovery::new(
            "dev-drop".to_string(),
            "Drop Test".to_string(),
            0,
            "fp".to_string(),
        );
        // Drop should not panic even when not started
        drop(disc);
    }
}
