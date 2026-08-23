// Shared helpers for the `external_grpc_integration.rs` scenario-family split
// (#7730). Included via `#[path = "external_grpc/common.rs"] mod common;`
// from the crate-root test binary — cargo does NOT auto-discover this file
// as its own test target (mirrors the existing `tests/support/` pattern
// already used by `grpc_dashboard_integration.rs`).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use maekon_core::config::{AuthMode, ExternalGrpcConfig, JwtAlgorithm};
use maekon_core::ports::audit_log::AuditLogPort;
use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};
use tonic::Code;

use maekon_web::grpc::external::cert_resolver::HotReloadCertResolver;
use maekon_web::grpc::external::ip_ban::IpBan;
use maekon_web::grpc::external::jwt_verifier::JwtVerifier;
use maekon_web::grpc::external::live_config::{LiveExternalConfig, LiveSnapshot};
use maekon_web::grpc::external::metrics::ExternalMetrics;
use maekon_web::grpc::external::mtls_verifier::MtlsVerifier;
use maekon_web::grpc::external::serve_external;
use maekon_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
use maekon_web::grpc::external::test_support::{
    install_rustls_crypto_provider, spawn_server_with_config_manager, test_cert_pair,
    test_jwt_keypair, NoopAudit, TestJwt,
};
use maekon_web::grpc::external::tls_config::load_certified_key;
use maekon_web::grpc::test_support::mock_system_monitor::MockSystemMonitor;
use maekon_web::grpc::LoadPolicy;
use maekon_web::proto::dashboard::v1::dashboard_service_client::DashboardServiceClient;
use maekon_web::proto::dashboard::v1::GetAgentInfoRequest;

// #7730: `in_memory_storage` lives under `tests/support/` (adapter-crate
// boundary — see `tests/support/in_memory_storage.rs`), not `test_support`.
// Re-exported here so `crate::common::in_memory_storage` works the same as
// every other shared helper in this module for the sibling scenario-family
// files.
pub(crate) use crate::in_memory_storage_support::in_memory_storage;

// ── Shutdown pair helper ─────────────────────────────────────────────────────

/// Create a fresh `(shutdown_tx, shutdown_rx)` pair for one server instance.
///
/// Each test server needs its own pair so signals don't cross test boundaries.
/// The returned `Arc<Sender<bool>>` must be kept alive (or explicitly dropped)
/// to control when the watcher / expiry tasks exit.
pub(crate) fn make_test_shutdown_pair() -> (
    Arc<tokio::sync::watch::Sender<bool>>,
    tokio::sync::watch::Receiver<bool>,
) {
    let (tx, rx) = tokio::sync::watch::channel(false);
    (Arc::new(tx), rx)
}

// ── Port allocator ───────────────────────────────────────────────────────────

/// Lowest port this allocator will hand out. Below Linux's default
/// `net.ipv4.ip_local_port_range` floor of 32768 and macOS's 49152, so the
/// kernel never assigns one of these to an outgoing connection.
const PORT_FLOOR: u16 = 20_000;
/// Ports reserved per process. Tests consume one each; ~40 exist today.
const PORT_WINDOW: u16 = 200;
/// Number of disjoint windows between `PORT_FLOOR` and 32000.
const PORT_SLOTS: u16 = 60;

/// The first port of this process's window.
///
/// Two collision sources were observed on CI (`EADDRINUSE` on 44218 and 44220,
/// both `serve_external` binds timing out after 5s) and both are addressed here.
///
/// The old base, 44200, sat **inside** Linux's ephemeral range — the previous
/// comment said so and treated it as tolerable. It is not: between this
/// allocator's probe bind and the server's real bind, the kernel is free to
/// hand that exact port to an outgoing connection from any process on the box.
///
/// The counter is also per-process, so two CI jobs sharing a self-hosted runner
/// both started at 44200 and walked into each other from the first test. Keying
/// the window off the pid gives concurrent jobs disjoint ranges.
fn port_window_base() -> u16 {
    let slot = (std::process::id() as u16) % PORT_SLOTS;
    PORT_FLOOR + slot * PORT_WINDOW
}

/// Offset within this process's window. Kept as an offset rather than an
/// absolute port so the modulo below keeps allocation inside the window
/// without a read-modify-write race between the wrap and the next handout.
static NEXT_OFFSET: AtomicU16 = AtomicU16::new(0);

/// Acquire one test port, verified free at the moment of allocation.
///
/// **This does not reserve the port.** The probe listener is dropped before the
/// caller's server binds, so a race remains open in that gap; it is now narrow
/// and no longer fed by the two mechanisms above. Closing it entirely means
/// binding `:0` and handing the live `TcpListener` to `serve_external` via
/// `serve_with_incoming`, which is a production-API change and deliberately not
/// bundled with this test-only fix.
pub(crate) fn next_test_port() -> u16 {
    let base = port_window_base();
    loop {
        let offset = NEXT_OFFSET.fetch_add(1, Ordering::Relaxed) % PORT_WINDOW;
        let port = base + offset;
        // Verify the port is free by binding a std listener momentarily.
        if std::net::TcpListener::bind(format!("127.0.0.1:{port}")).is_ok() {
            return port;
        }
        // Port in use; try the next one.
    }
}

/// Build an `ExternalGrpcSpawnConfig` for JWT-only mode.
///
/// Returns `(config, port)` where `port` is 0 (OS-assigned).
pub(crate) fn make_jwt_config(
    jwt_pub_key_path: &std::path::Path,
) -> (ExternalGrpcSpawnConfig, SocketAddr) {
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(128);
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
                maekon_core::config::LoadThresholds::default(),
            )),
        })),
    };
    (cfg, bind_addr)
}

/// Build an `ExternalGrpcSpawnConfig` for mTLS-only mode.
pub(crate) fn make_mtls_config(
    ca_pem_path: &std::path::Path,
) -> (ExternalGrpcSpawnConfig, SocketAddr) {
    let (cert_path, key_path) = test_cert_pair();
    let certified_key = load_certified_key(&cert_path, &key_path).expect("load certified key");
    let cert_resolver = Arc::new(HotReloadCertResolver::new(certified_key));

    let (event_tx, _) = tokio::sync::broadcast::channel(128);
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
                maekon_core::config::LoadThresholds::default(),
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
pub(crate) async fn spawn_server(
    cfg: ExternalGrpcSpawnConfig,
) -> (tokio::task::JoinHandle<()>, u16) {
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
pub(crate) async fn make_tls_channel(
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

// ── Helper: assert RPC reached the authenticated service (got business data) ─

/// Call `GetAgentInfo` and assert auth succeeded (handler returned Ok or a
/// terminal domain error that isn't Unauthenticated / Cancelled). After Task 9
/// wired the real `DashboardServiceImpl`, a successful auth handshake yields
/// an Ok response carrying `AgentInfoResponse` with version + platform.
pub(crate) async fn assert_reaches_service(client: &mut DashboardServiceClient<Channel>) {
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
pub(crate) async fn assert_reaches_service_with_bearer(channel: Channel, token: &str) {
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
pub(crate) async fn send_bad_bearer(channel: Channel, token: &str) -> tonic::Status {
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
pub(crate) fn test_cfg_with_external_enabled(
    jwt_pub_key_path: &std::path::Path,
) -> maekon_core::config::AppConfig {
    let (cert_path, key_path) = test_cert_pair();
    let mut cfg = maekon_core::config::AppConfig::default_config();
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
pub(crate) fn make_jwt_spawn_config_for_reload(
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

    let (event_tx, _) = tokio::sync::broadcast::channel(128);
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
type SeedFn = Box<dyn FnOnce(&mut maekon_core::config::AppConfig)>;

/// Test fixture that owns a running external gRPC server + ConfigReloadTask
/// + their backing ConfigManager and LiveExternalConfig snapshot.
pub(crate) struct LiveReloadHarness {
    /// Real ConfigManager — call `update_with` to mutate config and trigger
    /// the watch-channel reload.
    pub(crate) cfg_mgr: Arc<maekon_core::config_manager::ConfigManager>,
    /// LiveExternalConfig snapshot — read via `live.snapshot()`.
    pub(crate) live: Arc<LiveExternalConfig>,
    // every test does, but the field is public-by-convention so adding new
    // metrics-aware tests doesn't require a struct edit.
    pub(crate) metrics: Arc<ExternalMetrics>,
    /// JWT keypair (held so `enc_key` stays valid for token minting via
    /// `harness.jwt_kp.enc_key`).
    pub(crate) jwt_kp: TestJwt,
    /// The bound port returned by the spawn helper.
    pub(crate) port: u16,
    /// Shutdown sender — clone of the one wired into the spawn config.
    /// Tests that need graceful-exit verification (vs `shutdown()`'s abort
    /// semantics) call `shutdown_tx.send_replace(true)` and then await the
    /// `reload_handle` directly via destructuring.
    pub(crate) shutdown_tx: Arc<tokio::sync::watch::Sender<bool>>,
    pub(crate) server_handle: tokio::task::JoinHandle<()>,
    pub(crate) reload_handle: tokio::task::JoinHandle<()>,
    /// Tempfile holding the on-disk config; drop closes it.
    pub(crate) _tmp: tempfile::NamedTempFile,
}

impl LiveReloadHarness {
    pub(crate) fn builder() -> LiveReloadHarnessBuilder {
        LiveReloadHarnessBuilder::default()
    }

    /// Abort the server + reload task and await them. Idempotent enough for
    /// test teardown — always safe to call once at the end of a test.
    pub(crate) async fn shutdown(self) {
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
    pub(crate) async fn wait_for_streaming(&self, expected: bool, timeout: Duration, msg: &str) {
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
pub(crate) struct LiveReloadHarnessBuilder {
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
    pub(crate) fn seed<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut maekon_core::config::AppConfig) + 'static,
    {
        self.seed = Some(Box::new(f));
        self
    }

    /// Initial `LiveSnapshot.streaming_enabled` value (default: `true`).
    /// Should match the resolved value of the seeded config so the first
    /// request observes a coherent state.
    pub(crate) fn initial_streaming(mut self, on: bool) -> Self {
        self.initial_streaming_enabled = on;
        self
    }

    /// Initial `LiveSnapshot.load_policy` (default: `LoadPolicy::new(default
    /// thresholds)`). Pass a policy built via
    /// `LoadPolicy::try_new_with_started_at(.., past_anchor)` to bypass the
    /// 30-second WARMUP for D27 tests.
    pub(crate) fn initial_load_policy(mut self, p: Arc<LoadPolicy>) -> Self {
        self.initial_load_policy = Some(p);
        self
    }

    pub(crate) async fn build(self) -> LiveReloadHarness {
        let jwt_kp = test_jwt_keypair();
        let pub_key_path = jwt_kp.pub_pem_path.clone();

        let tmp = tempfile::NamedTempFile::new().expect("tempfile create");
        let cfg_mgr = Arc::new(
            maekon_core::config_manager::ConfigManager::with_path(tmp.path().to_path_buf())
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
                maekon_core::config::LoadThresholds::default(),
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
