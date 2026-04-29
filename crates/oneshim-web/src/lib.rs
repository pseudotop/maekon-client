// Cast safety: dashboard metrics, report values — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide. See
// docs/reviews/2026-04-21-p2-missing-const-for-fn-decision.md.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: see decision doc.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening: mutex guards must not be held across I/O or
// long-running work unless intentionally kept for atomicity (use
// function-level #[allow] with reason). See
// docs/reviews/2026-04-21-p2-significant-drop-tightening-spec.md.
// Test code is exempt — mock implementations use intentionally-simple lock
// patterns for clarity over performance.
#![deny(clippy::significant_drop_tightening)]
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! # oneshim-web
//!
//! ## Hexagonal Architecture — ADR-001 §7 (Port Location Rules)
//!
//! ### Violation 1 — `oneshim-automation` concrete types in AppState — RESOLVED
//!
//! **Status**: Migration steps 1-6 completed.
//!   - `AuditLogPort` defined in `oneshim-core/src/ports/audit_log.rs`
//!   - `AutomationPort` defined in `oneshim-core/src/ports/automation.rs`
//!   - `GuiInteractionError` moved to `oneshim-core::error`
//!   - `AuditEntry`, `AuditStatus`, `AuditLevel`, `AuditStats` in `oneshim-core::models::audit`
//!   - `AppState` uses `Arc<dyn AuditLogPort>` and `Arc<dyn AutomationPort>`
//!   - `AuditLogAdapter` in `oneshim-automation::audit` bridges `AuditLogger` to the port
//!
//! **Remaining**: `oneshim-automation` moved to `[dev-dependencies]` — only used
//!   for test-only `AutomationController` construction in `automation_gui::tests`.
//!
//! ### Violation 2 — `oneshim-storage` concrete types — RESOLVED
//!
//! **Status**: All 4 migration steps completed.
//!   - 14 row types promoted to `oneshim-core::models::storage_records`
//!   - `WebStorage` trait moved to `oneshim-core/src/ports/web_storage.rs`
//!   - `impl WebStorage for SqliteStorage` moved to `oneshim-storage::sqlite::web_storage_impl`
//!   - `oneshim-storage` moved to `[dev-dependencies]` (test-only `SqliteStorage::open_in_memory`)

pub mod app_state;
pub mod embedded;
pub mod error;
#[cfg(feature = "grpc-dashboard")]
pub mod grpc;
pub mod handlers;
#[cfg(feature = "grpc-dashboard")]
pub mod proto;
pub mod routes;
pub mod runtime_bindings;
pub mod services;
pub mod storage_port;
pub mod update_control;

pub use app_state::*;

use crate::storage_port::WebStorage;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Router;
use oneshim_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use oneshim_core::config::{CredentialBackendKind, WebConfig};
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_core::ports::automation::AutomationPort;
use oneshim_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationSessionPort,
};
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use oneshim_core::ports::runtime_log_provider::RuntimeLogProvider;
use oneshim_core::ports::secret_store::{SecretStore, SecretStoreSet};
use oneshim_core::ports::system_info_provider::SystemInfoProvider;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, oneshot, watch};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::debug;
use tracing::{error, info, warn};

pub use oneshim_api_contracts::stream::{
    AiRuntimeStatus, FrameUpdate, IdleUpdate, MetricsUpdate, RealtimeEvent,
};

pub use oneshim_core::config::WebConfig as CoreWebConfig;
pub use runtime_bindings::{
    AnalysisRuntimeBindings, AutomationRuntimeBindings, CoreRuntimeBindings,
    IntegrationRuntimeBindings, SecretRuntimeBindings, SessionRuntimeBindings,
    WebServerRuntimeBindings,
};

const EVENT_CHANNEL_CAPACITY: usize = 256;

const MAX_PORT_ATTEMPTS: u16 = 10;
const INTEGRATION_TOKEN_HEADER: &str = "x-oneshim-integration-token";

pub struct WebServer {
    config: WebConfig,
    state: AppState,
    bound_port_state: Option<Arc<AtomicU16>>,
    bound_port_notifier: Option<oneshot::Sender<u16>>,
}

impl WebServer {
    pub fn new(storage: Arc<dyn WebStorage>, config: WebConfig) -> Self {
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            config,
            state: AppState::with_core(storage, event_tx),
            bound_port_state: None,
            bound_port_notifier: None,
        }
    }

    pub fn with_update_control(mut self, control: update_control::UpdateControl) -> Self {
        self.state.core.update_control = Some(control);
        self
    }

    pub fn with_config_manager(mut self, config_manager: ConfigManager) -> Self {
        self.state.core.config_manager = Some(config_manager);
        self
    }

    pub fn with_default_secret_backend_kind(
        mut self,
        default_secret_backend_kind: CredentialBackendKind,
    ) -> Self {
        self.state.secrets.default_backend_kind = default_secret_backend_kind;
        self
    }

    pub fn with_secret_store(mut self, secret_store: Arc<dyn SecretStore>) -> Self {
        self.state.secrets.store = Some(secret_store);
        self
    }

    pub fn with_secret_stores(mut self, secret_stores: SecretStoreSet) -> Self {
        self.state.secrets.stores = Some(secret_stores);
        self
    }

    pub fn with_audit_logger(mut self, logger: Arc<dyn AuditLogPort>) -> Self {
        self.state.automation.audit_logger = Some(logger);
        self
    }

    pub fn with_automation_controller(mut self, controller: Arc<dyn AutomationPort>) -> Self {
        self.state.automation.controller = Some(controller);
        self
    }

    pub fn with_ai_runtime_status(mut self, status: AiRuntimeStatus) -> Self {
        self.state.automation.ai_runtime_status = Some(status);
        self
    }

    pub fn with_pii_sanitizer(mut self, sanitizer: Arc<dyn PiiSanitizer>) -> Self {
        self.state.diagnostics.pii_sanitizer = Some(sanitizer);
        self
    }

    pub fn with_runtime_log_provider(mut self, provider: Arc<dyn RuntimeLogProvider>) -> Self {
        self.state.diagnostics.runtime_log_provider = Some(provider);
        self
    }

    pub fn with_system_info_provider(mut self, provider: Arc<dyn SystemInfoProvider>) -> Self {
        self.state.diagnostics.system_info_provider = Some(provider);
        self
    }

    /// Wire the `LiveExternalConfig` Arc into `DiagnosticsState` so the
    /// `GET /api/external-grpc/live-config` endpoint can serve live snapshots.
    /// Only available when the `grpc-dashboard-external` feature is enabled.
    #[cfg(feature = "grpc-dashboard-external")]
    pub fn with_external_grpc_live(
        mut self,
        live: Arc<crate::grpc::external::live_config::LiveExternalConfig>,
    ) -> Self {
        self.state.diagnostics.external_grpc_live = Some(live);
        self
    }

    /// Wire the `ExternalMetrics` Arc into `DiagnosticsState` so the
    /// `GET /api/external-grpc/live-config` endpoint can report
    /// `config_reload_task_alive`.
    /// Only available when the `grpc-dashboard-external` feature is enabled.
    #[cfg(feature = "grpc-dashboard-external")]
    pub fn with_external_grpc_metrics(
        mut self,
        metrics: Arc<crate::grpc::external::metrics::ExternalMetrics>,
    ) -> Self {
        self.state.diagnostics.external_grpc_metrics = Some(metrics);
        self
    }

    pub fn with_integration_runtime_status(
        mut self,
        status: IntegrationOutboundRuntimeStatus,
    ) -> Self {
        self.state.integration.runtime_status = Some(status);
        self
    }

    pub fn with_integration_auth(mut self, auth: Arc<dyn IntegrationAuthPort>) -> Self {
        self.state.integration.auth = Some(auth);
        self
    }

    pub fn with_integration_session(mut self, session: Arc<dyn IntegrationSessionPort>) -> Self {
        self.state.integration.session = Some(session);
        self
    }

    pub fn with_integration_outbox(mut self, outbox: Arc<dyn IntegrationOutboxPort>) -> Self {
        self.state.integration.outbox = Some(outbox);
        self
    }

    pub fn with_integration_inbox(mut self, inbox: Arc<dyn IntegrationInboxPort>) -> Self {
        self.state.integration.inbox = Some(inbox);
        self
    }

    pub fn with_integration_inbox_store(
        mut self,
        inbox_store: Arc<dyn IntegrationInboxStorePort>,
    ) -> Self {
        self.state.integration.inbox_store = Some(inbox_store);
        self
    }

    pub fn with_integration_audit(mut self, audit: Arc<dyn IntegrationAuditPort>) -> Self {
        self.state.integration.audit = Some(audit);
        self
    }

    pub fn event_sender(&self) -> broadcast::Sender<RealtimeEvent> {
        self.state.core.event_tx.clone()
    }

    pub fn with_event_tx(mut self, event_tx: broadcast::Sender<RealtimeEvent>) -> Self {
        self.state.core.event_tx = event_tx;
        self
    }

    pub fn with_frames_dir(mut self, dir: std::path::PathBuf) -> Self {
        self.state.core.frames_dir = Some(dir);
        self
    }

    pub fn with_bound_port_state(mut self, bound_port_state: Arc<AtomicU16>) -> Self {
        self.bound_port_state = Some(bound_port_state);
        self
    }

    pub fn with_bound_port_notifier(mut self, bound_port_notifier: oneshot::Sender<u16>) -> Self {
        self.bound_port_notifier = Some(bound_port_notifier);
        self
    }

    pub fn with_runtime_bindings(mut self, bindings: WebServerRuntimeBindings) -> Self {
        let WebServerRuntimeBindings {
            core,
            secrets,
            automation,
            integration,
            analysis,
            session,
        } = bindings;

        if let Some(event_tx) = core.event_tx {
            self.state.core.event_tx = event_tx;
        }
        if let Some(frames_dir) = core.frames_dir {
            self.state.core.frames_dir = Some(frames_dir);
        }
        if let Some(frame_storage) = core.frame_storage {
            self.state.core.frame_storage = Some(frame_storage);
        }
        if let Some(config_manager) = core.config_manager {
            self.state.core.config_manager = Some(config_manager);
        }
        if let Some(update_control) = core.update_control {
            self.state.core.update_control = Some(update_control);
        }

        if let Some(default_secret_backend_kind) = secrets.default_secret_backend_kind {
            self.state.secrets.default_backend_kind = default_secret_backend_kind;
        }
        if let Some(secret_store) = secrets.secret_store {
            self.state.secrets.store = Some(secret_store);
        }
        if let Some(secret_stores) = secrets.secret_stores {
            self.state.secrets.stores = Some(secret_stores);
        }

        if let Some(audit_logger) = automation.audit_logger {
            self.state.automation.audit_logger = Some(audit_logger);
        }
        if let Some(automation_controller) = automation.automation_controller {
            self.state.automation.controller = Some(automation_controller);
        }
        if let Some(ai_runtime_status) = automation.ai_runtime_status {
            self.state.automation.ai_runtime_status = Some(ai_runtime_status);
        }

        if let Some(integration_runtime_status) = integration.integration_runtime_status {
            self.state.integration.runtime_status = Some(integration_runtime_status);
        }
        if let Some(integration_auth) = integration.integration_auth {
            self.state.integration.auth = Some(integration_auth);
        }
        if let Some(integration_session) = integration.integration_session {
            self.state.integration.session = Some(integration_session);
        }
        if let Some(integration_outbox) = integration.integration_outbox {
            self.state.integration.outbox = Some(integration_outbox);
        }
        if let Some(integration_inbox) = integration.integration_inbox {
            self.state.integration.inbox = Some(integration_inbox);
        }
        if let Some(integration_inbox_store) = integration.integration_inbox_store {
            self.state.integration.inbox_store = Some(integration_inbox_store);
        }
        if let Some(integration_audit) = integration.integration_audit {
            self.state.integration.audit = Some(integration_audit);
        }
        if let Some(integration_runtime_telemetry) = integration.integration_runtime_telemetry {
            self.state.integration.runtime_telemetry = Some(integration_runtime_telemetry);
        }

        if let Some(override_store) = analysis.override_store {
            self.state.analysis.override_store = Some(override_store);
        }
        if let Some(recluster_requested) = analysis.recluster_requested {
            self.state.analysis.recluster_requested = Some(recluster_requested);
        }
        if let Some(coaching_engine) = analysis.coaching_engine {
            self.state.analysis.coaching_engine = Some(coaching_engine);
        }
        if let Some(session_manager) = session.session_manager {
            self.state.session.manager = Some(session_manager);
        }
        self
    }

    /// TCP 바인딩 없이 Router만 반환 — Tauri 커스텀 프로토콜 등에서 사용
    pub fn build_router(state: AppState) -> Router {
        use axum::http::HeaderValue;
        use tower_http::cors::AllowOrigin;

        // localhost origin만 허용 (tauri:// + http://127.0.0.1:{port range})
        let allowed_origins: Vec<HeaderValue> = (10090..=10099)
            .flat_map(|port| {
                [
                    format!("http://127.0.0.1:{port}").parse().ok(),
                    format!("http://localhost:{port}").parse().ok(),
                ]
                .into_iter()
                .flatten()
            })
            .chain(std::iter::once(
                "tauri://localhost".parse().expect("static URL"),
            ))
            // Vite dev server for cargo tauri dev
            .chain(std::iter::once(
                "http://127.0.0.1:5273".parse().expect("static URL"),
            ))
            .collect();

        let cors = CorsLayer::new()
            .allow_origin(AllowOrigin::list(allowed_origins))
            .allow_methods(Any)
            .allow_headers(Any);

        let internal_api =
            routes::api_routes().route_layer(middleware::from_fn(require_loopback_client));
        let integration_api = routes::integration_routes().route_layer(
            middleware::from_fn_with_state(state.clone(), require_integration_auth),
        );

        Router::new()
            .nest("/api", internal_api)
            .nest("/integration/v1", integration_api)
            .fallback(loopback_only_static)
            .layer(CompressionLayer::new())
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .with_state(state)
    }

    pub async fn run(self, mut shutdown_rx: watch::Receiver<bool>) -> Result<(), std::io::Error> {
        let Self {
            config,
            state,
            bound_port_state,
            mut bound_port_notifier,
        } = self;

        let integration_auth_configured = config
            .integration_auth_token
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty());
        let host = if config.allow_external && integration_auth_configured {
            "0.0.0.0"
        } else {
            if config.allow_external && !integration_auth_configured {
                warn!(
                    "External access requested but web.integration_auth_token is not configured; falling back to loopback-only binding"
                );
            }
            "127.0.0.1"
        };

        let app = Self::build_router(state);

        let base_port = config.port;
        let mut last_error = None;

        for attempt in 0..MAX_PORT_ATTEMPTS {
            let port = base_port.saturating_add(attempt);

            if port < base_port && attempt > 0 {
                break;
            }

            let addr: SocketAddr = match format!("{}:{}", host, port).parse() {
                Ok(a) => a,
                Err(e) => {
                    error!("{}:{} - {}", host, port, e);
                    continue; // next port attempt
                }
            };

            match TcpListener::bind(addr).await {
                Ok(listener) => {
                    if attempt > 0 {
                        warn!("port {} not-available, port {}", base_port, port);
                    }
                    if let Some(shared_port) = &bound_port_state {
                        shared_port.store(port, Ordering::Relaxed);
                    }
                    if let Some(port_tx) = bound_port_notifier.take() {
                        if let Err(e) = port_tx.send(port) {
                            debug!("channel send failed: {e}");
                        }
                    }
                    info!("server started: http://{}", addr);

                    axum::serve(
                        listener,
                        app.into_make_service_with_connect_info::<SocketAddr>(),
                    )
                    .with_graceful_shutdown(async move {
                        loop {
                            if *shutdown_rx.borrow() {
                                info!("server ended received");
                                break;
                            }
                            if shutdown_rx.changed().await.is_err() {
                                break;
                            }
                        }
                    })
                    .await?;

                    info!("server ended");
                    return Ok(());
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::AddrInUse {
                        warn!("port {} in progress, next port attempt...", port);
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
                    "port {}-{} 모두 사용 not-available",
                    base_port,
                    base_port.saturating_add(MAX_PORT_ATTEMPTS - 1)
                ),
            )
        }))
    }

    pub fn url(&self) -> String {
        let port = self
            .bound_port_state
            .as_ref()
            .map(|shared_port| shared_port.load(Ordering::Relaxed))
            .unwrap_or(self.config.port);
        format!("http://localhost:{port}")
    }
}

async fn require_loopback_client(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if addr.ip().is_loopback() {
        return next.run(request).await;
    }

    crate::error::ApiError::Forbidden(
        "The internal /api surface is available only from loopback clients.".to_string(),
    )
    .into_response()
}

async fn require_integration_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(config_manager) = state.core.config_manager.as_ref() else {
        return crate::error::ApiError::ServiceUnavailable(
            "Integration API is unavailable because config management is not initialized."
                .to_string(),
        )
        .into_response();
    };

    let expected_token = config_manager
        .get()
        .web
        .integration_auth_token
        .unwrap_or_default()
        .trim()
        .to_string();

    if expected_token.is_empty() {
        return crate::error::ApiError::ServiceUnavailable(
            "Integration API is not configured. Set web.integration_auth_token in config.json before using external access."
                .to_string(),
        )
        .into_response();
    }

    let header_token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            request
                .headers()
                .get(INTEGRATION_TOKEN_HEADER)
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        });

    if header_token.as_deref() != Some(expected_token.as_str()) {
        return crate::error::ApiError::Unauthorized(
            "Integration API requires a valid bearer token.".to_string(),
        )
        .into_response();
    }

    next.run(request).await
}

async fn loopback_only_static(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    uri: axum::http::Uri,
) -> Response {
    if addr.ip().is_loopback() {
        return embedded::serve_static(uri).await;
    }

    crate::error::ApiError::Forbidden(
        "The embedded dashboard is available only from loopback clients.".to_string(),
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};
    use oneshim_core::config::AppConfig;
    use oneshim_core::config_manager::ConfigManager;
    use oneshim_storage::sqlite::SqliteStorage;
    use tempfile::tempdir;
    use tower::ServiceExt;

    #[test]
    fn default_config() {
        let config = WebConfig::default();
        assert_eq!(config.port, oneshim_core::config::DEFAULT_WEB_PORT);
        assert!(!config.allow_external);
    }

    #[test]
    fn web_server_url() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let server = WebServer::new(storage, WebConfig::default());
        let expected = format!(
            "http://localhost:{}",
            oneshim_core::config::DEFAULT_WEB_PORT
        );
        assert_eq!(server.url(), expected);
    }

    #[test]
    fn web_server_url_prefers_bound_port_state() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let bound_port_state = Arc::new(AtomicU16::new(11091));
        let server =
            WebServer::new(storage, WebConfig::default()).with_bound_port_state(bound_port_state);

        assert_eq!(server.url(), "http://localhost:11091");
    }

    #[test]
    fn web_server_runtime_bindings_apply_scalar_runtime_state() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        let ai_runtime_status = AiRuntimeStatus {
            ocr_source: "remote".to_string(),
            llm_source: "subprocess_cli".to_string(),
            ocr_fallback_reason: None,
            llm_fallback_reason: None,
        };
        let integration_runtime_status = IntegrationOutboundRuntimeStatus {
            enabled: true,
            runtime_telemetry: None,
            ..IntegrationOutboundRuntimeStatus::default()
        };
        let frames_dir = std::path::PathBuf::from("/tmp/oneshim-web-runtime-bindings");

        let server = WebServer::new(storage, WebConfig::default()).with_runtime_bindings(
            WebServerRuntimeBindings {
                core: CoreRuntimeBindings {
                    event_tx: Some(event_tx.clone()),
                    frames_dir: Some(frames_dir.clone()),
                    ..Default::default()
                },
                secrets: SecretRuntimeBindings {
                    default_secret_backend_kind: Some(CredentialBackendKind::Env),
                    ..Default::default()
                },
                automation: AutomationRuntimeBindings {
                    ai_runtime_status: Some(ai_runtime_status.clone()),
                    ..Default::default()
                },
                integration: IntegrationRuntimeBindings {
                    integration_runtime_status: Some(integration_runtime_status.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
        );

        assert_eq!(server.state.core.event_tx.receiver_count(), 0);
        assert_eq!(server.state.core.frames_dir.as_ref(), Some(&frames_dir));
        assert_eq!(
            server.state.secrets.default_backend_kind,
            CredentialBackendKind::Env
        );
        let applied_ai_runtime_status = server.state.automation.ai_runtime_status.as_ref().unwrap();
        assert_eq!(
            applied_ai_runtime_status.ocr_source,
            ai_runtime_status.ocr_source
        );
        assert_eq!(
            applied_ai_runtime_status.llm_source,
            ai_runtime_status.llm_source
        );
        let applied_integration_runtime_status =
            server.state.integration.runtime_status.as_ref().unwrap();
        assert_eq!(
            applied_integration_runtime_status.enabled,
            integration_runtime_status.enabled
        );
    }

    #[tokio::test]
    async fn web_server_fallback_updates_bound_port_state() {
        // Bind port 0 to let the OS assign a free port, then use that port as the
        // "occupied" port. This avoids the flaky AddrInUse panic when another test
        // concurrently holds DEFAULT_WEB_PORT.
        let reserved_listener = TcpListener::bind(("127.0.0.1", 0u16)).await.unwrap();
        let occupied_port = reserved_listener.local_addr().unwrap().port();

        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let config = WebConfig {
            port: occupied_port,
            ..Default::default()
        };
        let bound_port_state = Arc::new(AtomicU16::new(config.port));
        let (bound_port_tx, bound_port_rx) = oneshot::channel();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server = WebServer::new(storage, config)
            .with_bound_port_state(bound_port_state.clone())
            .with_bound_port_notifier(bound_port_tx);

        let server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

        let fallback_port = tokio::time::timeout(std::time::Duration::from_secs(3), bound_port_rx)
            .await
            .unwrap()
            .unwrap();

        assert_ne!(fallback_port, occupied_port);
        assert_eq!(bound_port_state.load(Ordering::Relaxed), fallback_port);

        let _ = shutdown_tx.send(true);
        let server_result = tokio::time::timeout(std::time::Duration::from_secs(3), server_handle)
            .await
            .unwrap()
            .unwrap();

        assert!(server_result.is_ok());
        drop(reserved_listener);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn max_port_attempts_is_reasonable() {
        assert!(MAX_PORT_ATTEMPTS >= 1);
        assert!(MAX_PORT_ATTEMPTS <= 100);
    }

    #[test]
    fn port_overflow_protection() {
        let base_port: u16 = 65530;
        for attempt in 0..MAX_PORT_ATTEMPTS {
            let port = base_port.saturating_add(attempt);
            assert!(port >= base_port || port == u16::MAX);
        }
    }

    fn test_state_with_config_manager(config_manager: Option<ConfigManager>) -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        let mut state = AppState::with_core(storage, event_tx);
        state.core.config_manager = config_manager;
        state
    }

    fn config_manager_with_integration_token(token: &str) -> ConfigManager {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let manager = ConfigManager::with_path(config_path).unwrap();
        let mut config = AppConfig::default_config();
        config.web.integration_auth_token = Some(token.to_string());
        manager.update(config).unwrap();
        manager
    }

    #[tokio::test]
    async fn internal_api_rejects_non_loopback_clients() {
        let app = WebServer::build_router(test_state_with_config_manager(None)).layer(
            MockConnectInfo(SocketAddr::from(([192, 168, 0, 10], 43000))),
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ai/provider-surfaces")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn integration_api_requires_matching_token() {
        let app = WebServer::build_router(test_state_with_config_manager(Some(
            config_manager_with_integration_token("integration-secret"),
        )))
        .layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 24], 44000))));

        let unauthorized = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/integration/v1/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                Request::builder()
                    .uri("/integration/v1/status")
                    .header("authorization", "Bearer integration-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gzip_compression_applied_when_accept_encoding_present() {
        let app = WebServer::build_router(test_state_with_config_manager(None))
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 45000))));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stats/summary")
                    .header("accept-encoding", "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-encoding")
                .and_then(|v| v.to_str().ok()),
            Some("gzip"),
            "JSON responses should be gzip-compressed when client accepts gzip"
        );
    }
}
