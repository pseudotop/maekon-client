//! Custom TCP accept loop for the external gRPC server.
//!
//! Responsibilities (spec §2.2, §S5, §S7):
//! 1. Pre-TLS IP ban check — drop connections from banned IPs immediately.
//! 2. Connection cap — drop new connections once `max_connections` is reached.
//! 3. TCP_NODELAY — reduce latency for gRPC HTTP/2 frames.
//! 4. TLS handshake with 10-second timeout — record failure as IP ban signal.
//! 5. mTLS peer cert extraction + optional `MtlsVerifier` check.
//! 6. Wrap accepted stream in `PeerAwareStream` and forward to the tonic server
//!    via an `mpsc` channel that is consumed as a `ReceiverStream`.
//!
//! The accept loop runs until a `watch::Receiver<bool>` fires (signals shutdown).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};
use tokio_rustls::TlsAcceptor;
use tracing::{debug, warn};

use super::conn_info::{ActiveConnGuard, PeerAwareStream, PeerInfo};
use super::spawn_config::ExternalGrpcSpawnConfig;

/// How long to wait for a TLS handshake before dropping the connection.
const TLS_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Run the accept loop.
///
/// Accepts TCP connections from `listener`, performs the TLS handshake,
/// optionally checks the peer cert via `MtlsVerifier`, then sends each
/// accepted `PeerAwareStream` through `conn_tx` for tonic to serve.
///
/// The loop exits when `shutdown_rx` receives a change notification.
pub(crate) async fn run_accept_loop(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    cfg: Arc<ExternalGrpcSpawnConfig>,
    conn_tx: mpsc::Sender<
        Result<PeerAwareStream<tokio_rustls::server::TlsStream<TcpStream>>, std::io::Error>,
    >,
    active_conns: Arc<AtomicUsize>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    // Optional test-only panic injection point. Gated on `test-support` so the
    // external integration test binary — which enables `test-support` but is
    // compiled outside the regular `#[cfg(test)]` frame — can also trigger it.
    #[cfg(any(test, feature = "test-support"))]
    if PANIC_ON_FIRST_ACCEPT.load(Ordering::SeqCst) {
        PANIC_ON_FIRST_ACCEPT.store(false, Ordering::SeqCst);
        panic!("injected test panic in accept loop");
    }

    loop {
        tokio::select! {
            biased;

            _ = shutdown_rx.changed() => {
                debug!("external_grpc: accept loop received shutdown signal");
                break;
            }

            accept_result = listener.accept() => {
                let (tcp, remote) = match accept_result {
                    Ok(x) => x,
                    Err(e) => {
                        warn!(err = %e, "external_grpc: TCP accept error");
                        continue;
                    }
                };

                // S7 §7: TCP_NODELAY reduces per-frame latency for HTTP/2 multiplexing.
                let _ = tcp.set_nodelay(true);

                // S5: pre-TLS IP ban check — drop banned IPs before TLS overhead.
                if cfg.ip_ban.is_banned(remote) {
                    cfg.metrics
                        .ip_bans_blocked_total
                        .fetch_add(1, Ordering::Relaxed);
                    drop(tcp);
                    continue;
                }

                // S7 §8: connection cap — prevent resource exhaustion.
                let prev = active_conns.fetch_add(1, Ordering::Relaxed);
                if prev >= cfg.config.max_connections {
                    active_conns.fetch_sub(1, Ordering::Relaxed);
                    drop(tcp);
                    continue;
                }

                // Spawn per-connection task for TLS handshake (may be slow).
                let acceptor_c = acceptor.clone();
                let cfg_c = cfg.clone();
                let conn_tx_c = conn_tx.clone();
                let active_c = active_conns.clone();

                tokio::spawn(async move {
                    let tls_result = tokio::time::timeout(
                        TLS_HANDSHAKE_TIMEOUT,
                        acceptor_c.accept(tcp),
                    )
                    .await;

                    let tls_stream = match tls_result {
                        Ok(Ok(s)) => s,
                        Ok(Err(e)) => {
                            warn!(
                                remote = %remote,
                                err = %e,
                                "external_grpc: TLS handshake failed"
                            );
                            cfg_c.ip_ban.record_failure(remote);
                            active_c.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }
                        Err(_timeout) => {
                            warn!(
                                remote = %remote,
                                "external_grpc: TLS handshake timed out"
                            );
                            cfg_c.ip_ban.record_failure(remote);
                            active_c.fetch_sub(1, Ordering::Relaxed);
                            return;
                        }
                    };

                    // mTLS: extract peer cert if needed. The verifier re-checks
                    // lifetime + fingerprint allowlist beyond what rustls enforced
                    // at the CA level.
                    let needs_mtls = cfg_c
                        .config
                        .auth_mode
                        .is_some_and(|m| m.includes_mtls());
                    let peer_cert_der: Option<Vec<u8>> = if needs_mtls {
                        let session = tls_stream.get_ref().1;
                        let certs = session.peer_certificates();
                        match certs.and_then(|c| c.first()) {
                            Some(c) => {
                                if let Some(v) = &cfg_c.mtls_verifier {
                                    if v.verify(c.as_ref()).is_err() {
                                        warn!(
                                            remote = %remote,
                                            "external_grpc: mTLS cert verification failed"
                                        );
                                        cfg_c.ip_ban.record_failure(remote);
                                        active_c.fetch_sub(1, Ordering::Relaxed);
                                        return;
                                    }
                                }
                                Some(c.as_ref().to_vec())
                            }
                            None => None,
                        }
                    } else {
                        None
                    };

                    let peer_info = PeerInfo {
                        remote_addr: remote,
                        peer_cert_der,
                        // CN re-parsed in auth_layer from the DER bytes.
                        cert_subject_cn: None,
                        tls_version: "TLSv1.3".to_string(),
                    };
                    // RAII guard: decrements active_conns when the stream is dropped.
                    let guard = ActiveConnGuard::new(active_c.clone());
                    let wrapped = PeerAwareStream::new(tls_stream, peer_info, Some(guard));

                    // Forward to tonic. Drop on channel-closed (server shut down).
                    // If send fails, the guard in `wrapped` drops here and decrements.
                    if conn_tx_c.send(Ok(wrapped)).await.is_err() {
                        // Guard was moved into `wrapped`, which is now dropped here —
                        // the decrement happens automatically via Drop.
                    }
                    // NOTE: for all pre-stream-creation error paths above (TLS failure,
                    // handshake timeout, mTLS rejection) the manual fetch_sub calls
                    // handle the decrement because no guard was created yet.
                });
            }
        }
    }
}

/// Atomic flag for injecting a panic in the accept loop during tests.
/// Set to `true` before the first accept; the loop panics once then resets the flag.
///
/// Gated on `test-support` so integration tests in the outer (non-unit) test
/// frame can reach it (T16 panic respawn test). Keeping the gate narrow
/// ensures a release binary never exposes this attack surface.
#[cfg(any(test, feature = "test-support"))]
pub static PANIC_ON_FIRST_ACCEPT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(all(test, feature = "external-grpc-tools"))]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;

    use tokio::sync::watch;

    use super::super::ip_ban::IpBan;
    use super::super::metrics::ExternalMetrics;
    use oneshim_storage::sqlite::SqliteStorage;

    fn make_cert_resolver() -> Arc<super::super::cert_resolver::HotReloadCertResolver> {
        use rcgen::{CertificateParams, KeyPair};
        let kp = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        let cert = params.self_signed(&kp).unwrap();
        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(kp.serialize_der()).unwrap();
        let signing = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key_der).unwrap();
        let certified_key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert_der], signing));
        Arc::new(super::super::cert_resolver::HotReloadCertResolver::new(
            certified_key,
        ))
    }

    fn make_server_config(
        cert_resolver: Arc<super::super::cert_resolver::HotReloadCertResolver>,
    ) -> rustls::ServerConfig {
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(cert_resolver)
    }

    /// Build a minimal `ExternalGrpcSpawnConfig` for accept-loop tests.
    ///
    /// Uses `SqliteStorage::open_in_memory` (a `dev-dependency`) for storage;
    /// the accept-loop tests only exercise the pre-TLS IP-ban / cap path so
    /// no storage methods are ever called.
    async fn minimal_cfg(
        max_connections: usize,
        ip_ban: Arc<IpBan>,
    ) -> Arc<ExternalGrpcSpawnConfig> {
        use oneshim_core::config::{AuthMode, ExternalGrpcConfig};

        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"))
            as Arc<dyn crate::storage_port::WebStorage>;
        let (event_tx, _) = tokio::sync::broadcast::channel(1);

        // AuditLogAdapter wraps AuditLogger to implement AuditLogPort.
        let logger = Arc::new(tokio::sync::RwLock::new(
            oneshim_automation::audit::AuditLogger::new(64, 16),
        ));
        let audit_port = Arc::new(oneshim_automation::audit::AuditLogAdapter::new(logger))
            as Arc<dyn oneshim_core::ports::audit_log::AuditLogPort>;

        // MockSystemMonitor via test-support OR inline struct.
        // We use the inline struct to avoid requiring test-support feature flag here.
        struct DummyMonitor;
        #[async_trait::async_trait]
        impl oneshim_core::ports::monitor::SystemMonitor for DummyMonitor {
            async fn collect_metrics(
                &self,
            ) -> Result<oneshim_core::models::system::SystemMetrics, oneshim_core::error::CoreError>
            {
                Err(oneshim_core::error::CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: "dummy monitor".into(),
                })
            }
        }

        let (shutdown_tx_inner, shutdown_rx_inner) = tokio::sync::watch::channel(false);
        Arc::new(super::super::spawn_config::ExternalGrpcSpawnConfig {
            bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            config: ExternalGrpcConfig {
                enabled: true,
                auth_mode: Some(AuthMode::Jwt),
                max_connections,
                ..Default::default()
            },
            storage,
            system_monitor: Arc::new(DummyMonitor),
            event_tx,
            audit_port,
            cert_resolver: make_cert_resolver(),
            jwt_verifier: None,
            mtls_verifier: None,
            ip_ban,
            metrics: Arc::new(ExternalMetrics::new()),
            shutdown_rx: shutdown_rx_inner,
            shutdown_tx: Arc::new(shutdown_tx_inner),
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            live: std::sync::Arc::new(crate::grpc::external::live_config::LiveExternalConfig::new(
                crate::grpc::external::live_config::LiveSnapshot {
                    streaming_enabled: true,
                    load_policy: std::sync::Arc::new(crate::grpc::load_policy::LoadPolicy::new(
                        oneshim_core::config::LoadThresholds::default(),
                    )),
                },
            )),
        })
    }

    /// Accept loop rejects connections from a banned IP (pre-TLS, so no TLS
    /// client setup needed — connection is dropped immediately after TCP accept).
    #[tokio::test]
    async fn accept_loop_rejects_banned_ip() {
        super::super::test_support::install_rustls_crypto_provider();
        let ban = Arc::new(IpBan::new());
        let cfg = minimal_cfg(1024, ban.clone()).await;

        // Pre-seed ban: 5 failures → banned.
        let banned_addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        for _ in 0..5 {
            ban.record_failure(banned_addr);
        }
        assert!(
            ban.is_banned(banned_addr),
            "pre-condition: IP should be banned"
        );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_cfg = make_server_config(cfg.cert_resolver.clone());
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let (conn_tx, mut conn_rx) = mpsc::channel(8);
        let active = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(run_accept_loop(
            listener,
            acceptor,
            cfg.clone(),
            conn_tx,
            active.clone(),
            shutdown_rx,
        ));

        // Connect — the banned 127.0.0.1 will be dropped before TLS.
        let _client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        // No connection should appear on the conn_tx channel.
        assert!(
            conn_rx.try_recv().is_err(),
            "banned IP connection must be dropped before TLS"
        );
        // Metric must record the block.
        assert!(
            cfg.metrics.ip_bans_blocked_total.load(Ordering::Relaxed) >= 1,
            "ip_bans_blocked_total must be incremented"
        );

        let _ = shutdown_tx.send(true);
    }

    /// Accept loop drops connections when active_conns >= max_connections.
    #[tokio::test]
    async fn accept_loop_rejects_over_max_connections() {
        super::super::test_support::install_rustls_crypto_provider();
        let ban = Arc::new(IpBan::new());
        // max_connections = 0 → every connection is immediately over cap.
        let cfg = minimal_cfg(0, ban).await;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server_cfg = make_server_config(cfg.cert_resolver.clone());
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let (conn_tx, mut conn_rx) = mpsc::channel(8);
        let active = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        tokio::spawn(run_accept_loop(
            listener,
            acceptor,
            cfg,
            conn_tx,
            active,
            shutdown_rx,
        ));

        let _client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;

        assert!(
            conn_rx.try_recv().is_err(),
            "connection over cap must be dropped"
        );

        let _ = shutdown_tx.send(true);
    }

    /// Graceful shutdown: the accept loop exits when the shutdown watch fires.
    #[tokio::test]
    async fn graceful_shutdown_stops_accept_loop() {
        super::super::test_support::install_rustls_crypto_provider();
        let ban = Arc::new(IpBan::new());
        let cfg = minimal_cfg(1024, ban).await;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_cfg = make_server_config(cfg.cert_resolver.clone());
        let acceptor = TlsAcceptor::from(Arc::new(server_cfg));

        let (conn_tx, _conn_rx) = mpsc::channel(8);
        let active = Arc::new(AtomicUsize::new(0));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(run_accept_loop(
            listener,
            acceptor,
            cfg,
            conn_tx,
            active,
            shutdown_rx,
        ));

        // Signal shutdown immediately.
        shutdown_tx.send(true).unwrap();

        // Accept loop task should complete quickly.
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(
            result.is_ok(),
            "accept loop should exit within 2s of shutdown signal"
        );
    }
}
