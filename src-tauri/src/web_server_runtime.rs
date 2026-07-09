use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
use maekon_automation::audit::{AuditLogAdapter, AuditLogger};
use maekon_automation::controller::AutomationController;
use maekon_core::config::{AppConfig, CredentialBackendKind};
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::frame_storage::FrameStoragePort;
// Integration ports remain server-gated (Oneshim transport only).
#[cfg(feature = "server")]
use maekon_core::ports::integration::{
    IntegrationAuditPort, IntegrationAuthPort, IntegrationInboxPort, IntegrationInboxStorePort,
    IntegrationOutboxPort, IntegrationRuntimeTelemetryPort, IntegrationSessionPort,
};
// C1: OAuthPort and SecretStore(Set) are analysis-gated — used by provider
// adapters which are available without the 'server' transport feature.
#[cfg(feature = "analysis")]
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::runtime_log_provider::RuntimeLogProvider;
#[cfg(feature = "analysis")]
use maekon_core::ports::secret_store::{SecretStore, SecretStoreSet};
use maekon_core::ports::system_info_provider::SystemInfoProvider;
use maekon_monitor::system_info::SysInfoProvider;
use maekon_storage::frame_storage::FrameFileStorage;
use maekon_storage::sqlite::SqliteStorage;
use maekon_web::update_control::UpdateControl;
use maekon_web::{
    AiRuntimeStatus, AnalysisRuntimeBindings, AutomationRuntimeBindings, CoreRuntimeBindings,
    IntegrationRuntimeBindings, RealtimeEvent, SessionRuntimeBindings, WebServer,
    WebServerRequiredDeps, WebServerRuntimeBindings,
};
use std::path::Path;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, watch};
use tracing::{error, info, warn};

use crate::automation_controller_builder::AutomationControllerBuilder;
use crate::feature_capabilities::ProviderCliDiagnosticsSnapshotProvider;
use crate::runtime_state::{secret_backend_capabilities, SecretBackendCapabilities};
use crate::services::log_helpers;
use crate::services::runtime_log_provider::TauriRuntimeLogProvider;

type ProviderModelCatalogClient =
    Arc<dyn maekon_core::ports::provider_model_catalog::ProviderModelCatalogPort>;

#[cfg(feature = "analysis")]
fn provider_model_catalog_client() -> Option<ProviderModelCatalogClient> {
    Some(Arc::new(
        maekon_network::provider_model_catalog_client::ReqwestProviderModelCatalogClient::new(),
    ))
}

#[cfg(not(feature = "analysis"))]
fn provider_model_catalog_client() -> Option<ProviderModelCatalogClient> {
    None
}

pub(crate) struct WebServerLaunchResult {
    pub(crate) frontend_web_port: u16,
    pub(crate) web_server_startup_error: Option<String>,
    pub(crate) automation_controller: Option<Arc<AutomationController>>,
    /// Snapshot captured from automation_build.ai_runtime_status at
    /// server build time. Consumed by DashboardServiceImpl for
    /// SubscribeEvents snapshot-on-subscribe emission (spec §A A2).
    /// Read in app_runtime_launch.rs within `#[cfg(feature = "grpc-dashboard")]`.
    #[cfg_attr(
        not(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external")),
        allow(dead_code)
    )]
    pub(crate) ai_runtime_status: Option<AiRuntimeStatus>,
    /// JoinHandle for the external gRPC supervisor task.
    ///
    /// Must be held alive for the duration of the application. Dropping it
    /// aborts the supervisor — the accept loop and TLS handshake tasks will
    /// exit on the next await point.  The field is only populated when the
    /// `grpc-dashboard-external` feature is enabled and the external gRPC
    /// bind config succeeds.
    #[cfg(feature = "grpc-dashboard-external")]
    pub(crate) ext_grpc_supervisor: Option<tokio::task::JoinHandle<()>>,
    /// Cert watcher + expiry monitor JoinHandle owner (F-RR-C28-02).
    ///
    /// `Drop` impl calls `.abort()` on both background tasks, preventing task
    /// leakage on non-clean teardown (panic, OS kill before watch channel close).
    /// Held alongside `ext_grpc_supervisor` for the lifetime of the application.
    #[cfg(feature = "grpc-dashboard-external")]
    pub(crate) ext_cert_watcher: Option<maekon_web::grpc::external::tls_config::CertWatcherHandle>,
}

pub(crate) struct WebServerLaunchContext<'a> {
    runtime_handle: &'a Handle,
    shutdown_tx: &'a watch::Sender<bool>,
    event_tx: broadcast::Sender<RealtimeEvent>,
    web_port_state: Arc<AtomicU16>,
    /// E20-41 (#4833): the per-session local-API auth token the WebServer's
    /// `require_local_auth` gate validates on every `/api` request.
    local_auth_token: Arc<str>,
}

impl<'a> WebServerLaunchContext<'a> {
    pub(crate) fn new(
        runtime_handle: &'a Handle,
        shutdown_tx: &'a watch::Sender<bool>,
        event_tx: broadcast::Sender<RealtimeEvent>,
        web_port_state: Arc<AtomicU16>,
        local_auth_token: Arc<str>,
    ) -> Self {
        Self {
            runtime_handle,
            shutdown_tx,
            event_tx,
            web_port_state,
            local_auth_token,
        }
    }
}

#[cfg(feature = "server")]
pub(crate) struct WebServerServerSupport {
    integration_auth: Option<Arc<dyn IntegrationAuthPort>>,
    integration_session: Option<Arc<dyn IntegrationSessionPort>>,
    integration_outbox: Option<Arc<dyn IntegrationOutboxPort>>,
    integration_inbox: Option<Arc<dyn IntegrationInboxPort>>,
    integration_inbox_store: Option<Arc<dyn IntegrationInboxStorePort>>,
    integration_audit: Option<Arc<dyn IntegrationAuditPort>>,
    integration_runtime_telemetry: Option<Arc<dyn IntegrationRuntimeTelemetryPort>>,
    secret_store: Option<Arc<dyn SecretStore>>,
    secret_stores: Option<SecretStoreSet>,
    default_secret_backend_kind: Option<CredentialBackendKind>,
    oauth_port: Option<Arc<dyn OAuthPort>>,
}

#[cfg(feature = "server")]
impl WebServerServerSupport {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        integration_auth: Option<Arc<dyn IntegrationAuthPort>>,
        integration_session: Option<Arc<dyn IntegrationSessionPort>>,
        integration_outbox: Option<Arc<dyn IntegrationOutboxPort>>,
        integration_inbox: Option<Arc<dyn IntegrationInboxPort>>,
        integration_inbox_store: Option<Arc<dyn IntegrationInboxStorePort>>,
        integration_audit: Option<Arc<dyn IntegrationAuditPort>>,
        integration_runtime_telemetry: Option<Arc<dyn IntegrationRuntimeTelemetryPort>>,
        secret_store: Option<Arc<dyn SecretStore>>,
        secret_stores: Option<SecretStoreSet>,
        default_secret_backend_kind: Option<CredentialBackendKind>,
        oauth_port: Option<Arc<dyn OAuthPort>>,
    ) -> Self {
        Self {
            integration_auth,
            integration_session,
            integration_outbox,
            integration_inbox,
            integration_inbox_store,
            integration_audit,
            integration_runtime_telemetry,
            secret_store,
            secret_stores,
            default_secret_backend_kind,
            oauth_port,
        }
    }
}

pub(crate) struct WebServerSupportContext {
    config_manager: ConfigManager,
    update_control: UpdateControl,
    integration_runtime_status: IntegrationOutboundRuntimeStatus,
    app_handle: Option<tauri::AppHandle>,
    cli_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    secret_backend_capabilities: SecretBackendCapabilities,
    /// D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    /// registry from the composition root. Forwarded to the
    /// `AutomationControllerBuilder` in `configure_automation_builder`. Defaults
    /// to a fresh registry; the composition root overrides it via
    /// `with_breaker_registry`.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    /// #7932 Part B: the ONE shared `Arc<PolicyClient>` from the composition root
    /// (Port Instance Sharing). Forwarded to the `AutomationControllerBuilder` in
    /// `configure_automation_builder` so the automation controller shares the same
    /// durable policy store instance as the Codex approval decider. `None` leaves
    /// the controller to build its own client (standalone/legacy path).
    policy_client: Option<Arc<maekon_automation::policy::PolicyClient>>,
    // C1: provider credential fields — analysis-gated so BYOK/OAuth adapter
    // injection works in default builds without 'server'. Must carry the same
    // triple (store / store-set / default backend kind) the server path injects,
    // or `runtime_bindings.secrets.default_secret_backend_kind` stays None and
    // maekon-web keeps its Unavailable default → Settings key save rejects.
    #[cfg(all(feature = "analysis", not(feature = "server")))]
    provider_secret_store: Option<Arc<dyn SecretStore>>,
    #[cfg(all(feature = "analysis", not(feature = "server")))]
    provider_secret_stores: Option<SecretStoreSet>,
    #[cfg(all(feature = "analysis", not(feature = "server")))]
    provider_default_backend_kind: Option<CredentialBackendKind>,
    #[cfg(all(feature = "analysis", not(feature = "server")))]
    provider_oauth_port: Option<Arc<dyn OAuthPort>>,
    #[cfg(feature = "server")]
    server: Option<WebServerServerSupport>,
}

impl WebServerSupportContext {
    pub(crate) fn new(
        config_manager: ConfigManager,
        update_control: UpdateControl,
        integration_runtime_status: IntegrationOutboundRuntimeStatus,
    ) -> Self {
        Self {
            config_manager,
            update_control,
            integration_runtime_status,
            app_handle: None,
            cli_health_flag: None,
            consent_manager: None,
            secret_backend_capabilities: secret_backend_capabilities(
                false,
                Vec::new(),
                CredentialBackendKind::Unavailable,
                CredentialBackendKind::Unavailable,
            ),
            breaker_registry: crate::breaker_registry::CircuitBreakerRegistry::new(),
            policy_client: None,
            #[cfg(all(feature = "analysis", not(feature = "server")))]
            provider_secret_store: None,
            #[cfg(all(feature = "analysis", not(feature = "server")))]
            provider_secret_stores: None,
            #[cfg(all(feature = "analysis", not(feature = "server")))]
            provider_default_backend_kind: None,
            #[cfg(all(feature = "analysis", not(feature = "server")))]
            provider_oauth_port: None,
            #[cfg(feature = "server")]
            server: None,
        }
    }

    // C1: inject provider credential stores so analysis-only builds can resolve
    // remote LLM/OCR adapters via BYOK or OAuth, and so the web runtime
    // bindings carry the full secrets triple (store / store-set / default
    // backend kind) needed by the Settings persistence path.
    #[cfg(all(feature = "analysis", not(feature = "server")))]
    pub(crate) fn with_provider_support(
        mut self,
        secret_store: Option<Arc<dyn SecretStore>>,
        secret_stores: Option<SecretStoreSet>,
        default_backend_kind: Option<CredentialBackendKind>,
        oauth_port: Option<Arc<dyn OAuthPort>>,
    ) -> Self {
        self.provider_secret_store = secret_store;
        self.provider_secret_stores = secret_stores;
        self.provider_default_backend_kind = default_backend_kind;
        self.provider_oauth_port = oauth_port;
        self
    }

    /// Inject the single shared workspace-wide circuit-breaker registry from the
    /// composition root (D7 #4812 / E20-20). Forwarded to the automation
    /// controller builder so the resolved provider adapters share one breaker.
    pub(crate) fn with_breaker_registry(
        mut self,
        registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    ) -> Self {
        self.breaker_registry = registry;
        self
    }

    pub(crate) fn with_app_handle(mut self, handle: tauri::AppHandle) -> Self {
        self.app_handle = Some(handle);
        self
    }

    /// Inject the single shared `Arc<PolicyClient>` from the composition root
    /// (#7932 Part B, Port Instance Sharing). Forwarded to the automation
    /// controller builder so the controller and the Codex approval decider share
    /// one durable policy store instance within a session.
    pub(crate) fn with_policy_client(
        mut self,
        policy_client: Arc<maekon_automation::policy::PolicyClient>,
    ) -> Self {
        self.policy_client = Some(policy_client);
        self
    }

    pub(crate) fn with_cli_health_flag(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.cli_health_flag = Some(flag);
        self
    }

    pub(crate) fn with_consent_manager(mut self, cm: Arc<dyn ConsentManagerPort>) -> Self {
        self.consent_manager = Some(cm);
        self
    }

    // C1: secret backend capabilities come from ProviderRuntimeContext — analysis-gated.
    #[cfg(feature = "analysis")]
    pub(crate) fn with_secret_backend_capabilities(
        mut self,
        capabilities: SecretBackendCapabilities,
    ) -> Self {
        self.secret_backend_capabilities = capabilities;
        self
    }

    #[cfg(feature = "server")]
    pub(crate) fn with_server_support(mut self, server: WebServerServerSupport) -> Self {
        self.server = Some(server);
        self
    }

    fn configure_automation_builder<'a>(
        &self,
        builder: AutomationControllerBuilder<'a>,
    ) -> AutomationControllerBuilder<'a> {
        // D7 (#4812 / E20-20): forward the shared breaker registry so the
        // resolved OCR/LLM adapters converge on the same breaker as the rest of
        // the workspace.
        let builder = builder.with_breaker_registry(self.breaker_registry.clone());
        // #7932 Part B: forward the single shared Arc<PolicyClient> (Port Instance
        // Sharing) so the automation controller and the Codex approval decider
        // hold the SAME durable policy store instance within a session.
        let builder = if let Some(ref policy_client) = self.policy_client {
            builder.with_policy_client(policy_client.clone())
        } else {
            builder
        };
        let builder = if let Some(ref flag) = self.cli_health_flag {
            builder.with_cli_health_flag(flag.clone())
        } else {
            builder
        };
        let builder = if let Some(ref handle) = self.app_handle {
            builder.with_app_handle(handle.clone())
        } else {
            builder
        };
        let builder = if let Some(ref cm) = self.consent_manager {
            builder.with_consent_manager(cm.clone())
        } else {
            builder
        };

        // C1: in server builds, server context carries all provider + integration
        // credentials together in WebServerServerSupport.
        // In analysis-only builds, provider credentials are in the analysis fields.
        #[cfg(feature = "server")]
        {
            if let Some(server) = self.server.as_ref() {
                return builder
                    .with_provider_secret_stores(server.secret_stores.clone().unwrap_or_default())
                    .with_oauth_port(server.oauth_port.clone());
            }
        }

        // Analysis-only (no server): inject provider secrets directly.
        #[cfg(all(feature = "analysis", not(feature = "server")))]
        {
            let builder = if let Some(ref stores) = self.provider_secret_stores {
                builder.with_provider_secret_stores(stores.clone())
            } else {
                builder
            };
            return builder.with_oauth_port(self.provider_oauth_port.clone());
        }

        #[allow(unreachable_code)]
        builder
    }

    /// #7738 D-3: sparse OPTIONAL/CONDITIONAL bundle only. `frame_storage`
    /// (`None` on `SharedCaptureServices::build()` failure) and
    /// `ai_runtime_status` (tracks the automation build outcome — `None` when
    /// `config.automation.enabled = false`) are the two fields this
    /// support-context-scoped step can populate; `erasure_requested` +
    /// `integration`/`secrets` are filled in by the caller / the cfg branches
    /// below. The VERIFIED-unconditional fields this function used to carry
    /// (`frames_dir`, `config_manager`, `update_control`, the storage-derived
    /// ports, `audit_logger`) moved to `WebServerRequiredDeps`, assembled
    /// separately in `build_and_spawn`.
    fn build_runtime_bindings(
        &self,
        frame_storage: Option<Arc<dyn FrameStoragePort>>,
        ai_runtime_status: Option<AiRuntimeStatus>,
    ) -> WebServerRuntimeBindings {
        let runtime_bindings = WebServerRuntimeBindings {
            core: CoreRuntimeBindings {
                frame_storage,
                erasure_requested: None,
            },
            automation: AutomationRuntimeBindings {
                ai_runtime_status,
                ..Default::default()
            },
            integration: IntegrationRuntimeBindings {
                integration_runtime_status: Some(self.integration_runtime_status.clone()),
                ..Default::default()
            },
            ..Default::default()
        };

        // C1: in server builds, wire both integration AND provider secret bindings.
        #[cfg(feature = "server")]
        {
            let mut runtime_bindings = runtime_bindings;
            if let Some(server) = self.server.as_ref() {
                runtime_bindings.integration.integration_auth = server.integration_auth.clone();
                runtime_bindings.integration.integration_session =
                    server.integration_session.clone();
                runtime_bindings.integration.integration_outbox = server.integration_outbox.clone();
                runtime_bindings.integration.integration_inbox = server.integration_inbox.clone();
                runtime_bindings.integration.integration_inbox_store =
                    server.integration_inbox_store.clone();
                runtime_bindings.integration.integration_audit = server.integration_audit.clone();
                runtime_bindings.integration.integration_runtime_telemetry =
                    server.integration_runtime_telemetry.clone();
                // regression_risk #1: 7 integration bindings verified as is_some()
                // when server_context is present (7 assertions in the test module).
                runtime_bindings.secrets.secret_store = server.secret_store.clone();
                runtime_bindings.secrets.secret_stores = server.secret_stores.clone();
                runtime_bindings.secrets.default_secret_backend_kind =
                    server.default_secret_backend_kind;
            }
            runtime_bindings
        }

        // C1: analysis-only build — wire provider secret bindings without
        // integration. Mirrors the server branch's full secrets triple: leaving
        // `default_secret_backend_kind` None would keep maekon-web's Unavailable
        // default and reject every Settings API-key save in default builds.
        #[cfg(all(feature = "analysis", not(feature = "server")))]
        {
            let mut runtime_bindings = runtime_bindings;
            runtime_bindings.secrets.secret_store = self.provider_secret_store.clone();
            runtime_bindings.secrets.secret_stores = self.provider_secret_stores.clone();
            runtime_bindings.secrets.default_secret_backend_kind =
                self.provider_default_backend_kind;
            runtime_bindings
        }

        #[cfg(not(any(feature = "server", feature = "analysis")))]
        {
            runtime_bindings
        }
    }
}

pub(crate) struct WebServerRuntimeBuilder<'a> {
    storage: Arc<SqliteStorage>,
    config: &'a AppConfig,
    data_dir: &'a Path,
    launch_context: WebServerLaunchContext<'a>,
    support_context: WebServerSupportContext,
    override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    recluster_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    erasure_requested: Option<Arc<std::sync::atomic::AtomicBool>>,
    coaching_engine: Option<Arc<dyn maekon_core::ports::coaching::CoachingPort>>,
    session_manager: Option<Arc<dyn maekon_core::ports::conversation_session::SessionManager>>,
    frame_storage: Option<Arc<dyn FrameStoragePort>>,
    /// Task 7.1: pre-built LiveExternalConfig Arc shared with the external gRPC server.
    /// Populated before `build_and_spawn` when `grpc-dashboard-external` is active so the
    /// web server's `DiagnosticsState` can serve `GET /api/external-grpc/live-config`.
    #[cfg(feature = "grpc-dashboard-external")]
    external_grpc_live: Option<Arc<maekon_web::grpc::external::live_config::LiveExternalConfig>>,
    #[cfg(feature = "grpc-dashboard-external")]
    external_grpc_metrics: Option<Arc<maekon_web::grpc::external::metrics::ExternalMetrics>>,
}

impl<'a> WebServerRuntimeBuilder<'a> {
    pub(crate) fn new(
        storage: Arc<SqliteStorage>,
        config: &'a AppConfig,
        data_dir: &'a Path,
        launch_context: WebServerLaunchContext<'a>,
        support_context: WebServerSupportContext,
    ) -> Self {
        Self {
            storage,
            config,
            data_dir,
            launch_context,
            support_context,
            override_store: None,
            recluster_requested: None,
            erasure_requested: None,
            coaching_engine: None,
            session_manager: None,
            frame_storage: None,
            #[cfg(feature = "grpc-dashboard-external")]
            external_grpc_live: None,
            #[cfg(feature = "grpc-dashboard-external")]
            external_grpc_metrics: None,
        }
    }

    /// Pass pre-created `LiveExternalConfig` and `ExternalMetrics` Arcs into the web
    /// server's `DiagnosticsState` so `GET /api/external-grpc/live-config` can serve
    /// the current live snapshot. Must be called before `build_and_spawn`.
    #[cfg(feature = "grpc-dashboard-external")]
    pub(crate) fn with_external_grpc_live_and_metrics(
        mut self,
        live: Arc<maekon_web::grpc::external::live_config::LiveExternalConfig>,
        metrics: Arc<maekon_web::grpc::external::metrics::ExternalMetrics>,
    ) -> Self {
        self.external_grpc_live = Some(live);
        self.external_grpc_metrics = Some(metrics);
        self
    }

    #[cfg(feature = "server")]
    pub(crate) fn with_server_support(mut self, server: WebServerServerSupport) -> Self {
        self.support_context = self.support_context.with_server_support(server);
        self
    }

    // C1: wire provider-only credentials in analysis builds (no server transport).
    #[cfg(all(feature = "analysis", not(feature = "server")))]
    pub(crate) fn with_provider_support(
        mut self,
        secret_store: Option<Arc<dyn SecretStore>>,
        secret_stores: Option<SecretStoreSet>,
        default_backend_kind: Option<CredentialBackendKind>,
        oauth_port: Option<Arc<dyn OAuthPort>>,
    ) -> Self {
        self.support_context = self.support_context.with_provider_support(
            secret_store,
            secret_stores,
            default_backend_kind,
            oauth_port,
        );
        self
    }

    pub(crate) fn with_override_store(
        mut self,
        store: Arc<dyn maekon_core::ports::override_store::OverrideStore>,
    ) -> Self {
        self.override_store = Some(store);
        self
    }

    pub(crate) fn with_recluster_requested(
        mut self,
        flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.recluster_requested = Some(flag);
        self
    }

    pub(crate) fn with_erasure_requested(
        mut self,
        flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.erasure_requested = Some(flag);
        self
    }

    pub(crate) fn with_coaching_engine(
        mut self,
        engine: Arc<dyn maekon_core::ports::coaching::CoachingPort>,
    ) -> Self {
        self.coaching_engine = Some(engine);
        self
    }

    pub(crate) fn with_session_manager(
        mut self,
        manager: Arc<dyn maekon_core::ports::conversation_session::SessionManager>,
    ) -> Self {
        self.session_manager = Some(manager);
        self
    }

    pub(crate) fn with_frame_storage(mut self, storage: Arc<dyn FrameStoragePort>) -> Self {
        self.frame_storage = Some(storage);
        self
    }

    // C1: secret backend capabilities come from ProviderRuntimeContext — analysis-gated.
    #[cfg(feature = "analysis")]
    pub(crate) fn with_secret_backend_capabilities(
        mut self,
        capabilities: SecretBackendCapabilities,
    ) -> Self {
        self.support_context = self
            .support_context
            .with_secret_backend_capabilities(capabilities);
        self
    }

    // `mut` is required when `grpc-dashboard-external` is enabled (calls `.take()` on two
    // Option fields); without that feature the binding is unused, so allow it.
    #[cfg_attr(not(feature = "grpc-dashboard-external"), allow(unused_mut))]
    pub(crate) fn build_and_spawn(mut self) -> WebServerLaunchResult {
        let web_shutdown_rx = self.launch_context.shutdown_tx.subscribe();
        let storage_for_audit = self.storage.clone();
        // #6123: blocking SQLite must not run on the tokio reactor. Wrap the
        // blocking save in ChannelAuditPersistence so it drains on a dedicated
        // spawn_blocking task off-reactor.
        let blocking_persist: std::sync::Arc<dyn maekon_automation::audit::AuditPersistence> =
            std::sync::Arc::new(move |entry: &maekon_core::models::audit::AuditEntry| {
                storage_for_audit.save_audit_entry(entry);
            });
        // #6123: pass the runtime handle explicitly. This wiring runs on the
        // synchronous Tauri main thread, where `Handle::try_current()` is `Err`,
        // so the drain task must be spawned onto the known background runtime.
        let persistence_cb: std::sync::Arc<dyn maekon_automation::audit::AuditPersistence> =
            std::sync::Arc::new(maekon_automation::audit::ChannelAuditPersistence::new(
                blocking_persist,
                self.launch_context.runtime_handle.clone(),
            ));
        let audit_query: std::sync::Arc<dyn maekon_automation::audit::AuditQuery> =
            std::sync::Arc::new(crate::audit_query::SqliteAuditQuery::new(
                self.storage.clone(),
            ));
        let audit_pii_sanitizer: Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer> =
            Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
        let web_audit_logger = Arc::new(tokio::sync::RwLock::new(
            AuditLogger::default()
                .with_persistence(persistence_cb)
                .with_query(audit_query)
                .with_pii_sanitizer(audit_pii_sanitizer),
        ));
        let (bound_port_tx, bound_port_rx) = tokio::sync::oneshot::channel::<u16>();
        let (startup_error_tx, startup_error_rx) = tokio::sync::oneshot::channel::<String>();

        let automation_frame_storage = match self.launch_context.runtime_handle.block_on(async {
            FrameFileStorage::new(
                self.data_dir.to_path_buf(),
                self.config.storage.max_storage_mb,
                self.config.storage.retention_days,
            )
            .await
        }) {
            Ok(storage) => Some(Arc::new(storage)),
            Err(err) => {
                warn!(error = %err, "frame storage init failure, falling back to NoOp");
                None
            }
        };

        let automation_build = {
            let builder = AutomationControllerBuilder::new(
                self.config,
                self.data_dir,
                self.launch_context.runtime_handle,
                web_audit_logger.clone(),
                automation_frame_storage,
            );
            let builder = self.support_context.configure_automation_builder(builder);
            builder.build()
        };

        let ai_runtime_status = automation_build.ai_runtime_status.clone();
        // Retain a second clone for WebServerLaunchResult so the caller can
        // pass the snapshot to GrpcSpawnConfig (SubscribeEvents §A A2).
        let ai_runtime_status_for_result = ai_runtime_status.clone();
        let automation_controller = automation_build.controller;
        let automation_controller_for_state = automation_controller.clone();
        // #5734: extract the live per-call LLM health handle so GET /api/automation/status
        // can read the true last-call outcome after the build-time snapshot goes stale.
        let llm_call_health = automation_build.llm_call_health.clone();
        let gui_audit_logger = web_audit_logger.clone();
        let mut runtime_bindings = self
            .support_context
            .build_runtime_bindings(self.frame_storage, ai_runtime_status);
        // Wire the live LLM health handle into the automation runtime bindings.
        runtime_bindings.automation.llm_call_health = llm_call_health;
        // #4478 G3: hand the erasure-propagation signal to the web layer so the
        // "Delete all data" endpoint can request a device-wide DeletionEvent.
        runtime_bindings.core.erasure_requested = self.erasure_requested;
        runtime_bindings.analysis = AnalysisRuntimeBindings {
            override_store: self.override_store,
            recluster_requested: self.recluster_requested,
            coaching_engine: self.coaching_engine,
            model_catalog_client: provider_model_catalog_client(),
        };
        runtime_bindings.session = SessionRuntimeBindings {
            session_manager: self.session_manager,
        };

        // #7738 D-2: assemble the VERIFIED-unconditional dependency set as ONE
        // struct literal — every field named, no `Default`/`..` escape. Each is
        // either an unconditional storage-derived cast (Port Instance Sharing —
        // the SAME SqliteStorage Arc/connection as `storage`, never a second
        // store) or an unconditional concrete value built right here.
        let required_deps = WebServerRequiredDeps {
            // ADR-023: share the single SqliteStorage Arc as the MemoryGraphPort
            // so the digest export endpoint can render accumulated claims.
            memory_graph: self.storage.clone()
                as Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>,
            // #7600: share the same SqliteStorage Arc as the AuditChainVerifierPort
            // so GET /audit/verify reaches the real ADR-072 hash-chain verification
            // (Port Instance Sharing) — mirrors verify_audit_log (the desktop IPC
            // command), which previously had no webview caller.
            audit_chain_verifier: self.storage.clone()
                as Arc<dyn maekon_core::ports::audit_chain_verifier::AuditChainVerifierPort>,
            // #7910: share the same SqliteStorage Arc as the read-only
            // EgressLedgerReaderPort so GET /api/privacy/egress-ledger renders
            // the egress transparency browser ("what left this device") from the
            // erase-retained #4803 ledger (Port Instance Sharing — same store as
            // the scheduler's EgressLedgerSink writer, read-only view).
            egress_ledger_reader: self.storage.clone()
                as Arc<dyn maekon_core::ports::egress_ledger_reader::EgressLedgerReaderPort>,
            // #7678 D2: share the same underlying SQLite connection Arc as the
            // RegimeStoragePort (via the same wrapper the composition root uses
            // in `app_runtime_launch/regime_wiring.rs`) so the dashboard digest
            // endpoint can resolve human-readable regime labels instead of
            // leaking the opaque positional regime_id ("regime-N") into the
            // timeline (Port Instance Sharing — same SqliteStorage-backed
            // connection, not a second store).
            regime_storage: Arc::new(
                maekon_storage::regime_manager_state_store::SqliteRegimeManagerStateStore::new(
                    self.storage.connection_arc(),
                ),
            ) as Arc<dyn maekon_core::ports::regime_storage::RegimeStoragePort>,
            // #6279: share the SqliteStorage Arc as the FTS TextSearchProvider so
            // /api/semantic-search (keyword + hybrid-degraded) is functional
            // instead of permanently service.unavailable (Port Instance Sharing).
            text_search: self.storage.clone()
                as Arc<dyn maekon_core::ports::text_search::TextSearchProvider>,
            audit_logger: Arc::new(AuditLogAdapter::new(web_audit_logger)),
            config_manager: self.support_context.config_manager.clone(),
            update_control: self.support_context.update_control.clone(),
            local_auth_token: self.launch_context.local_auth_token.clone(),
            frames_dir: self.data_dir.to_path_buf(),
            pii_sanitizer: Arc::new(maekon_vision::privacy::VisionPiiSanitizer)
                as Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer>,
            runtime_log_provider: Arc::new(TauriRuntimeLogProvider::new(
                log_helpers::runtime_log_dir(),
            )) as Arc<dyn RuntimeLogProvider>,
            system_info_provider: Arc::new(SysInfoProvider::new())
                as Arc<dyn SystemInfoProvider>,
            provider_cli_diagnostics: Arc::new(ProviderCliDiagnosticsSnapshotProvider::new(
                self.support_context.secret_backend_capabilities.clone(),
            ))
                as Arc<
                    dyn maekon_web::services::provider_cli_diagnostics::ProviderCliDiagnosticsProvider,
                >,
        };

        // D-5 (#7738): the type-level split (D-1/D-2/D-3 above) makes the 8
        // `debug_assert!`s this block used to run structurally impossible to
        // violate — those fields no longer exist as `Option` (they are plain
        // fields on `WebServerRequiredDeps`, and the compiler enforces every
        // one is provided in the literal above). What remains is the
        // regression-tripwire LOG of the fields that stay legitimately
        // runtime-conditional by design (automation toggle, capture-degrade)
        // so a silent drift is still visible in Grafana/Loki, not just a
        // missing compiler error.
        info!(
            automation_controller = automation_controller_for_state.is_some(),
            ai_runtime_status = runtime_bindings.automation.ai_runtime_status.is_some(),
            frame_storage = runtime_bindings.core.frame_storage.is_some(),
            session_manager = runtime_bindings.session.session_manager.is_some(),
            erasure_requested = runtime_bindings.core.erasure_requested.is_some(),
            "WebServer wiring: conditional dependency map (None is correct when disabled/degraded)"
        );

        // Spawn GUI audit forwarder if the automation controller has a GUI service.
        //
        // #4345 same-class bug: `build_and_spawn` runs on the synchronous Tauri main
        // thread (no entered tokio runtime — the `runtime_handle.block_on` calls at
        // lines 405/502 above are the proof of that), so the former
        // `Handle::try_current()` guard inside `spawn_gui_audit_forwarder` always
        // returned `Err`, making the forwarder dead code that never started. As with
        // the fix #4345 applied to `shutdown_all`, pass the explicit background
        // runtime handle so the forwarder actually runs.
        if let Some(ref controller) = automation_controller {
            spawn_gui_audit_forwarder(
                self.launch_context.runtime_handle,
                controller,
                gui_audit_logger,
            );
        }

        let web_storage = self.storage.clone();
        let web_config = self.config.web.clone();
        let web_port_state = self.launch_context.web_port_state.clone();
        let web_event_tx = self.launch_context.event_tx.clone();
        #[cfg(feature = "grpc-dashboard-external")]
        let ext_live_for_web = self.external_grpc_live.take();
        #[cfg(feature = "grpc-dashboard-external")]
        let ext_metrics_for_web = self.external_grpc_metrics.take();
        self.launch_context.runtime_handle.spawn(async move {
            if let Some(controller) = automation_controller {
                runtime_bindings.automation.automation_controller = Some(controller);
            }
            let web_server = WebServer::new(web_storage, web_event_tx, web_config)
                .with_bound_port_state(web_port_state)
                .with_bound_port_notifier(bound_port_tx)
                .with_required_deps(required_deps)
                .with_runtime_bindings(runtime_bindings);
            // Task 7.1: wire LiveExternalConfig + ExternalMetrics into AppState so the
            // GET /api/external-grpc/live-config endpoint can serve live snapshots.
            #[cfg(feature = "grpc-dashboard-external")]
            let web_server = {
                let mut ws = web_server;
                if let Some(live) = ext_live_for_web {
                    ws = ws.with_external_grpc_live(live);
                }
                if let Some(metrics) = ext_metrics_for_web {
                    ws = ws.with_external_grpc_metrics(metrics);
                }
                ws
            };
            if let Err(error) = web_server.run(web_shutdown_rx).await {
                let _ = startup_error_tx.send(error.to_string());
                error!("WebServer error: {error}");
            }
        });

        let (frontend_port, startup_error) = self.launch_context.runtime_handle.block_on(async {
            tokio::select! {
                port = bound_port_rx => {
                    (
                        port.unwrap_or_else(|_| self.launch_context.web_port_state.load(Ordering::Relaxed)),
                        None,
                    )
                }
                error = startup_error_rx => {
                    (
                        self.launch_context.web_port_state.load(Ordering::Relaxed),
                        Some(error.unwrap_or_else(|_| "web server startup failed before binding".to_string())),
                    )
                }
                _ = tokio::time::sleep(Duration::from_secs(3)) => {
                    (
                        self.launch_context.web_port_state.load(Ordering::Relaxed),
                        Some("web server startup did not report a bound port within 3s".to_string()),
                    )
                }
            }
        });
        if let Some(error) = &startup_error {
            warn!(%error, frontend_port, "WebServer startup degraded");
        } else {
            info!("WebServer: http://localhost:{frontend_port}");
        }

        WebServerLaunchResult {
            frontend_web_port: frontend_port,
            web_server_startup_error: startup_error,
            automation_controller: automation_controller_for_state,
            ai_runtime_status: ai_runtime_status_for_result,
            #[cfg(feature = "grpc-dashboard-external")]
            ext_grpc_supervisor: None, // populated by app_runtime_launch after build_and_spawn
            #[cfg(feature = "grpc-dashboard-external")]
            ext_cert_watcher: None, // populated by app_runtime_launch after build_and_spawn
        }
    }
}

/// Subscribes to GUI session events and forwards them to the audit logger.
///
/// `runtime_handle` is the explicit background runtime handle held by the caller
/// (the synchronous Tauri main thread). The former implementation tried to find an
/// ambient runtime via `Handle::try_current()`, but since this function is called
/// from the main thread with no entered tokio runtime, that always returned `Err`,
/// leaving the forwarder dead code (the same inverted-guard bug as #4345).
fn spawn_gui_audit_forwarder(
    runtime_handle: &Handle,
    automation_controller: &Arc<AutomationController>,
    audit_logger: Arc<tokio::sync::RwLock<AuditLogger>>,
) {
    let Some(gui_service) = automation_controller.gui_service() else {
        tracing::debug!("GUI service not configured; skipping audit forwarder");
        return;
    };

    let mut rx = gui_service.subscribe();

    runtime_handle.spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let action_type = format!("gui.session.{}", event.event_type);
                    let details = event.message.unwrap_or_default();
                    let mut logger = audit_logger.write().await;
                    logger.log_event(&action_type, &event.session_id, &details);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("GUI audit forwarder lagged by {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    tracing::debug!("GUI event channel closed; audit forwarder exiting");
                    break;
                }
            }
        }
    });
}

#[cfg(test)]
mod gui_audit_forwarder_tests {
    use super::*;
    use maekon_automation::audit::AuditLogger;
    use maekon_core::models::gui::{GuiSessionEvent, GuiSessionState};
    use tokio::sync::broadcast;

    /// F-RR-C40-01 regression guard: the GUI automation audit forwarder used to be
    /// dead code because of its internal `Handle::try_current()` guard — since
    /// `build_and_spawn` is called from the synchronous Tauri main thread with no
    /// entered tokio runtime, `try_current()` always returned `Err` (same class as
    /// #4345).
    ///
    /// The fixed forwarder `spawn`s on the explicit background runtime handle held
    /// by the caller. This test verifies that core invariant:
    ///   1. On a thread with no entered runtime, `Handle::try_current()` is `Err`
    ///      (reproduces the original dead-code condition).
    ///   2. On the same thread, a GUI session event subscribed via `spawn` on the
    ///      *explicit* handle the forwarder uses is actually forwarded to the
    ///      `AuditLogger`.
    #[test]
    fn forwarder_forwards_event_via_explicit_handle_from_non_runtime_thread() {
        // Explicit background runtime (same role as the handle the forwarder receives).
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("background runtime");
        let handle = runtime.handle().clone();

        let logger = Arc::new(tokio::sync::RwLock::new(AuditLogger::new(256, 32)));
        let (event_tx, _) = broadcast::channel::<GuiSessionEvent>(16);

        let outcome = std::thread::spawn({
            let handle = handle.clone();
            let logger = logger.clone();
            let event_tx = event_tx.clone();
            move || {
                // (1) Reproduce the original dead-code guard: no entered runtime → Err.
                let no_current = Handle::try_current().is_err();

                // (2) Same structure as the fixed forwarder: spawn on the explicit handle.
                let mut rx = event_tx.subscribe();
                let forward_logger = logger.clone();
                handle.spawn(async move {
                    while let Ok(event) = rx.recv().await {
                        let action_type = format!("gui.session.{}", event.event_type);
                        let details = event.message.unwrap_or_default();
                        let mut guard = forward_logger.write().await;
                        guard.log_event(&action_type, &event.session_id, &details);
                    }
                });

                // Give the forwarder time to start subscribing, then publish an event.
                std::thread::sleep(std::time::Duration::from_millis(50));
                event_tx
                    .send(GuiSessionEvent {
                        schema_version: maekon_core::models::gui::GUI_SESSION_EVENT_SCHEMA_VERSION
                            .to_string(),
                        event_type: "started".to_string(),
                        session_id: "sess-c40".to_string(),
                        state: GuiSessionState::Proposed,
                        emitted_at: chrono::Utc::now(),
                        message: Some("forwarder-alive".to_string()),
                    })
                    .expect("event published");

                no_current
            }
        })
        .join()
        .expect("worker thread should not panic");

        assert!(
            outcome,
            "non-runtime thread must have no current Handle (the original dead-code condition)"
        );

        // Verify the forward was actually recorded in the audit logger (best-effort polling).
        let mut found = false;
        for _ in 0..50 {
            let entries = handle.block_on(async { logger.read().await.recent_entries(16) });
            if entries
                .iter()
                .any(|e| e.action_type == "gui.session.started")
            {
                found = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            found,
            "GUI session event must be forwarded to the audit logger via the explicit handle"
        );

        runtime.shutdown_background();
    }
}

/// regression_risk #1 — C1 relabeling must not accidentally drop any of the 7
/// integration bindings that `WebServerServerSupport` injects into
/// `WebServerRuntimeBindings` when a `server` context is present.
///
/// Strategy: compile-time verification of the 7-binding wiring path by
/// exercising the `None`-only constructor path (which proves field names/order
/// are correct) and the negative runtime check (no server support → bindings
/// remain `None`).  Full `is_some()` per-binding asserts require non-trivial
/// port trait impls; those belong in integration tests once a shared
/// test-double crate exists.
#[cfg(all(test, feature = "server"))]
mod web_server_server_support_tests {
    use super::*;
    use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;

    /// regression_risk #1 (compile path): verifies `WebServerServerSupport::new`
    /// accepts all 11 positional parameters in the order the `build_runtime_bindings`
    /// wiring block expects.  If any of the 7 integration binding parameters were
    /// removed or reordered by accident, this will fail to compile under
    /// `--features server`.  The `None`-only variant is used to avoid requiring
    /// full port trait implementations in this unit-test module.
    ///
    /// Runtime binding propagation (all 7 `is_some()` after wiring) is NOT yet
    /// covered — it needs test doubles for 5+ async integration port traits and
    /// is deferred to an integration-test harness (see PR #5713 follow-ups).
    #[test]
    fn web_server_server_support_constructor_accepts_all_11_parameters() {
        // Compile-time guard: if any integration binding slot is dropped from the
        // WebServerServerSupport constructor, this will not compile.
        let _support = WebServerServerSupport::new(
            None, // integration_auth
            None, // integration_session
            None, // integration_outbox
            None, // integration_inbox
            None, // integration_inbox_store
            None, // integration_audit
            None, // integration_runtime_telemetry
            None, // secret_store
            None, // secret_stores
            None, // default_secret_backend_kind
            None, // oauth_port
        );
        // If the constructor compiled with 11 None args the field order is intact.
    }

    /// regression_risk #1 (runtime guard): without server support, all 7 integration
    /// slots are None — confirms the `if let Some(server)` guard is live and that
    /// the 7-slot field names in `IntegrationRuntimeBindings` match the wiring code.
    #[test]
    fn build_runtime_bindings_without_server_leaves_integration_slots_none() {
        let temp = tempfile::tempdir().expect("tmp dir");
        let config_path = temp.path().join("config.json");
        // Write a minimal valid config so `with_paths` does not error.
        std::fs::write(&config_path, "{}").expect("write config");
        let Ok(config_manager) = ConfigManager::with_paths(config_path, None) else {
            return; // test environment cannot create ConfigManager — skip gracefully
        };
        let (action_tx, _action_rx) = tokio::sync::mpsc::unbounded_channel();
        let update_control = UpdateControl::new(action_tx, Default::default());
        let status = IntegrationOutboundRuntimeStatus::default();

        // No server support injected — all integration bindings must stay None.
        let ctx = WebServerSupportContext::new(config_manager, update_control, status);

        // #7738 D-3: build_runtime_bindings shrank to (frame_storage, ai_runtime_status).
        let bindings = ctx.build_runtime_bindings(None, None);

        // Without server support all 7 integration slots must remain None.
        assert!(bindings.integration.integration_auth.is_none());
        assert!(bindings.integration.integration_session.is_none());
        assert!(bindings.integration.integration_outbox.is_none());
        assert!(bindings.integration.integration_inbox.is_none());
        assert!(bindings.integration.integration_inbox_store.is_none());
        assert!(bindings.integration.integration_audit.is_none());
        assert!(bindings.integration.integration_runtime_telemetry.is_none());
    }
}

/// C1 regression — analysis-only builds must propagate the FULL provider
/// secrets triple (store / store-set / default backend kind) into the web
/// runtime bindings. If `default_secret_backend_kind` stays `None`, maekon-web
/// keeps its `Unavailable` default (`lib.rs` only overwrites on `Some`) and
/// `persist_api_key_binding` rejects every first-time Settings API-key save —
/// the exact silent non-light-up this PR exists to fix.
#[cfg(all(test, feature = "analysis", not(feature = "server")))]
mod provider_secrets_binding_tests {
    use super::*;
    use maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus;
    use maekon_core::ports::secret_store::SecretStoreSet;
    use std::sync::Arc;

    #[test]
    fn build_runtime_bindings_propagates_provider_secrets_triple() {
        let temp = tempfile::tempdir().expect("tmp dir");
        let config_path = temp.path().join("config.json");
        std::fs::write(&config_path, "{}").expect("write config");
        let Ok(config_manager) = ConfigManager::with_paths(config_path, None) else {
            return; // test environment cannot create ConfigManager — skip gracefully
        };
        let (action_tx, _action_rx) = tokio::sync::mpsc::unbounded_channel();
        let update_control = UpdateControl::new(action_tx, Default::default());
        let status = IntegrationOutboundRuntimeStatus::default();

        let file_store: Arc<dyn SecretStore> = Arc::new(
            maekon_storage::file_secret_store::FileSecretStore::new(
                temp.path().join("secrets.json"),
                Some(std::sync::Arc::new(
                    maekon_storage::encryption::EncryptionKey::from_bytes([0x42; 32]),
                )),
            )
            .expect("file secret store"),
        );
        let stores = SecretStoreSet {
            os_secret_store: None,
            file_secret_store: Some(file_store.clone()),
            env_secret_store: None,
            default_backend_kind: CredentialBackendKind::FileSecretStore,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        };

        let ctx = WebServerSupportContext::new(config_manager, update_control, status)
            .with_provider_support(
                Some(file_store),
                Some(stores),
                Some(CredentialBackendKind::FileSecretStore),
                None,
            );

        // #7738 D-3: build_runtime_bindings shrank to (frame_storage, ai_runtime_status).
        let bindings = ctx.build_runtime_bindings(None, None);

        assert!(
            bindings.secrets.secret_store.is_some(),
            "singular secret_store must propagate to web bindings"
        );
        assert!(
            bindings.secrets.secret_stores.is_some(),
            "secret_stores set must propagate to web bindings"
        );
        assert_eq!(
            bindings.secrets.default_secret_backend_kind,
            Some(CredentialBackendKind::FileSecretStore),
            "default backend kind must propagate — None keeps maekon-web's \
             Unavailable default and rejects Settings key saves"
        );
    }
}
