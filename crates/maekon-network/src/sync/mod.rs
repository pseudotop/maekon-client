//! Sync transport adapters (Phase 3b).
//!
//! - `RemoteSyncTransport` -- REST push/pull to a cloud endpoint.
//! - `LanSyncTransport` -- mDNS + HTTPS peer-to-peer (behind `lan-sync` feature).

pub mod reference_server;
pub mod remote_transport;
pub mod sync_crypto;
// #6923/#6925: shared response-body size cap (used by both remote + LAN transport).
pub(crate) mod http_body;

#[cfg(feature = "lan-sync")]
pub mod lan_crypto;
#[cfg(feature = "lan-sync")]
pub mod lan_discovery;
#[cfg(feature = "lan-sync")]
pub mod lan_server;
#[cfg(feature = "lan-sync")]
pub mod lan_tls;
#[cfg(feature = "lan-sync")]
mod lan_tofu_verifier;
#[cfg(feature = "lan-sync")]
pub mod lan_transport;

pub use remote_transport::RemoteSyncTransport;

#[cfg(feature = "lan-sync")]
pub use lan_transport::LanSyncTransport;
