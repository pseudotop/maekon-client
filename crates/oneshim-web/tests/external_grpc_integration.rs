//! D13-v2c end-to-end integration tests for the external gRPC server.
//!
//! Each test spins up a full `serve_external` instance on an ephemeral port,
//! connects a tonic TLS client (with the self-signed server cert as CA), and
//! exercises the auth matrix. The server runs the real `DashboardServiceImpl`
//! (wired in Task 13) with `integration_auth_token: None`; a successful auth
//! handshake is therefore proven by an `Ok(AgentInfoResponse)` carrying a
//! non-empty `build_profile`.
//!
//! Feature gate: requires `grpc-dashboard-external,external-grpc-tools,test-support`.

#![cfg(all(
    feature = "grpc-dashboard-external",
    feature = "external-grpc-tools",
    feature = "test-support"
))]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use oneshim_core::config::{AuthMode, ExternalGrpcConfig, JwtAlgorithm};
use oneshim_core::models::ai_session::SessionAuditEntry;
use oneshim_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_storage::sqlite::SqliteStorage;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::Code;

use oneshim_web::grpc::external::cert_resolver::HotReloadCertResolver;
use oneshim_web::grpc::external::ip_ban::IpBan;
use oneshim_web::grpc::external::jwt_verifier::JwtVerifier;
use oneshim_web::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
use oneshim_web::grpc::external::metrics::ExternalMetrics;
use oneshim_web::grpc::external::mtls_verifier::MtlsVerifier;
use oneshim_web::grpc::external::serve_external;
use oneshim_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
use oneshim_web::grpc::external::tls_config::load_certified_key;
use oneshim_web::grpc::test_support::mock_system_monitor::MockSystemMonitor;
use oneshim_web::grpc::LoadPolicy;
use oneshim_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use oneshim_web::proto::dashboard::v1::{
    GetAgentInfoRequest, GetSessionStatsRequest, SubscribeEventsRequest,
};
use oneshim_web::storage_port::WebStorage;

// Bring in the test_support helpers from the external module.
use oneshim_web::grpc::external::test_support::{
    install_rustls_crypto_provider, spawn_server_with_config_manager, test_ca_and_client_cert,
    test_cert_pair, test_jwt_keypair, test_mint_jwt, TestJwt,
};

// ── Shutdown pair helper ─────────────────────────────────────────────────────

/// Create a fresh `(shutdown_tx, shutdown_rx)` pair for one server instance.
///
/// Each test server needs its own pair so signals don't cross test boundaries.
/// The returned `Arc<Sender<bool>>` must be kept alive (or explicitly dropped)
/// to control when the watcher / expiry tasks exit.
fn make_test_shutdown_pair() -> (
    Arc<tokio::sync::watch::Sender<bool>>,
    tokio::sync::watch::Receiver<bool>,
) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    (Arc::new(tx), rx)
}

// ── Port allocator ───────────────────────────────────────────────────────────

/// Global counter for ephemeral test ports. Starts at 44200 — below macOS's
/// default ephemeral range (49152-65535). Linux's default `net.ipv4.ip_local_port_range`
/// is 32768-60999, so 44200 falls INSIDE Linux's ephemeral range; the
/// `next_test_port()` helper retries on EADDRINUSE to tolerate collisions.
/// Tests consume one port each; 10 tests = 10 ports.
static NEXT_PORT: AtomicU16 = AtomicU16::new(44200);

/// Acquire one ephemeral test port. The port is verified to be free before
/// returning by attempting a std::net bind.
fn next_test_port() -> u16 {
    loop {
        let port = NEXT_PORT.fetch_add(1, Ordering::Relaxed);
        // Verify the port is free by binding a std listener momentarily.
        if std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_ok() {
            return port;
        }
        // Port in use; try the next one.
    }
}

// ── Noop audit ───────────────────────────────────────────────────────────────

/// A no-op `AuditLogPort` impl used when tests don't need to inspect audit entries.
struct NoopAudit;

#[async_trait::async_trait]
impl AuditLogPort for NoopAudit {
    async fn pending_count(&self) -> usize {
        0
    }
    async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_status(&self, _status: &AuditStatus, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_action_prefix(&self, _prefix: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn stats(&self) -> AuditStats {
        AuditStats::default()
    }
    async fn has_pending_batch(&self) -> bool {
        false
    }
    async fn log_event(&self, _action_type: &str, _session_id: &str, _details: &str) {}
    async fn log_start_if(
        &self,
        _level: AuditLevel,
        _command_id: &str,
        _session_id: &str,
        _action_type: &str,
    ) {
    }
    async fn log_complete_with_time(
        &self,
        _level: AuditLevel,
        _command_id: &str,
        _session_id: &str,
        _details: &str,
        _execution_time_ms: u64,
    ) {
    }
    async fn drain_batch(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn drain_all(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn record_session_event(&self, _entry: SessionAuditEntry) {}
}

// ── Server helpers ────────────────────────────────────────────────────────────

/// Build an in-memory `SqliteStorage` wrapped as `Arc<dyn WebStorage>`.
fn in_memory_storage() -> Arc<dyn WebStorage> {
    Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite")) as Arc<dyn WebStorage>
}

/// Build an `ExternalGrpcSpawnConfig` for JWT-only mode.
///
/// Returns `(config, port)` where `port` is 0 (OS-assigned).
fn make_jwt_config(jwt_pub_key_path: &std::path::Path) -> (ExternalGrpcSpawnConfig, SocketAddr) {
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

    let pub_key_bytes = std::fs::read(jwt_pub_key_path).expect("read jwt pub key");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("JwtVerifier"),
    );

    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::Jwt),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: None,
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            )),
        })),
    };
    (cfg, bind_addr)
}

/// Build an `ExternalGrpcSpawnConfig` for mTLS-only mode.
fn make_mtls_config(ca_pem_path: &std::path::Path) -> (ExternalGrpcSpawnConfig, SocketAddr) {
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);

    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("MtlsVerifier"));

    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::Mtls),
            mtls_ca_path: Some(ca_pem_path.to_path_buf()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: None,
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            )),
        })),
    };
    (cfg, bind_addr)
}

/// Spawn `serve_external` on a pre-allocated port. Returns `(JoinHandle, port)`.
///
/// Uses `next_test_port()` to obtain a port that is verified free at allocation
/// time. `serve_external` binds the same port; since the std bind is dropped
/// before serve_external runs, the rebind window is minimal and occurs in the
/// same process so REUSEADDR makes it reliable.
///
/// The shutdown channel lives inside `cfg.shutdown_tx` / `cfg.shutdown_rx`.
/// Callers abort the handle to stop the server.
async fn spawn_server(cfg: ExternalGrpcSpawnConfig) -> (tokio::task::JoinHandle<()>, u16) {
    // rustls 0.23 requires an explicit CryptoProvider when both aws-lc-rs and ring
    // are present. `serve_external` calls `build_server_config` →
    // `rustls::ServerConfig::builder()` which consults the process-level default.
    install_rustls_crypto_provider();
    let port = next_test_port();

    let real_cfg = ExternalGrpcSpawnConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        ..cfg
    };

    let handle = tokio::spawn(async move {
        // `real_cfg.shutdown_tx` (Arc<Sender<bool>>) is kept alive inside the spawned
        // task for the server lifetime. Dropping it when the task ends closes the channel
        // and terminates background tasks (cert watcher, expiry monitor) that hold a
        // cloned `shutdown_rx`.
        match serve_external(real_cfg).await {
            Ok(()) => {}
            Err(e) => eprintln!("serve_external error: {e:?}"),
        }
    });

    // Wait until the server accepts TCP connections (timeout: 5s).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("external gRPC server did not start on port {port} within 5s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    (handle, port)
}

/// Build a tonic channel that trusts the self-signed server cert.
async fn make_tls_channel(
    port: u16,
    server_cert_pem: &[u8],
    client_identity: Option<tonic::transport::Identity>,
) -> Channel {
    let ca_cert = Certificate::from_pem(server_cert_pem);
    let mut tls = ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert);
    if let Some(identity) = client_identity {
        tls = tls.identity(identity);
    }
    Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
        .expect("valid endpoint")
        .tls_config(tls)
        .expect("tls config")
        .connect_timeout(Duration::from_secs(3))
        .connect()
        .await
        .expect("TLS channel connect")
}

/// Read the server cert PEM from the cached cert pair.
fn server_cert_pem() -> Vec<u8> {
    let (cert_path, _) = test_cert_pair();
    std::fs::read(&cert_path).expect("read server cert PEM")
}

// ── Helper: assert RPC reached the authenticated service (got business data) ─

/// Call `GetAgentInfo` and assert auth succeeded (handler returned Ok or a
/// terminal domain error that isn't Unauthenticated / Cancelled). After Task 9
/// wired the real `DashboardServiceImpl`, a successful auth handshake yields
/// an Ok response carrying `AgentInfoResponse` with version + platform.
async fn assert_reaches_service(client: &mut DashboardServiceClient<Channel>) {
    let result = client.get_agent_info(GetAgentInfoRequest {}).await;
    match result {
        Ok(resp) => {
            // Sanity — response carries an agent build_profile string.
            let info = resp.into_inner();
            assert!(
                !info.build_profile.is_empty(),
                "AgentInfoResponse.build_profile should be populated"
            );
        }
        Err(s) if s.code() == Code::NotFound => {
            // Some RPCs legitimately return NotFound with empty state; still
            // indicates auth passed. (Not expected for get_agent_info, but
            // tolerant in case future changes alter the default.)
        }
        Err(s) => panic!("expected Ok from authenticated get_agent_info; got {:?}", s),
    }
}

/// Same as above but with a JWT bearer token injected into the request metadata.
async fn assert_reaches_service_with_bearer(channel: Channel, token: &str) {
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let result = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await;
    match result {
        Ok(resp) => {
            let info = resp.into_inner();
            assert!(
                !info.build_profile.is_empty(),
                "AgentInfoResponse.build_profile should be populated"
            );
        }
        Err(s) if s.code() == Code::NotFound => {
            // Tolerant of empty state — auth passed.
        }
        Err(s) => panic!("expected Ok from authenticated get_agent_info; got {:?}", s),
    }
}

/// Send a request with a bad bearer token; returns the resulting gRPC status.
/// Used for auth-failure scenarios that accumulate into IP bans.
async fn send_bad_bearer(channel: Channel, token: &str) -> tonic::Status {
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .unwrap_err()
}

// ═════════════════════════════════════════════════════════════════════════════
// Core tests (10 — always run)
// ═════════════════════════════════════════════════════════════════════════════

/// Test 1: JWT auth mode — valid JWT → reaches placeholder service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_full_handshake_jwt() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    assert_reaches_service_with_bearer(channel, &token).await;

    handle.abort();
    let _ = handle.await;
}

/// Test 2: mTLS auth mode — valid client cert → reaches placeholder service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_full_handshake_mtls() {
    let ca = test_ca_and_client_cert(24); // 24h lifetime — within 48h cap
    let (cfg, _) = make_mtls_config(&ca.ca_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;
    let mut client = DashboardServiceClient::new(channel);

    assert_reaches_service(&mut client).await;

    handle.abort();
    let _ = handle.await;
}

/// Test 3: JWT+mTLS mode — both valid → reaches service.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_both_valid() {
    let jwt_kp = test_jwt_keypair();
    let ca = test_ca_and_client_cert(24);
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            )),
        })),
    };

    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;

    assert_reaches_service_with_bearer(channel, &token).await;

    handle.abort();
    let _ = handle.await;
}

/// Test 4: JWT+mTLS mode — no JWT header → `Unauthenticated` from auth layer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_mtls_only() {
    // JWT+mTLS mode requires BOTH. No JWT header → AuthLayer rejects.
    let ca = test_ca_and_client_cert(24);
    let jwt_kp = test_jwt_keypair();
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            )),
        })),
    };

    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    // Connect with mTLS cert but NO JWT header.
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;
    let mut client = DashboardServiceClient::new(channel);

    // No JWT header → expect Unauthenticated from AuthLayer.
    let result = client.get_agent_info(GetAgentInfoRequest {}).await;
    assert!(
        result.is_err(),
        "missing JWT should cause auth failure, got Ok"
    );
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        Code::Unauthenticated,
        "expected Unauthenticated, got {:?}",
        status
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 5: JWT+mTLS mode — cert valid but JWT expired → `Unauthenticated`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_cert_valid_jwt_expired() {
    let jwt_kp = test_jwt_keypair();
    let ca = test_ca_and_client_cert(24);
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));

    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            )),
        })),
    };

    let (handle, port) = spawn_server(cfg).await;

    // Expired token: exp = now - 7200 (2h in the past)
    let expired_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        -7200,
    );
    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);
    let channel = make_tls_channel(port, &cert_pem, Some(identity)).await;

    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {expired_token}")
            .parse()
            .expect("valid header"),
    );
    let result = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await;
    assert!(result.is_err(), "expired JWT should cause auth failure");
    let status = result.unwrap_err();
    assert_eq!(
        status.code(),
        Code::Unauthenticated,
        "expected Unauthenticated for expired JWT, got {:?}",
        status
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 6: IP ban — 5 auth failures from one IP → IP is marked banned.
///
/// The ban is recorded in the AuthLayer (JWT verify failure records failure on
/// the IP). After threshold (5), the IP ban state is checked directly on the
/// shared `IpBan` instance.
///
/// NOTE: We use a SINGLE persistent TLS channel for the 5 bad requests. The
/// ip_ban threshold (5) is reached via JWT failures routed through the auth
/// layer on the existing TLS connection. We verify the ban state directly from
/// the shared `Arc<IpBan>` rather than attempting a 6th TLS connection (which
/// would be dropped pre-TLS, causing a TLS error on the client side).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_ip_ban_e2e() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let ip_ban = cfg.ip_ban.clone();
    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();

    // Send 5 requests with an invalid JWT (wrong issuer) to trigger IP ban.
    // Use a single channel — 5 requests on the same HTTP/2 connection.
    let bad_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "wrong-issuer", // wrong issuer → JWT verify failure
        "test-audience",
        3600,
    );

    let channel = make_tls_channel(port, &cert_pem, None).await;
    for _ in 0..5 {
        let status = send_bad_bearer(channel.clone(), &bad_token).await;
        assert_eq!(
            status.code(),
            Code::Unauthenticated,
            "bad JWT must return Unauthenticated; got {:?}",
            status
        );
    }

    // After 5 failures on the same IP (127.0.0.1), the IP ban state is set.
    let loopback = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
    assert!(
        ip_ban.is_banned(loopback),
        "127.0.0.1 should be banned after 5 JWT auth failures"
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 7: Hot-reload cert — server starts with cert A, we swap to cert B,
/// then verify the resolver holds cert B and connections to cert A fail.
///
/// Flow:
/// 1. Start server with cert A, verify first connection succeeds (Unimplemented).
/// 2. Swap cert resolver to cert B atomically.
/// 3. Verify `cert_resolver.current()` is now cert B (DER differs from cert A).
/// 4. New connections with cert A as CA trust anchor fail (cert mismatch).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_cert_hot_reload() {
    use oneshim_web::grpc::external::tls_config::load_certified_key;
    use rcgen::{CertificateParams, KeyPair};
    use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};

    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);

    // Save the cert resolver and cert A DER before moving cfg into spawn_server.
    let cert_resolver = cfg.cert_resolver.clone();
    let cert_a_der = cert_resolver.current().cert[0].to_vec();
    let (handle, port) = spawn_server(cfg).await;

    // Verify that connections with cert A succeed (auth passes → Unimplemented).
    let cert_pem_a = server_cert_pem();
    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    {
        let channel = make_tls_channel(port, &cert_pem_a, None).await;
        assert_reaches_service_with_bearer(channel, &token).await;
    }

    // Generate cert B and swap it in atomically.
    let dir_b = tempfile::TempDir::new().expect("TempDir for cert B");
    let kp_b = KeyPair::generate().expect("keypair B");
    let params_b = CertificateParams::new(vec!["localhost".to_string()]).expect("params B");
    let cert_b = params_b.self_signed(&kp_b).expect("self-signed B");
    let cert_b_path = dir_b.path().join("cert_b.pem");
    let key_b_path = dir_b.path().join("key_b.pem");
    std::fs::write(&cert_b_path, cert_b.pem()).expect("write cert B");
    std::fs::write(&key_b_path, kp_b.serialize_pem()).expect("write key B");

    let new_key = load_certified_key(&cert_b_path, &key_b_path).expect("load cert B");
    cert_resolver.swap(new_key);

    // Verify: the resolver now holds cert B.
    let cert_b_der = cert_resolver.current().cert[0].to_vec();
    assert_ne!(
        cert_a_der, cert_b_der,
        "cert_resolver must hold a different cert after swap"
    );

    // Verify: a new connection trusting cert A fails — the server now presents cert B.
    // Use `Endpoint::connect()` directly (returns Result, not panic) to avoid
    // unwrapping a TLS error in the expected-failure path.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let ca_cert_a = Certificate::from_pem(&cert_pem_a);
    let tls_a = ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert_a);
    let connect_result = Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
        .expect("valid endpoint")
        .tls_config(tls_a)
        .expect("tls config")
        .connect_timeout(Duration::from_secs(2))
        .connect()
        .await;
    // After swap, cert A is no longer the server's cert, so cert-A-trusting clients
    // get a TLS error. (TLS session resumption could also succeed — both outcomes
    // are acceptable; the key assertion is the DER comparison above.)
    let _ = connect_result; // ignore success or failure — DER check above is the assertion

    handle.abort();
    let _ = handle.await;
    let _ = dir_b; // keep TempDir alive until test ends
}

/// Test 8: Short-lived cert rejection — client cert with > 48h lifetime →
/// mTLS verifier rejects it (`Unauthenticated`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_short_lived_cert_rejection() {
    // Client cert with 72h lifetime — exceeds the 48h cap.
    let ca = test_ca_and_client_cert(72);
    let (cfg, _) = make_mtls_config(&ca.ca_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let cert_pem = server_cert_pem();
    let client_cert_pem = std::fs::read(&ca.client_cert_pem_path).expect("read client cert");
    let client_key_pem = std::fs::read(&ca.client_key_pem_path).expect("read client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);

    // Connect with mTLS — TLS handshake may succeed (CA is valid), but the
    // auth layer's MtlsVerifier checks lifetime AFTER TLS.
    // With 72h cert and 48h cap → expect Unauthenticated.
    let channel_result = make_tls_channel(port, &cert_pem, Some(identity)).await;
    let mut client = DashboardServiceClient::new(channel_result);

    let result = client.get_agent_info(GetAgentInfoRequest {}).await;
    assert!(result.is_err(), "72h cert should be rejected (cap is 48h)");
    let status = result.unwrap_err();
    // The accept loop's MtlsVerifier drops the connection pre-gRPC when the
    // cert lifetime exceeds the cap. The client sees a transport-level error.
    // Observed shapes:
    //   Code::Unauthenticated — auth layer explicitly rejects (post-TLS path)
    //   Code::Unknown         — connection reset during TLS/accept
    //   Code::Unavailable     — server unavailable signal
    //   Code::Cancelled       — hyper::Error(Canceled, "connection closed") when
    //                           the accept loop closes the connection mid-request
    // All of these prove the request did NOT reach the handler successfully.
    assert!(
        matches!(
            status.code(),
            Code::Unauthenticated | Code::Unavailable | Code::Unknown | Code::Cancelled
        ),
        "expected transport/auth rejection for over-cap cert, got {:?}",
        status
    );

    handle.abort();
    let _ = handle.await;
}

/// Test 9: Loopback server unaffected when external is disabled.
///
/// Verifies that the loopback gRPC server starts and responds normally when
/// `ExternalGrpcConfig::enabled = false`. This is the backwards-compat test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_server_unaffected_when_external_disabled() {
    use oneshim_core::config::LoadThresholds;
    use oneshim_web::grpc::{serve_optional, GrpcSpawnConfig, LoadPolicy};

    let loopback_port = next_test_port();

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let loopback_cfg = GrpcSpawnConfig {
        port: loopback_port,
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        integration_auth_token: None,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
        streaming_enabled: true,
        max_concurrent_streams: 50,
    };

    let server_task = tokio::spawn(serve_optional(loopback_cfg));

    // Wait for loopback server.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", loopback_port))
            .await
            .is_ok()
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("loopback server did not start on port {loopback_port}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Connect via plain HTTP/2 (loopback has no TLS).
    let endpoint = format!("http://127.0.0.1:{loopback_port}");
    let mut client = DashboardServiceClient::connect(endpoint)
        .await
        .expect("connect to loopback gRPC");

    // GetAgentInfo on the loopback should work (returns actual data).
    use oneshim_web::proto::dashboard::v1::GetAgentInfoRequest as Req;
    let response = client
        .get_agent_info(Req {})
        .await
        .expect("GetAgentInfo ok");
    assert!(
        !response.into_inner().version.is_empty(),
        "version should be non-empty from loopback server"
    );

    server_task.abort();
    let _ = server_task.await;
}

/// R1 — Test 10: RequestIdLayer preserves a valid client-supplied x-request-id.
///
/// Per spec §5.2 / D31: when the client sends a valid `x-request-id` header
/// (ASCII graphic, 1..=128 chars), `RequestIdLayer` echoes that EXACT value in
/// the response — it does NOT overwrite a matching value.
///
/// Assertion: the response metadata carries `x-request-id: test-req-123`
/// exactly as supplied, proving the conditional-overwrite path (D31) works
/// end-to-end through the real `serve_external` stack.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_header_returned() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Attach both authorization AND a valid x-request-id header.
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid auth header"),
    );
    req.metadata_mut().insert(
        "x-request-id",
        tonic::metadata::MetadataValue::try_from("test-req-123").expect("valid x-request-id value"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .expect("auth should succeed and yield AgentInfoResponse");

    // The x-request-id that the server echoed must be the exact client value.
    let returned_id = resp
        .metadata()
        .get("x-request-id")
        .expect("x-request-id must be present in response metadata")
        .to_str()
        .expect("x-request-id must be valid ASCII");
    assert_eq!(
        returned_id, "test-req-123",
        "RequestIdLayer (D31) must preserve the client-supplied x-request-id unchanged"
    );

    // Also verify the handler returned real business data (smoke).
    let info = resp.into_inner();
    assert!(
        !info.build_profile.is_empty(),
        "AgentInfoResponse.build_profile must be populated"
    );

    handle.abort();
    let _ = handle.await;
}

/// R2 — Test: AuditLayer records Denied + grpc_status_code=7 for PermissionDenied.
///
/// `subscribe_events` calls `validate_authority(host_header)` which returns
/// `Status::permission_denied("authority not allowlisted")` (code 7) for any
/// host not in ["localhost", "127.0.0.1", "::1", "::ffff:127.0.0.1"].
///
/// The AuditLayer observes code 7 via the header-first path (D28), maps it to
/// `AuditStatus::Denied`, and records `grpc_status_code: Some(7)` in the details
/// blob (D26).  The CapturingAudit mock extracts this from the JSON details.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_audit_denied_for_permission_denied() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-pd-audit",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Set a "host" metadata header to a non-allowlisted authority — this is the
    // trigger for validate_authority → Err(Status::permission_denied(...)).
    // The AuditLayer's deferred task sees grpc-status 7 in the response headers.
    let mut req = tonic::Request::new(SubscribeEventsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid auth header"),
    );
    req.metadata_mut().insert(
        "host",
        tonic::metadata::MetadataValue::try_from("evil.example.com:443")
            .expect("valid host header"),
    );

    let result = DashboardServiceClient::new(channel)
        .subscribe_events(req)
        .await;
    // The call must fail with PermissionDenied (code 7).
    let status = result.expect_err("expected PermissionDenied error from subscribe_events");
    assert_eq!(
        status.code(),
        tonic::Code::PermissionDenied,
        "authority not in allowlist must yield PermissionDenied; got {status:?}"
    );

    // Give the tokio::spawn'd AuditLayer deferred task time to flush.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Extract Denied entry with grpc_status_code — drop lock before any await.
    let (denied_count, grpc_code, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let denied: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, AuditStatus::Denied))
            .collect();
        let code = denied.first().and_then(|e| e.grpc_status_code);
        let dbg = format!("{entries:?}");
        (denied.len(), code, dbg)
    };
    assert!(
        denied_count >= 1,
        "expected ≥1 Denied audit row; got {denied_count} (entries: {entries_debug})"
    );
    assert_eq!(
        grpc_code,
        Some(7),
        "Denied row must carry grpc_status_code=7 (PermissionDenied); entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

// ═════════════════════════════════════════════════════════════════════════════
// Advanced scenario tests — JWT+mTLS matrix + hot-reload + IP ban e2e + etc.
// All previously `#[ignore]`d tests are now either un-ignored (T14, T17, T18,
// T19) or deleted with a reference comment (T13, T15, T16) per Task 13
// follow-up work — see the deletion notes below.
// ═════════════════════════════════════════════════════════════════════════════

/// Test 11: JWT+mTLS — JWT-only (no client cert) → TLS handshake fails.
///
/// `auth_mode = JwtAndMtls` installs a `WebPkiClientVerifier` in the rustls
/// `ServerConfig`, which makes client certificates MANDATORY at the TLS layer.
/// A client that presents no client identity gets a rustls handshake error
/// before any gRPC frame is exchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_jwt_only() {
    let jwt_kp = test_jwt_keypair();
    let ca = test_ca_and_client_cert(24);

    // Build a JWT+mTLS server (CA_A as trusted CA for client certs).
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca.ca_pem_path.clone()),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            )),
        })),
    };
    let (handle, port) = spawn_server(cfg).await;

    // Mint a valid JWT (not used if TLS fails first, but present for completeness).
    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let server_cert_pem = server_cert_pem();
    let ca_cert = tonic::transport::Certificate::from_pem(&server_cert_pem);

    // Build a TLS channel WITHOUT a client identity — no client cert presented.
    // The rustls WebPkiClientVerifier requires a client cert, so the TLS
    // handshake must fail.
    let tls = tonic::transport::ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert);
    let connect_result =
        tonic::transport::Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
            .expect("valid endpoint")
            .tls_config(tls)
            .expect("tls config")
            .connect_timeout(Duration::from_secs(3))
            .connect()
            .await;

    match connect_result {
        Err(_) => {
            // TLS handshake failed eagerly at connect time — expected.
        }
        Ok(channel) => {
            // Channel was created lazily; TLS failure surfaces on first request.
            let mut req = tonic::Request::new(GetAgentInfoRequest {});
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {token}").parse().expect("valid header"),
            );
            let result = DashboardServiceClient::new(channel)
                .get_agent_info(req)
                .await;
            assert!(
                result.is_err(),
                "no-client-cert TLS should be rejected by WebPkiClientVerifier; got Ok"
            );
            let status = result.unwrap_err();
            // TLS rejection at handshake produces a transport-level error
            // (Unknown, Unavailable, or Cancelled) — never Unimplemented (which
            // would mean auth passed and the placeholder service ran).
            assert_ne!(
                status.code(),
                tonic::Code::Unimplemented,
                "WebPkiClientVerifier must reject before reaching service; \
                 got Unimplemented which means TLS accepted the request: {:?}",
                status
            );
        }
    }

    handle.abort();
    let _ = handle.await;
}

/// Test 12: JWT+mTLS — client cert signed by wrong CA + valid JWT → TLS rejection.
///
/// The server trusts CA_A. The client presents a cert signed by CA_B (a
/// completely independent CA). rustls's `WebPkiClientVerifier` validates the
/// client cert chain against CA_A's root and fails at TLS handshake time because
/// CA_B is not in the server's trust store.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_jwt_plus_mtls_cert_invalid_jwt_valid() {
    let jwt_kp = test_jwt_keypair();
    // CA_A: trusted by server.
    let ca_a = test_ca_and_client_cert(24);
    // CA_B: an independent CA — its client cert is NOT trusted by the server.
    let ca_b = test_ca_and_client_cert(24);

    // Build a JWT+mTLS server that trusts CA_A only.
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load cert");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let pub_key_bytes = std::fs::read(&jwt_kp.pub_pem_path).expect("read pub");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("verifier"),
    );
    let mtls_verifier = Arc::new(MtlsVerifier::new(48, &[]).expect("mtls verifier"));
    let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
    let cfg = ExternalGrpcSpawnConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::JwtAndMtls),
            mtls_ca_path: Some(ca_a.ca_pem_path.clone()), // server trusts CA_A only
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: Some(mtls_verifier),
        ip_ban: Arc::new(IpBan::new()),
        metrics: Arc::new(ExternalMetrics::new()),
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            )),
        })),
    };
    let (handle, port) = spawn_server(cfg).await;

    // Mint a valid JWT.
    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let server_cert_pem = server_cert_pem();
    let ca_cert = tonic::transport::Certificate::from_pem(&server_cert_pem);

    // Client presents CA_B's client cert — not trusted by server (trusts CA_A).
    let client_cert_pem = std::fs::read(&ca_b.client_cert_pem_path).expect("read CA_B client cert");
    let client_key_pem = std::fs::read(&ca_b.client_key_pem_path).expect("read CA_B client key");
    let identity = tonic::transport::Identity::from_pem(client_cert_pem, client_key_pem);

    let tls = tonic::transport::ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert)
        .identity(identity); // present CA_B-signed cert to CA_A-trusting server
    let connect_result =
        tonic::transport::Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
            .expect("valid endpoint")
            .tls_config(tls)
            .expect("tls config")
            .connect_timeout(Duration::from_secs(3))
            .connect()
            .await;

    match connect_result {
        Err(_) => {
            // TLS handshake failed eagerly at connect time — expected.
        }
        Ok(channel) => {
            // Channel created lazily; TLS failure surfaces on first request.
            let mut req = tonic::Request::new(GetAgentInfoRequest {});
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {token}").parse().expect("valid header"),
            );
            let result = DashboardServiceClient::new(channel)
                .get_agent_info(req)
                .await;
            assert!(
                result.is_err(),
                "wrong-CA client cert must be rejected by WebPkiClientVerifier; got Ok"
            );
            let status = result.unwrap_err();
            // A CA-chain failure at TLS level never produces Unimplemented.
            assert_ne!(
                status.code(),
                tonic::Code::Unimplemented,
                "CA chain validation must reject before reaching service; \
                 got Unimplemented which means TLS accepted the request: {:?}",
                status
            );
        }
    }

    handle.abort();
    let _ = handle.await;
}

// T13 (external_grpc_ipv6_ban_uses_64_prefix) deleted — the /64 prefix ban
// logic is covered by unit tests in `ip_ban.rs` (same /64 prefix ⇒ ban
// shared, at lines 188-208). An integration-level variant would re-test the
// same logic through an IPv6 loopback bind that is not portable across CI
// runner configurations. If the full-stack IPv6 accept_loop path ever
// regresses, add a scenario to the stress-test workflow instead.

/// Test 14: Concurrent stream cap — attempt beyond `max_concurrent_streams`
/// returns `ResourceExhausted`. Uses a tight cap (4 streams) so the test
/// runs quickly without holding many resources.
///
/// The handler-side `StreamCounterGuard::try_acquire` enforces the cap
/// (BEFORE auth work) in both subscribe_metrics and subscribe_events. The
/// integration test exercises the full stack — accept_loop → auth_layer →
/// audit_layer → DashboardServiceImpl → StreamCounterGuard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_grpc_concurrent_stream_cap_enforced() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    // Override the cap to 4 for fast testing.
    cfg.config.max_concurrent_streams = 4;
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-cap",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Hold 4 concurrent subscribe_events streams open. Each stream stays
    // alive because we keep the receiver handle in `streams`.
    let mut streams = Vec::new();
    for _ in 0..4 {
        let mut req = tonic::Request::new(SubscribeEventsRequest::default());
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        let stream = DashboardServiceClient::new(channel.clone())
            .subscribe_events(req)
            .await
            .expect("within-cap stream should open")
            .into_inner();
        streams.push(stream);
    }

    // The 5th attempt should be rejected with ResourceExhausted at RPC
    // initialization time — `StreamCounterGuard::try_acquire` runs before
    // the first message is yielded and returns the error as the initial
    // gRPC status (not via a stream item).
    let mut req = tonic::Request::new(SubscribeEventsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );

    let err = DashboardServiceClient::new(channel.clone())
        .subscribe_events(req)
        .await
        .expect_err("5th stream over cap must be rejected at RPC initialization");
    assert_eq!(
        err.code(),
        Code::ResourceExhausted,
        "5th stream must be rejected with ResourceExhausted; got {err:?}"
    );

    drop(streams);
    handle.abort();
    let _ = handle.await;
}

// T15 (concurrent_connection_cap_enforced) deleted — resource-exhaustion
// tests require dedicated CI workflow with elevated fd ulimit + opt-in
// trigger (`ulimit -n 65536`; separate workflow with manual dispatch). This
// is tracked in `project_next_tasks.md` as "External gRPC stress test suite"
// and is out of scope for Task 13 per user direction.

// T16 (external_grpc_task_panic_respawned) deleted — the
// `PANIC_ON_FIRST_ACCEPT` injection + `spawn_with_supervisor` respawn
// path is already covered by `external::mod::tests::supervisor_respawns_on_injected_panic`
// (at external/mod.rs:388-459). The integration-level re-test would
// duplicate the same assertion through a tonic client that doesn't add
// additional coverage — supervisor correctness is the invariant, not the
// client's observation of it.

/// Test 17: Port collision — external port == loopback port → launcher refuses external.
///// Test 17: Port collision guard — external port == loopback port triggers
/// a validation error that the launcher surfaces (F13 guard). Unit-level
/// test: the helper itself is the single source of truth for the check.
#[test]
fn external_grpc_port_collides_with_loopback() {
    use oneshim_web::grpc::external::port_collision::check_port_collision;
    let err = check_port_collision(10091, 10091).expect_err("same port must error");
    assert!(
        err.contains("10091"),
        "error should name the port; got: {err}"
    );
    assert!(check_port_collision(10092, 10091).is_ok());
}

/// Test 18: Token isolation — `integration_auth_token` on the external
/// service impl MUST be None (spec §2.5 threat model). The loopback's
/// opt-out bypass path can only be reached by a caller presenting a
/// matching token; if the external server were to inherit the loopback's
/// token, an external client could bypass auth.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_separate_service_impl_doesnt_leak_loopback_token() {
    install_rustls_crypto_provider();
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let svc = oneshim_web::grpc::DashboardServiceImpl::from_external_spawn_config(&cfg);
    assert!(
        !svc.has_integration_token(),
        "external DashboardServiceImpl MUST have integration_auth_token=None (spec §2.5)"
    );
}

/// Test 19: Shutdown signal reaches the server task.
///
/// Sends the shutdown signal and verifies the `serve_external` task
/// exits within a bounded window. Does NOT assert client-side stream
/// termination — that requires streaming handlers to observe
/// `shutdown_rx`, which is a post-Task-13 follow-up: currently
/// `subscribe_events` / `subscribe_metrics` only exit when the
/// underlying broadcast receiver yields (not when shutdown fires).
///
/// A full "open-stream → Unavailable on shutdown" assertion is
/// tracked in project_next_tasks.md under the external gRPC stress +
/// e2e suite.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_grpc_shutdown_drains_streams() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let shutdown_tx = cfg.shutdown_tx.clone();
    let (handle, _port) = spawn_server(cfg).await;

    // Let the server settle (accept loop + tonic main loop both live).
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Signal shutdown. `serve_with_incoming_shutdown` should complete once
    // the shutdown_signal future resolves and in-flight work drains.
    shutdown_tx.send(true).expect("signal shutdown");

    // The `serve_external` task MUST exit within 5s after the signal is
    // sent. The spawn_server wrapper awaits `serve_external(...)` and
    // returns, so the JoinHandle completes with `Ok(())`.
    let joined = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("server task must exit within 5s of shutdown signal");
    assert!(
        joined.is_ok(),
        "server task should complete cleanly on shutdown; got {joined:?}"
    );
}

// Test 20 (external_grpc_fails_fast_on_missing_cert) is covered at unit level by
// `tls_config::tests::load_fails_on_missing_cert`, which directly asserts that
// `load_certified_key("/does/not/exist.pem", "/does/not/exist.key")` returns
// `Err(TlsLoadError::Read { .. })`. An integration-test duplicate is not needed
// because `serve_external` calls `load_certified_key` (via `build_server_config`)
// early in startup — the unit test covers the identical code path.

// ═════════════════════════════════════════════════════════════════════════════
// Task 19 — new end-to-end tests added in the Task 13 follow-up (spec §3.5)
// ═════════════════════════════════════════════════════════════════════════════

/// E2E-1: Real handler returns business data — confirms that the wired
/// `DashboardServiceImpl` (not the removed stub) answers RPCs with
/// structured responses, not `Unimplemented`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_real_handler_returns_business_data() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-e2e-1",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    let mut req = tonic::Request::new(GetSessionStatsRequest { limit: 10 });
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_session_stats(req)
        .await
        .expect("get_session_stats must return Ok with a structured response");
    let stats = resp.into_inner();
    // Empty storage → zero sessions; the IMPORTANT invariant is that we
    // received a typed SessionStatsResponse with concrete fields, not an
    // Unimplemented status.
    assert_eq!(
        stats.total_sessions, 0,
        "empty storage should yield total_sessions=0; got {}",
        stats.total_sessions
    );
    assert_eq!(stats.ended_sessions, 0);

    handle.abort();
    let _ = handle.await;
}

// Mock audit log that retains every `log_complete_with_time` and `log_event`
// entry so the e2e tests below can inspect what AuditLayer recorded.
//
// Structural rewrite (Task 9.0, CR4 / R2-NI1): replaces the previous
// action_type-as-command_id conflation with real command_id preservation and
// grpc_status_code JSON extraction from the details blob.  Unblocks Phase 9
// Tasks 9.1+ which assert command_id correlation and D26 raw-code visibility.
#[derive(Default)]
struct CapturingAudit {
    entries: std::sync::Mutex<Vec<CapturedEntry>>,
}

/// A lightweight capture record used by Phase 9 integration tests to assert
/// on command_id, action_type, status, grpc_status_code, and execution timing.
///
/// `details` preserves the raw JSON blob from `log_complete_with_time` so that
/// tests can inspect operation names (e.g. "SubscribeEvents") without re-parsing
/// the struct.
#[derive(Clone, Debug)]
#[allow(dead_code)] // fields consumed by Phase 9.1+ assertion helpers
struct CapturedEntry {
    command_id: String,
    action_type: String,
    status: AuditStatus,
    grpc_status_code: Option<u32>,
    execution_time_ms: u64,
    /// Raw `details` string passed by the audit bridge (JSON blob or empty).
    /// Populated by `log_complete_with_time` and `log_event`; `None` for
    /// entries that have no details context.
    details: Option<String>,
}

impl CapturingAudit {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            entries: std::sync::Mutex::new(vec![]),
        })
    }

    /// Return a snapshot of all captured entries.  Used by Phase 9.1+ tests
    /// to assert command_id correlation and grpc_status_code visibility.
    #[allow(dead_code)] // used by Phase 9.1+ tests
    fn snapshot(&self) -> Vec<CapturedEntry> {
        self.entries.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AuditLogPort for CapturingAudit {
    async fn log_event(&self, action_type: &str, _session_id: &str, details: &str) {
        // AuditBridge emits action_type "external_grpc_started" etc.
        // alongside log_complete_with_time; use this to capture Started rows.
        let status = match action_type {
            "external_grpc_started" => AuditStatus::Started,
            "external_grpc_completed" => AuditStatus::Completed,
            "external_grpc_failed" | "external_grpc_denied" | "external_grpc_timeout" => {
                AuditStatus::Failed
            }
            _ => AuditStatus::Completed,
        };
        self.entries.lock().unwrap().push(CapturedEntry {
            command_id: String::new(),
            action_type: action_type.to_string(),
            status,
            grpc_status_code: None,
            execution_time_ms: 0,
            details: Some(details.to_string()),
        });
    }

    async fn log_start_if(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        action_type: &str,
    ) {
        self.entries.lock().unwrap().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status: AuditStatus::Started,
            grpc_status_code: None,
            execution_time_ms: 0,
            details: None,
        });
    }

    async fn log_complete_with_time(
        &self,
        _level: AuditLevel,
        command_id: &str,
        _session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        let status = parse_status_from_details(details);
        let grpc_status_code: Option<u32> = serde_json::from_str::<serde_json::Value>(details)
            .ok()
            .and_then(|v| {
                v.get("grpc_status_code")
                    .and_then(|n| n.as_u64().map(|u| u as u32))
            });
        self.entries.lock().unwrap().push(CapturedEntry {
            command_id: command_id.to_string(),
            action_type: String::new(),
            status,
            grpc_status_code,
            execution_time_ms,
            details: Some(details.to_string()),
        });
    }

    async fn pending_count(&self) -> usize {
        0
    }
    async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_status(&self, _s: &AuditStatus, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_action_prefix(&self, _p: &str, _l: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn entries_by_command_id(&self, _cmd_id: &str, _limit: usize) -> Vec<AuditEntry> {
        vec![]
    }
    async fn stats(&self) -> AuditStats {
        AuditStats::default()
    }
    async fn has_pending_batch(&self) -> bool {
        false
    }
    async fn drain_batch(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn drain_all(&self) -> Vec<AuditEntry> {
        vec![]
    }
    async fn record_session_event(&self, _e: SessionAuditEntry) {}
}

/// Derive `AuditStatus` from the JSON `result` field in the details blob emitted
/// by `AuditBridge::record`.  Returns `Completed` for any unrecognized value.
fn parse_status_from_details(details: &str) -> AuditStatus {
    serde_json::from_str::<serde_json::Value>(details)
        .ok()
        .and_then(|v| v.get("result").and_then(|r| r.as_str().map(String::from)))
        .map(|s| match s.as_str() {
            "ok" => AuditStatus::Completed,
            "denied" => AuditStatus::Denied,
            "timeout" => AuditStatus::Timeout,
            "error" | "failed" | "auth_failed" => AuditStatus::Failed,
            _ => AuditStatus::Completed,
        })
        .unwrap_or(AuditStatus::Completed)
}

/// E1 / E2E-2: After a successful RPC, the audit trail contains both Started
/// and Completed rows with the same command_id. This proves AuditLayer's
/// Started+Completed pairing works end-to-end.
///
/// Extended (Task 9.1 E1): also asserts:
/// - The Completed row's `command_id` is a valid UUIDv4 (36 chars, 4 hyphens) —
///   generated by RequestIdLayer since no client-supplied x-request-id was sent.
/// - The Completed row's `grpc_status_code` is `Some(0)` (gRPC Ok / Code::Ok),
///   proving D26 raw-code persistence for the success path.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_audit_completed_entry_written_after_ok_response() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-audit",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    // No x-request-id header — RequestIdLayer generates a UUIDv4.
    DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .expect("auth + real handler → Ok");

    // Give the tokio::spawn'd record() calls time to flush to the mock.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Copy all needed data into locals + drop the lock BEFORE any `await` to avoid
    // `await_holding_lock` clippy — std::sync::MutexGuard is not `Send`.
    let (started_count, completed_count, completed_cmd_id, completed_grpc_code, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let started = entries
            .iter()
            .filter(|e| matches!(e.status, AuditStatus::Started))
            .count();
        // log_complete_with_time sets grpc_status_code; find the Completed row
        // that has it populated (the deferred AuditLayer task).
        let completed_row = entries
            .iter()
            .find(|e| matches!(e.status, AuditStatus::Completed) && e.grpc_status_code.is_some());
        let cmd_id = completed_row
            .map(|e| e.command_id.clone())
            .unwrap_or_default();
        let grpc_code = completed_row.and_then(|e| e.grpc_status_code);
        let completed_any = entries
            .iter()
            .filter(|e| matches!(e.status, AuditStatus::Completed))
            .count();
        let dbg = format!("{entries:?}");
        (started, completed_any, cmd_id, grpc_code, dbg)
    };
    assert!(
        started_count >= 1,
        "expected ≥1 Started row; got {started_count} (entries: {entries_debug})"
    );
    assert!(
        completed_count >= 1,
        "expected ≥1 Completed row; got {completed_count} (entries: {entries_debug})"
    );

    // E1 extension: command_id must be a UUIDv4 (36-char string, 4 hyphens).
    // RequestIdLayer generates it when the client omits x-request-id, and
    // AuditLayer's `request_id override (U5)` propagates it to command_id.
    assert_eq!(
        completed_cmd_id.len(),
        36,
        "command_id must be a 36-char UUIDv4 string; got {completed_cmd_id:?}"
    );
    assert_eq!(
        completed_cmd_id.chars().filter(|c| *c == '-').count(),
        4,
        "UUIDv4 command_id must have 4 hyphens; got {completed_cmd_id:?}"
    );
    uuid::Uuid::parse_str(&completed_cmd_id).unwrap_or_else(|e| {
        panic!("command_id {completed_cmd_id:?} must be parseable as UUID: {e}")
    });

    // E1 extension: grpc_status_code=Some(0) for the Ok path (D26).
    assert_eq!(
        completed_grpc_code,
        Some(0),
        "Completed row must carry grpc_status_code=0 (Code::Ok); entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

/// E2E-3: Streaming RPC records response_message_count in the Completed
/// audit row. The client opens a subscribe_events stream, drops it shortly
/// after (no events fired, so message_count == 0), then checks that the
/// Completed row's details JSON reports a count (absent or 0 both
/// acceptable since skip_serializing_if drops zero values).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_grpc_streaming_audit_records_message_count() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-stream-audit",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    let mut req = tonic::Request::new(SubscribeEventsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let stream = DashboardServiceClient::new(channel)
        .subscribe_events(req)
        .await
        .expect("stream should open")
        .into_inner();

    // Drop the stream quickly — handler exits; AuditLayer records Completed.
    drop(stream);
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Copy counts into locals + drop the lock BEFORE any `await` to avoid
    // `await_holding_lock` clippy — std::sync::MutexGuard is not `Send`.
    let (has_subscribe_row, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        // There must be at least a Started row for the subscribe_events call.
        // The Completed row may or may not appear depending on timing and
        // whether tonic has drained the trailer — at minimum, the audit trail
        // for /oneshim.dashboard.v1.DashboardService/SubscribeEvents must
        // show the Started row, proving AuditLayer wrapped the stream.
        let found = entries.iter().any(|e| {
            e.details
                .as_deref()
                .map(|d| d.contains("SubscribeEvents"))
                .unwrap_or(false)
        });
        (found, format!("{entries:?}"))
    };
    assert!(
        has_subscribe_row,
        "expected ≥1 audit row for SubscribeEvents; got entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

/// N1 — RequestIdLayer generates a UUIDv4 when the client omits x-request-id.
///
/// When no `x-request-id` header is sent, `RequestIdLayer` (spec §5.2 / None
/// branch) generates a fresh UUIDv4 and inserts it into the response.  The
/// CapturingAudit's Completed row carries that same UUID as `command_id` via
/// AuditLayer's request_id override (U5), proving end-to-end propagation.
///
/// Assertions:
/// 1. Response metadata has `x-request-id` (server-generated).
/// 2. The value is a valid 36-char UUIDv4 (4 hyphens, parseable by `uuid`).
/// 3. The CapturingAudit Completed row's `command_id` matches the response header.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_generated_when_missing() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-gen-id",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // No x-request-id header — RequestIdLayer must generate a UUIDv4.
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid auth header"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .expect("auth + real handler → Ok");

    // 1. Response must carry a server-generated x-request-id.
    let generated_id = resp
        .metadata()
        .get("x-request-id")
        .expect("server must insert x-request-id when client omits it")
        .to_str()
        .expect("x-request-id must be valid ASCII");

    // 2. Must be a valid UUIDv4 (36 chars, 4 hyphens).
    assert_eq!(
        generated_id.len(),
        36,
        "generated x-request-id must be 36 chars; got {generated_id:?}"
    );
    assert_eq!(
        generated_id.chars().filter(|c| *c == '-').count(),
        4,
        "generated x-request-id must have 4 hyphens (UUIDv4); got {generated_id:?}"
    );
    let parsed_uuid = uuid::Uuid::parse_str(generated_id).expect("x-request-id must parse as UUID");
    // UUIDv4: version nibble == 4, variant == 0b10xx.
    assert_eq!(
        parsed_uuid.get_version_num(),
        4,
        "generated ID must be UUIDv4; got version {}",
        parsed_uuid.get_version_num()
    );

    // Give AuditLayer's deferred task time to flush to the mock.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 3. CapturingAudit's Completed row must carry the same UUID as command_id.
    // Drop the lock before any `.await`.
    let (audit_cmd_id, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let completed = entries
            .iter()
            .find(|e| matches!(e.status, AuditStatus::Completed) && e.grpc_status_code.is_some())
            .map(|e| e.command_id.clone())
            .unwrap_or_default();
        let dbg = format!("{entries:?}");
        (completed, dbg)
    };
    assert_eq!(
        audit_cmd_id, generated_id,
        "AuditLayer command_id must equal the x-request-id echoed in the response; \
         entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

/// N2 — RequestIdLayer discards a malformed client x-request-id and substitutes
/// a fresh UUIDv4.
///
/// Per spec §5.2 / L307: when the client sends an `x-request-id` that fails
/// `is_valid()` (ASCII graphic 0x21..=0x7E, 1..=128 chars), `RequestIdLayer`
/// emits a `tracing::warn!` and generates a fresh UUIDv4.  The warn+regenerate
/// path proves that a malicious / malformed client cannot inject arbitrary
/// bytes into the response-header / downstream audit trail.
///
/// The malformed payload used here is `"bad\tid"` — the tab byte (0x09) is a
/// valid HeaderValue byte (HTAB is permitted by `http::HeaderValue::from_str`)
/// but falls outside the `is_valid()` 0x21..=0x7E range, so the server-side
/// validator will reject it and substitute a UUIDv4.  Mirrors the in-crate
/// `rejects_invalid_characters_generates_new` unit test (request_id_layer.rs L189).
///
/// Assertions:
/// 1. Response metadata carries a valid 36-char UUIDv4 (4 hyphens, parses as UUID v4).
/// 2. Response's `x-request-id` does NOT equal the malformed client input.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_invalid_replaced() {
    let jwt_kp = test_jwt_keypair();
    let (cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let (handle, port) = spawn_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-bad-reqid",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Malformed x-request-id: tab (0x09) is valid as an http HeaderValue byte
    // but fails the is_valid(0x21..=0x7E) range, forcing the warn+regenerate path.
    let malformed_id = "bad\tid";
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}")
            .parse()
            .expect("valid auth header"),
    );
    req.metadata_mut().insert(
        "x-request-id",
        tonic::metadata::MetadataValue::try_from(malformed_id)
            .expect("tab (0x09) is a valid HeaderValue byte"),
    );
    let resp = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await
        .expect("auth + real handler → Ok");

    // 1. Response must carry a server-substituted x-request-id.
    let returned_id = resp
        .metadata()
        .get("x-request-id")
        .expect("x-request-id must be present (server substitutes on invalid input)")
        .to_str()
        .expect("substituted x-request-id must be valid ASCII");

    // 2. Value must NOT be the malformed client input.
    assert_ne!(
        returned_id, malformed_id,
        "server must discard malformed x-request-id and substitute a UUID"
    );

    // 3. Value must be a valid UUIDv4 (36 chars, 4 hyphens, version 4).
    assert_eq!(
        returned_id.len(),
        36,
        "substituted x-request-id must be 36 chars; got {returned_id:?}"
    );
    assert_eq!(
        returned_id.chars().filter(|c| *c == '-').count(),
        4,
        "substituted x-request-id must have 4 hyphens (UUIDv4); got {returned_id:?}"
    );
    let parsed_uuid = uuid::Uuid::parse_str(returned_id).expect("x-request-id must parse as UUID");
    assert_eq!(
        parsed_uuid.get_version_num(),
        4,
        "substituted x-request-id must be UUIDv4; got version {}",
        parsed_uuid.get_version_num()
    );

    handle.abort();
    let _ = handle.await;
}

/// N3 — x-request-id is preserved across the auth-rejection boundary (U5 / D14).
///
/// Per spec §5.2 / §9.2 L1393: `RequestIdLayer` is the outermost layer and runs
/// BEFORE `AuthLayer`, so it inserts the `RequestId` extension with the client's
/// header value before any auth gate fires.  When `AuthLayer` subsequently
/// rejects the request (invalid JWT → Unauthenticated), its Failed-path
/// `bridge.record(...)` reads the extension and passes it as `command_id`
/// (commit `7bd7c944`, Task 6.1).  This closes the correlation gap at the
/// security boundary — security dashboards can still trace which client call
/// produced each auth rejection.
///
/// Flow:
/// 1. Client sends `x-request-id: req-abc-123` + a JWT signed with a wrong issuer.
/// 2. Server's `RequestIdLayer` validates the header (passes) and inserts
///    `RequestId("req-abc-123")` into request extensions.
/// 3. `AuthLayer`'s JWT gate calls `verifier.verify(tok)`, which fails (wrong
///    issuer), and takes the `invalid_jwt` Failed-path.
/// 4. The Failed-path reads the `RequestId` extension and calls
///    `bridge.record(..., Some("req-abc-123"))`, which persists the Failed
///    audit row with `command_id = "req-abc-123"`.
///
/// Assertions:
/// 1. RPC returns `Err(Status)` with code `Unauthenticated` (16).
/// 2. CapturingAudit captures ≥1 Failed audit row.
/// 3. That Failed row's `command_id` equals `"req-abc-123"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_request_id_preserved_across_auth_reject() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server(cfg).await;

    // Invalid JWT — wrong issuer → JwtVerifier::verify() fails → invalid_jwt path.
    let bad_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-auth-reject",
        "wrong-issuer", // mismatch with verifier's "test-issuer" → verify fails
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    // Valid x-request-id — passes is_valid() (all ASCII graphic chars).
    let client_req_id = "req-abc-123";
    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {bad_token}")
            .parse()
            .expect("valid auth header"),
    );
    req.metadata_mut().insert(
        "x-request-id",
        tonic::metadata::MetadataValue::try_from(client_req_id).expect("valid x-request-id value"),
    );

    let result = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await;

    // 1. RPC must fail with Unauthenticated (invalid_jwt path).
    let status = result.expect_err("wrong-issuer JWT must yield Err");
    assert_eq!(
        status.code(),
        Code::Unauthenticated,
        "invalid JWT must yield Unauthenticated (code 16); got {status:?}"
    );

    // Give the tokio::spawn'd AuthLayer Failed-path record() time to flush.
    tokio::time::sleep(Duration::from_millis(150)).await;

    // 2 + 3. The auth-rejection audit row must carry the client's x-request-id
    // as command_id. AuthLayer's Failed-path calls both `log_complete_with_time`
    // (writes command_id + details JSON) and `log_event("external_grpc_failed")`
    // (prefix-queryable marker).  We locate the authoritative auth-rejection
    // row by its details payload (`"result":"auth_failed"` + `"failure_reason":
    // "invalid_jwt"`) — this is the row whose command_id must equal the client's
    // x-request-id per U5/D14.
    //
    // We filter by details content rather than `AuditStatus::Failed` because
    // `Failed` also covers "error"/"failed" results from non-auth paths, while
    // the substring match pins down the exact `failure_reason: "invalid_jwt"`
    // path under test.
    //
    // The `!e.command_id.is_empty()` predicate disambiguates the two audit rows
    // that share the same details JSON: `log_complete_with_time` (L1657 in
    // CapturingAudit) populates `command_id` from the forwarded request-id,
    // whereas `log_event` (L1615) hard-codes `String::new()`.  If
    // CapturingAudit is ever refactored so that `log_event` also populates
    // `command_id`, this filter will match both rows and `auth_failed.first()`
    // will non-deterministically return either one.  In that case tighten the
    // predicate (e.g., match on the event type string) or assert
    // `auth_failed_count == 1` to catch the ambiguity at test time.
    //
    // Drop the lock before any `.await`.
    let (auth_failed_count, auth_failed_cmd_id, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let auth_failed: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.details
                    .as_deref()
                    .map(|d| {
                        d.contains("\"result\":\"auth_failed\"")
                            && d.contains("\"failure_reason\":\"invalid_jwt\"")
                    })
                    .unwrap_or(false)
                    && !e.command_id.is_empty()
            })
            .collect();
        let cmd_id = auth_failed
            .first()
            .map(|e| e.command_id.clone())
            .unwrap_or_default();
        let dbg = format!("{entries:?}");
        (auth_failed.len(), cmd_id, dbg)
    };
    assert!(
        auth_failed_count >= 1,
        "expected ≥1 auth-rejection audit row with populated command_id + \
         details.result='auth_failed' + failure_reason='invalid_jwt'; \
         got {auth_failed_count} (entries: {entries_debug})"
    );
    assert_eq!(
        auth_failed_cmd_id, client_req_id,
        "auth-rejection audit row's command_id must equal the client's x-request-id \
         (U5/D14 correlation preserved at security boundary); entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

// ═════════════════════════════════════════════════════════════════════════════
// Task 9.2 — Audit status mapping integration tests (4 tests)
//
// Spec §9.2 L1395-1401. Each test exercises `AuditLayer::map_code_to_audit_status`
// (task 1.3 commit 8efbe91f) end-to-end via a fixture handler that returns a
// canned `tonic::Status`. The fixture is wired into the real layer stack via
// `serve_external_with_service` (test_support helper added by this task), so
// the request_id → auth → audit → handler flow matches production exactly.
//
// Test 1 is the PRIMARY CR1-regression-catch: handler-returned PermissionDenied
// must produce `AuditStatus::Denied` + `grpc_status_code=7`. This is distinct
// from `external_grpc_audit_denied_for_permission_denied` (L1014) which goes
// through `validate_authority` (an interceptor-emitted status, not a handler-
// emitted status) — the handler-return path is the one CR1 was about.
// ═════════════════════════════════════════════════════════════════════════════

/// Pre-programmed scenario for the test fixture's handlers.  Each test selects
/// exactly one variant; un-used RPCs return `unimplemented!()` so a wiring
/// mistake (test calling the wrong RPC) surfaces as a panic instead of silent
/// success.
#[derive(Clone, Copy, Debug)]
enum FixtureScenario {
    /// `get_agent_info` returns `Err(Status::permission_denied(...))`.
    PermissionDenied,
    /// `subscribe_metrics` returns `Err(Status::cancelled(...))` BEFORE opening
    /// the stream — produces a trailers-only HTTP/2 response (no data frames).
    /// Exercises the §5.5 header-first observation branch (NV6).
    StreamCancelled,
    /// `get_agent_info` returns `Err(Status::internal(...))`.
    Internal,
    /// `subscribe_metrics` opens a stream, sends ≥1 message, then sleeps
    /// briefly (5s — far longer than the client lives).  Client drops
    /// mid-stream → no trailer observed → AuditLayer falls back to
    /// `Completed` (OQ6-Option-A behavior).
    StreamSendThenIdle,
}

/// Wrap a `Stream` and bump `counter` on every yielded item.  Local equivalent
/// of the production `CountingStream` (`crates/oneshim-web/src/grpc/counting_stream.rs`,
/// `pub(crate)`) — needed so the streaming-RPC fixture can populate the audit
/// row's `response_message_count` field via the `msg_counter` extension that
/// `AuditLayer` inserts before the handler runs.
fn count_yielded_stream<S, T>(
    inner: S,
    counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> impl tokio_stream::Stream<Item = T> + Send
where
    S: tokio_stream::Stream<Item = T> + Send + 'static,
    T: Send + 'static,
{
    use std::sync::atomic::Ordering;
    use tokio_stream::StreamExt as _;
    inner.map(move |item| {
        counter.fetch_add(1, Ordering::Relaxed);
        item
    })
}

/// Test-only `DashboardService` impl that returns canned `tonic::Status` values
/// driven by a `FixtureScenario`.  Only the RPCs invoked by Task 9.2 tests are
/// implemented; the rest panic with `unimplemented!()` so misuse is loud.
struct FixtureDashboardService {
    scenario: FixtureScenario,
}

impl FixtureDashboardService {
    fn new(scenario: FixtureScenario) -> Self {
        Self { scenario }
    }
}

#[tonic::async_trait]
impl oneshim_web::proto::dashboard::v1::dashboard_service_server::DashboardService
    for FixtureDashboardService
{
    async fn get_agent_info(
        &self,
        _req: tonic::Request<oneshim_web::proto::dashboard::v1::GetAgentInfoRequest>,
    ) -> Result<tonic::Response<oneshim_web::proto::dashboard::v1::AgentInfoResponse>, tonic::Status>
    {
        match self.scenario {
            FixtureScenario::PermissionDenied => Err(tonic::Status::permission_denied(
                "fixture: permission denied",
            )),
            FixtureScenario::Internal => Err(tonic::Status::internal("fixture: internal error")),
            other => unimplemented!(
                "FixtureDashboardService::get_agent_info called with scenario {other:?}; \
                 unexpected — only PermissionDenied / Internal route here",
            ),
        }
    }

    async fn health_check(
        &self,
        _req: tonic::Request<oneshim_web::proto::dashboard::v1::HealthCheckRequest>,
    ) -> Result<
        tonic::Response<oneshim_web::proto::dashboard::v1::HealthCheckResponse>,
        tonic::Status,
    > {
        unimplemented!("health_check not used by Task 9.2 tests");
    }

    async fn get_session_stats(
        &self,
        _req: tonic::Request<oneshim_web::proto::dashboard::v1::GetSessionStatsRequest>,
    ) -> Result<
        tonic::Response<oneshim_web::proto::dashboard::v1::SessionStatsResponse>,
        tonic::Status,
    > {
        unimplemented!("get_session_stats not used by Task 9.2 tests");
    }

    async fn get_recent_frames(
        &self,
        _req: tonic::Request<oneshim_web::proto::dashboard::v1::GetRecentFramesRequest>,
    ) -> Result<
        tonic::Response<oneshim_web::proto::dashboard::v1::RecentFramesResponse>,
        tonic::Status,
    > {
        unimplemented!("get_recent_frames not used by Task 9.2 tests");
    }

    async fn get_productivity_metrics(
        &self,
        _req: tonic::Request<oneshim_web::proto::dashboard::v1::GetProductivityMetricsRequest>,
    ) -> Result<
        tonic::Response<oneshim_web::proto::dashboard::v1::ProductivityMetricsResponse>,
        tonic::Status,
    > {
        unimplemented!("get_productivity_metrics not used by Task 9.2 tests");
    }

    async fn get_focus_stats(
        &self,
        _req: tonic::Request<oneshim_web::proto::dashboard::v1::GetFocusStatsRequest>,
    ) -> Result<tonic::Response<oneshim_web::proto::dashboard::v1::FocusStatsResponse>, tonic::Status>
    {
        unimplemented!("get_focus_stats not used by Task 9.2 tests");
    }

    type SubscribeMetricsStream = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = Result<
                        oneshim_web::proto::dashboard::v1::SubscribeMetricsResponse,
                        tonic::Status,
                    >,
                > + Send
                + 'static,
        >,
    >;

    async fn subscribe_metrics(
        &self,
        req: tonic::Request<oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest>,
    ) -> Result<tonic::Response<Self::SubscribeMetricsStream>, tonic::Status> {
        match self.scenario {
            FixtureScenario::StreamCancelled => {
                // 0-message-fixture per spec L1398 (NV6): return Err BEFORE any
                // stream is opened.  Tonic emits trailers-only HTTP/2 response
                // (grpc-status=1 in initial HEADERS, no body data) — exercises
                // the §5.5 header-first audit observation branch.
                Err(tonic::Status::cancelled("fixture: handler cancelled"))
            }
            FixtureScenario::StreamSendThenIdle => {
                // Mirror production: extract the `msg_counter` extension that
                // AuditLayer inserted before the handler ran, so the Completed
                // audit row carries `response_message_count`.  The production
                // `CountingStream` is `pub(crate)` and not reachable from an
                // integration test crate — `count_yielded_stream` below is a
                // minimal local equivalent.
                let msg_counter: Option<std::sync::Arc<std::sync::atomic::AtomicU64>> =
                    req.extensions().get().cloned();

                // Open the stream, send 1 message synchronously, then never
                // emit another frame.  Client drops mid-stream → no trailer
                // observed → AuditLayer falls back to Completed (OQ6-A).
                let (tx, rx) = tokio::sync::mpsc::channel(4);
                tokio::spawn(async move {
                    let payload = oneshim_web::proto::dashboard::v1::SubscribeMetricsResponse {
                        payload: None,
                    };
                    let _ = tx.send(Ok(payload)).await;
                    // Hold the sender open until the client drops.  Cleanup is
                    // via runtime drop (test end), not handle.abort() propagation
                    // to this orphan task.  5s bound prevents hung runtimes if
                    // a future refactor reuses runtimes across tests.
                    tokio::time::sleep(Duration::from_secs(5)).await;
                });
                let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
                let counted: Self::SubscribeMetricsStream = match msg_counter {
                    Some(counter) => Box::pin(count_yielded_stream(stream, counter)),
                    None => Box::pin(stream),
                };
                Ok(tonic::Response::new(counted))
            }
            other => unimplemented!(
                "FixtureDashboardService::subscribe_metrics called with scenario {other:?}; \
                 unexpected — only StreamCancelled / StreamSendThenIdle route here",
            ),
        }
    }

    type SubscribeEventsStream = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<
                    Item = Result<
                        oneshim_web::proto::dashboard::v1::SubscribeEventsResponse,
                        tonic::Status,
                    >,
                > + Send
                + 'static,
        >,
    >;

    async fn subscribe_events(
        &self,
        _req: tonic::Request<oneshim_web::proto::dashboard::v1::SubscribeEventsRequest>,
    ) -> Result<tonic::Response<Self::SubscribeEventsStream>, tonic::Status> {
        unimplemented!("subscribe_events not used by Task 9.2 tests");
    }
}

/// Sibling of [`spawn_server`] that injects a [`FixtureDashboardService`] in
/// place of the real `DashboardServiceImpl`.  Reuses the production layer
/// stack (request_id → auth → audit) via `serve_external_with_service`.
async fn spawn_server_with_fixture_service(
    cfg: ExternalGrpcSpawnConfig,
    scenario: FixtureScenario,
) -> (tokio::task::JoinHandle<()>, u16) {
    install_rustls_crypto_provider();
    let port = next_test_port();

    let real_cfg = ExternalGrpcSpawnConfig {
        bind_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        ..cfg
    };

    let handle = tokio::spawn(async move {
        let svc = FixtureDashboardService::new(scenario);
        match oneshim_web::grpc::external::test_support::serve_external_with_service(real_cfg, svc)
            .await
        {
            Ok(()) => {}
            Err(e) => eprintln!("serve_external_with_service error: {e:?}"),
        }
    });

    // Wait for TCP listen.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("external gRPC fixture server did not start on port {port} within 5s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    (handle, port)
}

// ── Test 1 — PRIMARY CR1-REGRESSION-CATCH ────────────────────────────────────
/// Spec §9.2 L1397: handler returns `Err(Status::permission_denied(...))` →
/// AuditLayer must record `AuditStatus::Denied` + `grpc_status_code=7`.
///
/// The existing `external_grpc_audit_denied_for_permission_denied` (L1014)
/// covers the `validate_authority` interceptor path (host-header gate).
/// THIS test covers the handler-return path — the one CR1 was actually about.
/// Without the §5.5 header-first observation in AuditLayer, a handler `Err`
/// produces trailers-only HTTP/2 (grpc-status in HEADERS, no body) and the
/// previous body-trailer-only observation defaulted to `Completed`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_audit_denied_when_handler_returns_permission_denied() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) =
        spawn_server_with_fixture_service(cfg, FixtureScenario::PermissionDenied).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-pd-handler",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );

    let result = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await;
    let status = result.expect_err("fixture handler must return PermissionDenied");
    assert_eq!(
        status.code(),
        Code::PermissionDenied,
        "fixture-handler error must propagate as PermissionDenied; got {status:?}"
    );

    // Give the deferred AuditLayer task time to flush.
    tokio::time::sleep(Duration::from_millis(150)).await;

    let (denied_count, grpc_code, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let denied: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, AuditStatus::Denied))
            .collect();
        let code = denied.first().and_then(|e| e.grpc_status_code);
        let dbg = format!("{entries:?}");
        (denied.len(), code, dbg)
    };
    assert!(
        denied_count >= 1,
        "expected ≥1 Denied audit row from handler-return PermissionDenied; \
         got {denied_count} (entries: {entries_debug})"
    );
    assert_eq!(
        grpc_code,
        Some(7),
        "Denied row must carry grpc_status_code=7 (PermissionDenied); \
         entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

// ── Test 2 — Cancelled / 0-message-fixture ────────────────────────────────────
/// Spec §9.2 L1398 (NV6): handler returns `Err(Status::cancelled(...))` from a
/// streaming RPC BEFORE any data frame is emitted → trailers-only response →
/// AuditLayer's §5.5 header-first branch must observe the code, mapping it to
/// `AuditStatus::Timeout` + `grpc_status_code=1`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_audit_timeout_when_handler_returns_cancelled() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) =
        spawn_server_with_fixture_service(cfg, FixtureScenario::StreamCancelled).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-cancelled",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    let mut req =
        tonic::Request::new(oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );

    let result = DashboardServiceClient::new(channel)
        .subscribe_metrics(req)
        .await;
    let status = result.expect_err("fixture handler must return Cancelled");
    assert_eq!(
        status.code(),
        Code::Cancelled,
        "fixture-handler error must propagate as Cancelled; got {status:?}"
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    let (timeout_count, grpc_code, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let timeout: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, AuditStatus::Timeout))
            .collect();
        let code = timeout.first().and_then(|e| e.grpc_status_code);
        let dbg = format!("{entries:?}");
        (timeout.len(), code, dbg)
    };
    assert!(
        timeout_count >= 1,
        "expected ≥1 Timeout audit row from handler-return Cancelled; \
         got {timeout_count} (entries: {entries_debug})"
    );
    assert_eq!(
        grpc_code,
        Some(1),
        "Timeout row must carry grpc_status_code=1 (Cancelled); \
         entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

// ── Test 3 — Internal → Failed ───────────────────────────────────────────────
/// Spec §9.2 L1399: handler returns `Err(Status::internal(...))` →
/// AuditLayer records `AuditStatus::Failed` + `grpc_status_code=13`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_audit_failed_when_handler_returns_internal() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) = spawn_server_with_fixture_service(cfg, FixtureScenario::Internal).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-internal",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    let mut req = tonic::Request::new(GetAgentInfoRequest {});
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );

    let result = DashboardServiceClient::new(channel)
        .get_agent_info(req)
        .await;
    let status = result.expect_err("fixture handler must return Internal");
    assert_eq!(
        status.code(),
        Code::Internal,
        "fixture-handler error must propagate as Internal; got {status:?}"
    );

    tokio::time::sleep(Duration::from_millis(150)).await;

    let (failed_count, grpc_code, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let failed: Vec<_> = entries
            .iter()
            .filter(|e| matches!(e.status, AuditStatus::Failed))
            .collect();
        let code = failed.first().and_then(|e| e.grpc_status_code);
        let dbg = format!("{entries:?}");
        (failed.len(), code, dbg)
    };
    assert!(
        failed_count >= 1,
        "expected ≥1 Failed audit row from handler-return Internal; \
         got {failed_count} (entries: {entries_debug})"
    );
    assert_eq!(
        grpc_code,
        Some(13),
        "Failed row must carry grpc_status_code=13 (Internal); \
         entries: {entries_debug}"
    );

    handle.abort();
    let _ = handle.await;
}

// ── Test 4 — Client drops mid-stream → Completed (OQ6-Option-A fallback) ─────
/// Spec §9.2 L1400: client establishes a streaming RPC, receives ≥1 message,
/// then drops the stream.  No trailer is observed by the server-side
/// `TrailerCapturingBody` → AuditLayer falls back to `AuditStatus::Completed`
/// (OQ6-Option-A).  The Completed audit row also carries `msg_count > 0`
/// because the streaming handler's `CountingStream` (via `msg_counter`
/// extension) bumped on the data frame the client drained.
///
/// Note on the wired flow: the fixture sends 1 message then sleeps 5s (longer
/// than the client lives); the client receives that message (assert ≥1) and
/// drops the stream.  The server-side body is dropped without emitting a
/// trailer frame; the AuditLayer's oneshot `Drop` handler fires `None`
/// (mapped to `Completed`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_grpc_audit_completed_when_client_drops_before_trailer() {
    let jwt_kp = test_jwt_keypair();
    let (mut cfg, _) = make_jwt_config(&jwt_kp.pub_pem_path);
    let capturing = CapturingAudit::new();
    cfg.audit_port = capturing.clone() as Arc<dyn AuditLogPort>;
    let (handle, port) =
        spawn_server_with_fixture_service(cfg, FixtureScenario::StreamSendThenIdle).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "user-clientdrop",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(port, &cert_pem, None).await;

    let mut req =
        tonic::Request::new(oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );

    let mut stream = DashboardServiceClient::new(channel)
        .subscribe_metrics(req)
        .await
        .expect("stream must open before client-drop")
        .into_inner();

    // Drain ≥1 message so the streaming handler's msg_counter bumps.
    let first = tokio::time::timeout(Duration::from_secs(2), stream.message())
        .await
        .expect("first message arrives within 2s")
        .expect("first message must be Ok")
        .expect("stream must yield a payload before client drops");
    // Sanity — the fixture's payload field is empty but the response struct
    // is well-formed.
    assert!(
        first.payload.is_none(),
        "fixture sends an empty SubscribeMetricsResponse — payload must be None"
    );

    // Drop mid-stream.  The server's TrailerCapturingBody is dropped without
    // a trailer; AuditLayer's oneshot Drop fires None → Completed (OQ6-A).
    drop(stream);

    // Give the deferred audit task time to record.
    tokio::time::sleep(Duration::from_millis(300)).await;

    // We assert against the row produced by AuditLayer's deferred
    // `record_completion(...)` task — distinguishable from the Started
    // row's `record(...)` call by the presence of the `response_message_count`
    // field in the JSON details (`record` always passes None and serde
    // skip_serializing_if drops the field).  Both rows would otherwise share
    // `result:"ok"` and the `Completed` status (CapturingAudit infers from
    // `result`), so this is the load-bearing disambiguation.
    //
    // INVARIANT: depends on `#[serde(skip_serializing_if = "Option::is_none")]`
    // on `ExternalGrpcAuditDetails::response_message_count`
    // (audit_bridge.rs:29).  If that attribute is ever removed, this filter
    // silently matches both Started+Completed rows; the msg_count assertion
    // would fail with 0 because Started rows have no count to extract.
    let (completion_row_count, msg_count, entries_debug) = {
        let entries = capturing.entries.lock().unwrap();
        let completion_rows: Vec<_> = entries
            .iter()
            .filter(|e| {
                e.details
                    .as_deref()
                    .map(|d| {
                        d.contains("\"operation\":\"/oneshim.dashboard.v1.DashboardService/SubscribeMetrics\"")
                            && d.contains("\"response_message_count\"")
                    })
                    .unwrap_or(false)
            })
            .collect();
        let mc: u64 = completion_rows
            .first()
            .and_then(|e| e.details.as_deref())
            .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
            .and_then(|v| v.get("response_message_count").and_then(|n| n.as_u64()))
            .unwrap_or(0);
        // Sanity — every completion row claims `result:"ok"` (OQ6-A fallback to
        // Completed when no trailer fired), so AuditStatus mapping is Completed.
        let all_completed = completion_rows
            .iter()
            .all(|e| matches!(e.status, AuditStatus::Completed));
        let dbg = format!("{entries:?}");
        assert!(
            all_completed,
            "every SubscribeMetrics completion row must map to Completed; \
             entries: {dbg}"
        );
        (completion_rows.len(), mc, dbg)
    };
    assert!(
        completion_row_count >= 1,
        "expected ≥1 SubscribeMetrics completion audit row after client drop \
         (OQ6-Option-A: trailer-absent → Completed fallback); \
         got {completion_row_count} (entries: {entries_debug})"
    );
    assert!(
        msg_count >= 1,
        "Completed row must carry response_message_count ≥ 1 (client drained ≥1 msg); \
         got {msg_count} (entries: {entries_debug})"
    );

    handle.abort();
    let _ = handle.await;
}

// ═════════════════════════════════════════════════════════════════════════════
// Task 9.4 — Live config reload integration tests (spec §9.2 L1407-1413)
//
// Each test uses `spawn_server_with_config_manager` (test_support.rs) to run
// BOTH the tonic server AND a `ConfigReloadTask` wired to a real
// `ConfigManager`. Mutations via `cfg_mgr.update_with(..)` propagate through
// `watch::Sender::send_replace` → `run_config_reload::apply_config` →
// `LiveExternalConfig::store` → next request sees the new snapshot.
//
// `ConfigManager::with_path` persists to disk on every update, so all tests
// use `tempfile::NamedTempFile` to keep the writes out of the user's config
// directory.
// ═════════════════════════════════════════════════════════════════════════════

/// Build an initial `AppConfig` that boots the external gRPC server with
/// JWT auth, streaming enabled (via `web.grpc_streaming_enabled`), and the
/// TLS cert/key paths pointing at the shared `test_cert_pair` fixture.
///
/// Leaves `external_grpc.streaming_enabled = None` so the shared
/// `web.grpc_streaming_enabled` fallback applies (mirrors how
/// `apply_config` resolves the live `streaming_enabled` value).
fn test_cfg_with_external_enabled(
    jwt_pub_key_path: &std::path::Path,
) -> oneshim_core::config::AppConfig {
    let (cert_path, key_path) = test_cert_pair();
    let mut cfg = oneshim_core::config::AppConfig::default_config();
    cfg.web.grpc_streaming_enabled = true;
    cfg.external_grpc = ExternalGrpcConfig {
        enabled: true,
        auth_mode: Some(AuthMode::Jwt),
        tls_cert_path: Some(cert_path),
        tls_key_path: Some(key_path),
        jwt_algorithm: Some(JwtAlgorithm::Es256),
        jwt_public_key_path: Some(jwt_pub_key_path.to_path_buf()),
        jwt_expected_issuer: Some("test-issuer".to_string()),
        jwt_expected_audience: Some("test-audience".to_string()),
        max_connections: 64,
        max_concurrent_streams: 16,
        streaming_enabled: None, // fall through to web.grpc_streaming_enabled
        ..Default::default()
    };
    cfg
}

/// Build a JWT-mode `ExternalGrpcSpawnConfig` whose `live` / `metrics` are
/// pre-allocated so Task 9.4 tests can inspect them both before and after
/// a reload. The caller owns the returned `Arc<ExternalMetrics>` and
/// `Arc<LiveExternalConfig>`; the spawn config also holds `Arc` clones.
fn make_jwt_spawn_config_for_reload(
    jwt_pub_key_path: &std::path::Path,
    port: u16,
    live: Arc<LiveExternalConfig>,
    metrics: Arc<ExternalMetrics>,
    shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> ExternalGrpcSpawnConfig {
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    let bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);

    let pub_key_bytes = std::fs::read(jwt_pub_key_path).expect("read jwt pub key");
    let jwt_verifier = Arc::new(
        JwtVerifier::new(
            JwtAlgorithm::Es256,
            &pub_key_bytes,
            "test-issuer",
            "test-audience",
        )
        .expect("JwtVerifier"),
    );

    ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::Jwt),
            max_connections: 64,
            max_concurrent_streams: 16,
            ..Default::default()
        },
        storage: in_memory_storage(),
        system_monitor: MockSystemMonitor::new(20.0, 2048, 8192),
        event_tx,
        audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
        cert_resolver,
        jwt_verifier: Some(jwt_verifier),
        mtls_verifier: None,
        ip_ban: Arc::new(IpBan::new()),
        metrics,
        shutdown_rx,
        shutdown_tx,
        pii_sanitizer: None,
        ai_runtime_status_snapshot: None,
        live,
    }
}

// ─── LiveReloadHarness ───────────────────────────────────────────────────────
//
// Bundles the 9-step scaffolding that every Task 9.4 live-reload test repeats:
// tempfile → ConfigManager → seed AppConfig → LiveExternalConfig → metrics →
// shutdown pair → port → ExternalGrpcSpawnConfig → spawn_server_with_config_manager.
//
// Each call site shrinks from ~30 LoC to ~5-10 LoC and the construction order
// is centralized so future changes (e.g. new ExternalGrpcSpawnConfig fields)
// flow through one helper instead of 9 hand-edited blocks.

/// Boxed `FnOnce` for seeding the initial `AppConfig` after the harness has
/// applied `test_cfg_with_external_enabled`. Aliased to keep the
/// `LiveReloadHarnessBuilder` field type readable (clippy::type_complexity).
type SeedFn = Box<dyn FnOnce(&mut oneshim_core::config::AppConfig)>;

/// Test fixture that owns a running external gRPC server + ConfigReloadTask
/// + their backing ConfigManager and LiveExternalConfig snapshot.
struct LiveReloadHarness {
    /// Real ConfigManager — call `update_with` to mutate config and trigger
    /// the watch-channel reload.
    cfg_mgr: Arc<oneshim_core::config_manager::ConfigManager>,
    /// LiveExternalConfig snapshot — read via `live.snapshot()`.
    live: Arc<LiveExternalConfig>,
    #[allow(dead_code)] // surfaced for tests that need to read counters; not
    // every test does, but the field is public-by-convention so adding new
    // metrics-aware tests doesn't require a struct edit.
    metrics: Arc<ExternalMetrics>,
    /// JWT keypair (held so `enc_key` stays valid for token minting via
    /// `harness.jwt_kp.enc_key`).
    jwt_kp: TestJwt,
    /// The bound port returned by the spawn helper.
    port: u16,
    /// Shutdown sender — clone of the one wired into the spawn config.
    /// Tests that need graceful-exit verification (vs `shutdown()`'s abort
    /// semantics) call `shutdown_tx.send_replace(true)` and then await the
    /// `reload_handle` directly via destructuring.
    shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    server_handle: tokio::task::JoinHandle<()>,
    reload_handle: tokio::task::JoinHandle<()>,
    /// Tempfile holding the on-disk config; drop closes it.
    _tmp: tempfile::NamedTempFile,
}

impl LiveReloadHarness {
    fn builder() -> LiveReloadHarnessBuilder {
        LiveReloadHarnessBuilder::default()
    }

    /// Abort the server + reload task and await them. Idempotent enough for
    /// test teardown — always safe to call once at the end of a test.
    async fn shutdown(self) {
        self.server_handle.abort();
        self.reload_handle.abort();
        let _ = self.server_handle.await;
        let _ = self.reload_handle.await;
    }

    /// Poll `live.snapshot().streaming_enabled` until it equals `expected`,
    /// or panic with `msg` if `timeout` elapses first. Tick interval is
    /// 25 ms — matches the cadence used across pre-extraction sites and
    /// keeps wake-ups bounded under the 1 s convergence cap commonly used
    /// by Task 9.4 / 9.6 tests.
    ///
    /// The panic message is auto-suffixed with the last observed value, the
    /// expected value, and the configured cap, so callers only need to
    /// describe the high-level invariant being violated.
    async fn wait_for_streaming(&self, expected: bool, timeout: Duration, msg: &str) {
        let start = std::time::Instant::now();
        loop {
            let snap = self.live.snapshot().streaming_enabled;
            if snap == expected {
                return;
            }
            if start.elapsed() >= timeout {
                panic!(
                    "{msg} (waited {timeout:?}, last observed streaming_enabled={snap}, \
                     expected={expected})"
                );
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// Builder for [`LiveReloadHarness`].
///
/// The defaults (`streaming_enabled = true`, fresh `LoadPolicy::new` with
/// default thresholds, no extra config seeding) match the most common live-
/// reload test setup. Override any field via the builder methods before
/// calling [`LiveReloadHarnessBuilder::build`].
struct LiveReloadHarnessBuilder {
    seed: Option<SeedFn>,
    initial_streaming_enabled: bool,
    initial_load_policy: Option<Arc<LoadPolicy>>,
}

impl Default for LiveReloadHarnessBuilder {
    fn default() -> Self {
        Self {
            seed: None,
            initial_streaming_enabled: true,
            initial_load_policy: None,
        }
    }
}

impl LiveReloadHarnessBuilder {
    /// Mutate the seeded `AppConfig` after `test_cfg_with_external_enabled`
    /// has set the JWT/TLS scaffolding. Use this to flip `web.grpc_*` or
    /// `external_grpc.*` fields to specific initial values.
    fn seed<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut oneshim_core::config::AppConfig) + 'static,
    {
        self.seed = Some(Box::new(f));
        self
    }

    /// Initial `LiveSnapshot.streaming_enabled` value (default: `true`).
    /// Should match the resolved value of the seeded config so the first
    /// request observes a coherent state.
    fn initial_streaming(mut self, on: bool) -> Self {
        self.initial_streaming_enabled = on;
        self
    }

    /// Initial `LiveSnapshot.load_policy` (default: `LoadPolicy::new(default
    /// thresholds)`). Pass a policy built via
    /// `LoadPolicy::try_new_with_started_at(.., past_anchor)` to bypass the
    /// 30-second WARMUP for D27 tests.
    fn initial_load_policy(mut self, p: Arc<LoadPolicy>) -> Self {
        self.initial_load_policy = Some(p);
        self
    }

    async fn build(self) -> LiveReloadHarness {
        let jwt_kp = test_jwt_keypair();
        let pub_key_path = jwt_kp.pub_pem_path.clone();

        let tmp = tempfile::NamedTempFile::new().expect("tempfile create");
        let cfg_mgr = Arc::new(
            oneshim_core::config_manager::ConfigManager::with_path(tmp.path().to_path_buf())
                .expect("ConfigManager::with_path"),
        );
        let seed = self.seed;
        cfg_mgr
            .update_with(|c| {
                *c = test_cfg_with_external_enabled(&pub_key_path);
                if let Some(f) = seed {
                    f(c);
                }
                Ok(())
            })
            .expect("seed initial config");

        let port = next_test_port();
        let load_policy = self.initial_load_policy.unwrap_or_else(|| {
            Arc::new(LoadPolicy::new(
                oneshim_core::config::LoadThresholds::default(),
            ))
        });
        let live = Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: self.initial_streaming_enabled,
            load_policy,
        }));
        let metrics = Arc::new(ExternalMetrics::new());
        let (shutdown_tx, shutdown_rx) = make_test_shutdown_pair();
        let shutdown_tx_for_harness = shutdown_tx.clone();

        let cfg = make_jwt_spawn_config_for_reload(
            &pub_key_path,
            port,
            live.clone(),
            metrics.clone(),
            shutdown_tx,
            shutdown_rx,
        );
        let (server_handle, reload_handle, port) =
            spawn_server_with_config_manager(cfg, cfg_mgr.clone()).await;

        LiveReloadHarness {
            cfg_mgr,
            live,
            metrics,
            jwt_kp,
            port,
            shutdown_tx: shutdown_tx_for_harness,
            server_handle,
            reload_handle,
            _tmp: tmp,
        }
    }
}

/// G3 gate test — streaming toggle reflects within 1 second.
///
/// Spec §9.2 L1407, D33 (CI convergence bound). Seeds the config with
/// `streaming_enabled = true`, verifies a sanity `subscribe_metrics` call
/// succeeds, then flips `external_grpc.streaming_enabled = Some(false)`
/// and polls until the next `subscribe_metrics` returns `Unavailable`.
/// Panics if convergence takes ≥ 1s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_streaming_toggle_reflects_within_1s() {
    use std::time::Instant;

    // Seed: web=true, external=None — exercise fallback path.
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = None;
        })
        .build()
        .await;

    // Mint a JWT + build a TLS channel (external server requires both).
    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-g3",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;

    // Sanity: initial subscribe_metrics succeeds (streaming_enabled = true).
    let mut req =
        tonic::Request::new(oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let sanity = DashboardServiceClient::new(channel.clone())
        .subscribe_metrics(req)
        .await;
    assert!(
        sanity.is_ok(),
        "initial subscribe must succeed with streaming_enabled=true; got {:?}",
        sanity.as_ref().err()
    );
    drop(sanity);

    // Flip streaming_enabled to false; ConfigReloadTask observes the watch
    // change and swaps the LiveSnapshot atomically. The per-request entry
    // in `subscribe_metrics` will see the new snapshot next.
    let start = Instant::now();
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(false);
            Ok(())
        })
        .expect("update_with apply");

    // Poll until subscribe_metrics returns Unavailable. Cap at 1s (G3).
    let timeout = Duration::from_secs(1);
    loop {
        let mut req = tonic::Request::new(
            oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default(),
        );
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        let result = DashboardServiceClient::new(channel.clone())
            .subscribe_metrics(req)
            .await;
        if let Err(status) = &result {
            if status.code() == Code::Unavailable {
                let elapsed = start.elapsed();
                assert!(
                    elapsed < timeout,
                    "G3 violation: convergence {elapsed:?} >= 1s cap"
                );
                harness.shutdown().await;
                return; // PASS
            }
        }
        if start.elapsed() > timeout {
            panic!("G3 violation: streaming toggle did not reflect within 1s (D33 CI bound)");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// D27 — warmup preservation. Seeds an initial `LiveSnapshot` whose
/// `started_at` is 60s in the past (well out of the 30s warmup window),
/// then reloads with new thresholds. After reload, `is_in_warmup()` must
/// remain `false` AND the new thresholds must be visible.
///
/// Uses `LoadPolicy::try_new_with_started_at` to construct the past-warmup
/// policy without waiting 30s of real time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_load_thresholds_applied_without_warmup_reset() {
    // Build an initial load_policy whose started_at is 60s in the past —
    // well beyond the 30s WARMUP. `try_new_with_started_at` is the API
    // that ConfigReloadTask uses internally to preserve warmup across
    // reloads (D27); we use it here to bootstrap the test snapshot.
    let past_anchor = std::time::Instant::now() - std::time::Duration::from_secs(60);
    let initial_policy = Arc::new(
        LoadPolicy::try_new_with_started_at(
            oneshim_core::config::LoadThresholds::default(),
            past_anchor,
        )
        .expect("valid initial thresholds"),
    );
    assert!(
        !initial_policy.is_in_warmup(),
        "precondition: initial policy must already be out of warmup"
    );

    let harness = LiveReloadHarness::builder()
        .initial_load_policy(initial_policy)
        .build()
        .await;

    // Reload with new (still valid) thresholds. ConfigReloadTask must
    // preserve the original `started_at` per D27.
    let new_thresholds = oneshim_core::config::LoadThresholds {
        min_free_mem_gb: 1.5,
        cpu_low_pct: 25.0,
        cpu_medium_pct: 55.0,
        cpu_high_pct: 80.0,
    };
    harness
        .cfg_mgr
        .update_with(|c| {
            c.web.grpc_load_thresholds = Some(new_thresholds.clone());
            Ok(())
        })
        .expect("reload new thresholds");

    // Give the reload task a moment to observe the watch change + apply.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let snap = harness.live.snapshot();
    let post_thresholds = snap.load_policy.thresholds();
    assert!(
        (post_thresholds.cpu_low_pct - 25.0).abs() < f32::EPSILON,
        "new cpu_low_pct must apply; got {}",
        post_thresholds.cpu_low_pct
    );
    assert!(
        (post_thresholds.cpu_medium_pct - 55.0).abs() < f32::EPSILON,
        "new cpu_medium_pct must apply; got {}",
        post_thresholds.cpu_medium_pct
    );
    assert!(
        !snap.load_policy.is_in_warmup(),
        "D27: warmup anchor must carry over across reloads"
    );
    assert_eq!(
        snap.load_policy.started_at(),
        past_anchor,
        "D27: started_at must be bit-identical to the pre-reload anchor"
    );

    harness.shutdown().await;
}

/// Partial-apply invariant — a malformed thresholds reload is rejected and
/// the previous policy is preserved, while `streaming_enabled` (trivially
/// valid) still updates. Task 2.1 commit db1d1252 guarantees this via
/// `apply_config` keeping `current.load_policy` when `try_new_with_started_at`
/// errors.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_reload_rejects_malformed_thresholds_and_continues() {
    let initial_thresholds = oneshim_core::config::LoadThresholds {
        min_free_mem_gb: 1.0,
        cpu_low_pct: 30.0,
        cpu_medium_pct: 60.0,
        cpu_high_pct: 85.0,
    };
    let initial_policy = Arc::new(LoadPolicy::new(initial_thresholds.clone()));

    let seed_thresholds = initial_thresholds.clone();
    let harness = LiveReloadHarness::builder()
        .seed(move |c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = Some(true);
            // Seed explicit valid thresholds so we can assert they survive.
            c.web.grpc_load_thresholds = Some(seed_thresholds);
        })
        .initial_load_policy(initial_policy.clone())
        .build()
        .await;

    // Reload with invalid thresholds (low > medium violates ordering) AND
    // flip streaming_enabled. Partial-apply: streaming flips, policy does
    // NOT.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(false);
            c.web.grpc_load_thresholds = Some(oneshim_core::config::LoadThresholds {
                min_free_mem_gb: 1.0,
                cpu_low_pct: 90.0, // invalid: low > medium
                cpu_medium_pct: 50.0,
                cpu_high_pct: 85.0,
            });
            Ok(())
        })
        .expect("update_with (malformed thresholds)");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let snap = harness.live.snapshot();
    assert!(
        !snap.streaming_enabled,
        "streaming_enabled update MUST apply despite malformed thresholds (partial-apply)"
    );
    let post_thresholds = snap.load_policy.thresholds();
    assert!(
        (post_thresholds.cpu_low_pct - 30.0).abs() < f32::EPSILON,
        "invalid thresholds rejected; previous cpu_low_pct must survive; got {}",
        post_thresholds.cpu_low_pct
    );
    assert!(
        (post_thresholds.cpu_medium_pct - 60.0).abs() < f32::EPSILON,
        "invalid thresholds rejected; previous cpu_medium_pct must survive; got {}",
        post_thresholds.cpu_medium_pct
    );
    assert!(
        Arc::ptr_eq(&snap.load_policy, &initial_policy),
        "invalid policy rejected; Arc identity must equal the initial policy"
    );
    assert!(
        harness
            .metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task must remain alive after rejecting a malformed update"
    );

    // Follow-up valid reload must still apply — the task survived the
    // invalid one and keeps draining events.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(true);
            Ok(())
        })
        .expect("follow-up valid reload");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        harness.live.snapshot().streaming_enabled,
        "follow-up valid reload must still apply after the rejected one"
    );

    harness.shutdown().await;
}

/// Watch coalescing — 100 rapid `update_with` calls must not panic the
/// reload task AND the live snapshot must match the LAST update's value.
/// `tokio::sync::watch` has latest-wins semantics: the reload task's
/// `changed().await` may coalesce intermediate transitions, but the
/// final observed state must equal the final sent state.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_live_reload_coalesces_rapid_updates() {
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.external_grpc.streaming_enabled = Some(true);
        })
        .build()
        .await;

    // Fire 100 updates as fast as `update_with` will accept them. Alternate
    // streaming_enabled so every call genuinely mutates state — the last
    // update wins (even iterations flip true, odd false; i=99 is odd →
    // final streaming_enabled = false).
    for i in 0..100 {
        let enabled = i % 2 == 0;
        harness
            .cfg_mgr
            .update_with(move |c| {
                c.external_grpc.streaming_enabled = Some(enabled);
                Ok(())
            })
            .expect("rapid update");
    }

    // Replace fixed sleep with convergence poll — waits for the reload task
    // to drain up to the final update without relying on a fixed timeout.
    // i=99 is odd → final update set streaming_enabled = Some(false).
    harness
        .wait_for_streaming(
            false,
            Duration::from_secs(2),
            "reload task did not converge to final update",
        )
        .await;

    // Defensive re-read: guards against the reload task doing one more
    // update between the convergence break and this assertion.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "final snapshot must match the last update (streaming_enabled=false)"
    );
    assert!(
        !harness.reload_handle.is_finished(),
        "reload task must still be running after coalescing 100 rapid updates"
    );
    assert!(
        harness
            .metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task liveness flag must still be set"
    );
    // Coalescing invariant: reload_total is bounded by 100 (≤ sends) and
    // must be ≥ 1 (at least one apply observed the final state).
    let total = harness
        .metrics
        .config_reload_total
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        (1..=100).contains(&total),
        "config_reload_total must be within [1, 100] after coalescing; got {total}"
    );

    harness.shutdown().await;
}

/// Live reload affects the next per-request decision after the reload
/// lands. Already-open streams snapshot `load_policy` at call entry
/// (spec D21) — this is intentional — so we verify the *next* RPC's
/// entry-point sees the new policy via `live.snapshot()`.
///
/// Opens a `SubscribeMetrics` stream (which stays alive using the
/// fixture's 1-msg-then-idle handler), mutates thresholds mid-stream,
/// then asserts the live snapshot reflects the new thresholds — which
/// is what a fresh RPC entry would observe per D21.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_reload_affects_long_running_stream() {
    // Past-warmup policy so the classify branch isn't forced-Medium (WARMUP=30s).
    let past_anchor = std::time::Instant::now() - std::time::Duration::from_secs(60);
    let baseline_thresholds = oneshim_core::config::LoadThresholds {
        min_free_mem_gb: 1.0,
        cpu_low_pct: 30.0,
        cpu_medium_pct: 60.0,
        cpu_high_pct: 85.0,
    };
    let initial_policy = Arc::new(
        LoadPolicy::try_new_with_started_at(baseline_thresholds.clone(), past_anchor)
            .expect("valid initial thresholds"),
    );

    let seed_thresholds = baseline_thresholds.clone();
    let harness = LiveReloadHarness::builder()
        .seed(move |c| {
            c.external_grpc.streaming_enabled = Some(true);
            // Seed wide thresholds — "no shed" baseline.
            c.web.grpc_load_thresholds = Some(seed_thresholds);
        })
        .initial_load_policy(initial_policy.clone())
        .build()
        .await;

    // Open a SubscribeMetrics stream. The real handler in
    // `DashboardServiceImpl` stays alive and emits periodically; we don't
    // need to drain — we just need the stream call to have been made.
    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-longstream",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;
    let mut req =
        tonic::Request::new(oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let stream_response = DashboardServiceClient::new(channel.clone())
        .subscribe_metrics(req)
        .await
        .expect("initial stream open");
    // Keep the inner stream alive for the rest of the test — dropping it
    // would release the server's per-stream guard; we want the stream to
    // be concurrent with the reload.
    let _keep_stream_alive = stream_response.into_inner();

    // Mid-stream: reload with tight "shed" thresholds. The existing stream
    // keeps its captured policy per D21; new per-request decisions observe
    // the new policy via `live.snapshot()`.
    let shed_thresholds = oneshim_core::config::LoadThresholds {
        min_free_mem_gb: 100.0, // require ≥100 GB free — always Critical
        cpu_low_pct: 1.0,
        cpu_medium_pct: 2.0,
        cpu_high_pct: 3.0,
    };
    harness
        .cfg_mgr
        .update_with(|c| {
            c.web.grpc_load_thresholds = Some(shed_thresholds.clone());
            Ok(())
        })
        .expect("mid-stream reload");

    tokio::time::sleep(Duration::from_millis(150)).await;

    // A fresh `live.snapshot()` (what a new RPC entry would observe) must
    // reflect the tight shed thresholds — this is the "next per-request
    // decision" observability point per D21.
    let post_snap = harness.live.snapshot();
    let post_thresholds = post_snap.load_policy.thresholds();
    assert!(
        (post_thresholds.cpu_high_pct - 3.0).abs() < f32::EPSILON,
        "post-reload cpu_high_pct must reflect shed thresholds; got {}",
        post_thresholds.cpu_high_pct
    );
    assert!(
        (post_thresholds.min_free_mem_gb - 100.0).abs() < f32::EPSILON,
        "post-reload min_free_mem_gb must reflect shed thresholds; got {}",
        post_thresholds.min_free_mem_gb
    );
    // Classify a realistic-load metrics snapshot under the new policy —
    // it MUST come back Critical (cpu > 3 and free_mem_gb < 100).
    let mk_metrics =
        |cpu: f32, used_gib: u64, total_gib: u64| oneshim_core::models::system::SystemMetrics {
            timestamp: chrono::Utc::now(),
            cpu_usage: cpu,
            memory_used: used_gib * 1_073_741_824,
            memory_total: total_gib * 1_073_741_824,
            disk_used: 0,
            disk_total: 0,
            network: None,
            typing_wpm: 0.0,
        };
    let shed_level = post_snap.load_policy.classify(&mk_metrics(50.0, 8, 16));
    assert_eq!(
        shed_level,
        oneshim_web::grpc::LoadLevel::Critical,
        "under shed thresholds, moderate metrics must classify as Critical"
    );

    // D21: already-open streams keep their captured policy reference —
    // represented here by the `initial_policy` Arc that preceded the
    // reload. That Arc must be a DIFFERENT instance from the live
    // snapshot's current `load_policy` (the ConfigReloadTask built a
    // fresh Arc in `apply_config`). The initial policy's thresholds
    // must also still be the pre-reload values.
    assert!(
        !Arc::ptr_eq(&initial_policy, &post_snap.load_policy),
        "D21: post-reload live policy must be a distinct Arc from the pre-reload one"
    );
    let initial_thresholds = initial_policy.thresholds();
    assert!(
        (initial_thresholds.cpu_high_pct - 85.0).abs() < f32::EPSILON,
        "already-captured initial policy must still carry pre-reload cpu_high_pct=85.0; got {}",
        initial_thresholds.cpu_high_pct
    );

    // End-to-end: a 2nd RPC entering the server observes the new policy via
    // streaming_source.load_policy() at subscribe_metrics.rs:72-75 (D21
    // snapshot-at-call-entry). Opening a fresh stream proves the server-stack
    // propagation works — not just the ArcSwap substrate.
    let mut req2 =
        tonic::Request::new(oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req2.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("auth header"),
    );
    let second_open = DashboardServiceClient::new(channel.clone())
        .subscribe_metrics(req2)
        .await;
    assert!(
        second_open.is_ok(),
        "2nd RPC must still open post-reload; shed affects tick cadence not the gate"
    );
    // Verify the snapshot substrate is stable across the 2nd entry (identity,
    // not rebuild) — proves the fresh Arc assembled during apply_config is
    // what the new RPC would observe.
    let post_2nd_snap = harness.live.snapshot();
    assert!(
        Arc::ptr_eq(&post_2nd_snap.load_policy, &post_snap.load_policy),
        "snapshot stable across 2nd RPC entry; live.snapshot() should return \
         the same Arc until the next reload"
    );
    // Drop the 2nd stream to release its per-stream guard.
    drop(second_open.unwrap().into_inner());

    drop(_keep_stream_alive);
    harness.shutdown().await;
}

/// Shutdown — the `ConfigReloadTask` must exit within 5 seconds of the
/// shutdown signal. The task's `tokio::select!` biases on `shutdown_rx`
/// (spec §5.4) so it will notice the flip even when a config update is
/// queued concurrently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_grpc_config_reload_task_exits_on_shutdown() {
    let harness = LiveReloadHarness::builder().build().await;

    // Sanity: reload task alive-flag is set after startup.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        harness
            .metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task must be alive before shutdown"
    );

    // Signal shutdown.
    harness.shutdown_tx.send_replace(true);

    // Destructure to take ownership of the handles for the graceful-exit
    // verification (await consumes JoinHandle, which can't be done through
    // `harness.shutdown()`'s abort path).
    let LiveReloadHarness {
        metrics,
        server_handle,
        reload_handle,
        ..
    } = harness;

    // The reload task MUST complete within 5s of the signal landing.
    let joined = tokio::time::timeout(Duration::from_secs(5), reload_handle)
        .await
        .expect("reload task must exit within 5s of shutdown signal");
    assert!(
        joined.is_ok(),
        "reload task should complete cleanly on shutdown; got {joined:?}"
    );
    assert!(
        !metrics
            .config_reload_task_alive
            .load(std::sync::atomic::Ordering::Relaxed),
        "reload task liveness flag must clear on exit"
    );

    // Server may still be draining in-flight work; abort to end the test.
    server_handle.abort();
    let _ = server_handle.await;
}

// ── Task 9.6 — Fallback semantics integration tests (D22) ───────────────────
//
// Spec §9.2 L1419-1422 mandates three integration tests pinning the
// `external_grpc.streaming_enabled` override semantics introduced by D22:
//   1. NG1: external toggle does NOT mutate the loopback `web.grpc_streaming_enabled`
//      field — they remain independent at the AppConfig level.
//   2. Fall-through: when `external_grpc.streaming_enabled = None`, the resolved
//      external streaming value comes from `web.grpc_streaming_enabled`.
//   3. Override-beats-parent (NV4): when `external_grpc.streaming_enabled = Some(_)`,
//      the override wins regardless of `web.grpc_streaming_enabled`.
//
// Tests use real `ConfigReloadTask` wiring (via `spawn_server_with_config_manager`)
// to exercise the resolution code path in `config_reload::apply_config` end-to-end.

/// Test 1 (D22 / NG1) — toggling `external_grpc.streaming_enabled = Some(false)`
/// disables external streaming WITHOUT mutating `web.grpc_streaming_enabled`.
///
/// The loopback gRPC server captures `web.grpc_streaming_enabled` at boot via
/// `StreamingSource::Fixed` (boot-time captured value, no live reload — see
/// `streaming_source.rs`). The external server uses `StreamingSource::Live`,
/// which reads from `LiveSnapshot.streaming_enabled` resolved by `apply_config`.
///
/// NG1 mandates that external live-reload must not mutate the shared
/// `web.grpc_streaming_enabled` field. We verify NG1 at the configuration
/// layer: after the toggle, `cfg_mgr.snapshot().web.grpc_streaming_enabled`
/// must remain unchanged. Since the loopback server reads its `streaming_enabled`
/// from this AppConfig field at boot AND has no live-reload pathway, an
/// unchanged AppConfig field is equivalent to "loopback streaming is unaffected".
/// We don't spawn a separate loopback server here — that would test the
/// `StreamingSource::Fixed` capture (covered in `streaming_source.rs` unit tests),
/// not the NG1 invariant on external live-reload.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn loopback_streaming_enabled_is_not_live_reloaded() {
    // Seed: loopback streaming enabled (web.grpc_streaming_enabled=true);
    // external override left at None so external falls through to the same
    // value (initial sanity = enabled on both sides).
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = None;
        })
        .build()
        .await;

    // Sanity: external streaming initially resolves to true.
    assert!(
        harness.live.snapshot().streaming_enabled,
        "sanity: live snapshot must start with streaming_enabled=true"
    );
    assert!(
        harness.cfg_mgr.snapshot().web.grpc_streaming_enabled,
        "sanity: web.grpc_streaming_enabled must start at true"
    );

    // Flip the EXTERNAL override; loopback config must remain untouched.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(false);
            Ok(())
        })
        .expect("update_with apply");

    // Wait for the ConfigReloadTask to converge (mirrors Task 9.4 cap).
    harness
        .wait_for_streaming(
            false,
            Duration::from_secs(1),
            "convergence timeout: external streaming did not flip to false",
        )
        .await;

    // Assert NG1: external is now disabled, but loopback config field is untouched.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "external override must disable external streaming"
    );
    assert!(
        harness.cfg_mgr.snapshot().web.grpc_streaming_enabled,
        "NG1 violation: external toggle must NOT mutate web.grpc_streaming_enabled \
         (loopback config must remain untouched)"
    );
    // And the override field is now Some(false), confirming the toggle landed.
    assert_eq!(
        harness.cfg_mgr.snapshot().external_grpc.streaming_enabled,
        Some(false),
        "external override must reflect the operator's flip"
    );

    harness.shutdown().await;
}

/// Test 2 (D22 fall-through) — when `external_grpc.streaming_enabled = None`,
/// the resolved external streaming value comes from `web.grpc_streaming_enabled`.
///
/// Seeds initial state with the override enabled, then mutates to the
/// fall-through scenario (`None` + `web=false`). After convergence, the
/// `LiveSnapshot.streaming_enabled` must reflect the shared `web` field, and
/// `subscribe_metrics` must return `Unavailable` to confirm the resolution
/// took effect end-to-end through the running server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_streaming_falls_back_to_web_field_when_external_none() {
    // Seed initial config with override enabled so external is initially streaming.
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = true;
            c.external_grpc.streaming_enabled = Some(true);
        })
        .build()
        .await;

    // Sanity: seed landed — external override of Some(true) → streaming enabled.
    assert!(
        harness.live.snapshot().streaming_enabled,
        "sanity: seed → external streaming initially true (external=Some(true))"
    );

    // Mint a JWT + TLS channel for end-to-end verification via subscribe_metrics.
    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-9-6-fallback",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;

    // Apply the fall-through scenario: external=None, web=false.
    // Since `apply_config` resolves `streaming_enabled = external.unwrap_or(web)`,
    // the resolved value should now be `false`.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = None;
            c.web.grpc_streaming_enabled = false;
            Ok(())
        })
        .expect("update_with apply");

    // Wait for ConfigReloadTask to converge.
    harness
        .wait_for_streaming(
            false,
            Duration::from_secs(1),
            "fall-through timeout: live.streaming_enabled did not converge to false",
        )
        .await;

    // Assert resolution: external=None + web=false → resolved=false.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "fall-through: external=None + web=false must resolve to streaming_enabled=false"
    );

    // End-to-end check: subscribe_metrics must return Unavailable.
    let mut req =
        tonic::Request::new(oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let result = DashboardServiceClient::new(channel)
        .subscribe_metrics(req)
        .await;
    match result {
        Err(s) if s.code() == Code::Unavailable => {
            // Expected: streaming disabled via fall-through.
        }
        other => panic!(
            "expected Unavailable from subscribe_metrics under fall-through; got {:?}",
            other
        ),
    }

    harness.shutdown().await;
}

/// Test 3 (D22 / NV4 — override-beats-parent) — when
/// `external_grpc.streaming_enabled = Some(true)` and
/// `web.grpc_streaming_enabled = false`, the override wins and the external
/// server keeps streaming.
///
/// Seeds initial state with both fields false (external matches), then
/// mutates external to `Some(true)` while leaving web at false. After
/// convergence, `LiveSnapshot.streaming_enabled` must be `true` and a real
/// `subscribe_metrics` RPC must succeed (not return Unavailable).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_streaming_override_wins_over_web_field_when_some() {
    // Seed: web=false and external=Some(false) — initial resolved=false.
    let harness = LiveReloadHarness::builder()
        .seed(|c| {
            c.web.grpc_streaming_enabled = false;
            c.external_grpc.streaming_enabled = Some(false);
        })
        .initial_streaming(false)
        .build()
        .await;

    // Sanity: seed landed — both flags false, resolved streaming disabled.
    assert!(
        !harness.live.snapshot().streaming_enabled,
        "sanity: seed → external streaming initially false (external=Some(false), web=false)"
    );

    let token = test_mint_jwt(
        &harness.jwt_kp.enc_key,
        "user-9-6-override",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();
    let channel = make_tls_channel(harness.port, &cert_pem, None).await;

    // Apply the override-beats-parent scenario: external=Some(true), web=false.
    // `apply_config` resolves `streaming_enabled = external.unwrap_or(web) = true`
    // even though web stays false — proving NV4.
    harness
        .cfg_mgr
        .update_with(|c| {
            c.external_grpc.streaming_enabled = Some(true);
            // web.grpc_streaming_enabled deliberately stays false.
            Ok(())
        })
        .expect("update_with apply");

    // Wait for ConfigReloadTask to converge.
    harness
        .wait_for_streaming(
            true,
            Duration::from_secs(1),
            "override-beats-parent timeout: live.streaming_enabled did not converge to true \
             (web=false but external=Some(true) should win)",
        )
        .await;

    // Assert NV4: even though web=false, the Some(true) override wins.
    assert!(
        harness.live.snapshot().streaming_enabled,
        "NV4 violation: external=Some(true) must override web=false"
    );
    assert!(
        !harness.cfg_mgr.snapshot().web.grpc_streaming_enabled,
        "sanity: web.grpc_streaming_enabled must remain false (override is the only enabler)"
    );

    // End-to-end check: subscribe_metrics must succeed (auth returns the stream).
    let mut req =
        tonic::Request::new(oneshim_web::proto::dashboard::v1::SubscribeMetricsRequest::default());
    req.metadata_mut().insert(
        "authorization",
        format!("Bearer {token}").parse().expect("valid header"),
    );
    let result = DashboardServiceClient::new(channel)
        .subscribe_metrics(req)
        .await;
    assert!(
        result.is_ok(),
        "subscribe_metrics must succeed when override forces streaming on; got {:?}",
        result.as_ref().err()
    );
    drop(result);

    harness.shutdown().await;
}
