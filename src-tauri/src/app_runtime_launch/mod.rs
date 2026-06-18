use anyhow::Result;
use maekon_core::ports::coaching_storage::CoachingStoragePort;
use std::sync::Arc;
use tauri::AppHandle;
use tracing::info;

mod audio_wiring;
mod capture_wiring;
mod cua_safe_mode;
#[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
mod external_grpc;
mod launch_result;
mod regime_wiring;
mod session_wiring;
mod state_wiring;
#[cfg(feature = "local-suggestions")]
mod suggestion_wiring;
mod web_server_wiring;

use self::audio_wiring::build_audio_runtime_state;
use self::capture_wiring::build_capture_wiring;
pub(crate) use self::cua_safe_mode::cua_safe_mode_enabled;
use self::launch_result::generate_local_auth_token;
pub(crate) use self::launch_result::AppRuntimeLaunchResult;
use self::regime_wiring::build_regime_wiring;
use self::session_wiring::{build_session_manager, spawn_idle_reaper};
use self::state_wiring::{build_managed_state_builder, ManagedStateWiringParts};
#[cfg(feature = "local-suggestions")]
use self::suggestion_wiring::build_suggestion_wiring;
use self::web_server_wiring::build_web_automation_wiring;
use crate::agent_runtime::AgentRuntimeBuilder;
use crate::bootstrap_runtime::BootstrapRuntimeBundle;
use crate::launch_resources::LaunchCoreResourcesBuilder;
use crate::magic_overlay::MagicOverlayHandle;
use crate::runtime_bridges::RuntimeBridgeSpawner;
use crate::runtime_state::{
    AiSessionRuntimeState, AnalysisHealthFlags, AutomationRuntimeState, ConfigRuntimeState,
    DetectionRuntimeState, EmbeddingRuntimeState, SuggestionRuntimeState, SyncRuntimeState,
};
use crate::scheduler::shared_regime_state::SharedRegimeState;
#[cfg(feature = "server")]
use crate::server_runtime_context::ServerLaunchContext;

pub(crate) struct AppRuntimeLaunchBuilder {
    bootstrap: BootstrapRuntimeBundle,
    app_handle: AppHandle,
    offline_mode: bool,
}

impl AppRuntimeLaunchBuilder {
    pub(crate) fn new(bootstrap: BootstrapRuntimeBundle, app_handle: AppHandle) -> Self {
        Self {
            bootstrap,
            app_handle,
            offline_mode: false,
        }
    }

    pub(crate) fn with_offline_mode(mut self, offline_mode: bool) -> Self {
        self.offline_mode = offline_mode;
        self
    }

    pub(crate) fn build_and_spawn(self) -> Result<AppRuntimeLaunchResult> {
        let frontend_web_port = self.bootstrap.frontend_web_port();
        // E20-41 (#4833): per-session local-API auth token — see launch_result.rs.
        let local_auth_token = generate_local_auth_token();
        let integration_runtime_status = self.bootstrap.integration_runtime_status();

        let BootstrapRuntimeBundle {
            db_path,
            data_dir_path,
            config_manager,
            mut config,
            runtime_handle: handle,
            background_runtime,
            web_port,
            // C1: provider context available in all analysis builds.
            #[cfg(feature = "analysis")]
            provider,
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

        let health_probe = crate::app_runtime_launch_health_probe::execute_startup_probe(
            &handle,
            update_control.clone(),
        );

        let sqlite_storage = core_resources.storage_runtime.sqlite_storage.clone();
        let event_tx = core_resources.background_runtime.event_tx();
        let shutdown_tx = core_resources.background_runtime.shutdown_tx();

        // Shared flag for on-demand re-clustering: scheduler, web server, and Tauri IPC
        // all reference the same AtomicBool so any endpoint can trigger re-clustering.
        let recluster_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // ADR-023 / #4478 G3: shared flag so the web "Delete all data" endpoint can ask
        // the SyncEngine to propagate a device-wide DeletionEvent to LAN peers.
        let erasure_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let cua_safe_mode = cua_safe_mode_enabled();
        if cua_safe_mode {
            tracing::info!(
                "CUA safe mode enabled: automatic capture starts paused; manual capture remains available"
            );
        }

        let capture_wiring = build_capture_wiring(
            &handle,
            &data_dir_path,
            &config,
            core_resources.storage_runtime.encryption_key.clone(),
            cua_safe_mode,
        );
        let capture_paused = capture_wiring.capture_paused.clone();
        let indicator_visible = capture_wiring.indicator_visible.clone();
        let detection_active = capture_wiring.detection_active.clone();
        let shared_capture_services = capture_wiring.shared_capture_services.clone();
        let capture_consent_manager = capture_wiring.consent_manager.clone();

        // #4928 + #4801: finalize erasure wiring (install the shared deletion_flag + retry incomplete local deletions).
        capture_wiring::install_erasure_wiring(
            &handle,
            &sqlite_storage,
            &capture_consent_manager,
            &shared_capture_services,
        );

        // Connection status flags — start disconnected, updated by health check loop.
        let server_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let llm_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cli_connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        // Focus mode state — transient, not persisted across restarts.
        let focus_mode = Arc::new(crate::focus_mode::FocusModeState::new());

        // review4 F14: wire the boundary PII sanitizer onto the production coaching
        // engine. Without it `pii_sanitizer` is None and the documented template_text
        // sanitization is inert — raw {app_name}/{regime} interpolations reach the LLM
        // personalization prompt, the persisted message_template, and the notification
        // body. VisionPiiSanitizer + the configured level mirror the other surfaces.
        let coaching_engine = Arc::new(
            maekon_analysis::CoachingEngine::new(config.coaching.clone()).with_pii_sanitizer(
                Arc::new(maekon_vision::privacy::VisionPiiSanitizer),
                config.privacy.pii_filter_level,
            ),
        );
        let (regime_manager_arc, regime_classifier_arc, regime_storage) =
            build_regime_wiring(&config, &handle, sqlite_storage.clone());

        // E20-24 (#4816): local learning sink + pipeline are gated on
        // `local-suggestions` (default-on), not `server`, so OSS builds get the
        // on-device suggestion pipeline. The server feature ADDS network transport.
        #[cfg(feature = "local-suggestions")]
        let feedback_sink: Arc<
            dyn maekon_core::ports::feedback_signal_sink::FeedbackSignalSink,
        > = Arc::new(crate::feedback_sink::CompositeFeedbackSink::new(
            Some(coaching_engine.clone()),
            Some(regime_classifier_arc.clone()),
        ));

        #[cfg(feature = "local-suggestions")]
        let suggestion_wiring = build_suggestion_wiring(
            &self.app_handle,
            &handle,
            &config,
            sqlite_storage.clone(),
            feedback_sink.clone(),
        );
        #[cfg(feature = "local-suggestions")]
        let suggestion_manager = suggestion_wiring.manager.clone();

        #[cfg(not(feature = "local-suggestions"))]
        let suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>> = None;

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

        // D7 (#4812 / E20-20): single shared workspace-wide circuit-breaker registry
        // threaded into every network adapter (agent, session, automation).
        let breaker_registry = crate::breaker_registry::CircuitBreakerRegistry::new();

        // #6264: create the cross-device-sync IPC state up front so its shared
        // write-once slot can be threaded into the agent runtime (which builds
        // the SyncEngine asynchronously) AND registered as managed state below —
        // both observe the same `Arc<OnceLock<SyncEngine>>`.
        let sync_runtime_state = SyncRuntimeState::default();
        // #6266: same shared-slot pattern for the reloadable embedding model.
        let embedding_runtime_state = EmbeddingRuntimeState::default();

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
                erasure_requested.clone(),
                self.app_handle.clone(),
            )
            .with_sync_runtime_slot(sync_runtime_state.slot())
            .with_embedding_runtime_slot(embedding_runtime_state.slot())
            .with_vector_store(Arc::new(
                maekon_storage::sqlite::vector_store_impl::SqliteVectorStore::new(
                    sqlite_storage.connection_arc(),
                ),
            ));
            if let Some(ref capture_services) = shared_capture_services {
                builder = builder.with_shared_capture_services(capture_services.clone());
            }
            let builder = builder
                .with_offline_mode(self.offline_mode)
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
                .with_analysis_health_flag(analysis_health_flag.clone())
                // D7: shared circuit-breaker registry → agent path.
                .with_breaker_registry(breaker_registry.clone());
            #[cfg(feature = "local-suggestions")]
            let builder = builder
                .with_shared_suggestion_queue(suggestion_wiring.shared_queue.clone())
                .with_shared_scorer(suggestion_wiring.shared_scorer.clone());
            #[cfg(feature = "local-suggestions")]
            let builder = if let Some(ref mgr) = suggestion_manager {
                builder.with_suggestion_manager(mgr.clone())
            } else {
                builder
            };
            #[cfg(feature = "analysis")] // C1: OAuth coordinator from provider context.
            let builder = provider.configure_agent_builder(builder);
            builder.build()
        };
        agent_runtime.spawn_on(&handle, core_resources.background_runtime.shutdown_rx());
        info!("Agent started");

        let (session_manager, codex_approval_registry) = build_session_manager(
            &self.app_handle,
            &handle,
            sqlite_storage.clone(),
            &config,
            &data_dir_path,
            shared_regime_state.clone(),
            capture_consent_manager.clone(),
            breaker_registry.clone(),
        );
        spawn_idle_reaper(
            &handle,
            &session_manager,
            sqlite_storage.clone(),
            config.ai_session.audit_retention_days,
            reaper_shutdown_rx,
        );

        #[cfg(feature = "grpc-dashboard-external")]
        let mut ext_grpc_supervisor = None;
        #[cfg(feature = "grpc-dashboard-external")]
        let mut ext_cert_watcher = None;

        let web_automation_wiring = if config.web.enabled {
            #[allow(unused_mut)]
            let mut wiring = build_web_automation_wiring(
                &self.app_handle,
                &handle,
                &shutdown_tx,
                event_tx.clone(),
                web_port.clone(),
                local_auth_token.clone(),
                integration_runtime_status,
                config_manager.clone(),
                update_control.clone(),
                sqlite_storage.clone(),
                &config,
                &data_dir_path,
                recluster_requested.clone(),
                erasure_requested.clone(),
                coaching_engine.clone(),
                &session_manager,
                shared_capture_services.as_ref(),
                capture_consent_manager.clone(),
                cli_health_flag.clone(),
                breaker_registry.clone(),
                #[cfg(feature = "analysis")]
                &provider,
                #[cfg(feature = "server")]
                &server_context,
            );
            #[cfg(feature = "grpc-dashboard-external")]
            {
                ext_grpc_supervisor = wiring.ext_grpc_supervisor.take();
                ext_cert_watcher = wiring.ext_cert_watcher.take();
            }
            Some(wiring)
        } else {
            None
        };
        let automation_controller = web_automation_wiring
            .as_ref()
            .and_then(|wiring| wiring.automation_controller.clone());

        // Connection status is now driven by the health check loop —
        // no optimistic initialization. The loop reads adapter health flags
        // and updates connection flags as the single source of truth.

        core_resources.background_runtime.spawn_runtime_bridges();

        // Forward update status changes to Tauri frontend via broadcast → emit bridge.
        RuntimeBridgeSpawner::spawn_update_event_bridge(&handle, &self.app_handle, &update_control);

        let ai_session_runtime_state = AiSessionRuntimeState::new(
            session_manager.as_ref().map(|(sm, _)| sm.clone()),
            Some(sqlite_storage.clone()),
            config.ai_session.max_history_turns,
        );
        let audio_runtime_state = build_audio_runtime_state(
            &self.app_handle,
            config_manager.clone(),
            &config,
            capture_consent_manager.clone(),
            capture_paused.clone(),
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

        // #5703: live controller iff web.enabled (controller is built inside
        // build_web_automation_wiring) — None keeps all 7 automation IPCs at
        // service.unavailable, symmetric with the REST surface.
        let automation_runtime_state = AutomationRuntimeState::new(
            automation_controller
                .clone()
                .map(|c| c as Arc<dyn maekon_core::ports::automation::AutomationPort>),
        );

        // Compute analysis_health before `config` is moved into AppState.
        let analysis_health = if config.analysis.enabled && config.ai_provider.llm_api.is_some() {
            Some(AnalysisHealthFlags {
                primary_healthy: analysis_health_flag,
            })
        } else {
            None
        };

        let runtime_handle_for_writer = handle.clone();

        let state_builder = build_managed_state_builder(ManagedStateWiringParts {
            handle,
            background_runtime,
            config,
            sqlite_storage,
            update_control,
            update_action_tx,
            shutdown_tx,
            recluster_requested,
            magic_overlay,
            coaching_engine,
            capture_paused,
            indicator_visible,
            server_connected,
            llm_connected,
            cli_connected,
            focus_mode,
            shared_capture_services,
            capture_consent_manager,
            analysis_health,
            regime_storage,
            regime_manager_arc,
            config_runtime_state,
            ai_session_runtime_state,
            codex_approval_registry,
            audio_runtime_state,
            suggestion_runtime_state,
            detection_runtime_state,
            automation_runtime_state,
            sync_runtime_state,
            embedding_runtime_state,
        });
        // C1: provider credentials (OAuth, secret-backend profile) via analysis;
        // integration bindings (auth, session) via server.
        #[cfg(feature = "analysis")]
        let state_builder = provider.configure_state_builder(state_builder);
        #[cfg(feature = "server")]
        let state_builder = server_context.configure_state_builder(state_builder);

        // Phase 4 D11: scheduler is up — dispatch the self-healthy writer.
        crate::app_runtime_launch_health_probe::spawn_healthy_writer(
            health_probe.as_ref(),
            &runtime_handle_for_writer,
        );

        Ok(AppRuntimeLaunchResult {
            frontend_web_port,
            local_auth_token,
            state_builder,
            // F-RR-C36-01: include the handles in the returned struct so the
            // caller (setup.rs) can register them as Tauri managed state.
            #[cfg(feature = "grpc-dashboard-external")]
            ext_grpc_supervisor,
            #[cfg(feature = "grpc-dashboard-external")]
            ext_cert_watcher,
        })
    }
}
