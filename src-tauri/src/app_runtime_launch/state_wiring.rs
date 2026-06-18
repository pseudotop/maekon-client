use crate::bootstrap_runtime::ManagedBackgroundRuntime;
use crate::capture_services::SharedCaptureServices;
use crate::focus_mode::FocusModeState;
use crate::magic_overlay::MagicOverlayHandle;
use crate::runtime_state::{
    AiSessionRuntimeState, AnalysisHealthFlags, AppState, AudioRuntimeState,
    AutomationRuntimeState, CaptureContext, CodexApprovalRuntimeState, ConfigRuntimeState,
    ConnectionStatus, DetectionRuntimeState, EmbeddingRuntimeState, ManagedStateBuilder,
    SuggestionRuntimeState, SyncRuntimeState,
};
use maekon_core::config::AppConfig;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::regime_storage::RegimeStoragePort;
use maekon_storage::sqlite::SqliteStorage;
use maekon_web::update_control::{UpdateAction, UpdateControl};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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
    pub(super) shared_capture_services: Option<Arc<SharedCaptureServices>>,
    pub(super) capture_consent_manager: Arc<dyn ConsentManagerPort>,
    pub(super) analysis_health: Option<AnalysisHealthFlags>,
    pub(super) regime_storage: Arc<dyn RegimeStoragePort>,
    pub(super) regime_manager_arc: Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>,
    pub(super) config_runtime_state: ConfigRuntimeState,
    pub(super) ai_session_runtime_state: AiSessionRuntimeState,
    pub(super) codex_approval_registry: crate::provider_adapters::CodexApprovalRegistry,
    pub(super) audio_runtime_state: AudioRuntimeState,
    pub(super) suggestion_runtime_state: SuggestionRuntimeState,
    pub(super) detection_runtime_state: DetectionRuntimeState,
    /// Automation controller state — carries the live AutomationPort so IPC
    /// commands can execute presets and respond to confirmation requests. (#5703)
    pub(super) automation_runtime_state: AutomationRuntimeState,
    /// Cross-device-sync IPC state — carries the shared write-once SyncEngine
    /// slot the agent runtime populates once built. (#6264)
    pub(super) sync_runtime_state: SyncRuntimeState,
    /// Embedding hot-reload IPC state — carries the shared write-once
    /// ReloadableModel slot the agent runtime populates once built. (#6266)
    pub(super) embedding_runtime_state: EmbeddingRuntimeState,
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
    } = parts;

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
    // #5703: wire automation state so the live controller is reachable from all
    // 7 automation IPC commands (list_automation_presets, run_automation_preset,
    // execute_automation_hint, analyze_automation_scene, get_pending_confirmations,
    // confirm_automation_command, check_automation_available).
    .with_automation_runtime(automation_runtime_state)
    // #6264: wire sync state so the 4 cross-device-sync IPC commands reach the
    // live SyncEngine once the agent runtime populates the shared slot
    // (get_sync_status, trigger_sync, list_sync_peers, forget_sync_peer).
    .with_sync_runtime(sync_runtime_state)
    // #6266: wire embedding state so reload_embedding_model reaches the live
    // reloadable model once the agent runtime populates the shared slot.
    .with_embedding_runtime(embedding_runtime_state)
}
