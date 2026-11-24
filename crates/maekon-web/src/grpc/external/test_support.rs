//! Shared test helpers for external gRPC integration tests.
//!
//! Gated on `#[cfg(any(test, feature = "test-support"))]` — the `test-support`
//! feature is strictly opt-in (NEVER enabled by default or transitively via
//! `grpc-dashboard-external`). Integration tests must invoke with
//! `--features grpc-dashboard-external,external-grpc-tools,test-support`.
//!
//! **ES256 note**: the `ring` crypto backend (default for rcgen) does NOT support
//! RSA key generation at runtime (RSA key-gen requires platform PRNG that ring
//! intentionally omits). All JWT helpers here use ES256 (P-256 ECDSA) which ring
//! fully supports.
//!
//! **rcgen 0.14 signed_by API**: `CertificateParams::signed_by` takes two arguments:
//! `(self, public_key: &impl PublicKeyData, issuer: &Issuer<'_, impl SigningKey>)`.
//! Create the `Issuer` using `Issuer::from_ca_cert_pem`.
//!
//! # Phase 0-9 test fixture API (CR4)
//!
//! - [`fixture_bridge`] / [`fixture_metrics`] — Tasks 0.6, 3.1
//! - [`InnerEcho`] — Tasks 0.6, 3.1 (trailer-status simulation)
//! - [`AuthContext::fixture`] / [`PeerInfo::fixture`] — Tasks 0.6, 3.1, 6.1
//! - [`PassthroughInner`] — Task 6.1
//! - [`connect_loopback`] / [`req_with_valid_auth`] — Task 9.4 G3 test dependencies

mod audit;
mod crypto;
mod error;
mod service;
mod transport;

pub use audit::{
    fixture_bridge, fixture_metrics, CapturedEntry, CapturingAudit, MockRecorder, NoopAudit,
};

pub use crypto::{
    install_rustls_crypto_provider, server_cert_pem, test_ca_and_client_cert, test_cert_pair,
    test_jwt_keypair, test_mint_jwt, TestCaAndClient, TestJwt,
};
pub use error::FixtureError;
pub use service::{EchoBody, InnerEcho, PassthroughInner};
pub use transport::{
    connect_loopback, req_with_valid_auth, serve_external_with_service,
    spawn_server_with_config_manager, ServerHandle,
};

// ── AuthContext::fixture / PeerInfo::fixture ─────────────────────────────────

use super::conn_info::{AuthContext, AuthType, PeerInfo};

impl AuthContext {
    /// Canonical fixture used by Tasks 0.6, 3.1, 6.1, 9.4.
    ///
    /// Returns a deterministic `AuthContext` with `AuthType::Jwt`, a fixed
    /// `client_id`, a fixed `jti`, and a stable ULID-shaped `command_id`.
    pub fn fixture() -> Self {
        Self {
            auth_type: AuthType::Jwt,
            client_id: "test-client".into(),
            jti: Some("test-jti".into()),
            command_id: "01HXFIXTURE0000000000000000".into(),
        }
    }
}

impl PeerInfo {
    /// Canonical fixture used by Tasks 0.6, 3.1, 6.1, 9.4.
    ///
    /// Returns `127.0.0.1:50001` with no mTLS cert and TLS 1.3 version string.
    pub fn fixture() -> Self {
        Self {
            remote_addr: std::net::SocketAddr::from(([127, 0, 0, 1], 50001)),
            peer_cert_der: None,
            cert_subject_cn: None,
            tls_version: "TLSv1.3".into(),
        }
    }
}
