//! External gRPC stress test suite.
//!
//! Public gRPC behavior is documented in `docs/guides/external-grpc.md`.
//!
//! Three tests:
//! 1. `concurrent_connection_cap_enforced` — `max_connections = 1024`
//!    correctness + dynamic slot recovery.
//! 2. `fd_pressure_resilience` — 3 rounds of 1024-stream churn + post-loop
//!    survival, no fd leak.
//! 3. `ipv6_64_prefix_ban_full_stack` — `IpBan` accept_loop wiring on the
//!    IPv6 path: 5 auth failures from `[::1]` → 6th TCP closed before TLS.
//!
//! Compiled to an empty integration test binary unless the `stress-test`
//! feature is enabled. Run locally:
//!
//! ```sh
//! ulimit -n 65536
//! cargo test -p maekon-web --features stress-test \
//!   --test external_grpc_stress -- --test-threads=1 --nocapture
//! ```

#![cfg(feature = "stress-test")]

// #7730: `in_memory_storage()` lives under `tests/support/` (not the
// `maekon-web` library) because it needs `maekon-storage`, an adapter crate
// — see that file's doc comment + `scripts/check-crate-boundaries.sh`.
#[path = "support/in_memory_storage.rs"]
mod in_memory_storage_support;
use in_memory_storage_support::in_memory_storage;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use maekon_core::config::{AuthMode, ExternalGrpcConfig, JwtAlgorithm};
use maekon_core::ports::audit_log::AuditLogPort;
use tokio::task::JoinSet;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

use maekon_web::grpc::external::cert_resolver::HotReloadCertResolver;
use maekon_web::grpc::external::ip_ban::IpBan;
use maekon_web::grpc::external::jwt_verifier::JwtVerifier;
use maekon_web::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
use maekon_web::grpc::external::metrics::ExternalMetrics;
use maekon_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
// #7730: NoopAudit / server_cert_pem are shared with
// `tests/external_grpc_integration.rs` via `test_support` — de-duplicated
// from the verbatim copies that used to live in this file.
use maekon_web::grpc::external::test_support::{
    install_rustls_crypto_provider, server_cert_pem, spawn_server_with_config_manager,
    test_cert_pair, test_jwt_keypair, test_mint_jwt, NoopAudit, ServerHandle,
};
use maekon_web::grpc::external::tls_config::load_certified_key;
use maekon_web::grpc::test_support::mock_system_monitor::MockSystemMonitor;
use maekon_web::grpc::LoadPolicy;
use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use maekon_web::proto::dashboard::v1::{GetAgentInfoRequest, SubscribeEventsRequest};

// ── Shutdown pair helper ─────────────────────────────────────────────────────

fn make_test_shutdown_pair() -> (
    Arc<tokio::sync::watch::Sender<bool>>,
    tokio::sync::watch::Receiver<bool>,
) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    (Arc::new(tx), rx)
}

// ── Server config helper (stress variant) ─────────────────────────────────────
//
// Differs from make_jwt_config in external_grpc_integration.rs:151 in that
// max_connections + bind_addr are caller-controlled. JWT-only auth.

fn make_jwt_stress_config(
    jwt_pub_key_path: &std::path::Path,
    max_connections: usize,
    bind_addr: SocketAddr,
) -> ExternalGrpcSpawnConfig {
    let (cert_path, key_path) = test_cert_pair().expect("server certificate fixture");
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(128);

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
    ExternalGrpcSpawnConfig {
        bind_addr,
        config: ExternalGrpcConfig {
            enabled: true,
            auth_mode: Some(AuthMode::Jwt),
            max_connections,
            // Per-channel single stream: cap by max_connections, not stream
            // cap. Set high so stream cap is never the rejecting layer.
            // (max_concurrent_streams is `usize` — see maekon-core
            // crates/maekon-core/src/config/sections/external_grpc.rs:69.)
            max_concurrent_streams: max_connections.max(1024),
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
        // #7730: `ExternalGrpcSpawnConfig` consolidated the old flat
        // `load_policy` / `streaming_enabled` fields into `live:
        // Arc<LiveExternalConfig>` (see `spawn_config.rs`) — this file had
        // not been updated since that refactor (`grpc-stress.yml` is
        // workflow_dispatch-only, so the drift wasn't caught by default CI).
        live: Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: Arc::new(LoadPolicy::new(
                maekon_core::config::LoadThresholds::default(),
            )),
        })),
    }
}

// Own the actual bound server and reload task for deterministic startup/teardown.
struct StressServer {
    addr: SocketAddr,
    server: ServerHandle,
    reload: tokio::task::JoinHandle<()>,
    shutdown: Arc<tokio::sync::watch::Sender<bool>>,
    _manager: Arc<maekon_core::config_manager::ConfigManager>,
    _directory: tempfile::TempDir,
}

impl StressServer {
    async fn shutdown(self) {
        self.shutdown.send_replace(true);
        tokio::time::timeout(Duration::from_secs(5), self.server)
            .await
            .expect("server shutdown deadline")
            .expect("server task must not panic")
            .expect("server shutdown must succeed");
        tokio::time::timeout(Duration::from_secs(5), self.reload)
            .await
            .expect("reload shutdown deadline")
            .expect("reload task must not panic");
    }
}

async fn spawn_stress_server(cfg: ExternalGrpcSpawnConfig) -> StressServer {
    install_rustls_crypto_provider();
    let ip = cfg.bind_addr.ip();
    let shutdown = cfg.shutdown_tx.clone();
    let directory = tempfile::tempdir().expect("config directory");
    let manager = Arc::new(
        maekon_core::config_manager::ConfigManager::with_path(directory.path().join("config.json"))
            .expect("config manager"),
    );
    let (server, reload, port) = spawn_server_with_config_manager(cfg, manager.clone())
        .await
        .expect("real server readiness");
    StressServer {
        addr: SocketAddr::new(ip, port),
        server,
        reload,
        shutdown,
        _manager: manager,
        _directory: directory,
    }
}

// ── TLS channel helper (stress variant) ───────────────────────────────────────
//
// Returns Result<Channel, tonic::transport::Error> instead of panicking — the
// stress tests intentionally try to open over-cap channels and expect failure
// (Test 1 Phase 2). Each call produces a fresh Endpoint::connect() →
// distinct underlying TCP per V3.

async fn make_stress_tls_channel(
    addr: SocketAddr,
    server_cert_pem: &[u8],
) -> Result<Channel, tonic::transport::Error> {
    let ca_cert = Certificate::from_pem(server_cert_pem);
    let tls = ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(ca_cert);
    // Use ipv6 / ipv4 literal in the URI authority. tonic accepts both.
    let uri = if addr.is_ipv6() {
        format!("https://[{}]:{}", addr.ip(), addr.port())
    } else {
        format!("https://{}:{}", addr.ip(), addr.port())
    };
    Endpoint::from_shared(uri)
        .expect("valid endpoint")
        .tls_config(tls)
        .expect("tls config")
        .connect_timeout(Duration::from_secs(3))
        .connect()
        .await
}

// Verify the actual pre-TLS boundary instead of accepting arbitrary RPC errors.
async fn assert_transport_rejected(addr: SocketAddr, cert_pem: &[u8]) {
    use tokio::io::AsyncReadExt;
    let mut tcp = tokio::net::TcpStream::connect(addr)
        .await
        .expect("TCP reaches the listening server");
    let mut byte = [0u8; 1];
    let count = tokio::time::timeout(Duration::from_secs(1), tcp.read(&mut byte))
        .await
        .expect("admission rejection must close TCP before TLS")
        .expect("pre-TLS close must be observable");
    assert_eq!(count, 0, "admission rejection must send no TLS data");
    drop(tcp);

    let error = make_stress_tls_channel(addr, cert_pem)
        .await
        .expect_err("rejected admission must not complete TLS");
    let mut cause: &(dyn std::error::Error + 'static) = &error;
    let io_kind = loop {
        if let Some(error) = cause.downcast_ref::<std::io::Error>() {
            break Some(error.kind());
        }
        match cause.source() {
            Some(source) => cause = source,
            None => break None,
        }
    };
    assert!(
        matches!(
            io_kind,
            Some(std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset)
        ),
        "expected a peer-close transport cause, got {error:?}"
    );
}

#[cfg(target_os = "linux")]
fn process_fd_count() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("read this test process's descriptors")
        .collect::<Result<Vec<_>, _>>()
        .expect("enumerate every descriptor")
        .len()
}

#[cfg(target_os = "linux")]
async fn assert_fd_count_recovers(baseline: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = process_fd_count();
            if current <= baseline {
                eprintln!("descriptor recovery: baseline={baseline}, current={current}");
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "descriptor leak after connection cleanup: baseline={baseline}, current={}",
            process_fd_count()
        )
    });
}

// ── Server liveness probe (V4 fallback for active_connection_count) ──────────
//
// Polls a fresh unary GetAgentInfo round-trip until success or deadline.
// Used by Test 1 Phase 3 (slot recovery) and Test 2 post-loop check —
// production lacks a public active_connection_count accessor.

async fn poll_unary_until_success(
    addr: SocketAddr,
    cert_pem: &[u8],
    token: &str,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let mut last_err: Option<String> = None;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "poll_unary_until_success: deadline exceeded; last error: {}",
                last_err.unwrap_or_else(|| "<none observed>".into())
            ));
        }
        let channel = match make_stress_tls_channel(addr, cert_pem).await {
            Ok(c) => c,
            Err(e) => {
                last_err = Some(format!("connect: {e}"));
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let mut req = tonic::Request::new(GetAgentInfoRequest {});
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        match DashboardServiceClient::new(channel)
            .get_agent_info(req)
            .await
        {
            Ok(_) => return Ok(()),
            Err(e) => {
                last_err = Some(format!("rpc: {e}"));
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

// ── Streaming-open poll (slot-recovery retry) ────────────────────────────────
//
// Opens a fresh TLS channel + SubscribeEvents stream, retrying on transport
// errors until success or deadline. Used by Test 1 Phase 3 after
// poll_unary_until_success: the unary poll proves the server is alive, but
// its own last channel's ActiveConnGuard::drop (conn_info.rs:29) runs AFTER
// the client-side channel leaves scope and the TCP FIN reaches the server.
// On slow CI the decrement can still be in flight when a single retry fires,
// producing a transient over-cap silent drop (accept_loop.rs:85-91, which
// does NOT call record_failure so IpBan stays clean) that surfaces on the
// client as `tls handshake eof`. Retrying on a 50 ms cadence absorbs that
// propagation window without masking a real slot-recovery failure — the
// deadline still fails the test if the slot never frees.

async fn retry_stream_open_until_success(
    addr: SocketAddr,
    cert_pem: &[u8],
    token: &str,
    deadline: tokio::time::Instant,
) -> Result<(), String> {
    let mut last_err: Option<String> = None;
    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "retry_stream_open_until_success: deadline exceeded; last error: {}",
                last_err.unwrap_or_else(|| "<none observed>".into())
            ));
        }
        let channel = match make_stress_tls_channel(addr, cert_pem).await {
            Ok(c) => c,
            Err(e) => {
                last_err = Some(format!("connect: {e}"));
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        };
        let mut req = tonic::Request::new(SubscribeEventsRequest::default());
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        match DashboardServiceClient::new(channel)
            .subscribe_events(req)
            .await
        {
            Ok(_response) => return Ok(()),
            Err(e) => {
                last_err = Some(format!("subscribe_events: {e}"));
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

// ════════════════════════════════════════════════════════════════════════════
// Test 1: concurrent_connection_cap_enforced
// ════════════════════════════════════════════════════════════════════════════

/// Invariant: max_connections = N → N concurrent connections succeed; the
/// (N+1)th is rejected at the connection layer.
///
/// Phases (spec §4.1):
///   Phase 1: open 1024 concurrent channels (each with 1 subscribe_events
///            stream) and confirm all establish.
///   Phase 2: attempt the 1025th channel; expect transport-level failure.
///   Phase 3: drop one Phase-1 channel, poll for slot recovery (V4 fallback:
///            unary RPC), retry the 1025th — expect success.
///
/// fd estimate: ~2050 (1024 server + 1024 client + tokio + OS). ulimit -n
/// 65536 in the workflow provides 32× headroom.
///
/// Runtime estimate: ~5–15s (1024 TLS handshakes dominate Phase 1).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_connection_cap_enforced() {
    const CAP: usize = 1024;
    let jwt_kp = test_jwt_keypair().expect("JWT key fixture");
    let cfg = make_jwt_stress_config(
        &jwt_kp.pub_pem_path,
        CAP,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    );
    let server = spawn_stress_server(cfg).await;
    let addr = server.addr;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "stress-cap",
        "test-issuer",
        "test-audience",
        3600,
    )
    .expect("mint JWT fixture");
    let cert_pem = server_cert_pem().expect("read server certificate fixture");

    // ── Phase 1: open CAP concurrent streams ────────────────────────────────
    let mut tasks = JoinSet::new();
    for i in 0..CAP {
        let addr_c = addr;
        let cert_c = cert_pem.clone();
        let token_c = token.clone();
        tasks.spawn(async move {
            let channel = make_stress_tls_channel(addr_c, &cert_c)
                .await
                .map_err(|e| format!("channel {i} connect failed: {e}"))?;
            let mut req = tonic::Request::new(SubscribeEventsRequest::default());
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {token_c}").parse().expect("valid header"),
            );
            let stream = DashboardServiceClient::new(channel.clone())
                .subscribe_events(req)
                .await
                .map_err(|e| format!("stream {i} open failed: {e}"))?
                .into_inner();
            // Hold both channel + stream so the underlying TCP stays open.
            Ok::<(Channel, _), String>((channel, stream))
        });
    }

    let mut held = Vec::with_capacity(CAP);
    while let Some(joined) = tasks.join_next().await {
        let res = joined.expect("task panicked");
        let pair = res.unwrap_or_else(|e| panic!("Phase 1 failed: {e}"));
        held.push(pair);
    }
    assert_eq!(
        held.len(),
        CAP,
        "Phase 1 should establish all {CAP} streams"
    );

    // The cap is enforced before TLS, so no HTTP/2 or gRPC status is possible.
    assert_transport_rejected(addr, &cert_pem).await;

    // ── Phase 3: drop one slot, retry ───────────────────────────────────────
    // Dropping a held (Channel, Stream) pair on the client closes client-side TCP.
    // The server's ActiveConnGuard::drop (conn_info.rs:29) decrements only after
    // tonic sees the FIN and drops the PeerAwareStream wrapper.
    // poll_unary_until_success retries on a 50 ms cadence to absorb this
    // client→server propagation latency.
    drop(held.pop().expect("at least one held pair"));

    // V4 fallback: poll for liveness via fresh unary RPC.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    poll_unary_until_success(addr, &cert_pem, &token, deadline)
        .await
        .expect("unary RPC must succeed after slot freed");

    // Retry the (CAP)-th stream open. poll_unary above confirmed the server is
    // accepting connections, but its own last channel's ActiveConnGuard drop
    // may still be in flight on the server, so a single-shot retry can hit the
    // over-cap silent-drop path on slow CI runners (surfaces as tls handshake
    // EOF). retry_stream_open_until_success retries on a 50 ms cadence with a
    // 5 s deadline to absorb that propagation window.
    let retry_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    retry_stream_open_until_success(addr, &cert_pem, &token, retry_deadline)
        .await
        .expect("retry stream open after slot recovery must succeed within deadline");

    // Cleanup
    drop(held);
    server.shutdown().await;
}

// ════════════════════════════════════════════════════════════════════════════
// Test 2: fd_pressure_resilience
// ════════════════════════════════════════════════════════════════════════════

/// Invariant: 3 rounds of open-1024 / hold-200ms / drop-all do not leak fds
/// or kill the accept loop. Post-loop the server still serves a unary RPC
/// AND can accept another 1024 streams.
///
/// Regression targets (spec §4.2):
///   - accept_loop's Drop path on connection cleanup
///   - supervisor respawn fidelity (silent accept-loop death post-churn)
///   - tokio task leakage (spawned RPC handlers not joined on drop)
///
/// Runtime estimate: ~20–35s.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fd_pressure_resilience() {
    const CAP: usize = 1024;
    const ROUNDS: usize = 3;

    let jwt_kp = test_jwt_keypair().expect("JWT key fixture");
    let cfg = make_jwt_stress_config(
        &jwt_kp.pub_pem_path,
        CAP,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    );
    let server = spawn_stress_server(cfg).await;
    let addr = server.addr;
    #[cfg(target_os = "linux")]
    let baseline_fds = process_fd_count();

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "stress-fd",
        "test-issuer",
        "test-audience",
        3600,
    )
    .expect("mint JWT fixture");
    let cert_pem = server_cert_pem().expect("read server certificate fixture");

    for round in 0..ROUNDS {
        // ── 2a: open CAP concurrent streams ─────────────────────────────────
        let mut tasks = JoinSet::new();
        for i in 0..CAP {
            let addr_c = addr;
            let cert_c = cert_pem.clone();
            let token_c = token.clone();
            tasks.spawn(async move {
                let channel = make_stress_tls_channel(addr_c, &cert_c)
                    .await
                    .map_err(|e| format!("round {round} channel {i}: {e}"))?;
                let mut req = tonic::Request::new(SubscribeEventsRequest::default());
                req.metadata_mut().insert(
                    "authorization",
                    format!("Bearer {token_c}").parse().expect("valid header"),
                );
                let stream = DashboardServiceClient::new(channel.clone())
                    .subscribe_events(req)
                    .await
                    .map_err(|e| format!("round {round} stream {i}: {e}"))?
                    .into_inner();
                Ok::<(Channel, _), String>((channel, stream))
            });
        }

        let mut held = Vec::with_capacity(CAP);
        while let Some(joined) = tasks.join_next().await {
            let pair = joined
                .expect("task panicked")
                .unwrap_or_else(|e| panic!("round {round} setup failed: {e}"));
            held.push(pair);
        }
        assert_eq!(
            held.len(),
            CAP,
            "round {round}: should establish all {CAP} streams"
        );

        // ── 2b: hold ────────────────────────────────────────────────────────
        tokio::time::sleep(Duration::from_millis(200)).await;

        // ── 2c: drop all ────────────────────────────────────────────────────
        drop(held);

        // ── 2d: wait up to 5s for server-side cleanup (V4 fallback) ─────────
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        poll_unary_until_success(addr, &cert_pem, &token, deadline)
            .await
            .unwrap_or_else(|e| panic!("round {round} cleanup poll failed: {e}"));
        #[cfg(target_os = "linux")]
        assert_fd_count_recovers(baseline_fds).await;
    }

    // ── Post-loop verification ─────────────────────────────────────────────
    // P1: fresh unary RPC succeeds.
    let post_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    poll_unary_until_success(addr, &cert_pem, &token, post_deadline)
        .await
        .expect("post-loop unary RPC must succeed");

    // P2: open CAP new streams — no residual fd leak.
    let mut tasks = JoinSet::new();
    for i in 0..CAP {
        let addr_c = addr;
        let cert_c = cert_pem.clone();
        let token_c = token.clone();
        tasks.spawn(async move {
            let channel = make_stress_tls_channel(addr_c, &cert_c)
                .await
                .map_err(|e| format!("post channel {i}: {e}"))?;
            let mut req = tonic::Request::new(SubscribeEventsRequest::default());
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {token_c}").parse().expect("valid header"),
            );
            let stream = DashboardServiceClient::new(channel.clone())
                .subscribe_events(req)
                .await
                .map_err(|e| format!("post stream {i}: {e}"))?
                .into_inner();
            Ok::<(Channel, _), String>((channel, stream))
        });
    }
    let mut held = Vec::with_capacity(CAP);
    while let Some(joined) = tasks.join_next().await {
        let pair = joined
            .expect("task panicked")
            .unwrap_or_else(|e| panic!("post-loop fan-out failed: {e}"));
        held.push(pair);
    }
    assert_eq!(
        held.len(),
        CAP,
        "post-loop should still admit {CAP} streams (no fd leak)"
    );

    drop(held);
    server.shutdown().await;
    #[cfg(target_os = "linux")]
    assert_fd_count_recovers(baseline_fds).await;
}

// ════════════════════════════════════════════════════════════════════════════
// Test 3: ipv6_64_prefix_ban_full_stack
// ════════════════════════════════════════════════════════════════════════════

/// Five real JWT failures from IPv6 loopback trigger pre-TLS rejection.
/// This checks IPv6 accept-path wiring; distinct addresses in a /64 are covered
/// separately by the IpBan prefix unit contract.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ipv6_64_prefix_ban_full_stack() {
    const SMALL_CAP: usize = 32; // small; never the rejecting layer
    let jwt_kp = test_jwt_keypair().expect("JWT key fixture");
    let cfg = make_jwt_stress_config(
        &jwt_kp.pub_pem_path,
        SMALL_CAP,
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0),
    );
    let server = spawn_stress_server(cfg).await;
    let addr = server.addr;
    assert!(addr.is_ipv6(), "Test 3 must bind IPv6 ::1, got {addr}");

    let bad_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "ban-victim",
        "test-issuer",
        "wrong-audience", // intentional aud mismatch → JWT verifier rejects
        3600,
    )
    .expect("mint JWT fixture");
    let cert_pem = server_cert_pem().expect("read server certificate fixture");

    // Startup sends no probe connections, so exactly five authentication
    // failures must reach the service before the next connection is banned.
    for attempt in 0..5 {
        let channel = make_stress_tls_channel(addr, &cert_pem)
            .await
            .expect("IPv6 connection before the ban threshold");
        let mut req = tonic::Request::new(GetAgentInfoRequest {});
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {bad_token}").parse().expect("valid header"),
        );
        let error = DashboardServiceClient::new(channel)
            .get_agent_info(req)
            .await
            .expect_err("bad-audience token must be rejected");
        assert_eq!(
            error.code(),
            tonic::Code::Unauthenticated,
            "attempt {attempt}"
        );
    }
    assert_transport_rejected(addr, &cert_pem).await;

    server.shutdown().await;
}
