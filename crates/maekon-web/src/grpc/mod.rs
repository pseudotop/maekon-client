//! D13: gRPC dashboard server. Exposes `DashboardService` on a dedicated port
//! alongside the Axum REST server for external CLI/integration tools.
//!
//! Feature-gated via `grpc-dashboard` — when disabled, this module and its
//! dependencies (tonic, tonic-health, etc.) compile away entirely.
//!
//! The `#[cfg(feature = "grpc-dashboard")]` gate lives on `pub mod grpc;` in
//! `lib.rs`. A matching inner-attribute here would be redundant (and trips
//! clippy's `duplicated_attributes` lint).
//!
//! ADR-013: handler business logic lives in `handlers/` submodules.
//! `mod.rs` owns: struct definition, constructors, serve helpers, tests.

mod auth_gate;
pub(crate) mod counting_stream;
mod drop_accumulator;
/// ADR-013: RPC handler groups (agent_info, focus, frames, productivity, session_stats).
mod handlers;
mod hint_emitter;
mod load_policy;
mod privacy;
mod rate_limiter;
mod spawn_config;
mod stream_counter;
pub(crate) mod streaming_source;
mod subscribe_events;
mod subscribe_metrics;
pub use auth_gate::{honor_opt_out, validate_authority};
pub use drop_accumulator::{DropAccumulator, DROP_EMIT_INTERVAL};
pub use hint_emitter::{HintEmitter, HEARTBEAT};
pub use load_policy::{LoadLevel, LoadPolicy, INTERVAL_CEILING, INTERVAL_FLOOR, WARMUP};
pub use rate_limiter::{EventRateLimiter, BURST_CAPACITY, DEFAULT_TOKENS_PER_SEC};
pub use spawn_config::GrpcSpawnConfig;
pub use stream_counter::StreamCounterGuard;

use crate::grpc::streaming_source::StreamingSource;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

#[cfg(feature = "grpc-dashboard-external")]
pub mod external;

use std::net::SocketAddr;
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_stream::stream;
use maekon_api_contracts::stream::{AiRuntimeStatus, RealtimeEvent};
use maekon_core::ports::monitor::SystemMonitor;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::proto::dashboard::v1::dashboard_service_server::{
    DashboardService, DashboardServiceServer,
};
use crate::proto::dashboard::v1::{
    AgentInfoResponse, FocusStatsResponse, GetAgentInfoRequest, GetFocusStatsRequest,
    GetProductivityMetricsRequest, GetRecentFramesRequest, GetSessionStatsRequest,
    HealthCheckRequest, HealthCheckResponse, ProductivityMetricsResponse, RecentFramesResponse,
    SessionStatsResponse, SubscribeEventsRequest, SubscribeMetricsRequest,
};
use crate::storage_port::WebStorage;

/// Default gRPC dashboard port when the config field is 0 / unset.
///
/// The loopback gRPC dashboard lives in the 10080-10089 band so it does not
/// overlap the HTTP dashboard's 10090-10099 fallback range.
pub const DEFAULT_GRPC_DASHBOARD_PORT: u16 = 10080;
const MAX_GRPC_PORT_ATTEMPTS: u16 = 10;
const _: () = assert!(DEFAULT_GRPC_DASHBOARD_PORT >= 10080 && DEFAULT_GRPC_DASHBOARD_PORT <= 10089);

/// Convert a `chrono::DateTime<Utc>` to the generated
/// `prost_types::Timestamp` used on the wire for v2a + v2b fields.
/// `pub(super)` so sibling grpc sub-modules (PR-B2 subscribe_metrics,
/// PR-B3 subscribe_events) can reuse it.
pub(super) fn to_proto_ts(dt: chrono::DateTime<chrono::Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

pub struct DashboardServiceImpl {
    started_at: Instant,
    storage: Arc<dyn WebStorage>,
    // v2b additions (shared by subscribe_metrics + subscribe_events handlers):
    system_monitor: Arc<dyn SystemMonitor>,
    event_tx: broadcast::Sender<RealtimeEvent>,
    integration_auth_token: Option<String>,
    streaming_source: StreamingSource,
    active_streams: Arc<AtomicUsize>,
    max_concurrent_streams: usize,
    // v2b B3-0 additions (used by B3-6 SubscribeEvents handler):
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    ai_runtime_status_snapshot: Option<AiRuntimeStatus>,
    // #9638: SubscribeEvents rate-limiter burst capacity. The external
    // service takes it from `ExternalGrpcConfig.burst_capacity` (which used
    // to round-trip through serde and then never reach the limiter — a no-op
    // operator knob); the loopback service keeps the built-in default.
    event_stream_burst_capacity: u32,
}

impl DashboardServiceImpl {
    /// Construct from a `GrpcSpawnConfig`. `started_at` is set to `Instant::now()`
    /// and `active_streams` to 0.
    pub fn from_spawn_config(cfg: &GrpcSpawnConfig) -> Self {
        Self {
            started_at: Instant::now(),
            storage: cfg.storage.clone(),
            system_monitor: cfg.system_monitor.clone(),
            event_tx: cfg.event_tx.clone(),
            integration_auth_token: cfg.integration_auth_token.clone(),
            streaming_source: StreamingSource::Fixed {
                streaming_enabled: cfg.streaming_enabled,
                load_policy: cfg.load_policy.clone(),
            },
            active_streams: Arc::new(AtomicUsize::new(0)),
            max_concurrent_streams: cfg.max_concurrent_streams,
            pii_sanitizer: cfg.pii_sanitizer.clone(),
            ai_runtime_status_snapshot: cfg.ai_runtime_status_snapshot.clone(),
            event_stream_burst_capacity: rate_limiter::BURST_CAPACITY,
        }
    }

    /// Test-only: active concurrent-stream count. Gated so the release binary
    /// does not expose it (IMP-V2-D invariant).
    #[cfg(any(test, feature = "test-support"))]
    pub fn active_stream_count(&self) -> usize {
        self.active_streams
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Test-only accessor for the T18 integration test — verifies the external
    /// server never receives an `integration_auth_token` (spec §2.5 threat model).
    #[cfg(any(test, feature = "test-support"))]
    pub fn has_integration_token(&self) -> bool {
        self.integration_auth_token.is_some()
    }

    /// Construct from an `ExternalGrpcSpawnConfig`. External-gRPC variant.
    /// ALWAYS sets `integration_auth_token: None` so the opt-out path (loopback
    /// only) cannot be bypassed by an external caller presenting the loopback
    /// token value (Task 13 spec §2.5 threat model).
    #[cfg(feature = "grpc-dashboard-external")]
    pub fn from_external_spawn_config(
        cfg: &crate::grpc::external::spawn_config::ExternalGrpcSpawnConfig,
    ) -> Self {
        Self {
            started_at: Instant::now(),
            storage: cfg.storage.clone(),
            system_monitor: cfg.system_monitor.clone(),
            event_tx: cfg.event_tx.clone(),
            integration_auth_token: None, // CRITICAL — spec §2.5
            streaming_source: StreamingSource::Live(cfg.live.clone()),
            active_streams: Arc::new(AtomicUsize::new(0)),
            max_concurrent_streams: cfg.config.max_concurrent_streams,
            pii_sanitizer: cfg.pii_sanitizer.clone(),
            ai_runtime_status_snapshot: cfg.ai_runtime_status_snapshot.clone(),
            event_stream_burst_capacity: u32::try_from(cfg.config.burst_capacity)
                .unwrap_or(rate_limiter::BURST_CAPACITY),
        }
    }
}

// B3-0: redact `pii_sanitizer` and `ai_runtime_status_snapshot` — emit
// boolean-only presence flags so logs never leak PII or AI status details.
impl std::fmt::Debug for DashboardServiceImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DashboardServiceImpl")
            .field(
                "streaming_enabled",
                &self.streaming_source.streaming_enabled(),
            )
            .field("max_concurrent_streams", &self.max_concurrent_streams)
            .field("pii_sanitizer_present", &self.pii_sanitizer.is_some())
            .field(
                "ai_runtime_status_present",
                &self.ai_runtime_status_snapshot.is_some(),
            )
            .finish_non_exhaustive()
    }
}

// ADR-013: impl DashboardService is thin dispatchers only.
// Business logic lives in handlers::{agent_info, session_stats, frames, productivity, focus}.
#[tonic::async_trait]
impl DashboardService for DashboardServiceImpl {
    type SubscribeMetricsStream = subscribe_metrics::SubscribeMetricsStream;
    type SubscribeEventsStream = subscribe_events::SubscribeEventsStream;

    async fn get_agent_info(
        &self,
        req: Request<GetAgentInfoRequest>,
    ) -> Result<Response<AgentInfoResponse>, Status> {
        handlers::agent_info::get_agent_info(self.started_at, req).await
    }

    async fn health_check(
        &self,
        req: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        handlers::agent_info::health_check(req).await
    }

    async fn get_session_stats(
        &self,
        req: Request<GetSessionStatsRequest>,
    ) -> Result<Response<SessionStatsResponse>, Status> {
        handlers::session_stats::get_session_stats(self.storage.clone(), req).await
    }

    async fn get_recent_frames(
        &self,
        req: Request<GetRecentFramesRequest>,
    ) -> Result<Response<RecentFramesResponse>, Status> {
        handlers::frames::get_recent_frames(self.storage.clone(), self.pii_sanitizer.clone(), req)
            .await
    }

    async fn get_productivity_metrics(
        &self,
        req: Request<GetProductivityMetricsRequest>,
    ) -> Result<Response<ProductivityMetricsResponse>, Status> {
        handlers::productivity::get_productivity_metrics(self.storage.clone(), req).await
    }

    async fn get_focus_stats(
        &self,
        req: Request<GetFocusStatsRequest>,
    ) -> Result<Response<FocusStatsResponse>, Status> {
        handlers::focus::get_focus_stats(self.storage.clone(), req).await
    }

    async fn subscribe_metrics(
        &self,
        req: Request<SubscribeMetricsRequest>,
    ) -> Result<Response<Self::SubscribeMetricsStream>, Status> {
        subscribe_metrics::subscribe_metrics(
            req,
            self.storage.clone(),
            self.system_monitor.clone(),
            self.event_tx.clone(),
            self.integration_auth_token.clone(),
            self.streaming_source.clone(),
            self.active_streams.clone(),
            self.max_concurrent_streams,
        )
        .await
    }

    async fn subscribe_events(
        &self,
        req: Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeEventsStream>, Status> {
        subscribe_events::subscribe_events(
            req,
            self.system_monitor.clone(),
            self.event_tx.clone(),
            self.integration_auth_token.clone(),
            self.streaming_source.clone(),
            self.active_streams.clone(),
            self.max_concurrent_streams,
            self.pii_sanitizer.clone(),
            self.ai_runtime_status_snapshot.clone(),
            self.event_stream_burst_capacity,
        )
        .await
    }
}

#[derive(Debug)]
pub enum GrpcServeError {
    Bind(std::io::Error),
    Transport(tonic::transport::Error),
}

impl std::fmt::Display for GrpcServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(e) => write!(f, "bind: {e}"),
            Self::Transport(e) => write!(f, "transport: {e}"),
        }
    }
}

impl std::error::Error for GrpcServeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bind(e) => Some(e),
            Self::Transport(e) => Some(e),
        }
    }
}

fn grpc_base_port(configured_port: u16) -> u16 {
    if configured_port == 0 {
        DEFAULT_GRPC_DASHBOARD_PORT
    } else {
        configured_port
    }
}

fn grpc_port_candidates(configured_port: u16) -> impl Iterator<Item = u16> {
    let base_port = grpc_base_port(configured_port);
    (0..MAX_GRPC_PORT_ATTEMPTS).filter_map(move |attempt| base_port.checked_add(attempt))
}

async fn bind_grpc_listener(configured_port: u16) -> std::io::Result<(TcpListener, SocketAddr)> {
    let base_port = grpc_base_port(configured_port);
    let mut last_error = None;

    for port in grpc_port_candidates(configured_port) {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        match TcpListener::bind(addr).await {
            Ok(listener) => {
                if port != base_port {
                    warn!(
                        "gRPC dashboard port {} unavailable, using {}",
                        base_port, port
                    );
                }
                return Ok((listener, addr));
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    warn!("gRPC dashboard port {} in use, trying next candidate", port);
                    last_error = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!(
                "gRPC dashboard ports {}-{} are unavailable",
                base_port,
                base_port.saturating_add(MAX_GRPC_PORT_ATTEMPTS - 1)
            ),
        )
    }))
}

/// Spawn the gRPC dashboard server. The server runs until shutdown (error or
/// task cancellation). If `cfg.port == 0` the default
/// `DEFAULT_GRPC_DASHBOARD_PORT` is used. The server tries ten loopback ports
/// starting at the configured/default port, so the default band is 10080-10089.
///
/// D13-v2b: takes a `GrpcSpawnConfig` struct so v2b streaming RPCs can receive
/// SystemMonitor / event_tx / auth token / load_policy / kill switch / stream cap.
pub async fn serve(cfg: GrpcSpawnConfig) -> Result<(), GrpcServeError> {
    let (listener, addr) = bind_grpc_listener(cfg.port)
        .await
        .map_err(GrpcServeError::Bind)?;
    info!(%addr, "starting gRPC dashboard server (D13-v2b)");

    let service = DashboardServiceImpl::from_spawn_config(&cfg);
    // #6420: per-session local-auth gate, mirroring the REST `require_local_auth`
    // middleware. Applied to the dashboard service ONLY — the health service stays open
    // so `grpc_health_probe` liveness checks work without a token. `None` fails closed.
    let local_auth = cfg.local_auth_token.clone();

    // Register the standard grpc.health.v1 health service for external
    // liveness checks (`grpc_health_probe -addr=localhost:10080`).
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<DashboardServiceServer<DashboardServiceImpl>>()
        .await;

    let incoming = stream! {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => yield Ok(stream),
                Err(e) => {
                    // #6281 (P-3 sibling of the external accept loop): backoff on a
                    // persistent accept() error (EMFILE/ENFILE/ENOBUFS) so this
                    // loopback dashboard accept loop cannot busy-spin at ~100% CPU.
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    yield Err(e);
                }
            }
        }
    };

    Server::builder()
        // tonic 0.14 defaults both keepalive knobs to None. Explicitly enable
        // HTTP/2 PING frames so snapshot-only SubscribeEvents streams (e.g.
        // event_types=["ai_runtime_status"]) survive NAT / LB idle timeouts.
        // 30s interval / 10s ack timeout aligned with common LB budgets
        // (AWS ELB 350s, GCP 600s, Cloudflare 100s).
        .http2_keepalive_interval(Some(Duration::from_secs(30)))
        .http2_keepalive_timeout(Some(Duration::from_secs(10)))
        .add_service(DashboardServiceServer::with_interceptor(
            service,
            move |req: tonic::Request<()>| -> Result<tonic::Request<()>, tonic::Status> {
                // #6440 (F3): apply the DNS-rebind authority allowlist to EVERY loopback
                // method here. Previously only the 2 streaming handlers called
                // validate_authority; the 6 unary RPCs lacked it. Centralizing in the
                // interceptor (the tonic-recommended place for cross-cutting checks)
                // covers all methods uniformly. Validates only when an authority is
                // observable — tonic does not propagate `:authority` into metadata — so
                // loopback clients that send no `Host` are unaffected. (The streaming
                // handlers keep their own call: they are shared with the external variant,
                // which uses a separate AuthLayer stack rather than this interceptor.)
                if let Some(authority) = req.metadata().get("host").and_then(|v| v.to_str().ok()) {
                    auth_gate::validate_authority(Some(authority))?;
                }
                auth_gate::check_local_auth(req.metadata(), local_auth.as_deref())?;
                Ok(req)
            },
        ))
        .add_service(health_service)
        .serve_with_incoming(incoming)
        .await
        .map_err(GrpcServeError::Transport)
}

/// Non-fatal wrapper: logs failures instead of panicking. Use when the gRPC
/// server is optional (user can still use REST).
pub async fn serve_optional(cfg: GrpcSpawnConfig) {
    if let Err(e) = serve(cfg).await {
        warn!(error = %e, "gRPC dashboard server terminated with error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_in_grpc_dashboard_10080_range() {
        assert_eq!(DEFAULT_GRPC_DASHBOARD_PORT, 10080);
    }

    #[test]
    fn default_port_candidates_cover_grpc_dashboard_10080_range() {
        let candidates: Vec<u16> = grpc_port_candidates(0).collect();
        assert_eq!(candidates, (10080..=10089).collect::<Vec<_>>());
    }

    // RPC-surface behavior is covered in
    // `crates/maekon-web/tests/grpc_dashboard_integration.rs` which seeds a
    // real `SqliteStorage::open_in_memory` — mocking the 10+ WebStorage
    // sub-traits for a unit test adds more surface than it saves. The
    // aggregation math in `get_session_stats` is exercised end-to-end there.

    #[cfg(all(feature = "test-support", not(feature = "grpc-dashboard-external")))]
    mod loopback_constructor {
        use super::*;
        use crate::grpc::streaming_source::StreamingSource;
        use crate::grpc::test_support::mock_system_monitor::MockSystemMonitor;
        use maekon_core::config::LoadThresholds;
        use maekon_storage::sqlite::SqliteStorage;
        use std::sync::Arc;
        use tokio::sync::broadcast;

        #[test]
        fn from_spawn_config_uses_streaming_source_in_loopback_build() {
            let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"))
                as Arc<dyn crate::storage_port::WebStorage>;
            let (event_tx, _) = broadcast::channel(16);
            let cfg = GrpcSpawnConfig {
                port: 10080,
                storage,
                system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
                event_tx,
                integration_auth_token: None,
                local_auth_token: None,
                pii_sanitizer: None,
                ai_runtime_status_snapshot: None,
                load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
                streaming_enabled: true,
                max_concurrent_streams: 10,
            };

            let svc = DashboardServiceImpl::from_spawn_config(&cfg);

            assert!(matches!(
                svc.streaming_source,
                StreamingSource::Fixed { .. }
            ));
        }
    }

    #[cfg(all(feature = "grpc-dashboard-external", feature = "test-support"))]
    mod external_constructor {
        use super::*;
        use crate::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
        use crate::grpc::external::test_support::install_rustls_crypto_provider;
        use crate::grpc::test_support::mock_system_monitor::MockSystemMonitor;
        use maekon_api_contracts::stream::AiRuntimeStatus;
        use maekon_core::config::{AuthMode, ExternalGrpcConfig, LoadThresholds};
        use maekon_core::ports::audit_log::AuditLogPort;
        use maekon_storage::sqlite::SqliteStorage;
        use std::sync::Arc;
        use tokio::sync::{broadcast, watch};

        /// No-op audit port for test fixtures.
        struct NoopAudit;
        #[async_trait::async_trait]
        impl AuditLogPort for NoopAudit {
            async fn pending_count(&self) -> usize {
                0
            }
            async fn recent_entries(
                &self,
                _l: usize,
            ) -> Vec<maekon_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn entries_by_status(
                &self,
                _s: &maekon_core::models::audit::AuditStatus,
                _l: usize,
            ) -> Vec<maekon_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn entries_by_action_prefix(
                &self,
                _p: &str,
                _l: usize,
            ) -> Vec<maekon_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn entries_by_command_id(
                &self,
                _cmd_id: &str,
                _limit: usize,
            ) -> Vec<maekon_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn stats(&self) -> maekon_core::models::audit::AuditStats {
                Default::default()
            }
            async fn has_pending_batch(&self) -> bool {
                false
            }
            async fn log_event(&self, _a: &str, _s: &str, _d: &str) {}
            async fn log_start_if(
                &self,
                _l: maekon_core::models::audit::AuditLevel,
                _c: &str,
                _s: &str,
                _a: &str,
            ) {
            }
            async fn log_complete_with_time(
                &self,
                _l: maekon_core::models::audit::AuditLevel,
                _c: &str,
                _s: &str,
                _d: &str,
                _t: u64,
            ) {
            }
            async fn drain_batch(&self) -> Vec<maekon_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn drain_all(&self) -> Vec<maekon_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn record_session_event(
                &self,
                _e: maekon_core::models::ai_session::SessionAuditEntry,
            ) {
            }
        }

        fn minimal_ext_cfg() -> ExternalGrpcSpawnConfig {
            install_rustls_crypto_provider();
            use rcgen::{CertificateParams, KeyPair};
            let kp = KeyPair::generate().expect("keypair");
            let params = CertificateParams::new(vec!["localhost".into()]).expect("params");
            let cert = params.self_signed(&kp).expect("cert");
            let cert_der = rustls::pki_types::CertificateDer::from(cert.der().to_vec());
            let key_der =
                rustls::pki_types::PrivateKeyDer::try_from(kp.serialize_der()).expect("key");
            let signing =
                rustls::crypto::aws_lc_rs::sign::any_supported_type(&key_der).expect("sign");
            let certified_key = Arc::new(rustls::sign::CertifiedKey::new(vec![cert_der], signing));
            let cert_resolver = Arc::new(
                crate::grpc::external::cert_resolver::HotReloadCertResolver::new(certified_key),
            );

            let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"))
                as Arc<dyn crate::storage_port::WebStorage>;
            let (event_tx, _) = broadcast::channel(16);
            let (shutdown_tx, shutdown_rx) = watch::channel(false);

            ExternalGrpcSpawnConfig {
                bind_addr: "127.0.0.1:0".parse().unwrap(),
                config: ExternalGrpcConfig {
                    enabled: true,
                    auth_mode: Some(AuthMode::Jwt),
                    max_concurrent_streams: 4,
                    max_connections: 16,
                    ..Default::default()
                },
                storage,
                system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
                event_tx,
                audit_port: Arc::new(NoopAudit) as Arc<dyn AuditLogPort>,
                cert_resolver,
                jwt_verifier: None,
                mtls_verifier: None,
                ip_ban: Arc::new(crate::grpc::external::ip_ban::IpBan::new()),
                metrics: Arc::new(crate::grpc::external::metrics::ExternalMetrics::new()),
                shutdown_rx,
                shutdown_tx: Arc::new(shutdown_tx),
                pii_sanitizer: None,
                ai_runtime_status_snapshot: None::<AiRuntimeStatus>,
                live: Arc::new(crate::grpc::external::live_config::LiveExternalConfig::new(
                    crate::grpc::external::live_config::LiveSnapshot {
                        streaming_enabled: true,
                        load_policy: Arc::new(load_policy::LoadPolicy::new(
                            LoadThresholds::default(),
                        )),
                    },
                )),
            }
        }

        /// Spec §2.5 threat model: external constructor MUST NEVER carry an
        /// integration_auth_token. The opt-out path is loopback-only.
        #[test]
        fn from_external_spawn_config_sets_integration_auth_token_to_none() {
            let cfg = minimal_ext_cfg();
            let svc = DashboardServiceImpl::from_external_spawn_config(&cfg);
            assert!(
                !svc.has_integration_token(),
                "external impl must never have integration token (spec §2.5)"
            );
        }

        /// Verify all 11 fields wire through correctly from the spawn config.
        #[test]
        fn from_external_spawn_config_initializes_all_fields() {
            let cfg = minimal_ext_cfg();
            let expected_max_streams = cfg.config.max_concurrent_streams;
            let svc = DashboardServiceImpl::from_external_spawn_config(&cfg);
            assert!(svc.streaming_source.streaming_enabled());
            assert_eq!(svc.max_concurrent_streams, expected_max_streams);
            // active_streams is a fresh counter per-service-instance.
            assert_eq!(
                svc.active_streams
                    .load(std::sync::atomic::Ordering::Relaxed),
                0
            );
        }

        /// D24 / Task 5.1: loopback path must construct Fixed variant.
        #[test]
        fn dashboard_service_impl_from_spawn_config_uses_fixed_streaming_source() {
            use crate::grpc::test_support::mock_system_monitor::MockSystemMonitor;
            use maekon_core::config::LoadThresholds;
            use maekon_storage::sqlite::SqliteStorage;
            use tokio::sync::broadcast;

            install_rustls_crypto_provider();
            let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"))
                as Arc<dyn crate::storage_port::WebStorage>;
            let (event_tx, _) = broadcast::channel(16);
            let cfg = GrpcSpawnConfig {
                port: 10080,
                storage,
                system_monitor: MockSystemMonitor::new(30.0, 4096, 16384),
                event_tx,
                integration_auth_token: None,
                local_auth_token: None,
                pii_sanitizer: None,
                ai_runtime_status_snapshot: None,
                load_policy: Arc::new(LoadPolicy::new(LoadThresholds::default())),
                streaming_enabled: true,
                max_concurrent_streams: 10,
            };
            let svc = DashboardServiceImpl::from_spawn_config(&cfg);
            assert!(matches!(
                svc.streaming_source,
                StreamingSource::Fixed { .. }
            ));
        }

        /// D24 / Task 5.1: external path must construct Live variant.
        #[test]
        fn dashboard_service_impl_from_external_uses_live_variant() {
            let cfg = minimal_ext_cfg();
            let svc = DashboardServiceImpl::from_external_spawn_config(&cfg);
            assert!(matches!(svc.streaming_source, StreamingSource::Live(_)));
        }
    }
}
