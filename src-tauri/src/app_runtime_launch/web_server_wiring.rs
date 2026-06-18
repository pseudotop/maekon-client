#[cfg(feature = "grpc-dashboard-external")]
use super::external_grpc::build_external_spawn_config;
#[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
use super::external_grpc::resolve_loopback_grpc_port;
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
    .with_breaker_registry(breaker_registry);
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

    #[cfg(feature = "grpc-dashboard-external")]
    let (ext_shared_live, ext_shared_metrics) = {
        if config.external_grpc.enabled {
            let initial_streaming = config
                .external_grpc
                .streaming_enabled
                .unwrap_or(config.web.grpc_streaming_enabled);
            let initial_thresholds = config.web.grpc_load_thresholds.clone().unwrap_or_default();
            let initial_policy = maekon_web::grpc::LoadPolicy::try_new(initial_thresholds)
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        err = %e,
                        "external_grpc: invalid LoadThresholds at pre-creation; using defaults"
                    );
                    maekon_web::grpc::LoadPolicy::new(Default::default())
                });
            let live = Arc::new(
                maekon_web::grpc::external::live_config::LiveExternalConfig::new(
                    maekon_web::grpc::external::live_config::LiveSnapshot {
                        streaming_enabled: initial_streaming,
                        load_policy: Arc::new(initial_policy),
                    },
                ),
            );
            let metrics = Arc::new(maekon_web::grpc::external::metrics::ExternalMetrics::new());
            (Some(live), Some(metrics))
        } else {
            (None, None)
        }
    };
    #[cfg(feature = "grpc-dashboard-external")]
    let builder = match (&ext_shared_live, &ext_shared_metrics) {
        (Some(live), Some(metrics)) => {
            builder.with_external_grpc_live_and_metrics(live.clone(), metrics.clone())
        }
        _ => builder,
    };
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

    let web_server_runtime = builder.build_and_spawn();
    #[cfg(feature = "grpc-dashboard-external")]
    let mut web_server_runtime = web_server_runtime;

    #[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
    spawn_dashboard_grpc_servers(
        handle,
        event_tx,
        sqlite_storage.clone(),
        config,
        config_manager,
        local_auth_token,
        #[cfg(feature = "grpc-dashboard-external")]
        &mut web_server_runtime,
        #[cfg(not(feature = "grpc-dashboard-external"))]
        &web_server_runtime,
        #[cfg(feature = "grpc-dashboard-external")]
        ext_shared_live,
        #[cfg(feature = "grpc-dashboard-external")]
        ext_shared_metrics,
    );

    #[cfg(feature = "grpc-dashboard-external")]
    let ext_grpc_supervisor = web_server_runtime.ext_grpc_supervisor.take();
    #[cfg(feature = "grpc-dashboard-external")]
    let ext_cert_watcher = web_server_runtime.ext_cert_watcher.take();

    WebAutomationWiring {
        automation_controller: web_server_runtime.automation_controller,
        #[cfg(feature = "grpc-dashboard-external")]
        ext_grpc_supervisor,
        #[cfg(feature = "grpc-dashboard-external")]
        ext_cert_watcher,
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
fn spawn_dashboard_grpc_servers(
    handle: &tokio::runtime::Handle,
    event_tx: tokio::sync::broadcast::Sender<maekon_web::RealtimeEvent>,
    sqlite_storage: Arc<maekon_storage::sqlite::SqliteStorage>,
    config: &maekon_core::config::AppConfig,
    config_manager: maekon_core::config_manager::ConfigManager,
    // #6420: per-session local-auth token — the loopback gRPC dashboard requires it
    // (same token the REST `/api` surface enforces via `require_local_auth`).
    local_auth_token: Arc<str>,
    #[cfg(feature = "grpc-dashboard-external")]
    web_server_runtime: &mut crate::web_server_runtime::WebServerLaunchResult,
    #[cfg(not(feature = "grpc-dashboard-external"))]
    web_server_runtime: &crate::web_server_runtime::WebServerLaunchResult,
    #[cfg(feature = "grpc-dashboard-external")] ext_shared_live: Option<
        Arc<maekon_web::grpc::external::live_config::LiveExternalConfig>,
    >,
    #[cfg(feature = "grpc-dashboard-external")] ext_shared_metrics: Option<
        Arc<maekon_web::grpc::external::metrics::ExternalMetrics>,
    >,
) {
    #[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
    let shared_grpc_monitor: Arc<dyn maekon_core::ports::monitor::SystemMonitor> =
        Arc::new(maekon_monitor::system::SysInfoMonitor::new());

    #[cfg(feature = "grpc-dashboard")]
    {
        let grpc_port = resolve_loopback_grpc_port(config.web.grpc_port);
        let grpc_storage = sqlite_storage.clone() as Arc<dyn maekon_web::storage_port::WebStorage>;
        let thresholds = config.web.grpc_load_thresholds.clone().unwrap_or_default();
        let load_policy = Arc::new(maekon_web::grpc::LoadPolicy::new(thresholds));
        let grpc_pii_sanitizer = Arc::new(maekon_vision::privacy::VisionPiiSanitizer)
            as Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer>;
        let cfg = maekon_web::grpc::GrpcSpawnConfig {
            port: grpc_port,
            storage: grpc_storage,
            system_monitor: shared_grpc_monitor.clone(),
            event_tx: event_tx.clone(),
            integration_auth_token: config.web.integration_auth_token.clone(),
            local_auth_token: Some(local_auth_token),
            pii_sanitizer: Some(grpc_pii_sanitizer),
            ai_runtime_status_snapshot: web_server_runtime.ai_runtime_status.clone(),
            load_policy,
            streaming_enabled: config.web.grpc_streaming_enabled,
            max_concurrent_streams: config.web.grpc_max_concurrent_streams,
        };
        handle.spawn(async move {
            maekon_web::grpc::serve_optional(cfg).await;
        });
    }

    #[cfg(feature = "grpc-dashboard-external")]
    {
        let ext_cfg = &config.external_grpc;
        if !ext_cfg.enabled {
            return;
        }

        let loopback_port = resolve_loopback_grpc_port(config.web.grpc_port);
        if let Err(msg) = maekon_web::grpc::external::port_collision::check_port_collision(
            ext_cfg.port,
            loopback_port,
        ) {
            tracing::error!(
                external_port = ext_cfg.port,
                loopback_port,
                err = %msg,
                "external_grpc: port collides with loopback grpc port; disabling external server"
            );
            return;
        }
        if let Err(e) = ext_cfg.validate() {
            tracing::error!(err = %e, "external_grpc: config validation failed; disabling");
            return;
        }

        let ext_storage = sqlite_storage.clone() as Arc<dyn maekon_web::storage_port::WebStorage>;
        let ext_audit: Arc<dyn maekon_core::ports::audit_log::AuditLogPort> = {
            let storage_for_audit = sqlite_storage.clone();
            // #6123: blocking SQLite must not run on the tokio reactor. Wrap the
            // blocking save in ChannelAuditPersistence so it drains on a
            // dedicated spawn_blocking task off-reactor.
            let blocking_persist: Arc<dyn maekon_automation::audit::AuditPersistence> =
                Arc::new(move |entry: &maekon_core::models::audit::AuditEntry| {
                    storage_for_audit.save_audit_entry(entry);
                });
            // #6123: pass the runtime handle explicitly. This wiring runs on the
            // synchronous Tauri main thread, where `Handle::try_current()` is
            // `Err`, so spawn the drain task onto the known runtime handle.
            let persistence_cb: Arc<dyn maekon_automation::audit::AuditPersistence> =
                Arc::new(maekon_automation::audit::ChannelAuditPersistence::new(
                    blocking_persist,
                    handle.clone(),
                ));
            let audit_query: Arc<dyn maekon_automation::audit::AuditQuery> = Arc::new(
                crate::audit_query::SqliteAuditQuery::new(sqlite_storage.clone()),
            );
            let audit_pii_sanitizer: Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer> =
                Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
            let logger = Arc::new(tokio::sync::RwLock::new(
                maekon_automation::audit::AuditLogger::new(500, 50)
                    .with_persistence(persistence_cb)
                    .with_query(audit_query)
                    .with_pii_sanitizer(audit_pii_sanitizer),
            ));
            Arc::new(maekon_automation::audit::AuditLogAdapter::new(logger))
        };
        let ext_pii_sanitizer: Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer> =
            Arc::new(maekon_vision::privacy::VisionPiiSanitizer);
        let ext_ai_status = web_server_runtime.ai_runtime_status.clone();
        let ext_app_config_snapshot = Arc::new(config.clone());

        match handle.block_on(build_external_spawn_config(
            ext_cfg,
            ext_storage,
            shared_grpc_monitor,
            event_tx,
            ext_audit,
            Some(ext_pii_sanitizer),
            ext_ai_status,
            config_manager,
            ext_app_config_snapshot,
            ext_shared_live,
            ext_shared_metrics,
        )) {
            Ok((spawn_cfg, cert_watcher)) => {
                let ext_handle =
                    handle.block_on(maekon_web::grpc::external::spawn_with_supervisor(spawn_cfg));
                web_server_runtime.ext_grpc_supervisor = Some(ext_handle);
                web_server_runtime.ext_cert_watcher = Some(cert_watcher);
                tracing::info!(
                    bind = %format!("{}:{}", ext_cfg.bind_address, ext_cfg.port),
                    auth_mode = ?ext_cfg.auth_mode,
                    "external_grpc: server spawned"
                );
            }
            Err(e) => {
                tracing::error!(err = %e, "external_grpc: failed to build spawn config; disabling");
            }
        }
    }
}
