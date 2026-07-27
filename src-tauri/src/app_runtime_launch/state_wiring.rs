use super::audio_wiring::build_audio_runtime_state;
use super::session_wiring::SessionManagerLaunch;
use crate::bootstrap_runtime::ManagedBackgroundRuntime;
use crate::capture_services::SharedCaptureServices;
use crate::focus_mode::FocusModeState;
use crate::magic_overlay::MagicOverlayHandle;
use crate::runtime_state::{
    AiSessionRuntimeState, AnalysisHealthFlags, AppState, AutomationRuntimeState, CaptureContext,
    CodexApprovalRuntimeState, ConfigRuntimeState, ConnectionStatus, DetectionRuntimeState,
    EmbeddingRuntimeState, ManagedStateBuilder, SceneFinderSlot, SuggestionRuntimeState,
    SyncRuntimeState,
};
use crate::scheduler::shared_regime_state::SharedRegimeState;
use crate::suggestion_manager::SuggestionManager;
use maekon_automation::controller::AutomationController;
use maekon_core::config::AppConfig;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::regime_storage::RegimeStoragePort;
use maekon_storage::sqlite::SqliteStorage;
use maekon_web::update_control::{UpdateAction, UpdateControl};
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::Arc;
use tauri::AppHandle;

pub(super) struct ManagedStateWiringParts {
    pub(super) handle: tokio::runtime::Handle,
    pub(super) background_runtime: Arc<ManagedBackgroundRuntime>,
    pub(super) config: AppConfig,
    pub(super) sqlite_storage: Arc<SqliteStorage>,
    pub(super) update_control: UpdateControl,
    pub(super) update_action_tx: tokio::sync::mpsc::UnboundedSender<UpdateAction>,
    pub(super) shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub(super) recluster_requested: Arc<AtomicBool>,
    pub(super) magic_overlay: MagicOverlayHandle,
    pub(super) coaching_engine: Arc<maekon_analysis::CoachingEngine>,
    pub(super) capture_paused: Arc<AtomicBool>,
    pub(super) indicator_visible: Arc<AtomicBool>,
    pub(super) server_connected: Arc<AtomicBool>,
    pub(super) llm_connected: Arc<AtomicBool>,
    pub(super) cli_connected: Arc<AtomicBool>,
    pub(super) focus_mode: Arc<FocusModeState>,
    pub(super) focus_analyzer: Arc<maekon_analysis::focus_analyzer::FocusAnalyzer>,
    pub(super) shared_capture_services: Option<Arc<SharedCaptureServices>>,
    pub(super) capture_consent_manager: Arc<dyn ConsentManagerPort>,
    pub(super) regime_storage: Arc<dyn RegimeStoragePort>,
    pub(super) regime_manager_arc: Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>,
    pub(super) codex_approval_registry: crate::provider_adapters::CodexApprovalRegistry,
    /// Cross-device-sync IPC state — carries the shared write-once SyncEngine
    /// slot the agent runtime populates once built. (#6264)
    pub(super) sync_runtime_state: SyncRuntimeState,
    /// Embedding hot-reload IPC state — carries the shared write-once
    /// ReloadableModel slot the agent runtime populates once built. (#6266)
    pub(super) embedding_runtime_state: EmbeddingRuntimeState,
    // ── Raw inputs for the IPC runtime states built below ─────────────────────
    // The per-command runtime states (ai_session / audio / config / suggestion /
    // detection / automation) and `analysis_health` are assembled here rather
    // than in app_runtime_launch/mod.rs, keeping the launch module a small
    // composition root (ADR-013). No behaviour change — same constructors/order.
    pub(super) app_handle: AppHandle,
    pub(super) config_manager: maekon_core::config_manager::ConfigManager,
    pub(super) web_port: Arc<AtomicU16>,
    pub(super) session_manager: SessionManagerLaunch,
    pub(super) suggestion_manager: Option<Arc<SuggestionManager>>,
    pub(super) shared_regime_state: Arc<SharedRegimeState>,
    pub(super) detection_active: Arc<AtomicBool>,
    pub(super) scene_finder_slot: SceneFinderSlot,
    /// Live automation controller iff `web.enabled` — `None` keeps all automation
    /// IPCs at service.unavailable, symmetric with the REST surface. (#5703)
    pub(super) automation_controller: Option<Arc<AutomationController>>,
    /// Analysis primary-health flag; folded into `AnalysisHealthFlags` only when
    /// analysis is enabled and an LLM API is configured.
    pub(super) analysis_health_flag: Arc<AtomicBool>,
}

pub(super) fn build_managed_state_builder(parts: ManagedStateWiringParts) -> ManagedStateBuilder {
    let ManagedStateWiringParts {
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
        shared_capture_services,
        capture_consent_manager,
        regime_storage,
        regime_manager_arc,
        codex_approval_registry,
        sync_runtime_state,
        embedding_runtime_state,
        app_handle,
        config_manager,
        web_port,
        session_manager,
        suggestion_manager,
        shared_regime_state,
        detection_active,
        scene_finder_slot,
        automation_controller,
        analysis_health_flag,
    } = parts;

    // Build the per-command IPC runtime states from the raw inputs. Same
    // constructors/order as the former app_runtime_launch/mod.rs block; `config`
    // is read (borrowed) before it is moved into `AppState` below, and
    // `magic_overlay` is cloned for the states that need it before it is moved.
    let ai_session_runtime_state = AiSessionRuntimeState::new(
        session_manager.as_ref().map(|(sm, _)| sm.clone()),
        Some(sqlite_storage.clone()),
        config.ai_session.max_history_turns,
    );
    let audio_runtime_state = build_audio_runtime_state(
        &app_handle,
        config_manager.clone(),
        &config,
        capture_consent_manager.clone(),
        capture_paused.clone(),
        // #8059: persist transcripts through the shared SQLite storage (coerced
        // to the transcript port), so a transcription survives restart and is
        // reachable from keyword search.
        Some(sqlite_storage.clone()),
    );
    let config_runtime_state = ConfigRuntimeState::new(config_manager, web_port);
    let suggestion_runtime_state =
        SuggestionRuntimeState::new(suggestion_manager, Some(magic_overlay.clone()))
            .with_shared_regime(shared_regime_state);
    let detection_runtime_state = DetectionRuntimeState::with_scene_finder_slot(
        detection_active,
        scene_finder_slot,
        Some(magic_overlay.clone()),
    );
    // #5703: live controller iff web.enabled — None keeps all 7 automation IPCs at
    // service.unavailable, symmetric with the REST surface.
    let automation_runtime_state = AutomationRuntimeState::new(
        automation_controller.map(|c| c as Arc<dyn maekon_core::ports::automation::AutomationPort>),
    );
    // Compute analysis_health before `config` is moved into AppState.
    let analysis_health = if config.analysis.enabled && config.ai_provider.llm_api.is_some() {
        Some(AnalysisHealthFlags {
            primary_healthy: analysis_health_flag,
        })
    } else {
        None
    };

    ManagedStateBuilder::new(
        AppState {
            runtime_handle: handle,
            background_runtime,
            config,
            storage: sqlite_storage.clone(),
            update_control: Some(update_control),
            update_action_tx,
            shutdown_tx,
            recluster_requested,
            magic_overlay: Some(magic_overlay),
            coaching_engine: Some(
                coaching_engine as Arc<dyn maekon_core::ports::coaching::CoachingPort>,
            ),
            capture_paused,
            indicator_visible,
            connection: ConnectionStatus {
                server_connected,
                llm_connected,
                cli_connected,
            },
            focus_mode,
            focus_analyzer: Some(focus_analyzer),
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
                    maekon_vision::work_classifier::RuleBasedClassifier,
                )),
            },
            analysis_health,
            regime_storage: Some(regime_storage),
            regime_manager_snapshot: Some(regime_manager_arc),
        },
        config_runtime_state,
    )
    .with_ai_session_runtime(ai_session_runtime_state)
    .with_codex_approval_runtime(CodexApprovalRuntimeState::new(codex_approval_registry))
    .with_audio_runtime(audio_runtime_state)
    .with_suggestion_runtime(suggestion_runtime_state)
    .with_detection_runtime(detection_runtime_state)
    // #5703: wire automation state so the live controller is reachable from the
    // confirmation-flow IPC commands (get_pending_confirmations,
    // confirm_automation_command). #7600: the other 5 automation IPCs
    // (list_automation_presets, run_automation_preset, execute_automation_hint,
    // analyze_automation_scene, check_automation_available) were removed as
    // dead duplicates — the frontend drives them via the embedded HTTP API.
    .with_automation_runtime(automation_runtime_state)
    // #6264: wire sync state so the 4 cross-device-sync IPC commands reach the
    // live SyncEngine once the agent runtime populates the shared slot
    // (get_sync_status, trigger_sync, list_sync_peers, forget_sync_peer).
    .with_sync_runtime(sync_runtime_state)
    // #6266: wire embedding state so reload_embedding_model reaches the live
    // reloadable model once the agent runtime populates the shared slot.
    .with_embedding_runtime(embedding_runtime_state)
}
