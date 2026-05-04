use anyhow::Result;
use oneshim_core::consent::ConsentManager;
use oneshim_core::ports::coaching_storage::CoachingStoragePort;
use oneshim_core::ports::session_context_store::SessionContextStorePort;
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tracing::info;

use crate::agent_runtime::AgentRuntimeBuilder;
use crate::bootstrap_runtime::BootstrapRuntimeBundle;
use crate::capture_services::SharedCaptureServices;
use crate::launch_resources::LaunchCoreResourcesBuilder;
use crate::magic_overlay::MagicOverlayHandle;
use crate::runtime_bridges::RuntimeBridgeSpawner;
use crate::runtime_state::{
    AiSessionRuntimeState, AnalysisHealthFlags, AppState, AudioContext, AudioRuntimeState,
    CaptureContext, ConfigRuntimeState, ConnectionStatus, DetectionRuntimeState,
    ManagedStateBuilder, SuggestionRuntimeState,
};
use crate::scheduler::shared_regime_state::SharedRegimeState;
#[cfg(feature = "server")]
use crate::server_runtime_context::ServerLaunchContext;
use crate::session_context::SessionContextAssembler;
use crate::session_manager::SessionManagerImpl;
use crate::web_server_runtime::{
    WebServerLaunchContext, WebServerRuntimeBuilder, WebServerSupportContext,
};

const CUA_SAFE_MODE_ENV: &str = "ONESHIM_CUA_SAFE_MODE";

pub(crate) fn cua_safe_mode_enabled() -> bool {
    cua_safe_mode_enabled_from(std::env::var(CUA_SAFE_MODE_ENV).ok().as_deref())
}

fn cua_safe_mode_enabled_from(value: Option<&str>) -> bool {
    value.map(str::trim).is_some_and(|value| {
        matches!(value, "1")
            || value.eq_ignore_ascii_case("true")
            || value.eq_ignore_ascii_case("yes")
            || value.eq_ignore_ascii_case("on")
    })
}

fn initial_capture_flags(show_indicator: bool, cua_safe_mode: bool) -> (bool, bool) {
    let capture_paused = cua_safe_mode;
    let indicator_visible = show_indicator;
    (capture_paused, indicator_visible)
}

pub(crate) struct AppRuntimeLaunchResult {
    pub(crate) frontend_web_port: u16,
    pub(crate) state_builder: ManagedStateBuilder,
}

pub(crate) struct AppRuntimeLaunchBuilder {
    bootstrap: BootstrapRuntimeBundle,
    app_handle: AppHandle,
}

impl AppRuntimeLaunchBuilder {
    pub(crate) fn new(bootstrap: BootstrapRuntimeBundle, app_handle: AppHandle) -> Self {
        Self {
            bootstrap,
            app_handle,
        }
    }

    pub(crate) fn build_and_spawn(self) -> Result<AppRuntimeLaunchResult> {
        let frontend_web_port = self.bootstrap.frontend_web_port();
        let integration_runtime_status = self.bootstrap.integration_runtime_status();

        let BootstrapRuntimeBundle {
            db_path,
            data_dir_path,
            config_manager,
            mut config,
            runtime_handle: handle,
            background_runtime,
            web_port,
            #[cfg(feature = "server")]
            server,
            #[cfg(not(feature = "server"))]
                integration_runtime_status: _integration_runtime_status,
        } = self.bootstrap;

        // Auto-generate installation ID for staged rollout bucketing.
        if config.update.installation_id.is_none() {
            let new_id = uuid::Uuid::new_v4().to_string();
            if let Err(e) = config_manager.update_with(|c| {
                c.update.installation_id = Some(new_id.clone());
                Ok(())
            }) {
                tracing::warn!("Failed to persist installation_id: {e}");
            }
            config.update.installation_id = Some(new_id);
        }

        #[cfg(feature = "server")]
        let server_context = ServerLaunchContext::from_bootstrap(server);

        let core_resources = LaunchCoreResourcesBuilder::new(
            &config,
            &db_path,
            &data_dir_path,
            &handle,
            self.app_handle.clone(),
        )
        .build()?;
        let update_control = core_resources.update_runtime.update_control.clone();
        let update_action_tx = core_resources.update_runtime.update_action_tx.clone();

        // Phase 4 D11: post-install self-healthy probe + rolled-back-notification
        // scan. Runs BEFORE any scheduler loop spawns. If the probe escalates
        // to RollbackRequired (two consecutive failed boots on this version
        // without a self-healthy marker), the helper calls execute_rollback
        // which terminates this process. The probe instance is kept alive
        // until the scheduler-ready point near the function's end invokes
        // `spawn_healthy_writer` (30s uptime marker, spec §4.5).
        let health_probe = crate::app_runtime_launch_health_probe::execute_startup_probe(
            &handle,
            update_control.clone(),
        );

        let sqlite_storage = core_resources.storage_runtime.sqlite_storage.clone();
        let encryption_key = core_resources.storage_runtime.encryption_key.clone();
        let event_tx = core_resources.background_runtime.event_tx();
        let shutdown_tx = core_resources.background_runtime.shutdown_tx();

        // Shared flag for on-demand re-clustering: scheduler, web server, and Tauri IPC
        // all reference the same AtomicBool so any endpoint can trigger re-clustering.
        let recluster_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let cua_safe_mode = cua_safe_mode_enabled();
        if cua_safe_mode {
            tracing::info!(
                "CUA safe mode enabled: automatic capture starts paused; manual capture remains available"
            );
        }

        // Shared capture pause flag: scheduler monitor loop, tray menu, and IPC commands
        // all reference the same AtomicBool to toggle capture on/off. CUA safe mode
        // starts paused so Computer Use checks do not continuously record the user's
        // active desktop. The panel remains available so manual capture still works.
        let (capture_paused_initial, indicator_visible_initial) =
            initial_capture_flags(config.indicator.show_border, cua_safe_mode);
        let capture_paused =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(capture_paused_initial));
        let indicator_visible = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            indicator_visible_initial,
        ));
        let detection_active = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Connection status flags — start disconnected, updated by health check loop.
        let server_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let llm_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cli_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Focus mode state — transient, not persisted across restarts.
        let focus_mode = Arc::new(crate::focus_mode::FocusModeState::new());

        // Shared capture services are reused by scheduler and IPC commands so capture
        // semantics stay aligned across background monitoring and ad-hoc user actions.
        let shared_capture_services = match handle.block_on(SharedCaptureServices::build(
            &data_dir_path,
            &config,
            encryption_key.clone(),
        )) {
            Ok(services) => Some(Arc::new(services)),
            Err(error) => {
                tracing::warn!("shared capture services init failed: {error}");
                None
            }
        };
        let capture_consent_manager = shared_capture_services
            .as_ref()
            .map(|services| services.consent_manager.clone())
            .unwrap_or_else(|| Arc::new(ConsentManager::new(data_dir_path.join("consent.json"))));

        // --- Phase 3 composition root ---
        //
        // Construct the three Arc handles that are shared across AppState,
        // the agent runtime scheduler, and the feedback pipeline:
        //   - regime_manager_arc: used by analysis pipeline (via
        //     with_regime_handles) AND by AppState.regime_manager_snapshot
        //     (for the shutdown save guard).
        //   - regime_classifier_arc: analysis pipeline + CompositeFeedbackSink.
        //   - coaching_engine: shared with scheduler, web server, IPC, and
        //     the feedback sink. Moved earlier to satisfy the sink's needs.
        //
        // Hydrate the RegimeManager from persisted storage BEFORE handing
        // it to the scheduler, so the scheduler sees the restored set on
        // first classify() / active_regimes() call.
        let coaching_engine = Arc::new(oneshim_analysis::CoachingEngine::new(
            config.coaching.clone(),
        ));
        let regime_manager_arc = Arc::new(parking_lot::Mutex::new(
            oneshim_analysis::RegimeManager::new(&config.analysis.tiered_memory),
        ));
        let regime_classifier_arc = Arc::new(parking_lot::Mutex::new(
            oneshim_analysis::RegimeClassifier::new(1.5),
        ));
        let regime_storage: Arc<dyn oneshim_core::ports::regime_storage::RegimeStoragePort> =
            Arc::new(
                oneshim_storage::regime_manager_state_store::SqliteRegimeManagerStateStore::new(
                    sqlite_storage.connection_arc(),
                ),
            );
        {
            match handle.block_on(regime_storage.load_all()) {
                Ok(regimes) if !regimes.is_empty() => {
                    let count = regimes.len();
                    regime_manager_arc.lock().hydrate_from(regimes);
                    tracing::info!(count, "regime manager hydrated from storage");
                }
                Ok(_) => tracing::info!("regime manager: no persisted state, starting fresh"),
                Err(e) => tracing::warn!(
                    error = %e,
                    "regime manager hydrate failed; starting fresh"
                ),
            }
        }

        // Build the CompositeFeedbackSink once, thread it into the
        // FeedbackSender below (Some(sink)) so accept/reject signals fan
        // out to both CoachingEngine and the regime classifier on the
        // user-path inline (~10 ms budget, ADR-017).
        //
        // Gated on `server` because FeedbackSender is only constructed
        // under that feature — the sink has no consumer otherwise.
        #[cfg(feature = "server")]
        let feedback_sink: Arc<
            dyn oneshim_core::ports::feedback_signal_sink::FeedbackSignalSink,
        > = Arc::new(crate::feedback_sink::CompositeFeedbackSink::new(
            Some(coaching_engine.clone()),
            Some(regime_classifier_arc.clone()),
        ));

        // SuggestionManager for overlay panel (A3).
        // The queue Arc is created here and passed to BOTH the SuggestionManager
        // and the agent runtime (which builds SuggestionReceiver). This ensures
        // SSE-received suggestions appear in IPC queries via get_pending_suggestions.
        #[cfg(feature = "server")]
        let shared_suggestion_queue = Arc::new(tokio::sync::Mutex::new(
            oneshim_suggestion::queue::SuggestionQueue::new(config.analysis.max_suggestions),
        ));

        // Restore pending suggestions from SQLite into the queue.
        #[cfg(feature = "server")]
        {
            let pending = sqlite_storage
                .list_suggestions_by_state("pending", 50)
                .unwrap_or_default();
            if !pending.is_empty() {
                let mut queue = handle.block_on(shared_suggestion_queue.lock());
                let mut restored = 0usize;
                for record in pending {
                    if let Some(suggestion) = record.try_into_suggestion() {
                        if queue.push(suggestion) {
                            restored += 1;
                        }
                    }
                }
                if restored > 0 {
                    tracing::info!(count = restored, "restored suggestions from storage");
                }
            }
        }

        #[cfg(feature = "server")]
        let shared_scorer = Arc::new(tokio::sync::Mutex::new(
            oneshim_suggestion::scorer::FeedbackScorer::new(),
        ));
        #[cfg(feature = "server")]
        let suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>> = {
            use oneshim_network::auth::TokenManager;
            use oneshim_network::http_client::HttpApiClient;

            #[allow(deprecated)] // Fallback to non-TLS TokenManager when TLS config unavailable
            let token_manager = Arc::new(
                TokenManager::new_with_tls(
                    &config.server.base_url,
                    &config.tls,
                    Some(config.request_timeout()),
                )
                .unwrap_or_else(|_| TokenManager::new(&config.server.base_url)),
            );

            #[cfg(feature = "grpc")]
            let api_result: Result<
                Arc<dyn oneshim_core::ports::api_client::ApiClient>,
                _,
            > = {
                use oneshim_network::grpc::{GrpcApiAdapter, GrpcConfig, UnifiedClient};
                let grpc_config = GrpcConfig::from_core_with_rest_tls(
                    &config.grpc,
                    &config.server.base_url,
                    &config.tls,
                );
                match (
                    UnifiedClient::new(grpc_config, token_manager.clone()),
                    HttpApiClient::new_with_tls(
                        &config.server.base_url,
                        token_manager.clone(),
                        config.request_timeout(),
                        &config.tls,
                    ),
                ) {
                    (Ok(unified), Ok(http_fallback)) => Ok(Arc::new(GrpcApiAdapter::new(
                        Arc::new(unified),
                        http_fallback,
                    ))),
                    (Err(e), _) => Err(anyhow::anyhow!("UnifiedClient init failed: {e}")),
                    (_, Err(e)) => Err(anyhow::anyhow!("HttpApiClient init failed: {e}")),
                }
            };

            #[cfg(not(feature = "grpc"))]
            let api_result: Result<
                Arc<dyn oneshim_core::ports::api_client::ApiClient>,
                _,
            > = {
                HttpApiClient::new_with_tls(
                    &config.server.base_url,
                    token_manager,
                    config.request_timeout(),
                    &config.tls,
                )
                .map(|c| Arc::new(c) as Arc<dyn oneshim_core::ports::api_client::ApiClient>)
                .map_err(|e| anyhow::anyhow!("{e}"))
            };

            match api_result {
                Ok(api) => {
                    let history = Arc::new(tokio::sync::Mutex::new(
                        oneshim_suggestion::history::SuggestionHistory::new(100),
                    ));
                    let feedback =
                        Arc::new(oneshim_suggestion::feedback::FeedbackSender::new_with_sink(
                            api,
                            Some(feedback_sink.clone()),
                        ));
                    let deferred = Arc::new(tokio::sync::Mutex::new(
                        oneshim_suggestion::deferred::DeferredManager::new(50),
                    ));
                    let retry_queue = Arc::new(tokio::sync::Mutex::new(
                        oneshim_suggestion::feedback_retry::FeedbackRetryQueue::new(100, 5),
                    ));
                    Some(Arc::new(crate::suggestion_manager::SuggestionManager::new(
                        shared_suggestion_queue.clone(),
                        history,
                        feedback,
                        shared_scorer.clone(),
                        deferred,
                        retry_queue,
                        sqlite_storage.clone(),
                    )))
                }
                Err(e) => {
                    tracing::warn!("SuggestionManager init skipped: {e}");
                    None
                }
            }
        };
        #[cfg(not(feature = "server"))]
        let suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>> = None;

        // Restore deferred suggestions and pending feedbacks from SQLite.
        #[cfg(feature = "server")]
        if let Some(ref mgr) = suggestion_manager {
            // A. Deferred suggestions → DeferredManager or queue (if already due)
            let deferred_records = sqlite_storage
                .list_suggestions_by_state("deferred", 50)
                .unwrap_or_default();
            if !deferred_records.is_empty() {
                let total = deferred_records.len();
                let entries: Vec<_> = deferred_records
                    .into_iter()
                    .filter_map(|record| {
                        let resurface_at = record
                            .resurface_at
                            .as_ref()
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|dt| dt.with_timezone(&chrono::Utc))?;
                        let created_at = chrono::DateTime::parse_from_rfc3339(&record.created_at)
                            .ok()
                            .map(|dt| dt.with_timezone(&chrono::Utc))?;
                        let suggestion = record.try_into_suggestion()?;
                        Some((suggestion, created_at, resurface_at))
                    })
                    .collect();
                if entries.len() < total {
                    tracing::warn!(
                        dropped = total - entries.len(),
                        "skipped malformed deferred records"
                    );
                }

                let mut deferred_mgr = handle.block_on(mgr.deferred().lock());
                let already_due = deferred_mgr.restore(entries);
                let deferred_count = deferred_mgr.pending_count();
                drop(deferred_mgr);

                if !already_due.is_empty() {
                    let mut queue = handle.block_on(shared_suggestion_queue.lock());
                    for s in already_due {
                        queue.push(s);
                    }
                }
                if deferred_count > 0 {
                    tracing::info!(count = deferred_count, "restored deferred suggestions");
                }
            }

            // B. Pending feedbacks → FeedbackRetryQueue
            // Note: enqueue() recalculates next_retry_at from the attempt count,
            // so the persisted schedule is not honored exactly. This is acceptable
            // — SQLite is the durability guarantee, in-memory queue is best-effort.
            let pending_feedbacks = sqlite_storage
                .list_pending_feedbacks(100)
                .unwrap_or_default();
            if !pending_feedbacks.is_empty() {
                let cutoff = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
                let mut rq = handle.block_on(mgr.retry_queue().lock());
                let mut fb_count = 0usize;
                for record in pending_feedbacks {
                    // Skip orphaned rows older than 7 days
                    if record.created_at < cutoff {
                        let _ = sqlite_storage.delete_pending_feedback(&record.suggestion_id);
                        continue;
                    }
                    if let Some((sid, ft, comment, attempts, next_retry)) =
                        record.into_domain_parts()
                    {
                        rq.enqueue(oneshim_suggestion::feedback_retry::PendingFeedback {
                            suggestion_id: sid,
                            feedback_type: ft,
                            comment,
                            attempts,
                            next_retry_at: next_retry,
                        });
                        fb_count += 1;
                    }
                }
                if fb_count > 0 {
                    tracing::info!(count = fb_count, "restored pending feedbacks for retry");
                }
            }
        }

        // Adapter-side health flags — written by adapters on success/failure,
        // read by the health check loop. The loop is the single source of truth
        // for connection status.
        let server_health_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let llm_health_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cli_health_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Analysis provider health flag — shared between the FallbackAnalysisProvider
        // (written on success/failure) and AppState (read by get_analysis_health IPC).
        // Starts `true` (optimistic); flipped to `false` on first primary failure.
        let analysis_health_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));

        #[cfg(feature = "server")]
        server_context
            .spawn_integration_loops(&core_resources.background_runtime, sqlite_storage.clone());

        // CoachingEngine was already constructed above (Phase 3 composition
        // root) so the FeedbackSender sink could be wired at the FeedbackSender
        // construction site.
        let coaching_storage: Arc<dyn CoachingStoragePort> = sqlite_storage.clone();

        // Create MagicOverlay handle (window created at startup in setup.rs)
        let magic_overlay =
            MagicOverlayHandle::new(self.app_handle.clone(), config.coaching.overlay_mode);

        // Shared SharedRegimeState — single instance used by both SessionManager (context
        // assembler) and Scheduler (monitor/coaching loops). Created before both consumers.
        let shared_regime_state = Arc::new(SharedRegimeState::new());

        // Obtain shutdown receiver for idle reaper before core_resources is consumed.
        let reaper_shutdown_rx = core_resources.background_runtime.shutdown_rx();

        let agent_runtime = {
            let mut builder = AgentRuntimeBuilder::new(
                sqlite_storage.clone(),
                sqlite_storage.clone(),
                sqlite_storage.clone(),
                sqlite_storage.clone(),
                &data_dir_path,
                &config,
                config_manager.clone(),
                recluster_requested.clone(),
                self.app_handle.clone(),
            )
            .with_vector_store(Arc::new(
                oneshim_storage::sqlite::vector_store_impl::SqliteVectorStore::new(
                    sqlite_storage.connection_arc(),
                ),
            ));
            if let Some(ref capture_services) = shared_capture_services {
                builder = builder.with_shared_capture_services(capture_services.clone());
            }
            let builder = builder
                .with_offline_mode(false)
                .with_event_tx(
                    core_resources
                        .background_runtime
                        .agent_event_tx(config.web.enabled),
                )
                .with_calibration_writer(sqlite_storage.clone())
                .with_calibration_reader(sqlite_storage.clone())
                .with_override_store(sqlite_storage.clone())
                .with_consent_manager(capture_consent_manager.clone())
                .with_coaching_engine(coaching_engine.clone())
                .with_coaching_storage(coaching_storage.clone())
                .with_regime_handles(regime_manager_arc.clone(), regime_classifier_arc.clone())
                .with_magic_overlay(magic_overlay.clone())
                .with_overlay_driver(Arc::new(
                    crate::magic_overlay_driver::MagicOverlayDriver::new(self.app_handle.clone()),
                ))
                .with_capture_paused(capture_paused.clone())
                .with_detection_active(detection_active.clone())
                .with_focus_mode(focus_mode.clone())
                .with_shared_regime(shared_regime_state.clone())
                .with_health_flags(
                    server_health_flag.clone(),
                    llm_health_flag.clone(),
                    cli_health_flag.clone(),
                )
                .with_connection_flags(
                    server_connected.clone(),
                    llm_connected.clone(),
                    cli_connected.clone(),
                )
                .with_tray_app_handle(self.app_handle.clone())
                .with_suggestions_enabled(config.suggestions.enabled)
                .with_analysis_health_flag(analysis_health_flag.clone());
            #[cfg(feature = "server")]
            let builder = builder
                .with_shared_suggestion_queue(shared_suggestion_queue)
                .with_shared_scorer(shared_scorer);
            #[cfg(feature = "server")]
            let builder = if let Some(ref mgr) = suggestion_manager {
                builder.with_suggestion_manager(mgr.clone())
            } else {
                builder
            };
            #[cfg(feature = "server")]
            let builder = server_context.configure_agent_builder(builder);
            builder.build()
        };
        agent_runtime.spawn_on(&handle, core_resources.background_runtime.shutdown_rx());
        info!("Agent started");

        // Session manager wiring: AuditLogAdapter + SessionContextAssembler.
        // Creates a SEPARATE AuditLogger instance (not shared with web_server_runtime)
        // because the web server's logger is scoped to its own lifecycle and not exposed.
        let session_manager = {
            let storage_for_audit = sqlite_storage.clone();
            let persistence_cb: Arc<dyn oneshim_automation::audit::AuditPersistence> =
                Arc::new(move |entry: &oneshim_core::models::audit::AuditEntry| {
                    storage_for_audit.save_audit_entry(entry);
                });
            let audit_query: Arc<dyn oneshim_automation::audit::AuditQuery> = Arc::new(
                crate::audit_query::SqliteAuditQuery::new(sqlite_storage.clone()),
            );
            let audit_logger = Arc::new(tokio::sync::RwLock::new(
                oneshim_automation::audit::AuditLogger::new(500, 50)
                    .with_persistence(persistence_cb)
                    .with_query(audit_query),
            ));
            let audit_port: Arc<dyn oneshim_core::ports::audit_log::AuditLogPort> = Arc::new(
                oneshim_automation::audit::AuditLogAdapter::new(audit_logger),
            );

            let session_config = Arc::new(config.ai_session.clone());
            let idle_reaper_interval =
                std::time::Duration::from_secs(session_config.health_check_interval_secs);
            let session_context_store: Arc<dyn SessionContextStorePort> = sqlite_storage.clone();

            let context_assembler = Arc::new(SessionContextAssembler::new(
                session_context_store,
                Arc::new(config.clone()),
                shared_regime_state.clone(),
            ));

            // Resolve provider secret backend so HttpApi sessions can look up
            // API keys via CredentialSource::StoredSecret (keychain / file / env).
            let secret_store = {
                let config_dir = oneshim_core::config_manager::ConfigManager::config_dir()
                    .unwrap_or_else(|_| data_dir_path.to_path_buf());
                let os_store = crate::provider_secret_backend::create_os_secret_store(&config_dir);
                match crate::provider_secret_backend::resolve_provider_secret_backend(
                    &config_dir,
                    os_store,
                ) {
                    Ok(r) => r.secret_store,
                    Err(e) => {
                        tracing::debug!("provider secret backend unavailable: {e}");
                        None
                    }
                }
            };

            let mut manager =
                SessionManagerImpl::new(session_config, audit_port, Some(context_assembler));
            if let Some(store) = secret_store {
                manager = manager.with_secret_store(store);
            }
            manager = manager.with_app_handle(self.app_handle.clone());
            Some((Arc::new(manager), idle_reaper_interval))
        };

        // Spawn idle reaper background task — periodically calls reap_idle_sessions
        // to transition Active→Idle→Terminated for sessions that exceed the idle timeout.
        if let Some((ref sm, idle_reaper_interval)) = session_manager {
            let sm_clone = sm.clone();
            let ss_clone: Arc<dyn oneshim_core::ports::session_storage::SessionStoragePort> =
                sqlite_storage.clone();
            let retention_days = config.ai_session.audit_retention_days;
            let mut shutdown_rx = reaper_shutdown_rx;
            handle.spawn(async move {
                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(idle_reaper_interval) => {
                            sm_clone.reap_idle_sessions().await;
                            // Purge expired persisted sessions
                            if let Ok(count) = ss_clone.purge_expired(retention_days).await {
                                if count > 0 {
                                    tracing::info!("purged {count} expired session records");
                                }
                            }
                        }
                        _ = shutdown_rx.changed() => break,
                    }
                }
            });
            info!("idle reaper background task started");
        }

        let automation_controller = if config.web.enabled {
            let launch_context = WebServerLaunchContext::new(
                &handle,
                &shutdown_tx,
                event_tx.clone(),
                web_port.clone(),
            );
            let support_context = WebServerSupportContext::new(
                config_manager.clone(),
                update_control.clone(),
                integration_runtime_status,
            )
            .with_app_handle(self.app_handle.clone())
            .with_cli_health_flag(cli_health_flag.clone());
            let mut builder = WebServerRuntimeBuilder::new(
                sqlite_storage.clone(),
                &config,
                &data_dir_path,
                launch_context,
                support_context,
            )
            .with_override_store(sqlite_storage.clone())
            .with_recluster_requested(recluster_requested.clone())
            .with_coaching_engine(
                coaching_engine.clone() as Arc<dyn oneshim_core::ports::coaching::CoachingPort>
            );
            if let Some((ref sm, _)) = session_manager {
                builder = builder.with_session_manager(sm.clone()
                    as Arc<dyn oneshim_core::ports::conversation_session::SessionManager>);
            }
            if let Some(ref capture_services) = shared_capture_services {
                builder = builder.with_frame_storage(capture_services.frame_storage.clone());
            }
            // Task 7.1: pre-create LiveExternalConfig + ExternalMetrics Arcs so they can be
            // shared between the web server's DiagnosticsState (for live-config REST) and the
            // external gRPC server (for streaming control). Created here, before build_and_spawn,
            // so both consumers hold a reference to the same underlying ArcSwap cell.
            // Only built when the external feature is compiled and the feature is enabled in config.
            #[cfg(feature = "grpc-dashboard-external")]
            let (ext_shared_live, ext_shared_metrics) = {
                if config.external_grpc.enabled {
                    let initial_streaming = config
                        .external_grpc
                        .streaming_enabled
                        .unwrap_or(config.web.grpc_streaming_enabled);
                    let initial_thresholds =
                        config.web.grpc_load_thresholds.clone().unwrap_or_default();
                    let initial_policy = oneshim_web::grpc::LoadPolicy::try_new(initial_thresholds)
                        .unwrap_or_else(|e| {
                            tracing::warn!(
                                err = %e,
                                "external_grpc: invalid LoadThresholds at pre-creation; using defaults"
                            );
                            oneshim_web::grpc::LoadPolicy::new(Default::default())
                        });
                    let live = std::sync::Arc::new(
                        oneshim_web::grpc::external::live_config::LiveExternalConfig::new(
                            oneshim_web::grpc::external::live_config::LiveSnapshot {
                                streaming_enabled: initial_streaming,
                                load_policy: std::sync::Arc::new(initial_policy),
                            },
                        ),
                    );
                    let metrics = std::sync::Arc::new(
                        oneshim_web::grpc::external::metrics::ExternalMetrics::new(),
                    );
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
            #[cfg(feature = "server")]
            let builder = server_context.configure_web_server_builder(builder);
            let web_server_runtime = builder.build_and_spawn();

            // D13: spawn gRPC dashboard server alongside Axum REST, when the
            // `grpc-dashboard` feature is compiled in. Port resolution
            // priority (highest first):
            //   1. ONESHIM_DASHBOARD_GRPC_PORT env var (ops/CI override)
            //   2. `config.web.grpc_port` (user config)
            //   3. `oneshim_web::grpc::DEFAULT_GRPC_DASHBOARD_PORT` (10080)
            // If the default/configured port is busy, oneshim-web tries the
            // next loopback port in its ten-port band. If all candidates fail,
            // the server task logs a warn and exits — REST continues normally.
            //
            // D13-v2b: pass a GrpcSpawnConfig so streaming RPCs receive
            // SystemMonitor + event_tx + auth token + load_policy + kill switch + cap.
            #[cfg(feature = "grpc-dashboard")]
            {
                let grpc_port = resolve_loopback_grpc_port(config.web.grpc_port);
                let grpc_storage = sqlite_storage.clone()
                    as std::sync::Arc<dyn oneshim_web::storage_port::WebStorage>;
                // Fresh SysInfoMonitor — cheap, sysinfo-backed, independent of
                // agent_runtime's instance. Each call does a fresh poll, no
                // internal cache to share.
                let grpc_monitor =
                    std::sync::Arc::new(oneshim_monitor::system::SysInfoMonitor::new())
                        as std::sync::Arc<dyn oneshim_core::ports::monitor::SystemMonitor>;
                let thresholds = config.web.grpc_load_thresholds.clone().unwrap_or_default();
                let load_policy =
                    std::sync::Arc::new(oneshim_web::grpc::LoadPolicy::new(thresholds));
                let grpc_pii_sanitizer =
                    std::sync::Arc::new(oneshim_vision::privacy::VisionPiiSanitizer)
                        as std::sync::Arc<dyn oneshim_core::ports::pii_sanitizer::PiiSanitizer>;
                let cfg = oneshim_web::grpc::GrpcSpawnConfig {
                    port: grpc_port,
                    storage: grpc_storage,
                    system_monitor: grpc_monitor,
                    event_tx: event_tx.clone(),
                    integration_auth_token: config.web.integration_auth_token.clone(),
                    pii_sanitizer: Some(grpc_pii_sanitizer),
                    ai_runtime_status_snapshot: web_server_runtime.ai_runtime_status.clone(),
                    load_policy,
                    streaming_enabled: config.web.grpc_streaming_enabled,
                    max_concurrent_streams: config.web.grpc_max_concurrent_streams,
                };
                handle.spawn(async move {
                    oneshim_web::grpc::serve_optional(cfg).await;
                });
            }

            // D13-v2c: External gRPC binding — TLS + JWT/mTLS authenticated server on
            // a separate port. Feature-gated; default builds pay zero cost.
            // F13: cross-config port collision guard prevents both servers from
            // binding to the same port.
            #[cfg(feature = "grpc-dashboard-external")]
            {
                let ext_cfg = &config.external_grpc;
                if ext_cfg.enabled {
                    // F13: read the loopback port the same way the loopback spawn does,
                    // then refuse to start the external server if they collide.
                    let loopback_port = resolve_loopback_grpc_port(config.web.grpc_port);
                    if let Err(msg) =
                        oneshim_web::grpc::external::port_collision::check_port_collision(
                            ext_cfg.port,
                            loopback_port,
                        )
                    {
                        tracing::error!(
                            external_port = ext_cfg.port,
                            loopback_port,
                            err = %msg,
                            "external_grpc: port collides with loopback grpc port; disabling external server"
                        );
                    } else if let Err(e) = ext_cfg.validate() {
                        tracing::error!(err = %e, "external_grpc: config validation failed; disabling");
                    } else {
                        let ext_storage = sqlite_storage.clone()
                            as std::sync::Arc<dyn oneshim_web::storage_port::WebStorage>;
                        let ext_monitor =
                            std::sync::Arc::new(oneshim_monitor::system::SysInfoMonitor::new())
                                as std::sync::Arc<dyn oneshim_core::ports::monitor::SystemMonitor>;
                        let ext_audit: std::sync::Arc<
                            dyn oneshim_core::ports::audit_log::AuditLogPort,
                        > = {
                            let storage_for_audit = sqlite_storage.clone();
                            let persistence_cb: std::sync::Arc<
                                dyn oneshim_automation::audit::AuditPersistence,
                            > = std::sync::Arc::new(
                                move |entry: &oneshim_core::models::audit::AuditEntry| {
                                    storage_for_audit.save_audit_entry(entry);
                                },
                            );
                            let audit_query: std::sync::Arc<
                                dyn oneshim_automation::audit::AuditQuery,
                            > = std::sync::Arc::new(crate::audit_query::SqliteAuditQuery::new(
                                sqlite_storage.clone(),
                            ));
                            let logger = std::sync::Arc::new(tokio::sync::RwLock::new(
                                oneshim_automation::audit::AuditLogger::new(500, 50)
                                    .with_persistence(persistence_cb)
                                    .with_query(audit_query),
                            ));
                            std::sync::Arc::new(oneshim_automation::audit::AuditLogAdapter::new(
                                logger,
                            ))
                        };
                        let ext_pii_sanitizer: std::sync::Arc<
                            dyn oneshim_core::ports::pii_sanitizer::PiiSanitizer,
                        > = std::sync::Arc::new(oneshim_vision::privacy::VisionPiiSanitizer);
                        let ext_ai_status = web_server_runtime.ai_runtime_status.clone();
                        let ext_app_config_snapshot = std::sync::Arc::new(config.clone());

                        match handle.block_on(build_external_spawn_config(
                            ext_cfg,
                            ext_storage,
                            ext_monitor,
                            event_tx.clone(),
                            ext_audit,
                            Some(ext_pii_sanitizer),
                            ext_ai_status,
                            config_manager.clone(),
                            ext_app_config_snapshot,
                            ext_shared_live.clone(),
                            ext_shared_metrics.clone(),
                        )) {
                            Ok(spawn_cfg) => {
                                let _ext_handle = handle.block_on(
                                    oneshim_web::grpc::external::spawn_with_supervisor(spawn_cfg),
                                );
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
            }

            web_server_runtime.automation_controller
        } else {
            None
        };

        // Connection status is now driven by the health check loop —
        // no optimistic initialization. The loop reads adapter health flags
        // and updates connection flags as the single source of truth.

        core_resources.background_runtime.spawn_runtime_bridges();

        // Forward update status changes to Tauri frontend via broadcast → emit bridge.
        RuntimeBridgeSpawner::spawn_update_event_bridge(&handle, &self.app_handle, &update_control);

        // Audio capture and STT engine — wired when audio feature + config enabled.
        let model_dir: std::path::PathBuf = self
            .app_handle
            .path()
            .app_data_dir()
            .map(|d| d.join("models"))
            .unwrap_or_else(|_| std::path::PathBuf::from("models"));

        let audio_capture: Option<Arc<dyn oneshim_core::ports::audio_capture::AudioCapturePort>> = {
            #[cfg(feature = "audio")]
            {
                if config.audio.enabled {
                    Some(Arc::new(oneshim_audio::AudioCapture::new()))
                } else {
                    tracing::debug!("audio capture disabled by config");
                    None
                }
            }
            #[cfg(not(feature = "audio"))]
            {
                None
            }
        };

        // Build STT engine with full fallback chain (local + cloud) based on config.
        let stt_engine: Option<Arc<dyn oneshim_core::ports::stt_provider::SttProvider>> = {
            use oneshim_core::config::SttProviderKind;

            if !config.audio.enabled {
                None
            } else {
                // Build local provider (if model available)
                let local_provider: Option<
                    Arc<dyn oneshim_core::ports::stt_provider::SttProvider>,
                > = {
                    #[cfg(feature = "stt")]
                    {
                        let model_path = if config.audio.whisper_model_path.is_empty() {
                            #[cfg(feature = "download")]
                            {
                                model_dir.join(oneshim_audio::model_downloader::model_filename(
                                    config.audio.model_size,
                                ))
                            }
                            #[cfg(not(feature = "download"))]
                            {
                                self.app_handle
                                    .path()
                                    .resource_dir()
                                    .map(|d| d.join("ggml-base.bin"))
                                    .unwrap_or_default()
                            }
                        } else {
                            std::path::PathBuf::from(&config.audio.whisper_model_path)
                        };
                        if model_path.exists() {
                            match oneshim_audio::WhisperSttProvider::new(
                                &model_path,
                                config.audio.language,
                            ) {
                                Ok(provider) => {
                                    tracing::info!("Whisper STT loaded: {}", model_path.display());
                                    Some(Arc::new(provider) as _)
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to load Whisper model: {e}");
                                    None
                                }
                            }
                        } else {
                            tracing::info!(
                                "Whisper model not found at {}; local STT unavailable",
                                model_path.display()
                            );
                            None
                        }
                    }
                    #[cfg(not(feature = "stt"))]
                    {
                        None
                    }
                };

                // Build cloud provider (if key configured)
                let cloud_provider: Option<
                    Arc<dyn oneshim_core::ports::stt_provider::SttProvider>,
                > = {
                    #[cfg(feature = "cloud-stt")]
                    {
                        if !config.audio.cloud_api_key.is_empty() {
                            match oneshim_audio::CloudSttProvider::new(
                                config.audio.cloud_api_key.clone(),
                                config.audio.cloud_stt_endpoint.clone(),
                                config.audio.language,
                                config.audio.cloud_timeout_secs,
                            ) {
                                Ok(p) => Some(Arc::new(p) as _),
                                Err(e) => {
                                    tracing::warn!("Failed to create cloud STT: {e}");
                                    None
                                }
                            }
                        } else {
                            None
                        }
                    }
                    #[cfg(not(feature = "cloud-stt"))]
                    {
                        None
                    }
                };

                // Assemble final provider based on config preference
                match config.audio.stt_provider {
                    SttProviderKind::Cloud => match (cloud_provider, local_provider) {
                        (Some(cloud), Some(local)) => {
                            tracing::info!("STT startup: Cloud with local fallback");
                            Some(Arc::new(crate::fallback_stt::FallbackSttProvider::new(
                                cloud, local,
                            )) as _)
                        }
                        (Some(cloud), None) => {
                            tracing::info!("STT startup: Cloud only (no local model)");
                            Some(cloud)
                        }
                        (None, Some(local)) => {
                            tracing::warn!(
                                "Cloud STT unavailable at startup, falling back to local"
                            );
                            Some(local)
                        }
                        (None, None) => None,
                    },
                    SttProviderKind::Local => {
                        if local_provider.is_some() {
                            tracing::info!("STT startup: Local provider");
                        }
                        local_provider
                    }
                }
            }
        };

        let model_downloader: Option<
            Arc<dyn oneshim_core::ports::model_downloader::ModelDownloader>,
        > = {
            #[cfg(feature = "download")]
            {
                Some(Arc::new(oneshim_audio::WhisperModelDownloader::new()))
            }
            #[cfg(not(feature = "download"))]
            {
                None
            }
        };

        let ai_session_runtime_state = AiSessionRuntimeState::new(
            session_manager.as_ref().map(|(sm, _)| sm.clone()),
            Some(sqlite_storage.clone()),
            config.ai_session.max_history_turns,
        );
        let audio_runtime_state = AudioRuntimeState::new(
            config_manager.clone(),
            // D13 (CONS-PC04): pass ConsentManager + capture_paused for start_audio_capture gate.
            Some(capture_consent_manager.clone()),
            capture_paused.clone(),
            AudioContext {
                capture: audio_capture,
                stt_engine: Arc::new(tokio::sync::RwLock::new(stt_engine)),
                model_downloader,
                model_dir,
                downloading: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                download_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                vad_state: Arc::new(parking_lot::Mutex::new("idle".into())),
            },
        );
        let config_runtime_state =
            ConfigRuntimeState::new(config_manager.clone(), web_port.clone());
        let suggestion_runtime_state =
            SuggestionRuntimeState::new(suggestion_manager.clone(), Some(magic_overlay.clone()));
        let detection_runtime_state = DetectionRuntimeState::new(
            detection_active.clone(),
            automation_controller
                .as_ref()
                .and_then(|controller| controller.scene_finder().cloned()),
            Some(magic_overlay.clone()),
        );

        // Compute analysis_health before `config` is moved into AppState.
        let analysis_health = if config.analysis.enabled && config.ai_provider.llm_api.is_some() {
            Some(AnalysisHealthFlags {
                primary_healthy: analysis_health_flag,
            })
        } else {
            None
        };

        // Phase 4 D11: clone the Handle BEFORE the AppState construction
        // moves `handle` into `runtime_handle`. The scheduler-ready point
        // below dispatches `spawn_healthy_writer` onto this runtime, which
        // must work even when this function is called synchronously from
        // Tauri's `setup` callback — on macOS the callback fires inside
        // `applicationDidFinishLaunching` BEFORE any tokio context is
        // current on the main thread, so the previous `tokio::spawn`
        // panicked with "no reactor running".
        let runtime_handle_for_writer = handle.clone();

        let state_builder = ManagedStateBuilder::new(
            AppState {
                runtime_handle: handle,
                background_runtime,
                config,
                storage: sqlite_storage.clone(),
                update_control: Some(update_control),
                update_action_tx,
                shutdown_tx,
                recluster_requested: recluster_requested.clone(),
                magic_overlay: Some(magic_overlay),
                coaching_engine: Some(
                    coaching_engine as Arc<dyn oneshim_core::ports::coaching::CoachingPort>,
                ),
                capture_paused,
                indicator_visible,
                connection: ConnectionStatus {
                    server_connected,
                    llm_connected,
                    cli_connected,
                },
                focus_mode,
                capture: CaptureContext {
                    frame_processor: shared_capture_services
                        .as_ref()
                        .map(|services| services.frame_processor.clone()),
                    frame_storage: shared_capture_services
                        .as_ref()
                        .map(|services| services.frame_storage.clone()),
                    activity_monitor: shared_capture_services
                        .as_ref()
                        .map(|services| services.activity_monitor.clone()),
                    accessibility_extractor: shared_capture_services
                        .as_ref()
                        .and_then(|services| services.accessibility_extractor.clone()),
                    consent_manager: Some(capture_consent_manager),
                    work_classifier: Some(Arc::new(
                        oneshim_vision::work_classifier::RuleBasedClassifier,
                    )),
                },
                analysis_health,
                regime_storage: Some(regime_storage.clone()),
                regime_manager_snapshot: Some(regime_manager_arc.clone()),
            },
            config_runtime_state,
        )
        .with_ai_session_runtime(ai_session_runtime_state)
        .with_audio_runtime(audio_runtime_state)
        .with_suggestion_runtime(suggestion_runtime_state)
        .with_detection_runtime(detection_runtime_state);
        #[cfg(feature = "server")]
        let state_builder = server_context.configure_state_builder(state_builder);

        // Phase 4 D11: scheduler is up — dispatch the self-healthy writer.
        crate::app_runtime_launch_health_probe::spawn_healthy_writer(
            health_probe.as_ref(),
            &runtime_handle_for_writer,
        );

        Ok(AppRuntimeLaunchResult {
            frontend_web_port,
            state_builder,
        })
    }
}

#[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
fn resolve_loopback_grpc_port(configured_port: u16) -> u16 {
    std::env::var("ONESHIM_DASHBOARD_GRPC_PORT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            if configured_port == 0 {
                oneshim_web::grpc::DEFAULT_GRPC_DASHBOARD_PORT
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
async fn build_external_spawn_config(
    cfg: &oneshim_core::config::ExternalGrpcConfig,
    storage: std::sync::Arc<dyn oneshim_web::storage_port::WebStorage>,
    system_monitor: std::sync::Arc<dyn oneshim_core::ports::monitor::SystemMonitor>,
    event_tx: tokio::sync::broadcast::Sender<oneshim_api_contracts::stream::RealtimeEvent>,
    audit_port: std::sync::Arc<dyn oneshim_core::ports::audit_log::AuditLogPort>,
    pii_sanitizer: Option<std::sync::Arc<dyn oneshim_core::ports::pii_sanitizer::PiiSanitizer>>,
    ai_runtime_status_snapshot: Option<oneshim_api_contracts::stream::AiRuntimeStatus>,
    config_manager: oneshim_core::config_manager::ConfigManager,
    app_config_snapshot: std::sync::Arc<oneshim_core::config::AppConfig>,
    pre_live: Option<std::sync::Arc<oneshim_web::grpc::external::live_config::LiveExternalConfig>>,
    pre_metrics: Option<std::sync::Arc<oneshim_web::grpc::external::metrics::ExternalMetrics>>,
) -> anyhow::Result<oneshim_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig> {
    use anyhow::Context as _;
    use oneshim_web::grpc::external::{
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
    {
        use oneshim_web::grpc::external::tls_config::{spawn_cert_watcher, spawn_expiry_monitor};
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
        tokio::spawn(async move {
            if let Err(e) =
                spawn_cert_watcher(cert_path, key_path, watcher_resolver, watcher_shutdown).await
            {
                tracing::warn!(err = %e, "external_grpc: cert watcher failed to start");
            }
        });
        spawn_expiry_monitor(cert_resolver.clone(), watcher_metrics, expiry_shutdown);
    }

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
        let initial_policy = oneshim_web::grpc::LoadPolicy::try_new(initial_thresholds)
            .context("Invalid LoadThresholds at boot — check config.web.grpc_load_thresholds")?;
        std::sync::Arc::new(
            oneshim_web::grpc::external::live_config::LiveExternalConfig::new(
                oneshim_web::grpc::external::live_config::LiveSnapshot {
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
    tokio::spawn(async move {
        oneshim_web::grpc::external::config_reload::run_config_reload(
            live_for_reload,
            metrics_for_reload,
            config_rx,
            shutdown_rx_for_reload,
        )
        .await;
    });

    let jwt_verifier = if cfg.auth_mode.is_some_and(|m| m.includes_jwt()) {
        let pub_pem = std::fs::read(
            cfg.jwt_public_key_path
                .as_ref()
                .context("jwt_public_key_path required for JWT auth mode")?,
        )?;
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
            std::fs::read_to_string(p)?
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

    Ok(
        oneshim_web::grpc::external::spawn_config::ExternalGrpcSpawnConfig {
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
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cua_safe_mode_parser_accepts_explicit_truthy_values() {
        for value in ["1", "true", "TRUE", "yes", "on", " On "] {
            assert!(
                cua_safe_mode_enabled_from(Some(value)),
                "{value:?} should enable CUA safe mode"
            );
        }
    }

    #[test]
    fn cua_safe_mode_parser_rejects_absent_or_falsey_values() {
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("false"),
            Some("off"),
            Some("no"),
        ] {
            assert!(
                !cua_safe_mode_enabled_from(value),
                "{value:?} should not enable CUA safe mode"
            );
        }
    }

    #[test]
    fn cua_safe_mode_starts_paused_but_keeps_indicator_available() {
        assert_eq!(initial_capture_flags(true, true), (true, true));
        assert_eq!(initial_capture_flags(false, true), (true, false));
        assert_eq!(initial_capture_flags(true, false), (false, true));
    }
}
