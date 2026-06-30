// Cast safety: dashboard metrics, report values — precision loss acceptable.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
// P2 PR-C: `missing_const_for_fn` accepted crate-wide.
// Rationale: const-viral cascade + nursery false-positive rate outweigh the value.
#![allow(clippy::missing_const_for_fn)]
// P2 remaining-nursery-lints: stylistic/cosmetic nursery lints accepted crate-wide.
#![allow(
    clippy::use_self,
    clippy::option_if_let_else,
    clippy::redundant_pub_crate
)]
// P2 PR-A nursery-hardening: mutex guards must not be held across I/O or
// long-running work unless intentionally kept for atomicity (use
// function-level #[allow] with reason). See
// rationale embedded in this module.
// Test code is exempt — mock implementations use intentionally-simple lock
// patterns for clarity over performance.
#![deny(clippy::significant_drop_tightening)]
#![cfg_attr(test, allow(clippy::significant_drop_tightening))]

//! # maekon-web
//!
//! ## Hexagonal Architecture — ADR-001 §7 (Port Location Rules)
//!
//! ### Violation 1 — `maekon-automation` concrete types in AppState — RESOLVED
//!
//! **Status**: Migration steps 1-6 completed.
//!   - `AuditLogPort` defined in `maekon-core/src/ports/audit_log.rs`
//!   - `AutomationPort` defined in `maekon-core/src/ports/automation.rs`
//!   - `GuiInteractionError` moved to `maekon-core::error`
//!   - `AuditEntry`, `AuditStatus`, `AuditLevel`, `AuditStats` in `maekon-core::models::audit`
//!   - `AppState` uses `Arc<dyn AuditLogPort>` and `Arc<dyn AutomationPort>`
//!   - `AuditLogAdapter` in `maekon-automation::audit` bridges `AuditLogger` to the port
//!
//! **Remaining**: `maekon-automation` moved to `[dev-dependencies]` — only used
//!   for test-only `AutomationController` construction in `automation_gui::tests`.
//!
//! ### Violation 2 — `maekon-storage` concrete types — RESOLVED
//!
//! **Status**: All 4 migration steps completed.
//!   - 14 row types promoted to `maekon-core::models::storage_records`
//!   - `WebStorage` trait moved to `maekon-core/src/ports/web_storage.rs`
//!   - `impl WebStorage for SqliteStorage` moved to `maekon-storage::sqlite::web_storage_impl`
//!   - `maekon-storage` moved to `[dev-dependencies]` (test-only `SqliteStorage::open_in_memory`)

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

use crate::services::provider_cli_diagnostics::ProviderCliDiagnosticsProvider;
use crate::storage_port::WebStorage;
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::response::Response;
use axum::Router;
use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use maekon_core::config::{CredentialBackendKind, WebConfig};
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::automation::AutomationPort;
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationSessionPort,
};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::runtime_log_provider::RuntimeLogProvider;
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use maekon_core::ports::system_info_provider::SystemInfoProvider;
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

pub use maekon_api_contracts::stream::{
    AiRuntimeStatus, FrameUpdate, IdleUpdate, MetricsUpdate, RealtimeEvent,
};

pub use maekon_core::config::WebConfig as CoreWebConfig;
pub use runtime_bindings::{
    AnalysisRuntimeBindings, AutomationRuntimeBindings, CoreRuntimeBindings,
    IntegrationRuntimeBindings, SecretRuntimeBindings, SessionRuntimeBindings,
    WebServerRuntimeBindings,
};

const EVENT_CHANNEL_CAPACITY: usize = 256;

const MAX_PORT_ATTEMPTS: u16 = maekon_core::config::WEB_PORT_FALLBACK_ATTEMPTS;
const INTEGRATION_TOKEN_HEADER: &str = "x-maekon-integration-token";
/// E20-41 (#4833): per-session local-API auth token channels. The header is the
/// primary channel for `fetch` (works cross-origin via CORS). EventSource/SSE
/// cannot set headers, so it uses the `?local_auth=` query (cross-origin Tauri;
/// redacted from logs) or the `maekon_local_auth` cookie (same-origin browser).
const LOCAL_AUTH_HEADER: &str = "x-local-auth";
const LOCAL_AUTH_COOKIE: &str = "maekon_local_auth";

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

    /// E20-41 (#4833): inject the ephemeral per-session local-API auth token that
    /// `require_local_auth` validates on every `/api` request. Generated by
    /// src-tauri at launch and also injected into the legit Tauri WebView, so a
    /// different local user (who reaches the same loopback port but never transited
    /// Tauri) has no token. `None` (default, for tests/standalone) ⇒ the gate fails
    /// closed (401 on all `/api`).
    pub fn with_local_auth_token(mut self, token: Arc<str>) -> Self {
        self.state.auth.local_auth_token = Some(token);
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

    pub fn with_provider_cli_diagnostics_provider(
        mut self,
        provider: Arc<dyn ProviderCliDiagnosticsProvider>,
    ) -> Self {
        self.state.diagnostics.provider_cli_diagnostics = Some(provider);
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
        if let Some(memory_graph) = core.memory_graph {
            self.state.core.memory_graph = Some(memory_graph);
        }
        if let Some(erasure_requested) = core.erasure_requested {
            self.state.core.erasure_requested = Some(erasure_requested);
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
        // #5734: forward the live per-call LLM health handle so GET /api/automation/status
        // can read the true last-call outcome at request time.
        if let Some(llm_call_health) = automation.llm_call_health {
            self.state.automation.llm_call_health = Some(llm_call_health);
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
        if let Some(model_catalog_client) = analysis.model_catalog_client {
            self.state.analysis.model_catalog_client = Some(model_catalog_client);
        }
        // #6279: wire the text-search provider so /api/semantic-search is no
        // longer permanently inert (keyword + hybrid-degraded modes).
        if let Some(text_search) = analysis.text_search {
            self.state.analysis.text_search = Some(text_search);
        }
        if let Some(session_manager) = session.session_manager {
            self.state.session.manager = Some(session_manager);
        }
        self
    }

    /// Return only the Router without binding TCP — used by the Tauri custom
    /// protocol handler and similar callers.
    pub fn build_router(mut state: AppState) -> Router {
        use axum::http::HeaderValue;
        use tower_http::cors::AllowOrigin;

        // #6117: construct the SINGLE, server-lifetime settings-policy audit
        // writer here — the one place every server-construction path (the
        // `run` loop, the Tauri custom-protocol handler, and tests) converges,
        // and which always runs inside a Tokio runtime so the writer's
        // background drain task can be spawned. Storing the `Arc` on `AppState`
        // means every per-request `SettingsWebContext` clone shares this one
        // writer, instead of the prior per-request spawn/abort cycle that
        // dropped security-policy audit events on every settings save.
        if state.automation.policy_audit_writer.is_none() {
            if let Some(audit_logger) = state.automation.audit_logger.clone() {
                if tokio::runtime::Handle::try_current().is_ok() {
                    state.automation.policy_audit_writer = Some(std::sync::Arc::new(
                        crate::services::settings_policy_service::PolicyAuditWriter::new(
                            audit_logger,
                        ),
                    ));
                } else {
                    warn!(
                        "build_router called outside a Tokio runtime — settings-policy audit writer not started (#6117)"
                    );
                }
            }
        }

        // Allow localhost origins only (tauri:// + http://127.0.0.1:{port range})
        let allowed_origins: Vec<HeaderValue> = (maekon_core::config::DEFAULT_WEB_PORT
            ..=maekon_core::config::DEFAULT_WEB_PORT_END)
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
            .chain(std::iter::once(
                "http://tauri.localhost".parse().expect("static URL"),
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

        // E20-41 (#4833): the internal /api requires BOTH a loopback client AND a
        // valid per-session local-auth token (defense in depth). These route_layers
        // are the by-construction chokepoint covering every /api route. Order matters:
        // the LAST .route_layer is the OUTERMOST (runs first), so require_loopback_client
        // stays outermost — a non-loopback client gets 403 (unchanged) before the token
        // is ever inspected; a loopback client then faces the token gate.
        let internal_api = routes::api_routes()
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                require_local_auth,
            ))
            .route_layer(middleware::from_fn(require_loopback_client));
        let integration_api = routes::integration_routes().route_layer(
            middleware::from_fn_with_state(state.clone(), require_integration_auth),
        );

        Router::new()
            .nest("/api", internal_api)
            .nest("/integration/v1", integration_api)
            .fallback(loopback_only_static)
            .layer(CompressionLayer::new())
            .layer(cors)
            // E20-41 (#4833): log the request PATH only — never the query string —
            // so an SSE `?local_auth=<token>` never reaches Loki/Grafana/OTel logs.
            .layer(
                TraceLayer::new_for_http().make_span_with(|request: &axum::extract::Request| {
                    tracing::info_span!(
                        "request",
                        method = %request.method(),
                        path = %trace_request_path_for_log(request),
                    )
                }),
            )
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
            warn!("External integration API enabled on 0.0.0.0; protect web.integration_auth_token as a high-entropy secret");
            "0.0.0.0"
        } else {
            if config.allow_external && !integration_auth_configured {
                warn!("External access requested but web.integration_auth_token is not configured; falling back to loopback-only binding");
            }
            "127.0.0.1"
        };

        let app = Self::build_router(state);

        let base_port = config.port;
        let final_port = maekon_core::config::DEFAULT_WEB_PORT_END;
        let max_attempts = if base_port <= final_port {
            MAX_PORT_ATTEMPTS.min(final_port - base_port + 1)
        } else {
            0
        };
        let mut last_error = None;

        for attempt in 0..max_attempts {
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
                    "ports {}-{} are all in use, none available",
                    base_port, final_port
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

/// E20-41 (#4833): require a valid per-session local-API auth token on every
/// `/api` request. Combined with `require_loopback_client`, this stops a different
/// local user on a multi-user host (RDP/Citrix) — who can reach the same loopback
/// port but never transited the legit Tauri WebView that holds the token — from
/// reading this user's settings / audit export. Fails CLOSED: no provisioned token
/// ⇒ 401 (never a no-op skip). Constant-time compare avoids a timing oracle.
async fn require_local_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // CORS preflight (OPTIONS) carries no auth header; let the outer CorsLayer
    // answer it instead of 401-ing the browser's preflight.
    if request.method() == axum::http::Method::OPTIONS {
        return next.run(request).await;
    }

    // Fail closed: a missing session token rejects everything — never skip.
    let Some(expected) = state.auth.local_auth_token.clone() else {
        return crate::error::ApiError::Unauthorized(
            "The local API requires session authentication.".to_string(),
        )
        .into_response();
    };

    // Accept the token via X-Local-Auth, Authorization: Bearer, or the
    // maekon_local_auth cookie. The query-param channel is intentionally limited
    // to EventSource endpoints because it is easy to copy/share as a URL.
    let presented = request
        .headers()
        .get(LOCAL_AUTH_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            request
                .headers()
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.strip_prefix("Bearer "))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        })
        .or_else(|| read_local_auth_cookie(request.headers()))
        .or_else(|| {
            if local_auth_query_allowed(request.method(), request.uri()) {
                read_local_auth_query(request.uri())
            } else {
                None
            }
        });

    let authorized = presented
        .as_deref()
        .map(|token| {
            use subtle::ConstantTimeEq;
            bool::from(token.as_bytes().ct_eq(expected.as_bytes()))
        })
        .unwrap_or(false);

    if authorized {
        next.run(request).await
    } else {
        crate::error::ApiError::Unauthorized(
            "Invalid or missing local authentication token.".to_string(),
        )
        .into_response()
    }
}

/// Read the `local_auth` token from the request query string — the cross-origin
/// SSE channel (EventSource can set no header, and a Tauri-document cookie never
/// reaches the 127.0.0.1 origin). Safe only because the TraceLayer logs the path
/// without the query (see `build_router`). The token is hex, so no URL-decode.
fn read_local_auth_query(uri: &axum::http::Uri) -> Option<String> {
    uri.query()?.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        if key == "local_auth" && !value.is_empty() {
            Some(value.to_string())
        } else {
            None
        }
    })
}

fn local_auth_query_allowed(method: &axum::http::Method, uri: &axum::http::Uri) -> bool {
    if method != axum::http::Method::GET {
        return false;
    }

    matches!(
        uri.path(),
        "/stream" | "/update/stream" | "/api/stream" | "/api/update/stream"
    )
}

/// Read the `maekon_local_auth` token from the Cookie header — the same-origin
/// (browser) SSE channel. Mirrors `read_cookie_capability_token`.
fn read_local_auth_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|raw_cookie| raw_cookie.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(name, value)| {
            if name.trim() != LOCAL_AUTH_COOKIE {
                return None;
            }
            let value = value.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        })
}

async fn require_integration_auth(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let remote_ip = addr.ip();
    if state
        .auth
        .integration_auth_rate_limiter
        .is_locked_out(remote_ip)
        .await
    {
        return crate::error::ApiError::TooManyRequests(
            "Integration API authentication is temporarily locked for this client.".to_string(),
        )
        .into_response();
    }

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

    // Constant-time comparison — the integration token gates the externally
    // reachable `/integration/v1` surface (no loopback layer when
    // allow_external=true), so a byte-by-byte `!=` would be a timing oracle.
    // Mirrors require_local_auth (#5639).
    let authorized = header_token
        .as_deref()
        .map(|token| {
            use subtle::ConstantTimeEq;
            bool::from(token.as_bytes().ct_eq(expected_token.as_bytes()))
        })
        .unwrap_or(false);

    if !authorized {
        let locked = state
            .auth
            .integration_auth_rate_limiter
            .record_failure(remote_ip)
            .await;
        if locked {
            warn!(
                remote_ip = %remote_ip,
                "Integration API authentication failures reached temporary lockout"
            );
        }
        return crate::error::ApiError::Unauthorized(
            "Integration API requires a valid bearer token.".to_string(),
        )
        .into_response();
    }

    state
        .auth
        .integration_auth_rate_limiter
        .record_success(remote_ip)
        .await;
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

fn trace_request_path_for_log(request: &Request) -> &str {
    request.uri().path()
}

/// E20-41 (#4833): shared test scaffolding so handler tests satisfy the new
/// local-auth gate without each test having to thread a token. Seeds a known
/// session token into the state AND injects the matching `X-Local-Auth` header
/// into every request, so existing inline `Request::builder()` tests pass
/// unchanged. `require_loopback_client` still runs (MockConnectInfo = loopback).
#[cfg(test)]
pub(crate) mod test_local_auth {
    use crate::app_state::AppState;
    use axum::extract::connect_info::MockConnectInfo;
    use std::net::SocketAddr;
    use std::sync::Arc;

    pub(crate) const TEST_LOCAL_AUTH_TOKEN: &str = "test-local-auth-token-e20-41";

    /// Build the production router with the local-auth gate satisfied for tests.
    pub(crate) fn authed_loopback_router(mut state: AppState) -> axum::Router {
        state.auth.local_auth_token = Some(Arc::from(TEST_LOCAL_AUTH_TOKEN));
        crate::WebServer::build_router(state)
            .layer(axum::middleware::map_request(
                |mut req: axum::extract::Request| async move {
                    req.headers_mut().insert(
                        super::LOCAL_AUTH_HEADER,
                        axum::http::HeaderValue::from_static(TEST_LOCAL_AUTH_TOKEN),
                    );
                    req
                },
            ))
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{header, Request, StatusCode};
    use http_body_util::BodyExt;
    use maekon_core::config::AppConfig;
    use maekon_core::config_manager::ConfigManager;
    use maekon_storage::sqlite::SqliteStorage;
    use tempfile::tempdir;
    use tower::ServiceExt;

    #[test]
    fn default_config() {
        let config = WebConfig::default();
        assert_eq!(config.port, maekon_core::config::DEFAULT_WEB_PORT);
        assert!(!config.allow_external);
    }

    #[test]
    fn web_server_url() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let server = WebServer::new(storage, WebConfig::default());
        let expected = format!("http://localhost:{}", maekon_core::config::DEFAULT_WEB_PORT);
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

    #[tokio::test]
    async fn cors_allows_tauri_localhost_origin_for_embedded_webview() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let state = AppState::with_core(storage, event_tx);
        // E20-41 (#4833): authed_loopback_router seeds the local-auth token + header.
        let app = crate::test_local_auth::authed_loopback_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/metrics")
                    .header("Origin", "http://tauri.localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .and_then(|value| value.to_str().ok()),
            Some("http://tauri.localhost")
        );
    }

    #[tokio::test]
    async fn cors_preflight_allows_tauri_origin_without_local_auth_header() {
        let app = WebServer::build_router(test_state_with_config_manager(None))
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));

        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/metrics")
                    .header(header::ORIGIN, "http://tauri.localhost")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("http://tauri.localhost")
        );
    }

    #[tokio::test]
    async fn static_fallback_serves_loopback_dashboard_placeholder() {
        let app = WebServer::build_router(test_state_with_config_manager(None))
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html")));
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let body = std::str::from_utf8(&body).unwrap();
        assert!(body.contains("Maekon"));
    }

    #[tokio::test]
    async fn static_fallback_rejects_non_loopback_clients() {
        let app = WebServer::build_router(test_state_with_config_manager(None)).layer(
            MockConnectInfo(SocketAddr::from(([192, 168, 0, 10], 43000))),
        );

        let response = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn eventsource_query_auth_returns_sse_without_header_or_cookie() {
        let response = gated_app("secret")
            .oneshot(
                Request::builder()
                    .uri("/api/stream?local_auth=secret")
                    .header(header::ORIGIN, "http://tauri.localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream")));
    }

    #[test]
    fn trace_request_path_for_log_omits_query_token() {
        let request = Request::builder()
            .uri("/api/stream?local_auth=secret&cursor=next")
            .body(Body::empty())
            .unwrap();

        assert_eq!(trace_request_path_for_log(&request), "/api/stream");
    }

    #[test]
    fn memory_graph_binding_is_applied_to_core_state() {
        // ADR-023 web-render wiring: a CoreRuntimeBindings.memory_graph flows into
        // CoreState so the digest export endpoint can render accumulated claims.
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let server = WebServer::new(storage.clone(), WebConfig::default()).with_runtime_bindings(
            WebServerRuntimeBindings {
                core: CoreRuntimeBindings {
                    event_tx: Some(event_tx),
                    memory_graph: Some(
                        storage as Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>,
                    ),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        assert!(
            server.state.core.memory_graph.is_some(),
            "memory_graph binding must populate CoreState"
        );
    }

    #[test]
    fn web_server_runtime_bindings_apply_scalar_runtime_state() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(128);
        let ai_runtime_status = AiRuntimeStatus {
            ocr_source: "remote".to_string(),
            llm_source: "subprocess_cli".to_string(),
            ocr_fallback_reason: None,
            llm_fallback_reason: None,
            llm_healthy: None, // no health tracking in test
        };
        let integration_runtime_status = IntegrationOutboundRuntimeStatus {
            enabled: true,
            runtime_telemetry: None,
            ..IntegrationOutboundRuntimeStatus::default()
        };
        let frames_dir = std::path::PathBuf::from("/tmp/maekon-web-runtime-bindings");

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
        let mut reserved_listener = None;
        for port in maekon_core::config::DEFAULT_WEB_PORT..maekon_core::config::DEFAULT_WEB_PORT_END
        {
            let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await else {
                continue;
            };
            if TcpListener::bind(("127.0.0.1", port + 1)).await.is_ok() {
                reserved_listener = Some(listener);
                break;
            }
        }
        let reserved_listener =
            reserved_listener.expect("an allowed web port with a free fallback must be available");
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

        // Contract: after graceful shutdown the server task exits with Ok(()).
        // The exact unit is () — no payload to pin beyond the Ok discriminant,
        // but we collapse the hedge to propagate the real error on failure (#5594).
        assert_eq!(
            server_result.expect("server task must complete without error after graceful shutdown"),
            ()
        );
        drop(reserved_listener);
    }

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn max_port_attempts_is_reasonable() {
        assert!(MAX_PORT_ATTEMPTS >= 1);
        assert!(MAX_PORT_ATTEMPTS <= 100);
    }

    #[test]
    fn fallback_attempts_never_exceed_csp_range() {
        let base_port = maekon_core::config::DEFAULT_WEB_PORT_END - 4;
        let final_port = maekon_core::config::DEFAULT_WEB_PORT_END;
        let max_attempts = MAX_PORT_ATTEMPTS.min(final_port - base_port + 1);
        let attempted: Vec<u16> = (0..max_attempts)
            .map(|attempt| base_port.saturating_add(attempt))
            .collect();

        assert_eq!(attempted.first().copied(), Some(base_port));
        assert_eq!(attempted.last().copied(), Some(final_port));
        assert!(attempted.iter().all(|port| *port <= final_port));
    }

    fn test_state_with_config_manager(config_manager: Option<ConfigManager>) -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(128);
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

    const STRONG_INTEGRATION_TOKEN: &str = "integration-secret-0123456789abcdef";

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
            config_manager_with_integration_token(STRONG_INTEGRATION_TOKEN),
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

        // Wrong token must be rejected — pins the constant-time compare's
        // reject branch (#5639), not just the missing-token path above.
        let wrong = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/integration/v1/status")
                    .header("authorization", "Bearer integration-secre")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                Request::builder()
                    .uri("/integration/v1/status")
                    .header(
                        "authorization",
                        format!("Bearer {STRONG_INTEGRATION_TOKEN}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn integration_api_temporarily_locks_out_repeated_failures_by_remote_ip() {
        let app = WebServer::build_router(test_state_with_config_manager(Some(
            config_manager_with_integration_token(STRONG_INTEGRATION_TOKEN),
        )))
        .layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 24], 44000))));

        for _ in 0..5 {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/integration/v1/status")
                        .header("authorization", "Bearer wrong-token")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }

        let locked = app
            .oneshot(
                Request::builder()
                    .uri("/integration/v1/status")
                    .header(
                        "authorization",
                        format!("Bearer {STRONG_INTEGRATION_TOKEN}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(locked.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn gzip_compression_applied_when_accept_encoding_present() {
        // E20-41 (#4833): authed_loopback_router seeds the local-auth token + header.
        let app =
            crate::test_local_auth::authed_loopback_router(test_state_with_config_manager(None));

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

    // ── E20-41 (#4833): local-auth gate ─────────────────────────────────
    // The token check (require_local_auth) is transport-agnostic — it reads HTTP
    // headers/cookies, identical over any socket — so the oneshot+MockConnectInfo
    // harness exercises the REAL production middleware (unlike OS peer-cred, which
    // differs by socket family). A real-TcpListener case is added below as an
    // anti-fail-open-theater guard per memory `peercred_on_tcp_fail_open`.

    fn gated_app(token: &str) -> axum::Router {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let mut state = AppState::with_core(storage, event_tx);
        state.auth.local_auth_token = Some(Arc::from(token));
        WebServer::build_router(state).layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    async fn metrics_status(app: axum::Router, req: Request<Body>) -> StatusCode {
        app.oneshot(req).await.unwrap().status()
    }

    #[tokio::test]
    async fn local_auth_rejects_request_without_token() {
        let status = metrics_status(
            gated_app("secret"),
            Request::builder()
                .uri("/api/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn local_auth_accepts_valid_header_token() {
        let status = metrics_status(
            gated_app("secret"),
            Request::builder()
                .uri("/api/metrics")
                .header(LOCAL_AUTH_HEADER, "secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn local_auth_rejects_wrong_token() {
        let status = metrics_status(
            gated_app("secret"),
            Request::builder()
                .uri("/api/metrics")
                .header(LOCAL_AUTH_HEADER, "wrong")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn local_auth_accepts_cookie_token_for_sse() {
        // EventSource/SSE cannot set headers; the maekon_local_auth cookie is its channel.
        let status = metrics_status(
            gated_app("secret"),
            Request::builder()
                .uri("/api/metrics")
                .header("cookie", format!("{LOCAL_AUTH_COOKIE}=secret"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn local_auth_rejects_query_token_for_non_sse_routes() {
        // Cross-origin EventSource needs ?local_auth= on the SSE allowlist only.
        // Normal REST endpoints must keep using header/cookie auth so a copied
        // URL is not a full local-API credential.
        let status = metrics_status(
            gated_app("secret"),
            Request::builder()
                .uri("/api/metrics?local_auth=secret")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn local_auth_exempts_options_preflight() {
        // CORS preflight carries no auth header; must not be 401'd.
        let status = metrics_status(
            gated_app("secret"),
            Request::builder()
                .method("OPTIONS")
                .uri("/api/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_ne!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn local_auth_fails_closed_when_no_token_provisioned() {
        // No token in state ⇒ every /api request 401 (fail-closed, NEVER skip).
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let state = AppState::with_core(storage, event_tx); // local_auth_token = None
        let app = WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))));
        let status = metrics_status(
            app,
            Request::builder()
                .uri("/api/metrics")
                .header(LOCAL_AUTH_HEADER, "anything")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn local_auth_non_loopback_is_forbidden_before_token_gate() {
        // require_loopback_client runs OUTERMOST: a non-loopback client gets 403
        // (preserved behavior) before the token gate is consulted — not 401.
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let mut state = AppState::with_core(storage, event_tx);
        state.auth.local_auth_token = Some(Arc::from("secret"));
        let app = WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([10, 0, 0, 5], 40000))));
        let status = metrics_status(
            app,
            Request::builder()
                .uri("/api/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn local_auth_gate_blocks_over_real_loopback_socket() {
        // Anti-fail-open-theater (memory: peercred_on_tcp_fail_open): exercise the
        // gate over a REAL AF_INET TcpListener + reqwest client, not only a mock.
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(8);
        let mut state = AppState::with_core(storage, event_tx);
        state.auth.local_auth_token = Some(Arc::from("real-socket-token"));
        let app = WebServer::build_router(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });

        let client = reqwest::Client::new();
        let base = format!("http://127.0.0.1:{}/api/metrics", addr.port());

        // No token ⇒ 401.
        let no_token = client.get(&base).send().await.unwrap();
        assert_eq!(no_token.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Correct token ⇒ gate passes (not 401).
        let ok = client
            .get(&base)
            .header(LOCAL_AUTH_HEADER, "real-socket-token")
            .send()
            .await
            .unwrap();
        assert_ne!(ok.status(), reqwest::StatusCode::UNAUTHORIZED);

        // Wrong token ⇒ 401.
        let wrong = client
            .get(&base)
            .header(LOCAL_AUTH_HEADER, "nope")
            .send()
            .await
            .unwrap();
        assert_eq!(wrong.status(), reqwest::StatusCode::UNAUTHORIZED);

        server.abort();
    }
}
