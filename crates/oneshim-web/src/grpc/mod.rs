//! D13: gRPC dashboard server. Exposes `DashboardService` on a dedicated port
//! alongside the Axum REST server for external CLI/integration tools.
//!
//! Feature-gated via `grpc-dashboard` — when disabled, this module and its
//! dependencies (tonic, tonic-health, etc.) compile away entirely.
//!
//! The `#[cfg(feature = "grpc-dashboard")]` gate lives on `pub mod grpc;` in
//! `lib.rs`. A matching inner-attribute here would be redundant (and trips
//! clippy's `duplicated_attributes` lint).

mod auth_gate;
pub(crate) mod counting_stream;
mod drop_accumulator;
mod hint_emitter;
mod load_policy;
mod rate_limiter;
mod spawn_config;
mod stream_counter;
#[cfg(feature = "grpc-dashboard-external")]
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

#[cfg(feature = "grpc-dashboard-external")]
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
use oneshim_api_contracts::stream::{AiRuntimeStatus, RealtimeEvent};
use oneshim_core::ports::monitor::SystemMonitor;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use tracing::{info, warn};

use crate::proto::dashboard::v1::dashboard_service_server::{
    DashboardService, DashboardServiceServer,
};
use crate::proto::dashboard::v1::health_check_response::Status as HealthStatus;
use crate::proto::dashboard::v1::{
    recent_frames_response, AgentInfoResponse, FocusStatsResponse, GetAgentInfoRequest,
    GetFocusStatsRequest, GetProductivityMetricsRequest, GetRecentFramesRequest,
    GetSessionStatsRequest, HealthCheckRequest, HealthCheckResponse, MetricBucket,
    ProductivityMetricsResponse, RecentFramesResponse, SessionStatsResponse,
    SubscribeEventsRequest, SubscribeMetricsRequest,
};
use crate::storage_port::WebStorage;

/// Default gRPC dashboard port when the config field is 0 / unset.
///
/// The loopback gRPC dashboard lives in the 10080-10089 band so it does not
/// overlap the HTTP dashboard's 10090-10099 fallback range.
pub const DEFAULT_GRPC_DASHBOARD_PORT: u16 = 10080;
const MAX_GRPC_PORT_ATTEMPTS: u16 = 10;
const _: () = assert!(DEFAULT_GRPC_DASHBOARD_PORT >= 10080 && DEFAULT_GRPC_DASHBOARD_PORT <= 10089);

/// Default sample size when `GetSessionStatsRequest::limit == 0`.
const DEFAULT_SESSION_STATS_LIMIT: usize = 1000;

/// Default + hard cap for GetRecentFrames.
const DEFAULT_RECENT_FRAMES_LIMIT: u32 = 50;
const MAX_RECENT_FRAMES_LIMIT: u32 = 500;
const DEFAULT_RECENT_FRAMES_SINCE_HOURS: u32 = 1;
const MAX_RECENT_FRAMES_SINCE_HOURS: u32 = 168; // 7 days

/// Default + hard cap for GetProductivityMetrics.
const DEFAULT_METRICS_SINCE_HOURS: u32 = 24;
const MAX_METRICS_SINCE_HOURS: u32 = 168;

/// Default + hard cap for GetFocusStats.
const DEFAULT_FOCUS_DAYS: u32 = 7;
const MAX_FOCUS_DAYS: u32 = 90;

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
    // D24 / Task 5.1: dual-mode streaming config.
    // Under grpc-dashboard-external both raw fields are replaced by StreamingSource.
    // Under plain grpc-dashboard the raw fields are retained for loopback-only builds.
    #[cfg(feature = "grpc-dashboard-external")]
    streaming_source: StreamingSource,
    #[cfg(not(feature = "grpc-dashboard-external"))]
    load_policy: Arc<LoadPolicy>,
    #[cfg(not(feature = "grpc-dashboard-external"))]
    streaming_enabled: bool,
    active_streams: Arc<AtomicUsize>,
    max_concurrent_streams: usize,
    // v2b B3-0 additions (used by B3-6 SubscribeEvents handler):
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    ai_runtime_status_snapshot: Option<AiRuntimeStatus>,
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
            #[cfg(feature = "grpc-dashboard-external")]
            streaming_source: StreamingSource::Fixed {
                streaming_enabled: cfg.streaming_enabled,
                load_policy: cfg.load_policy.clone(),
            },
            #[cfg(not(feature = "grpc-dashboard-external"))]
            load_policy: cfg.load_policy.clone(),
            #[cfg(not(feature = "grpc-dashboard-external"))]
            streaming_enabled: cfg.streaming_enabled,
            active_streams: Arc::new(AtomicUsize::new(0)),
            max_concurrent_streams: cfg.max_concurrent_streams,
            pii_sanitizer: cfg.pii_sanitizer.clone(),
            ai_runtime_status_snapshot: cfg.ai_runtime_status_snapshot.clone(),
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
                // D24 / Task 5.1: read through streaming_source under external feature;
                // fall back to the raw field for plain grpc-dashboard builds.
                &{
                    #[cfg(feature = "grpc-dashboard-external")]
                    {
                        self.streaming_source.streaming_enabled()
                    }
                    #[cfg(not(feature = "grpc-dashboard-external"))]
                    {
                        self.streaming_enabled
                    }
                },
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

#[tonic::async_trait]
impl DashboardService for DashboardServiceImpl {
    type SubscribeMetricsStream = subscribe_metrics::SubscribeMetricsStream;
    type SubscribeEventsStream = subscribe_events::SubscribeEventsStream;

    async fn get_agent_info(
        &self,
        _req: Request<GetAgentInfoRequest>,
    ) -> Result<Response<AgentInfoResponse>, Status> {
        let platform = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "windows") {
            "windows"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else {
            "unknown"
        };

        let build_profile = if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        };

        let response = AgentInfoResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            build_profile: build_profile.to_string(),
            uptime_secs: self.started_at.elapsed().as_secs() as i64,
            platform: platform.to_string(),
        };

        Ok(Response::new(response))
    }

    async fn health_check(
        &self,
        _req: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        // v1: always SERVING once the server is up. Future iters can probe
        // storage / scheduler state for a richer readiness signal.
        let response = HealthCheckResponse {
            status: HealthStatus::Serving as i32,
            message: String::new(),
        };
        Ok(Response::new(response))
    }

    async fn get_session_stats(
        &self,
        req: Request<GetSessionStatsRequest>,
    ) -> Result<Response<SessionStatsResponse>, Status> {
        let limit = match req.into_inner().limit {
            0 => DEFAULT_SESSION_STATS_LIMIT,
            n => n as usize,
        };

        // `list_session_stats` is synchronous + hits SQLite; run under
        // spawn_blocking to avoid starving tokio's async runtime.
        let storage = self.storage.clone();
        let rows = tokio::task::spawn_blocking(move || storage.list_session_stats(limit))
            .await
            .map_err(|e| Status::internal(format!("spawn_blocking join: {e}")))?
            .map_err(|e| Status::internal(format!("list_session_stats: {e}")))?;

        // Aggregate in-memory. For very large result sets this is O(n) and
        // cheap — small structs, no string allocation.
        let total_sessions = rows.len() as u32;
        let mut ended_sessions: u32 = 0;
        let mut total_events: u64 = 0;
        let mut total_frames: u64 = 0;
        let mut total_idle_secs: u64 = 0;
        let mut total_duration_secs: i64 = 0;
        for row in &rows {
            total_events += row.total_events;
            total_frames += row.total_frames;
            total_idle_secs += row.total_idle_secs;
            if let Some(ended_at) = row.ended_at {
                ended_sessions += 1;
                let duration = (ended_at - row.started_at).num_seconds();
                if duration > 0 {
                    total_duration_secs += duration;
                }
            }
        }
        let avg_duration_secs = if ended_sessions > 0 {
            total_duration_secs as f64 / f64::from(ended_sessions)
        } else {
            0.0
        };

        Ok(Response::new(SessionStatsResponse {
            total_sessions,
            ended_sessions,
            avg_duration_secs,
            total_events,
            total_frames,
            total_idle_secs,
        }))
    }

    async fn get_recent_frames(
        &self,
        req: Request<GetRecentFramesRequest>,
    ) -> Result<Response<RecentFramesResponse>, Status> {
        let req = req.into_inner();
        let limit = match req.limit {
            0 => DEFAULT_RECENT_FRAMES_LIMIT,
            n => n.min(MAX_RECENT_FRAMES_LIMIT),
        };
        let since_hours = match req.since_hours {
            0 => DEFAULT_RECENT_FRAMES_SINCE_HOURS,
            n => n.min(MAX_RECENT_FRAMES_SINCE_HOURS),
        };

        let to = chrono::Utc::now();
        let from = to - chrono::Duration::hours(i64::from(since_hours));

        let storage = self.storage.clone();
        let limit_usize = limit as usize;
        let records =
            tokio::task::spawn_blocking(move || storage.get_frames(from, to, limit_usize))
                .await
                .map_err(|e| Status::internal(format!("spawn_blocking join: {e}")))?
                .map_err(|e| Status::internal(format!("get_frames: {e}")))?;

        let frames = records
            .into_iter()
            .map(|r| recent_frames_response::FrameMetadata {
                frame_id: r.id,
                captured_at: r.timestamp,
                trigger_type: r.trigger_type,
                app_name: r.app_name,
                window_title: r.window_title,
                importance: r.importance,
                resolution_w: r.resolution_w,
                resolution_h: r.resolution_h,
            })
            .collect();

        Ok(Response::new(RecentFramesResponse { frames }))
    }

    async fn get_productivity_metrics(
        &self,
        req: Request<GetProductivityMetricsRequest>,
    ) -> Result<Response<ProductivityMetricsResponse>, Status> {
        let since_hours = match req.into_inner().since_hours {
            0 => DEFAULT_METRICS_SINCE_HOURS,
            n => n.min(MAX_METRICS_SINCE_HOURS),
        };
        let from =
            (chrono::Utc::now() - chrono::Duration::hours(i64::from(since_hours))).to_rfc3339();

        let storage = self.storage.clone();
        let records = tokio::task::spawn_blocking(move || storage.list_hourly_metrics_since(&from))
            .await
            .map_err(|e| Status::internal(format!("spawn_blocking join: {e}")))?
            .map_err(|e| Status::internal(format!("list_hourly_metrics_since: {e}")))?;

        let buckets = records
            .into_iter()
            .map(|r| {
                // Parse the RFC3339 hour key ("YYYY-MM-DDTHH:00:00Z") produced
                // by the SQLite metrics aggregation. On parse failure, fall back
                // to None so the bucket is still emitted with an absent start
                // rather than being silently dropped.
                let start = chrono::DateTime::parse_from_rfc3339(&r.hour)
                    .ok()
                    .map(|dt| to_proto_ts(dt.with_timezone(&chrono::Utc)));
                MetricBucket {
                    start,
                    cpu_avg_pct: r.cpu_avg,
                    // HourlyMetricsRecord stores bytes; convert to MB for the wire field.
                    memory_avg_mb: r.memory_avg as f64 / 1_048_576.0,
                    // Keystroke/click counters are not tracked in the hourly
                    // metrics table (v2a); they land in v2b streaming buckets.
                    active_keystrokes: 0,
                    active_mouse_clicks: 0,
                }
            })
            .collect();

        Ok(Response::new(ProductivityMetricsResponse { buckets }))
    }

    async fn get_focus_stats(
        &self,
        req: Request<GetFocusStatsRequest>,
    ) -> Result<Response<FocusStatsResponse>, Status> {
        let days = match req.into_inner().days {
            0 => DEFAULT_FOCUS_DAYS,
            n => n.min(MAX_FOCUS_DAYS),
        };

        let storage = self.storage.clone();
        let days_usize = days as usize;
        let records =
            tokio::task::spawn_blocking(move || storage.get_recent_focus_metrics(days_usize))
                .await
                .map_err(|e| Status::internal(format!("spawn_blocking join: {e}")))?
                .map_err(|e| Status::internal(format!("get_recent_focus_metrics: {e}")))?;

        let bucket_count = records.len() as u32;
        let mut total_active_secs: u64 = 0;
        let mut total_deep_work_secs: u64 = 0;
        let mut total_communication_secs: u64 = 0;
        let mut total_interruptions: u32 = 0;
        let mut focus_score_sum: f32 = 0.0;
        let mut longest_focus_secs: u64 = 0;
        for (_date, m) in &records {
            total_active_secs += m.total_active_secs;
            total_deep_work_secs += m.deep_work_secs;
            total_communication_secs += m.communication_secs;
            total_interruptions = total_interruptions.saturating_add(m.interruption_count);
            focus_score_sum += m.focus_score;
            if m.max_focus_duration_secs > longest_focus_secs {
                longest_focus_secs = m.max_focus_duration_secs;
            }
        }
        let avg_focus_score = if bucket_count > 0 {
            focus_score_sum / bucket_count as f32
        } else {
            0.0
        };

        Ok(Response::new(FocusStatsResponse {
            bucket_count,
            total_active_secs,
            total_deep_work_secs,
            total_communication_secs,
            total_interruptions,
            avg_focus_score,
            longest_focus_secs,
        }))
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
            // D24 / Task 5.2: external feature passes StreamingSource for
            // atomic per-call snapshot reads (D21). Loopback keeps pair.
            #[cfg(feature = "grpc-dashboard-external")]
            self.streaming_source.clone(),
            #[cfg(not(feature = "grpc-dashboard-external"))]
            self.load_policy.clone(),
            #[cfg(not(feature = "grpc-dashboard-external"))]
            self.streaming_enabled,
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
            // D24 / Task 5.2: external feature passes StreamingSource for
            // atomic per-call snapshot reads (D21). Loopback keeps pair.
            #[cfg(feature = "grpc-dashboard-external")]
            self.streaming_source.clone(),
            #[cfg(not(feature = "grpc-dashboard-external"))]
            self.load_policy.clone(),
            #[cfg(not(feature = "grpc-dashboard-external"))]
            self.streaming_enabled,
            self.active_streams.clone(),
            self.max_concurrent_streams,
            self.pii_sanitizer.clone(),
            self.ai_runtime_status_snapshot.clone(),
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

    // Register the standard grpc.health.v1 health service for external
    // liveness checks (`grpc_health_probe -addr=localhost:10080`).
    let (health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<DashboardServiceServer<DashboardServiceImpl>>()
        .await;

    let incoming = stream! {
        loop {
            yield listener.accept().await.map(|(stream, _)| stream);
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
        .add_service(DashboardServiceServer::new(service))
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
    // `crates/oneshim-web/tests/grpc_dashboard_integration.rs` which seeds a
    // real `SqliteStorage::open_in_memory` — mocking the 10+ WebStorage
    // sub-traits for a unit test adds more surface than it saves. The
    // aggregation math in `get_session_stats` is exercised end-to-end there.

    #[cfg(all(feature = "grpc-dashboard-external", feature = "test-support"))]
    mod external_constructor {
        use super::*;
        use crate::grpc::external::spawn_config::ExternalGrpcSpawnConfig;
        use crate::grpc::external::test_support::install_rustls_crypto_provider;
        use crate::grpc::test_support::mock_system_monitor::MockSystemMonitor;
        use oneshim_api_contracts::stream::AiRuntimeStatus;
        use oneshim_core::config::{AuthMode, ExternalGrpcConfig, LoadThresholds};
        use oneshim_core::ports::audit_log::AuditLogPort;
        use oneshim_storage::sqlite::SqliteStorage;
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
            ) -> Vec<oneshim_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn entries_by_status(
                &self,
                _s: &oneshim_core::models::audit::AuditStatus,
                _l: usize,
            ) -> Vec<oneshim_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn entries_by_action_prefix(
                &self,
                _p: &str,
                _l: usize,
            ) -> Vec<oneshim_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn entries_by_command_id(
                &self,
                _cmd_id: &str,
                _limit: usize,
            ) -> Vec<oneshim_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn stats(&self) -> oneshim_core::models::audit::AuditStats {
                Default::default()
            }
            async fn has_pending_batch(&self) -> bool {
                false
            }
            async fn log_event(&self, _a: &str, _s: &str, _d: &str) {}
            async fn log_start_if(
                &self,
                _l: oneshim_core::models::audit::AuditLevel,
                _c: &str,
                _s: &str,
                _a: &str,
            ) {
            }
            async fn log_complete_with_time(
                &self,
                _l: oneshim_core::models::audit::AuditLevel,
                _c: &str,
                _s: &str,
                _d: &str,
                _t: u64,
            ) {
            }
            async fn drain_batch(&self) -> Vec<oneshim_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn drain_all(&self) -> Vec<oneshim_core::models::audit::AuditEntry> {
                vec![]
            }
            async fn record_session_event(
                &self,
                _e: oneshim_core::models::ai_session::SessionAuditEntry,
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
            use oneshim_core::config::LoadThresholds;
            use oneshim_storage::sqlite::SqliteStorage;
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
