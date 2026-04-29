//! External gRPC stress test suite.
//!
//! See `docs/superpowers/specs/2026-04-24-grpc-stress-test-suite-design.md`
//! and `docs/superpowers/plans/2026-04-24-grpc-stress-test-suite-plan.md`.
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
//! cargo test -p oneshim-web --features stress-test \
//!   --test external_grpc_stress -- --test-threads=1 --nocapture
//! ```

#![cfg(feature = "stress-test")]

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use oneshim_core::config::{AuthMode, ExternalGrpcConfig, JwtAlgorithm};
use oneshim_core::models::ai_session::SessionAuditEntry;
use oneshim_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_storage::sqlite::SqliteStorage;
use tokio::task::JoinSet;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

use oneshim_web::grpc::external::cert_resolver::HotReloadCertResolver;
use oneshim_web::grpc::external::ip_ban::IpBan;
use oneshim_web::grpc::external::jwt_verifier::JwtVerifier;
use oneshim_web::grpc::external::metrics::ExternalMetrics;
use oneshim_web::grpc::external::serve_external;
use oneshim_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
use oneshim_web::grpc::external::test_support::{
    install_rustls_crypto_provider, test_cert_pair, test_jwt_keypair, test_mint_jwt,
};
use oneshim_web::grpc::external::tls_config::load_certified_key;
use oneshim_web::grpc::test_support::mock_system_monitor::MockSystemMonitor;
use oneshim_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use oneshim_web::proto::dashboard::v1::{GetAgentInfoRequest, SubscribeEventsRequest};
use oneshim_web::storage_port::WebStorage;

// ── Noop audit ───────────────────────────────────────────────────────────────
//
// Local duplicate of the NoopAudit at tests/external_grpc_integration.rs:92.
// Stress tests do not assert on audit content — see spec §10.2 (test-only PR,
// no semantic coupling on features2-owned audit semantics).

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

// ── Shutdown pair helper ─────────────────────────────────────────────────────

fn make_test_shutdown_pair() -> (
    Arc<tokio::sync::watch::Sender<bool>>,
    tokio::sync::watch::Receiver<bool>,
) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    (Arc::new(tx), rx)
}

fn in_memory_storage() -> Arc<dyn WebStorage> {
    Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory SQLite")) as Arc<dyn WebStorage>
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
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(16);

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
            // (max_concurrent_streams is `usize` — see oneshim-core
            // crates/oneshim-core/src/config/sections/external_grpc.rs:69.)
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
        load_policy: std::sync::Arc::new(oneshim_web::grpc::LoadPolicy::new(
            oneshim_core::config::LoadThresholds::default(),
        )),
        streaming_enabled: true,
    }
}

// ── Server spawn helper ──────────────────────────────────────────────────────
//
// Mirrors spawn_server in external_grpc_integration.rs:253. Uses the OS-assigned
// port (caller passes bind_addr with port 0). Returns the actual bound port
// observed via TCP probing — not via a port allocator, since serve_external
// rebinds the port itself and we do not have a public accessor for the resolved
// port.
//
// Strategy: caller passes bind_addr with port 0; we replicate the bind ourselves
// to discover the OS-assigned port, drop our listener, then pass the resolved
// addr to serve_external. The window between drop and rebind is tight (same
// process, same thread, no TIME_WAIT because drop() immediately releases the
// port) but NOT theoretically zero. If a flaky "AddrInUse" ever appears in CI,
// retry in a loop or use a free-port allocator (external_grpc_integration.rs:74).

async fn spawn_stress_server(
    mut cfg: ExternalGrpcSpawnConfig,
) -> (tokio::task::JoinHandle<()>, SocketAddr) {
    install_rustls_crypto_provider();

    // Bind once locally to discover the OS-assigned port for the requested
    // family (v4 vs v6), then close and let serve_external rebind it.
    let std_listener =
        std::net::TcpListener::bind(cfg.bind_addr).expect("std bind for port discovery");
    let bound = std_listener.local_addr().expect("local_addr");
    drop(std_listener);
    cfg.bind_addr = bound;

    let probe_addr = bound;
    let handle = tokio::spawn(async move {
        if let Err(e) = serve_external(cfg).await {
            eprintln!("serve_external error: {e:?}");
        }
    });

    // Wait until the server accepts TCP connections (timeout: 5s).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(probe_addr).await.is_ok() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("external gRPC server did not start at {probe_addr} within 5s");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    (handle, probe_addr)
}

fn server_cert_pem() -> Vec<u8> {
    let (cert_path, _) = test_cert_pair();
    std::fs::read(&cert_path).expect("read server cert PEM")
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
    let jwt_kp = test_jwt_keypair();
    let cfg = make_jwt_stress_config(
        &jwt_kp.pub_pem_path,
        CAP,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    );
    let (handle, addr) = spawn_stress_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "stress-cap",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();

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

    // ── Phase 2: (CAP+1)th attempt rejected ─────────────────────────────────
    //
    // Cap rejection is silent (V2: TCP dropped before TLS). From the client
    // side this manifests as one of:
    //   - Endpoint::connect fails (TLS handshake error / EOF).
    //   - Channel created but first RPC fails with transport error.
    // Either is acceptable; we assert that the over-cap path eventually errors.
    let over_cap_result = async {
        let channel = make_stress_tls_channel(addr, &cert_pem).await?;
        let mut req = tonic::Request::new(SubscribeEventsRequest::default());
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {token}").parse().expect("valid header"),
        );
        DashboardServiceClient::new(channel)
            .subscribe_events(req)
            .await
            .map(|_| ())
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
    }
    .await;
    assert!(
        over_cap_result.is_err(),
        "(CAP+1)th channel must be rejected; got: {over_cap_result:?}"
    );

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
    handle.abort();
    let _ = handle.await;
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

    let jwt_kp = test_jwt_keypair();
    let cfg = make_jwt_stress_config(
        &jwt_kp.pub_pem_path,
        CAP,
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
    );
    let (handle, addr) = spawn_stress_server(cfg).await;

    let token = test_mint_jwt(
        &jwt_kp.enc_key,
        "stress-fd",
        "test-issuer",
        "test-audience",
        3600,
    );
    let cert_pem = server_cert_pem();

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
    handle.abort();
    let _ = handle.await;
}

// ════════════════════════════════════════════════════════════════════════════
// Test 3: ipv6_64_prefix_ban_full_stack
// ════════════════════════════════════════════════════════════════════════════

/// Invariant: IpBan::record_failure (auth_layer / accept_loop on TLS error)
/// + IpBan::is_banned (accept_loop pre-TLS) are wired on the IPv6 path.
/// After ≥5 total failures from [::1] (auth rejections + TLS probe failure
/// from the readiness probe in spawn_stress_server), subsequent connections
/// from [::1] are rejected before TLS handshake (V1 verified accept_loop:77
/// ordering).
///
/// Single-/128 limitation (spec §4.3 known limitations): CI loopback is one
/// ::1/128. All attempts share that /128. The test verifies WIRING
/// (IpBan called on IPv6 accept path), not the /64 prefix logic — which
/// is unit-tested at ip_ban.rs::ipv6_64_prefix_shared_ban.
///
/// Runtime estimate: ~2–5s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ipv6_64_prefix_ban_full_stack() {
    const SMALL_CAP: usize = 32; // small; never the rejecting layer
    let jwt_kp = test_jwt_keypair();
    let cfg = make_jwt_stress_config(
        &jwt_kp.pub_pem_path,
        SMALL_CAP,
        SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0),
    );
    let (handle, addr) = spawn_stress_server(cfg).await;
    assert!(addr.is_ipv6(), "Test 3 must bind IPv6 ::1, got {addr}");

    let bad_token = test_mint_jwt(
        &jwt_kp.enc_key,
        "ban-victim",
        "test-issuer",
        "wrong-audience", // intentional aud mismatch → JWT verifier rejects
        3600,
    );
    let cert_pem = server_cert_pem();

    // ── Phase 1: accumulate ≥ 5 failures from [::1] ─────────────────────────
    //
    // The IpBan THRESHOLDS ladder in ip_ban.rs requires 5 total failures within
    // the sliding window for a 60s ban. `record_failure` is called both by
    // auth_layer (JWT rejection) AND by accept_loop's TLS-handshake-failure path.
    //
    // spawn_stress_server's readiness probe makes a raw TCP connection from [::1]
    // that immediately closes without a TLS handshake, causing accept_loop to
    // call record_failure([::1]) once. This means the IpBan counter may already
    // be at 1 before Phase 1 RPCs begin, so only 4 more auth failures are needed
    // to hit the threshold of 5.
    //
    // Strategy: loop up to MAX_PHASE1_RPC_ATTEMPTS (8, well above threshold) RPC
    // attempts. If make_stress_tls_channel fails (pre-TLS rejection) the ban is
    // already active — break early. This makes the test self-calibrating regardless
    // of how many probe-induced failures the startup probe contributes.
    // MAX_PHASE1_RPC_ATTEMPTS must exceed IpBan threshold (5) by >=3 to
    // tolerate variable probe-induced failure count from spawn_stress_server.
    // See accept_loop.rs:77/114/123 for the record_failure call sites.
    const MAX_PHASE1_RPC_ATTEMPTS: usize = 8;
    let mut rpc_done = 0_usize;
    // Plan §Task 5 had `for attempt in 0..5`; deviated to self-calibrating
    // MAX_PHASE1_RPC_ATTEMPTS=8 because the spawn_stress_server TCP probe
    // contributes record_failure([::1]) via accept_loop.rs:114. See commit
    // 9fb6f488 for deviation rationale.
    for attempt in 0..MAX_PHASE1_RPC_ATTEMPTS {
        let channel = match make_stress_tls_channel(addr, &cert_pem).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "Phase 1 attempt {attempt}: pre-TLS rejection after {rpc_done} RPCs: {e}"
                );
                break;
            }
        };
        let mut req = tonic::Request::new(GetAgentInfoRequest {});
        req.metadata_mut().insert(
            "authorization",
            format!("Bearer {bad_token}").parse().expect("valid header"),
        );
        let err = DashboardServiceClient::new(channel)
            .get_agent_info(req)
            .await
            .expect_err(&format!(
                "attempt {attempt}: bad-aud token must be rejected"
            ));
        assert_eq!(
            err.code(),
            tonic::Code::Unauthenticated,
            "attempt {attempt}: expected Unauthenticated; got {err:?}"
        );
        rpc_done += 1;
    }
    assert!(
        rpc_done > 0,
        "Phase 1: at least one RPC must be served before ban. Zero RPCs implies \
         probe contributed >=5 record_failure calls (unexpected — see \
         accept_loop.rs:77/114/123) OR IpBan was banned from server start (regression)."
    );

    // Brief wait for ip_ban state to commit (record_failure is sync but the
    // channel-shutdown side of the auth-failure path is async).
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ── Phase 2: next attempt rejected before TLS ────────────────────────────
    //
    // accept_loop:77 calls ip_ban.is_banned(remote) immediately after TCP
    // accept. is_banned == true → accept_loop drops the TCP. From the client
    // side, the TLS handshake fails (server closed connection mid-handshake)
    // OR the connect itself fails. Either is acceptable — we assert any
    // error path, not a specific gRPC Code.
    let result = make_stress_tls_channel(addr, &cert_pem).await;
    let banned_path_result: Result<(), Box<dyn std::error::Error + Send + Sync>> = match result {
        Err(e) => Err(Box::new(e)),
        Ok(channel) => {
            // Channel created lazily; the failure may surface on first RPC.
            let mut req = tonic::Request::new(GetAgentInfoRequest {});
            req.metadata_mut().insert(
                "authorization",
                format!("Bearer {bad_token}").parse().expect("valid header"),
            );
            DashboardServiceClient::new(channel)
                .get_agent_info(req)
                .await
                .map(|_| ())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })
        }
    };
    // Regression guard (spec §4.3 / V1): banned_path must fail via transport-level
    // rejection (accept_loop:77 drops TCP) OR an Unavailable RPC — NOT via
    // Unauthenticated (which would indicate accept_loop:77 is a no-op and
    // auth_layer is still being reached, meaning the ban wiring regressed).
    match &banned_path_result {
        Ok(()) => panic!(
            "Phase 2: 6th attempt from ::1 must fail. It succeeded — ban wiring \
             regression or /128 sliding-window expired."
        ),
        Err(e) => {
            let s = format!("{e}");
            assert!(
                !s.contains("Unauthenticated"),
                "Phase 2: got Unauthenticated from ::1 after ban threshold met — \
                 this implies accept_loop.rs:77 is NOT rejecting before TLS; \
                 auth_layer is still being reached. Ban wiring regression. Got: {s}"
            );
        }
    }

    handle.abort();
    let _ = handle.await;
}
