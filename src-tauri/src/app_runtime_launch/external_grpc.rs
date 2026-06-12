#[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
pub(super) fn resolve_loopback_grpc_port(configured_port: u16) -> u16 {
    std::env::var("MAEKON_DASHBOARD_GRPC_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            if configured_port == 0 {
                maekon_web::grpc::DEFAULT_GRPC_DASHBOARD_PORT
            } else {
                configured_port
            }
        })
}

/// Build an `ExternalGrpcSpawnConfig` from the external gRPC section of `AppConfig`.
///
/// Loads TLS key material, constructs the `HotReloadCertResolver`, and
/// optionally builds `JwtVerifier` / `MtlsVerifier` according to `auth_mode`.
/// Returns `Err` if any required path is missing or key material fails to load.
///
/// `pre_live` and `pre_metrics` are pre-created Arcs shared with the web server's
/// `DiagnosticsState` (Task 7.1). When `Some`, they are used directly; when
/// `None`, fresh instances are created (backwards-compatible for tests).
#[cfg(feature = "grpc-dashboard-external")]
#[allow(clippy::too_many_arguments)]
pub(super) async fn build_external_spawn_config(
    cfg: &maekon_core::config::ExternalGrpcConfig,
    storage: std::sync::Arc<dyn maekon_web::storage_port::WebStorage>,
    system_monitor: std::sync::Arc<dyn maekon_core::ports::monitor::SystemMonitor>,
    event_tx: tokio::sync::broadcast::Sender<maekon_api_contracts::stream::RealtimeEvent>,
    audit_port: std::sync::Arc<dyn maekon_core::ports::audit_log::AuditLogPort>,
    pii_sanitizer: Option<std::sync::Arc<dyn maekon_core::ports::pii_sanitizer::PiiSanitizer>>,
    ai_runtime_status_snapshot: Option<maekon_api_contracts::stream::AiRuntimeStatus>,
    config_manager: maekon_core::config_manager::ConfigManager,
    app_config_snapshot: std::sync::Arc<maekon_core::config::AppConfig>,
    pre_live: Option<std::sync::Arc<maekon_web::grpc::external::live_config::LiveExternalConfig>>,
    pre_metrics: Option<std::sync::Arc<maekon_web::grpc::external::metrics::ExternalMetrics>>,
) -> anyhow::Result<(
    maekon_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig,
    maekon_web::grpc::external::tls_config::CertWatcherHandle,
)> {
    use anyhow::Context as _;
    use maekon_web::grpc::external::{
        cert_resolver::HotReloadCertResolver, ip_ban::IpBan, jwt_verifier::JwtVerifier,
        metrics::ExternalMetrics, mtls_verifier::MtlsVerifier, tls_config::load_certified_key,
    };

    let cert = load_certified_key(
        cfg.tls_cert_path
            .as_ref()
            .context("tls_cert_path required when external_grpc.enabled")?,
        cfg.tls_key_path
            .as_ref()
            .context("tls_key_path required when external_grpc.enabled")?,
    )?;
    let cert_resolver = std::sync::Arc::new(HotReloadCertResolver::new(cert));

    // Create the shutdown channel shared by: cert watcher, expiry monitor, and tonic server.
    // The Sender Arc is kept alive in ExternalGrpcSpawnConfig.shutdown_tx, which is held by
    // spawn_with_supervisor for the server lifetime. Dropping the last Arc clone closes the
    // channel and causes all shutdown_rx listeners to exit.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let shutdown_tx = std::sync::Arc::new(shutdown_tx);

    // Spawn cert hot-reload watcher and daily expiry monitor.
    // Both are best-effort: errors are logged but do not prevent startup.
    let metrics_arc = pre_metrics.unwrap_or_else(|| std::sync::Arc::new(ExternalMetrics::new()));
    let (cert_task, expiry_task) = {
        use maekon_web::grpc::external::tls_config::{spawn_cert_watcher, spawn_expiry_monitor};
        let cert_path = cfg
            .tls_cert_path
            .clone()
            .context("tls_cert_path required when external_grpc.enabled")?;
        let key_path = cfg
            .tls_key_path
            .clone()
            .context("tls_key_path required when external_grpc.enabled")?;
        let watcher_resolver = cert_resolver.clone();
        let watcher_metrics = metrics_arc.clone();
        let watcher_shutdown = shutdown_rx.clone();
        let expiry_shutdown = shutdown_rx.clone();
        // spawn_cert_watcher is async and blocks only on initial watcher setup;
        // the actual file-watch loop runs inside a spawned task.
        // Both JoinHandles are stored in CertWatcherHandle so that Drop aborts
        // the tasks on non-clean teardown (F-RR-C28-02, cycle 26 sibling #3733/#3749).
        let cert_task = tokio::spawn(async move {
            if let Err(e) =
                spawn_cert_watcher(cert_path, key_path, watcher_resolver, watcher_shutdown).await
            {
                tracing::warn!(err = %e, "external_grpc: cert watcher failed to start");
            }
        });
        let expiry_task =
            spawn_expiry_monitor(cert_resolver.clone(), watcher_metrics, expiry_shutdown);
        (cert_task, expiry_task)
    };

    // Initial LiveSnapshot construction (spec §5.7 / D23 / D30).
    // When `pre_live` is Some (Task 7.1 shared-Arc path), use it directly so the web
    // server's DiagnosticsState and the gRPC server share a single ArcSwap cell.
    let live = if let Some(pre) = pre_live {
        pre
    } else {
        // cfg.streaming_enabled overrides the shared fallback in app_config_snapshot.web.
        let initial_streaming = cfg
            .streaming_enabled
            .unwrap_or(app_config_snapshot.web.grpc_streaming_enabled);
        let initial_thresholds = app_config_snapshot
            .web
            .grpc_load_thresholds
            .clone()
            .unwrap_or_default();
        let initial_policy = maekon_web::grpc::LoadPolicy::try_new(initial_thresholds)
            .context("Invalid LoadThresholds at boot — check config.web.grpc_load_thresholds")?;
        std::sync::Arc::new(
            maekon_web::grpc::external::live_config::LiveExternalConfig::new(
                maekon_web::grpc::external::live_config::LiveSnapshot {
                    streaming_enabled: initial_streaming,
                    load_policy: std::sync::Arc::new(initial_policy),
                },
            ),
        )
    };

    // Spawn ConfigReloadTask fire-and-forget, matching cert_watcher pattern per D30.
    // Watches for AppConfig changes via ConfigManager and atomically swaps LiveSnapshot.
    let config_rx = config_manager.subscribe();
    let shutdown_rx_for_reload = shutdown_rx.clone();
    let live_for_reload = live.clone();
    let metrics_for_reload = metrics_arc.clone();
    let reload_task = tokio::spawn(async move {
        maekon_web::grpc::external::config_reload::run_config_reload(
            live_for_reload,
            metrics_for_reload,
            config_rx,
            shutdown_rx_for_reload,
        )
        .await;
    });

    let jwt_verifier = if cfg.auth_mode.is_some_and(|m| m.includes_jwt()) {
        let pub_pem = tokio::fs::read(
            cfg.jwt_public_key_path
                .as_ref()
                .context("jwt_public_key_path required for JWT auth mode")?,
        )
        .await?;
        Some(std::sync::Arc::new(JwtVerifier::new(
            cfg.jwt_algorithm
                .context("jwt_algorithm required for JWT auth mode")?,
            &pub_pem,
            cfg.jwt_expected_issuer
                .as_deref()
                .context("jwt_expected_issuer required for JWT auth mode")?,
            cfg.jwt_expected_audience
                .as_deref()
                .context("jwt_expected_audience required for JWT auth mode")?,
        )?))
    } else {
        None
    };

    let mtls_verifier = if cfg.auth_mode.is_some_and(|m| m.includes_mtls()) {
        let allowlist = if let Some(p) = &cfg.mtls_fingerprint_allowlist_path {
            tokio::fs::read_to_string(p)
                .await?
                .lines()
                .map(String::from)
                .collect()
        } else {
            Vec::new()
        };
        Some(std::sync::Arc::new(MtlsVerifier::new(
            cfg.mtls_max_cert_lifetime_hours,
            &allowlist,
        )?))
    } else {
        None
    };

    let cert_watcher = maekon_web::grpc::external::tls_config::CertWatcherHandle {
        _shutdown_tx: shutdown_tx.clone(),
        _cert_task: cert_task,
        _expiry_task: expiry_task,
        _reload_task: reload_task,
    };

    Ok((
        maekon_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig {
            bind_addr: std::net::SocketAddr::new(cfg.bind_address, cfg.port),
            config: cfg.clone(),
            storage,
            system_monitor,
            event_tx,
            audit_port,
            cert_resolver,
            jwt_verifier,
            mtls_verifier,
            ip_ban: std::sync::Arc::new(IpBan::new()),
            metrics: metrics_arc,
            shutdown_rx,
            shutdown_tx,
            pii_sanitizer,
            ai_runtime_status_snapshot,
            live,
        },
        cert_watcher,
    ))
}
