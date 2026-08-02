use anyhow::Result;
use std::sync::Arc;
use tauri::AppHandle;
use tracing::info;

mod audio_wiring;
#[cfg(feature = "server")]
mod auth_wiring;
mod capture_wiring;
mod coaching_wiring;
mod cua_safe_mode;
#[cfg(any(feature = "grpc-dashboard", feature = "grpc-dashboard-external"))]
mod external_grpc;
mod flags_wiring;
mod launch_result;
mod regime_wiring;
mod session_wiring;
mod state_wiring;
#[cfg(feature = "local-suggestions")]
mod suggestion_wiring;
mod web_server_wiring;

#[cfg(feature = "server")]
use self::auth_wiring::install_shared_token_manager;
use self::capture_wiring::{build_capture_reauth_gate, build_capture_wiring};
use self::coaching_wiring::build_coaching_wiring;
pub(crate) use self::cua_safe_mode::{cua_safe_mode_enabled, precreate_auxiliary_webviews};
pub(crate) use self::launch_result::AppRuntimeLaunchResult;
use self::launch_result::{ensure_installation_id, generate_local_auth_token};
use self::regime_wiring::build_regime_wiring;
use self::session_wiring::{build_session_manager, build_shared_policy_client, spawn_idle_reaper};
use self::state_wiring::{build_managed_state_builder, ManagedStateWiringParts};
#[cfg(feature = "local-suggestions")]
use self::suggestion_wiring::build_suggestion_wiring;
use self::web_server_wiring::{build_web_automation_wiring, ensure_web_server_ready};
use crate::agent_runtime::AgentRuntimeBundle;
use crate::agent_runtime_support::TauriNotifier;
use crate::bootstrap_runtime::BootstrapRuntimeBundle;
use crate::launch_resources::LaunchCoreResourcesBuilder;
use crate::magic_overlay::MagicOverlayHandle;
use crate::runtime_bridges::RuntimeBridgeSpawner;
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
        let mut frontend_web_port = self.bootstrap.frontend_web_port();
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

        ensure_installation_id(&config_manager, &mut config);

        // #8044: the ONE capture-history re-auth gate — see capture_wiring.rs.
        let reauth_gate = build_capture_reauth_gate(&config);

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

        // Shared re-clustering flag used by scheduler, web server, and IPC.
        let recluster_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Shared erasure flag lets local deletion fan out to LAN peers.
        let erasure_requested = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

        let cua_safe_mode = cua_safe_mode_enabled();
        if cua_safe_mode {
            tracing::info!("CUA safe mode enabled: automatic capture starts paused; manual capture remains available");
        }

        // #8039: fail-closed on a capture-services build failure (see capture_wiring.rs).
        let capture_wiring = build_capture_wiring(
            &handle,
            &data_dir_path,
            &config,
            core_resources.storage_runtime.encryption_key.clone(),
            cua_safe_mode,
        )?;
        let capture_paused = capture_wiring.capture_paused.clone();
        let indicator_visible = capture_wiring.indicator_visible.clone();
        let detection_active = capture_wiring.detection_active.clone();
        let shared_capture_services = capture_wiring.shared_capture_services.clone();
        let capture_consent_manager = capture_wiring.consent_manager.clone();
        // One FocusAnalyzer instance is shared by scheduler loops and Tauri
        // consent commands. This lets a runtime own-field revoke clear unfinished
        // workflow-pattern state in the same object immediately.
        // #9671 review B2: this injected instance OVERWRITES the one the
        // support builder constructs, so the notification master-switch gate
        // must be applied HERE — a raw TauriNotifier at this site silently
        // bypassed notification.enabled for every focus-analyzer toast.
        let focus_notifier: Arc<dyn maekon_core::ports::notifier::DesktopNotifier> =
            Arc::new(crate::agent_runtime_support::GatedNotifier::new(
                Arc::new(TauriNotifier::new(self.app_handle.clone())),
                config_manager.clone(),
            ));
        let focus_analyzer = Arc::new(
            maekon_analysis::focus_analyzer::FocusAnalyzer::with_defaults(
                sqlite_storage.clone(),
                focus_notifier,
            ),
        );

        // #4928 + #4801: finalize erasure wiring (install the shared deletion_flag + retry incomplete local deletions).
        capture_wiring::install_erasure_wiring(
            &handle,
            &sqlite_storage,
            &capture_consent_manager,
            &shared_capture_services,
            &config_manager,
        );

        #[cfg(feature = "server")]
        server_context.spawn_integration_loops(
            &core_resources.background_runtime,
            sqlite_storage.clone(),
            capture_consent_manager.clone(),
        );

        // Shared connection/health flags + cross-loop runtime slots (created once
        // here; see flags_wiring.rs for the per-field rationale). Destructured
        // into the same local names the wiring below expects.
        let flags_wiring::RuntimeFlagsWiring {
            server_connected,
            llm_connected,
            cli_connected,
            server_health_flag,
            llm_health_flag,
            cli_health_flag,
            analysis_health_flag,
            breaker_registry,
            sync_runtime_state,
            embedding_runtime_state,
            scene_finder_slot,
        } = flags_wiring::build_runtime_flags();

        // Focus mode state — transient, not persisted across restarts.
        let focus_mode = Arc::new(crate::focus_mode::FocusModeState::new());

        // #7913 T2.1b: PII-sanitized coaching engine + shared storage handle,
        // built once here with learned effectiveness hydrated before the loops.
        let (coaching_engine, coaching_storage) =
            build_coaching_wiring(&config, &handle, sqlite_storage.clone());
        let (regime_manager_arc, regime_classifier_arc, regime_storage) =
            build_regime_wiring(&config, &handle, sqlite_storage.clone());

        // #9459: the ONE ONESHIM login session — built, keychain-restored and
        // registered in the `TokenManagerState` IPC slot here (see
        // auth_wiring.rs), BEFORE both of its consumers below, so one sign-in is
        // observed by every upload/REST/SSE transport.
        #[cfg(feature = "server")]
        let shared_token_manager =
            install_shared_token_manager(&self.app_handle, &config, &handle, &data_dir_path);

        // OSS builds keep on-device suggestions; `server` only adds network
        // transport. The composite feedback sink over the shared regime
        // classifier is built inside build_suggestion_wiring (its sole
        // consumer).
        #[cfg(feature = "local-suggestions")]
        let suggestion_wiring = build_suggestion_wiring(
            &self.app_handle,
            &handle,
            &config,
            sqlite_storage.clone(),
            regime_classifier_arc.clone(),
            #[cfg(feature = "server")]
            shared_token_manager.clone(),
        );
        #[cfg(feature = "local-suggestions")]
        let suggestion_manager = suggestion_wiring.manager.clone();

        #[cfg(not(feature = "local-suggestions"))]
        let suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>> = None;

        // Create MagicOverlay handle (window created at startup in setup.rs)
        let magic_overlay =
            MagicOverlayHandle::new(self.app_handle.clone(), config.coaching.overlay_mode);

        // Shared SharedRegimeState — single instance used by both SessionManager (context
        // assembler) and Scheduler (monitor/coaching loops). Created before both consumers.
        let shared_regime_state = Arc::new(SharedRegimeState::new());

        // Obtain shutdown receiver for idle reaper before core_resources is consumed.
        let reaper_shutdown_rx = core_resources.background_runtime.shutdown_rx();

        let agent_runtime = {
            let mut builder = AgentRuntimeBundle::new(
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
            .with_scene_finder_slot(scene_finder_slot.clone())
            .with_vector_store(Arc::new(
                maekon_storage::sqlite::vector_store_impl::SqliteVectorStore::new(
                    sqlite_storage.connection_arc(),
                ),
            ));
            // #8039: unconditional now — `shared_capture_services` is always
            // present (build_capture_wiring fails closed on error).
            builder = builder.with_shared_capture_services(shared_capture_services.clone());
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
                .with_focus_analyzer(focus_analyzer.clone())
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
            // #9459: the SAME Arc the suggestion wiring and the IPC slot hold.
            #[cfg(feature = "server")]
            let builder = builder.with_shared_token_manager(shared_token_manager.clone());
            #[cfg(feature = "local-suggestions")]
            let builder = if let Some(ref mgr) = suggestion_manager {
                builder.with_suggestion_manager(mgr.clone())
            } else {
                builder
            };
            #[cfg(feature = "analysis")] // C1: OAuth coordinator from provider context.
            let builder = provider.configure_agent_builder(builder);
            builder
        };
        agent_runtime.spawn_on(&handle, core_resources.background_runtime.shutdown_rx());
        info!("Agent started");

        // #7932 Part B: the ONE shared Arc<PolicyClient> injected into BOTH the
        // Codex approval decider (via build_session_manager) AND the automation
        // controller (via the web wiring below). See build_shared_policy_client
        // for the Port Instance Sharing rationale.
        let shared_policy_client = build_shared_policy_client(&data_dir_path);

        let (session_manager, codex_approval_registry) = build_session_manager(
            &self.app_handle,
            &handle,
            sqlite_storage.clone(),
            &config,
            &data_dir_path,
            shared_regime_state.clone(),
            capture_consent_manager.clone(),
            breaker_registry.clone(),
            shared_policy_client.clone(),
            // #8050: SAME Arc the scheduler holds as `llm_ok`, so chat sends and
            // the health-check loop observe one shared LLM-connectivity flag.
            llm_health_flag.clone(),
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
                reauth_gate.clone(),
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
                Some(&shared_capture_services),
                capture_consent_manager.clone(),
                cli_health_flag.clone(),
                breaker_registry.clone(),
                // #7932 Part B: same shared Arc<PolicyClient> the Codex decider holds.
                shared_policy_client.clone(),
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
        if let Some(wiring) = web_automation_wiring.as_ref() {
            frontend_web_port = wiring.frontend_web_port;
            // The local web surface is required by the Tauri frontend. Continuing
            // after a readiness timeout injects an unbound port into the WebView;
            // frontend retries can then fall back to standalone mock data and make
            // a disconnected desktop session look healthy (#8201).
            ensure_web_server_ready(wiring.web_server_startup_error.as_deref())?;
        }
        let automation_controller = web_automation_wiring
            .as_ref()
            .and_then(|wiring| wiring.automation_controller.clone());
        // Populate the shared slot here because automation builds after scheduler startup.
        if let Some(scene_finder) = automation_controller
            .as_ref()
            .and_then(|controller| controller.scene_finder().cloned())
        {
            let _ = scene_finder_slot.set(scene_finder);
        }

        // Connection status is driven by the health check loop, which mirrors the
        // adapter health flags into the connection flags each tick (the single
        // source of truth). The optimistic initial values above only cover the
        // window before the first tick; from then on the real adapter writers
        // (heartbeat/upload, AuditingSession send, automation command result)
        // own the values (#8050).

        core_resources.background_runtime.spawn_runtime_bridges();

        // Forward update status changes to Tauri frontend via broadcast → emit bridge.
        RuntimeBridgeSpawner::spawn_update_event_bridge(&handle, &self.app_handle, &update_control);

        // The per-command IPC runtime states (ai_session / audio / config /
        // suggestion / detection / automation) and `analysis_health` are now
        // assembled inside `build_managed_state_builder` from the raw inputs
        // threaded below — see state_wiring.rs (ADR-013 composition-root slimming).
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
            focus_analyzer,
            shared_capture_services: Some(shared_capture_services),
            capture_consent_manager,
            regime_storage,
            regime_manager_arc,
            codex_approval_registry,
            sync_runtime_state,
            embedding_runtime_state,
            // Raw inputs — the IPC runtime states + analysis_health are built from
            // these inside build_managed_state_builder (state_wiring.rs).
            app_handle: self.app_handle.clone(),
            config_manager: config_manager.clone(),
            web_port: web_port.clone(),
            session_manager,
            suggestion_manager: suggestion_manager.clone(),
            shared_regime_state: shared_regime_state.clone(),
            detection_active,
            scene_finder_slot,
            automation_controller,
            analysis_health_flag,
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
            // #8044: carry the shared re-auth gate so setup.rs can register the
            // ReauthRuntimeState managed state for the biometric/PIN command.
            reauth_gate,
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
