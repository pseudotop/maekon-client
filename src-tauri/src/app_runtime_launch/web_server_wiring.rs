#[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
use super::external_grpc::{spawn_dashboard_grpc_servers, DashboardGrpcSharedState};
use super::session_wiring::SessionManagerLaunch;
#[cfg(feature = "analysis")]
use crate::provider_runtime_context::ProviderRuntimeContext;
#[cfg(feature = "server")]
use crate::server_runtime_context::ServerLaunchContext;
use crate::web_server_runtime::{
    WebServerLaunchContext, WebServerRuntimeBuilder, WebServerSupportContext,
};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use std::sync::Arc;

pub(super) struct WebAutomationWiring {
    pub(super) frontend_web_port: u16,
    pub(super) web_server_startup_error: Option<String>,
    pub(super) automation_controller:
        Option<Arc<maekon_automation::controller::AutomationController>>,
    #[cfg(feature = "grpc-dashboard-external")]
    pub(super) ext_grpc_supervisor: Option<tokio::task::JoinHandle<()>>,
    #[cfg(feature = "grpc-dashboard-external")]
    pub(super) ext_cert_watcher: Option<maekon_web::grpc::external::tls_config::CertWatcherHandle>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn build_web_automation_wiring(
    app_handle: &tauri::AppHandle,
    handle: &tokio::runtime::Handle,
    shutdown_tx: &tokio::sync::watch::Sender<bool>,
    event_tx: tokio::sync::broadcast::Sender<maekon_web::RealtimeEvent>,
    web_port: Arc<std::sync::atomic::AtomicU16>,
    local_auth_token: Arc<str>,
    integration_runtime_status: maekon_api_contracts::integration::IntegrationOutboundRuntimeStatus,
    config_manager: maekon_core::config_manager::ConfigManager,
    update_control: maekon_web::update_control::UpdateControl,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    config: &maekon_core::config::AppConfig,
    data_dir_path: &std::path::Path,
    recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    erasure_requested: Arc<std::sync::atomic::AtomicBool>,
    coaching_engine: Arc<maekon_analysis::CoachingEngine>,
    session_manager: &SessionManagerLaunch,
    shared_capture_services: Option<&Arc<crate::capture_services::SharedCaptureServices>>,
    capture_consent_manager: Arc<dyn ConsentManagerPort>,
    cli_health_flag: Arc<std::sync::atomic::AtomicBool>,
    // D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    // registry from the composition root, forwarded to the web server's
    // automation controller builder.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    // #7932 Part B: the ONE shared Arc<PolicyClient> from the composition root,
    // forwarded to the automation controller builder so the controller shares one
    // durable policy store instance with the Codex approval decider.
    policy_client: Arc<maekon_automation::policy::PolicyClient>,
    #[cfg(feature = "analysis")] provider_context: &ProviderRuntimeContext,
    #[cfg(feature = "server")] server_context: &ServerLaunchContext,
) -> WebAutomationWiring {
    let launch_context = WebServerLaunchContext::new(
        handle,
        shutdown_tx,
        event_tx.clone(),
        web_port,
        // #6420: clone so the loopback gRPC dashboard can require the same per-session
        // local-auth token the REST server enforces (the original is threaded below).
        local_auth_token.clone(),
    );
    let support_context = WebServerSupportContext::new(
        config_manager.clone(),
        update_control,
        integration_runtime_status,
    )
    .with_app_handle(app_handle.clone())
    .with_cli_health_flag(cli_health_flag)
    .with_consent_manager(capture_consent_manager)
    .with_breaker_registry(breaker_registry)
    // #7932 Part B: share the single Arc<PolicyClient> with the automation
    // controller (Port Instance Sharing).
    .with_policy_client(policy_client);
    let mut builder = WebServerRuntimeBuilder::new(
        sqlite_storage.clone(),
        config,
        data_dir_path,
        launch_context,
        support_context,
    )
    .with_override_store(sqlite_storage.clone())
    .with_recluster_requested(recluster_requested)
    .with_erasure_requested(erasure_requested)
    .with_coaching_engine(coaching_engine as Arc<dyn maekon_core::ports::coaching::CoachingPort>);
    if let Some((sm, _)) = session_manager {
        builder = builder.with_session_manager(
            sm.clone() as Arc<dyn maekon_core::ports::conversation_session::SessionManager>
        );
    }
    if let Some(capture_services) = shared_capture_services {
        builder = builder.with_frame_storage(capture_services.frame_storage.clone());
    }

    #[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
    let dashboard_grpc_shared = DashboardGrpcSharedState::new(config);
    #[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
    let builder = dashboard_grpc_shared.configure_web_server_builder(builder);
    // C1: wire provider (BYOK/OAuth) credentials under 'analysis' so default
    // builds without 'server' can resolve remote LLM/OCR provider adapters.
    // server always implies analysis (Cargo feature graph), so the 'server but
    // not analysis' branch is unreachable in practice but is kept for correctness.
    #[cfg(all(feature = "analysis", feature = "server"))]
    let builder = server_context.configure_web_server_builder(
        builder,
        provider_context.secret_backend_capabilities(),
        provider_context
            .provider_secret_backend
            .secret_store
            .clone(),
        Some(provider_context.provider_secret_stores.clone()),
        Some(provider_context.provider_secret_backend.backend_kind),
        provider_context.oauth_port.clone(),
    );
    // Analysis-only (no server): wire provider credentials without integration.
    // Pass the full secrets triple — the same values the server branch hands to
    // configure_web_server_builder — so Settings key persistence works.
    #[cfg(all(feature = "analysis", not(feature = "server")))]
    let builder = builder
        .with_provider_support(
            provider_context
                .provider_secret_backend
                .secret_store
                .clone(),
            Some(provider_context.provider_secret_stores.clone()),
            Some(provider_context.provider_secret_backend.backend_kind),
            provider_context.oauth_port.clone(),
        )
        .with_secret_backend_capabilities(provider_context.secret_backend_capabilities());

    #[allow(unused_mut)]
    let mut web_server_runtime = builder.build_and_spawn();

    #[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
    spawn_dashboard_grpc_servers(
        handle,
        event_tx,
        sqlite_storage.clone(),
        config,
        config_manager,
        local_auth_token,
        &mut web_server_runtime,
        dashboard_grpc_shared,
    );

    #[cfg(feature = "grpc-dashboard-external")]
    let ext_grpc_supervisor = web_server_runtime.ext_grpc_supervisor.take();
    #[cfg(feature = "grpc-dashboard-external")]
    let ext_cert_watcher = web_server_runtime.ext_cert_watcher.take();

    WebAutomationWiring {
        frontend_web_port: web_server_runtime.frontend_web_port,
        web_server_startup_error: web_server_runtime.web_server_startup_error,
        automation_controller: web_server_runtime.automation_controller,
        #[cfg(feature = "grpc-dashboard-external")]
        ext_grpc_supervisor,
        #[cfg(feature = "grpc-dashboard-external")]
        ext_cert_watcher,
    }
}
