//! D13 V2c external gRPC binding.
//! Feature-gated: compile iff `grpc-dashboard-external` enabled.

pub mod accept_loop;
pub mod audit_bridge;
pub(crate) mod audit_layer;
pub mod auth_layer;
pub mod cert_resolver;
pub mod config_reload;
pub mod conn_info;
pub mod ip_ban;
pub mod jwt_verifier;
pub mod live_config;
pub mod metrics;
pub mod mtls_verifier;
pub mod port_collision;
pub(crate) mod request_id_layer;
pub mod spawn_config;
pub mod tls_config;
pub(crate) mod trailer_body;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use futures::FutureExt as _;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{error, info, warn};

use crate::proto::dashboard::v1::dashboard_service_server::DashboardServiceServer;

use self::accept_loop::run_accept_loop;
use self::audit_bridge::AuditBridge;
use self::audit_layer::AuditLayer;
use self::auth_layer::AuthLayer;
use self::spawn_config::ExternalGrpcSpawnConfig;
use self::tls_config::TlsLoadError;

// ── TLS helpers ──────────────────────────────────────────────────────────────

/// Build a `rustls::ServerConfig` from the given cert resolver.
///
/// If `mtls_ca_bytes` is `Some`, install a `WebPkiClientVerifier` requiring
/// the client to present a certificate signed by the supplied CA(s).
/// Otherwise, `with_no_client_auth()` is used (JWT-only auth mode).
///
/// ALPN: `["h2"]` is always advertised — required for gRPC (HTTP/2) clients
/// that use TLS ALPN negotiation (e.g., tonic with `tls-aws-lc`).
pub fn build_server_config(
    cert_resolver: Arc<cert_resolver::HotReloadCertResolver>,
    mtls_ca_bytes: Option<Vec<u8>>,
) -> Result<rustls::ServerConfig, TlsLoadError> {
    let mut cfg = if let Some(ca_bytes) = mtls_ca_bytes {
        use rustls::server::WebPkiClientVerifier;
        use rustls::RootCertStore;

        let mut roots = RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut ca_bytes.as_slice()) {
            let c = cert.map_err(|e| TlsLoadError::ParseCert(e.to_string()))?;
            roots
                .add(c)
                .map_err(|e| TlsLoadError::ParseCert(e.to_string()))?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| TlsLoadError::ParseCert(e.to_string()))?;
        rustls::ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_cert_resolver(cert_resolver)
    } else {
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_cert_resolver(cert_resolver)
    };
    // Advertise HTTP/2 via ALPN — required for gRPC clients doing TLS ALPN negotiation.
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Ok(cfg)
}

// ── serve_external ───────────────────────────────────────────────────────────

/// Errors returned by `serve_external`.
#[derive(Debug)]
pub enum ServeExternalError {
    /// TCP bind failed.
    Bind(std::io::Error),
    /// TLS config failed.
    Tls(TlsLoadError),
    /// tonic transport error.
    Tonic(tonic::transport::Error),
}

impl std::fmt::Display for ServeExternalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(e) => write!(f, "bind: {e}"),
            Self::Tls(e) => write!(f, "tls: {e}"),
            Self::Tonic(e) => write!(f, "tonic: {e}"),
        }
    }
}

/// Start the external gRPC server.
///
/// Binds a `TcpListener`, runs the custom accept loop (IP ban + TLS handshake),
/// and serves through a tonic `Server` with `AuthLayer` middleware.
///
/// Returns when the shutdown signal in `cfg.shutdown_rx` fires or on a fatal error.
pub async fn serve_external(cfg: ExternalGrpcSpawnConfig) -> Result<(), ServeExternalError> {
    // `integration_auth_token: None` is enforced by `from_external_spawn_config`
    // (Task 13 spec §2.5 threat model).
    let service = crate::grpc::DashboardServiceImpl::from_external_spawn_config(&cfg);
    serve_external_inner(cfg, service, "external_grpc").await
}

/// Shared core of [`serve_external`] and the test-only
/// `serve_external_with_service`. Production and test entries diverge only in
/// (a) which `DashboardService` impl is wired, and (b) the log tag used to
/// distinguish prod vs test bind/shutdown lines. TLS, accept loop, layer
/// stack, HTTP/2 hardening, and shutdown handling are bit-for-bit identical
/// — keeping them in one place ensures lint coverage and behavior parity.
async fn serve_external_inner<T>(
    cfg: ExternalGrpcSpawnConfig,
    service: T,
    log_tag: &'static str,
) -> Result<(), ServeExternalError>
where
    T: crate::proto::dashboard::v1::dashboard_service_server::DashboardService,
{
    let shutdown = cfg.shutdown_rx.clone();
    let listener = tokio::net::TcpListener::bind(cfg.bind_addr)
        .await
        .map_err(ServeExternalError::Bind)?;
    let bound_addr = listener.local_addr().map_err(ServeExternalError::Bind)?;
    info!(%bound_addr, "{log_tag}: server bound");

    // Load mTLS CA bytes if needed.
    let mtls_ca_bytes: Option<Vec<u8>> = if cfg.config.auth_mode.is_some_and(|m| m.includes_mtls())
    {
        if let Some(ref ca_path) = cfg.config.mtls_ca_path {
            Some(std::fs::read(ca_path).map_err(|e| {
                ServeExternalError::Tls(TlsLoadError::Read {
                    path: ca_path.clone(),
                    source: e,
                })
            })?)
        } else {
            None
        }
    } else {
        None
    };

    let server_config = build_server_config(cfg.cert_resolver.clone(), mtls_ca_bytes)
        .map_err(ServeExternalError::Tls)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));

    let (conn_tx, conn_rx) = tokio::sync::mpsc::channel(64);
    let active_conns = Arc::new(AtomicUsize::new(0));
    let cfg_arc = Arc::new(cfg);

    // Spawn the custom accept loop.
    tokio::spawn(run_accept_loop(
        listener,
        acceptor,
        cfg_arc.clone(),
        conn_tx,
        active_conns,
        shutdown.clone(),
    ));

    // Build the auth + audit layer stack.
    let auth_mode = cfg_arc
        .config
        .auth_mode
        .unwrap_or(oneshim_core::config::AuthMode::Jwt);
    let audit_bridge = Arc::new(AuditBridge::new(cfg_arc.audit_port.clone()));
    let auth_layer = AuthLayer {
        auth_mode,
        jwt_verifier: cfg_arc.jwt_verifier.clone(),
        mtls_verifier: cfg_arc.mtls_verifier.clone(),
        ip_ban: cfg_arc.ip_ban.clone(),
        metrics: cfg_arc.metrics.clone(),
        audit_bridge: audit_bridge.clone(),
    };
    let audit_layer = AuditLayer {
        bridge: audit_bridge,
        metrics: cfg_arc.metrics.clone(),
    };

    let stream = ReceiverStream::new(conn_rx);

    let shutdown_signal = {
        let mut rx = shutdown.clone();
        async move {
            let _ = rx.changed().await;
        }
    };

    // HTTP/2 hardening per spec §S7.
    // Note: http2_keepalive_interval / http2_keepalive_timeout are ignored by
    // tonic 0.14 when using serve_with_incoming — documented in tonic source.
    // The concurrency_limit_per_connection and timeout settings ARE applied.
    // max_concurrent_streams from config (default from ExternalGrpcConfig::default).
    //
    // Layer ordering (tonic 0.14 / tower 0.5): empirically, `Server::builder`
    // applies layers FIFO from the request perspective — the FIRST `.layer()`
    // call becomes the OUTERMOST and runs first on ingress. Verified at
    // runtime via an AuditLayer debug print: with `auth` first and `audit`
    // second, AuditLayer saw AuthContext=Some (auth had already run).
    // Ordering below gives request flow: `request_id → auth → audit → handler`.
    // RequestIdLayer is OUTERMOST per D14 revised / U5 so auth-rejected audit
    // rows correlate with client's x-request-id.
    let concurrency = cfg_arc.config.max_concurrent_streams;
    tonic::transport::Server::builder()
        .concurrency_limit_per_connection(concurrency)
        .timeout(Duration::from_secs(60))
        .layer(request_id_layer::RequestIdLayer) // OUTERMOST per D14 revised / U5
        .layer(auth_layer) // runs SECOND on request ingress
        .layer(audit_layer) // innermost — runs AFTER auth
        .add_service(DashboardServiceServer::new(service).max_decoding_message_size(1_048_576))
        .serve_with_incoming_shutdown(stream, shutdown_signal)
        .await
        .map_err(ServeExternalError::Tonic)?;

    info!("{log_tag}: server shut down cleanly");
    Ok(())
}

// ── Supervisor ────────────────────────────────────────────────────────────────

/// Spawn the external gRPC server with a panic-recovery supervisor.
///
/// If `serve_external` panics, it is caught with `AssertUnwindSafe + catch_unwind`.
/// The supervisor respawns once (with an exponential back-off starting at 1 second).
/// A second panic within 30 seconds of the first causes the supervisor to give up.
///
/// The `cfg.shutdown_tx` `Arc` is held by the spawned task for the supervisor lifetime.
/// When the supervisor exits (clean shutdown, fatal error, or double-panic give-up),
/// the last `Arc` clone is dropped, which closes the watch channel and unblocks the
/// cert watcher and expiry monitor tasks so they exit too.
///
/// The returned `JoinHandle` can be aborted to stop the supervisor entirely.
pub async fn spawn_with_supervisor(cfg: ExternalGrpcSpawnConfig) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Keep `_tx` alive for the supervisor lifetime. Dropping it signals
        // shutdown to all `shutdown_rx` listeners (cert watcher, expiry monitor,
        // accept loop, tonic server) via the closed-channel Err path.
        let _tx = cfg.shutdown_tx.clone();
        let mut backoff = Duration::from_secs(1);
        let mut last_panic_at: Option<std::time::Instant> = None;

        loop {
            let cfg_clone = cfg.clone();
            let fut = serve_external(cfg_clone);
            let result = std::panic::AssertUnwindSafe(fut).catch_unwind().await;

            match result {
                Ok(Ok(())) => {
                    // Clean shutdown — exit supervisor loop.
                    break;
                }
                Ok(Err(e)) => {
                    // Non-panic error (e.g. bind failure after restart) — give up.
                    error!(err = %e, "external_grpc: server error, not retrying");
                    break;
                }
                Err(_panic) => {
                    let now = std::time::Instant::now();
                    if let Some(prev) = last_panic_at {
                        if now.duration_since(prev) < Duration::from_secs(30) {
                            error!("external_grpc: server panicked twice within 30s — giving up");
                            break;
                        }
                    }
                    last_panic_at = Some(now);
                    warn!(
                        backoff = ?backoff,
                        "external_grpc: server panicked, respawning after backoff"
                    );
                    tokio::time::sleep(backoff).await;
                    backoff = std::cmp::min(backoff * 2, Duration::from_secs(10));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use tokio::sync::watch;

    /// `build_server_config` without mTLS succeeds (no CA bytes).
    #[test]
    fn build_server_config_without_mtls_uses_no_client_auth() {
        test_support::install_rustls_crypto_provider();
        use cert_resolver::HotReloadCertResolver;
        use rcgen::{CertificateParams, KeyPair};

        let kp = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        let cert = params.self_signed(&kp).unwrap();
        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(kp.serialize_der()).unwrap();
        let signing = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key_der).unwrap();
        let certified_key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert_der], signing));
        let resolver = Arc::new(HotReloadCertResolver::new(certified_key));

        let result = build_server_config(resolver, None);
        assert!(result.is_ok(), "no-mTLS config should succeed");
    }

    /// `serve_external` binds a port and the port becomes connectable.
    ///
    /// Uses port 0 (OS-assigned) to avoid port conflicts.
    /// The test completes by sending the shutdown signal before tonic starts
    /// serving any RPC (no client handshake needed — just bind check).
    #[tokio::test]
    async fn server_binds_configured_port() {
        use oneshim_storage::sqlite::SqliteStorage;

        // Install rustls CryptoProvider so this test is order-independent within
        // the test binary (earlier tests in the same process usually install it,
        // but relying on that is fragile — see iter-6 review).
        test_support::install_rustls_crypto_provider();

        // Use port 0 so the OS picks an available port.
        let bind_addr = std::net::SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

        let (kp, resolver) = make_test_cert_resolver();
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"))
            as Arc<dyn crate::storage_port::WebStorage>;
        let (event_tx, _) = tokio::sync::broadcast::channel(1);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let shutdown_tx = Arc::new(shutdown_tx);

        let logger = Arc::new(tokio::sync::RwLock::new(
            oneshim_automation::audit::AuditLogger::new(64, 16),
        ));
        let audit_port = Arc::new(oneshim_automation::audit::AuditLogAdapter::new(logger))
            as Arc<dyn oneshim_core::ports::audit_log::AuditLogPort>;

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

        let cfg = ExternalGrpcSpawnConfig {
            bind_addr,
            config: oneshim_core::config::ExternalGrpcConfig {
                enabled: true,
                auth_mode: Some(oneshim_core::config::AuthMode::Jwt),
                max_connections: 16,
                ..Default::default()
            },
            storage,
            system_monitor: Arc::new(DummyMonitor),
            event_tx,
            audit_port,
            cert_resolver: resolver.clone(),
            jwt_verifier: None,
            mtls_verifier: None,
            ip_ban: Arc::new(ip_ban::IpBan::new()),
            metrics: Arc::new(metrics::ExternalMetrics::new()),
            shutdown_rx,
            shutdown_tx: shutdown_tx.clone(),
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            live: Arc::new(crate::grpc::external::live_config::LiveExternalConfig::new(
                crate::grpc::external::live_config::LiveSnapshot {
                    streaming_enabled: true,
                    load_policy: Arc::new(crate::grpc::load_policy::LoadPolicy::new(
                        oneshim_core::config::LoadThresholds::default(),
                    )),
                },
            )),
        };

        // Start the server in a task. It will bind and then wait for shutdown.
        let server_task = tokio::spawn(serve_external(cfg));

        // Give the server a moment to bind.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Signal shutdown.
        let _ = shutdown_tx.send(true);

        // Server task should complete within 2 seconds.
        let result = tokio::time::timeout(Duration::from_secs(2), server_task).await;
        assert!(
            result.is_ok(),
            "server should shut down within 2s of signal"
        );

        let _ = kp; // keep alive
    }

    /// Supervisor catches an injected panic (from `PANIC_ON_FIRST_ACCEPT`),
    /// waits for backoff, then the second iteration starts cleanly.
    ///
    /// The test uses `#[ignore]` because the panic injection + backoff sleep
    /// makes it a ~2s test that is slow on CI. Run with `-- --ignored` locally.
    #[ignore]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn supervisor_respawns_on_injected_panic() {
        use oneshim_storage::sqlite::SqliteStorage;

        let bind_addr = std::net::SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let (_kp, resolver) = make_test_cert_resolver();
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"))
            as Arc<dyn crate::storage_port::WebStorage>;
        let (event_tx, _) = tokio::sync::broadcast::channel(1);
        let logger = Arc::new(tokio::sync::RwLock::new(
            oneshim_automation::audit::AuditLogger::new(64, 16),
        ));
        let audit_port = Arc::new(oneshim_automation::audit::AuditLogAdapter::new(logger))
            as Arc<dyn oneshim_core::ports::audit_log::AuditLogPort>;

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

        let (supervisor_shutdown_tx, supervisor_shutdown_rx) = watch::channel(false);
        let cfg = ExternalGrpcSpawnConfig {
            bind_addr,
            config: oneshim_core::config::ExternalGrpcConfig {
                enabled: true,
                auth_mode: Some(oneshim_core::config::AuthMode::Jwt),
                max_connections: 16,
                ..Default::default()
            },
            storage,
            system_monitor: Arc::new(DummyMonitor),
            event_tx,
            audit_port,
            cert_resolver: resolver,
            jwt_verifier: None,
            mtls_verifier: None,
            ip_ban: Arc::new(ip_ban::IpBan::new()),
            metrics: Arc::new(metrics::ExternalMetrics::new()),
            shutdown_rx: supervisor_shutdown_rx,
            shutdown_tx: Arc::new(supervisor_shutdown_tx),
            pii_sanitizer: None,
            ai_runtime_status_snapshot: None,
            live: Arc::new(crate::grpc::external::live_config::LiveExternalConfig::new(
                crate::grpc::external::live_config::LiveSnapshot {
                    streaming_enabled: true,
                    load_policy: Arc::new(crate::grpc::load_policy::LoadPolicy::new(
                        oneshim_core::config::LoadThresholds::default(),
                    )),
                },
            )),
        };

        // Arm the panic injector.
        accept_loop::PANIC_ON_FIRST_ACCEPT.store(true, std::sync::atomic::Ordering::SeqCst);

        let handle = spawn_with_supervisor(cfg).await;

        // Allow time for: first attempt to start → panic → supervisor backoff (1s) → second attempt.
        tokio::time::sleep(Duration::from_millis(2500)).await;

        // The supervisor should still be running (second iteration running cleanly).
        assert!(
            !handle.is_finished(),
            "supervisor should still be running after respawn"
        );

        handle.abort();
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn make_test_cert_resolver() -> (rcgen::KeyPair, Arc<cert_resolver::HotReloadCertResolver>) {
        use rcgen::{CertificateParams, KeyPair};
        let kp = KeyPair::generate().unwrap();
        let params = CertificateParams::new(vec!["localhost".into()]).unwrap();
        let cert = params.self_signed(&kp).unwrap();
        let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
        let key_der = rustls::pki_types::PrivateKeyDer::try_from(kp.serialize_der()).unwrap();
        let signing = rustls::crypto::aws_lc_rs::sign::any_supported_type(&key_der).unwrap();
        let certified_key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert_der], signing));
        let resolver = Arc::new(cert_resolver::HotReloadCertResolver::new(certified_key));
        (kp, resolver)
    }
}
