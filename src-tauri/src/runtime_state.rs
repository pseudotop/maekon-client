use maekon_core::config::{AppConfig, CredentialBackendKind};
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::accessibility::AccessibilityExtractor;
use maekon_core::ports::audio_capture::AudioCapturePort;
use maekon_core::ports::coaching::CoachingPort;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::element_finder::ElementFinder;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::integration::{IntegrationAuthPort, IntegrationSessionPort};
use maekon_core::ports::model_downloader::ModelDownloader;
use maekon_core::ports::monitor::ActivityMonitor;
use maekon_core::ports::oauth::OAuthPort;
use maekon_core::ports::session_storage::SessionStoragePort;
use maekon_core::ports::stt_provider::SttProvider;
use maekon_core::ports::vision::FrameProcessor;
use maekon_core::ports::work_classifier::WorkTypeClassifier;
use maekon_storage::sqlite::SqliteStorage;
use maekon_web::update_control::{UpdateAction, UpdateControl};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::Arc;
use tauri::{App, Manager};

use crate::magic_overlay::MagicOverlayHandle;
use crate::scheduler::shared_regime_state::SharedRegimeState;
use crate::session_manager::SessionManagerImpl;
use crate::suggestion_manager::SuggestionManager;

#[cfg(feature = "analysis")]
pub(crate) type OAuthCoordinator =
    Option<Arc<maekon_network::oauth::refresh_coordinator::TokenRefreshCoordinator>>;
#[cfg(not(feature = "analysis"))]
pub(crate) type OAuthCoordinator = Option<()>;

/// Shared finder slot observed by IPC state and scheduler even when automation builds later.
pub(crate) type SceneFinderSlot = Arc<std::sync::OnceLock<Arc<dyn ElementFinder>>>;

/// Health flags for the analysis LLM provider fallback chain.
pub struct AnalysisHealthFlags {
    pub primary_healthy: Arc<AtomicBool>,
}

/// Groups frame processor, frame storage, activity monitor, accessibility
/// extractor, and consent manager used by IPC capture commands (A1, A2).
pub struct CaptureContext {
    /// Frame processor for on-demand capture (A1, A2).
    pub frame_processor: Option<Arc<dyn FrameProcessor>>,
    /// Frame file storage for persisting captured images.
    pub frame_storage: Option<Arc<dyn FrameStoragePort>>,
    /// Activity monitor for current window context (A1, A2).
    pub activity_monitor: Option<Arc<dyn ActivityMonitor>>,
    /// Accessibility extractor for scene analysis (A2).
    pub accessibility_extractor: Option<Arc<dyn AccessibilityExtractor>>,
    /// Consent manager for PII level gating in accessibility extraction (A2).
    pub consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// Work type classifier for scene analysis (A2).
    pub work_classifier: Option<Arc<dyn WorkTypeClassifier>>,
}

/// Audio capture, STT engine, and model management for voice input.
pub struct AudioContext {
    pub capture: Option<Arc<dyn AudioCapturePort>>,
    /// RwLock allows hot-reload after model download.
    pub stt_engine: Arc<tokio::sync::RwLock<Option<Arc<dyn SttProvider>>>>,
    pub model_downloader: Option<Arc<dyn ModelDownloader>>,
    pub model_dir: PathBuf,
    /// Prevents concurrent downloads.
    pub downloading: Arc<AtomicBool>,
    /// Cancel flag for active download — set to true to abort.
    pub download_cancel: Arc<AtomicBool>,
    /// VAD state: "idle", "listening", "speech", "transcribing".
    /// Tracked at IPC layer — not inside VadDetector.
    pub vad_state: Arc<parking_lot::Mutex<String>>,
}

impl AudioContext {
    pub(crate) fn disabled(model_dir: PathBuf) -> Self {
        Self {
            capture: None,
            stt_engine: Arc::new(tokio::sync::RwLock::new(None)),
            model_downloader: None,
            model_dir,
            downloading: Arc::new(AtomicBool::new(false)),
            download_cancel: Arc::new(AtomicBool::new(false)),
            vad_state: Arc::new(parking_lot::Mutex::new("idle".into())),
        }
    }
}

/// Feature-scoped Tauri managed state for AI session commands and shutdown hooks.
pub struct AiSessionRuntimeState {
    manager: Option<Arc<SessionManagerImpl>>,
    session_storage: Option<Arc<dyn SessionStoragePort>>,
    max_history_turns: u32,
}

impl Default for AiSessionRuntimeState {
    fn default() -> Self {
        Self {
            manager: None,
            session_storage: None,
            max_history_turns: 100,
        }
    }
}

impl AiSessionRuntimeState {
    pub(crate) fn new(
        manager: Option<Arc<SessionManagerImpl>>,
        session_storage: Option<Arc<dyn SessionStoragePort>>,
        max_history_turns: u32,
    ) -> Self {
        Self {
            manager,
            session_storage,
            max_history_turns,
        }
    }

    pub(crate) fn manager_impl(&self) -> Option<Arc<SessionManagerImpl>> {
        self.manager.clone()
    }

    pub(crate) fn session_storage(&self) -> Option<Arc<dyn SessionStoragePort>> {
        self.session_storage.clone()
    }

    pub(crate) fn daily_token_budget(&self) -> Option<u64> {
        self.manager
            .as_ref()
            .map(|manager| manager.config.daily_token_budget)
    }

    pub(crate) fn max_history_turns(&self) -> u32 {
        self.max_history_turns
    }

    pub(crate) async fn shutdown_all(&self) {
        if let Some(manager) = &self.manager {
            manager.shutdown_all().await;
        }
    }
}

/// Feature-scoped Tauri managed state holding the Codex approval registry
/// (E21 #5044). Holds the SAME `Arc<Mutex<HashMap<..>>>` the real
/// `CodexUiApprovalHook` writes to, so the `respond_codex_approval` IPC command
/// can resolve the parked oneshot for a `request_id`. Default is an empty map
/// (no approvals can be resolved until the hook + decider are wired).
#[derive(Clone)]
pub struct CodexApprovalRuntimeState {
    registry: crate::provider_adapters::CodexApprovalRegistry,
}

impl Default for CodexApprovalRuntimeState {
    fn default() -> Self {
        Self {
            registry: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }
}

impl CodexApprovalRuntimeState {
    /// Wrap an existing registry Arc — used to install the SAME instance the
    /// hook writes to (single-instance dead-writer guard).
    pub(crate) fn new(registry: crate::provider_adapters::CodexApprovalRegistry) -> Self {
        Self { registry }
    }

    pub(crate) fn registry(&self) -> &crate::provider_adapters::CodexApprovalRegistry {
        &self.registry
    }
}

/// Feature-scoped Tauri managed state for audio capture and STT commands.
pub struct AudioRuntimeState {
    config_manager: ConfigManager,
    /// ConsentManager for GDPR-compliant gate check in start_audio_capture (CONS-PC04 / D13).
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// Tray-pause veto flag for start_audio_capture gate (CONS-PC02 / D13).
    capture_paused: Arc<AtomicBool>,
    audio: AudioContext,
    /// Immediate VAD re-gate signal. Fired by the pause handler (and, in future,
    /// a consent-revoke command) so a privacy-revoking gesture tears down the
    /// microphone at once rather than waiting for the ≤2 s backstop tick.
    audio_regate: Arc<tokio::sync::Notify>,
    /// Remembers (at the pause edge) whether VAD was running, so an unpause can
    /// auto-restart it. One-shot: set at pause, read-and-cleared at unpause.
    vad_resume_pending: Arc<AtomicBool>,
}

impl AudioRuntimeState {
    pub(crate) fn new(
        config_manager: ConfigManager,
        consent_manager: Option<Arc<dyn ConsentManagerPort>>,
        capture_paused: Arc<AtomicBool>,
        audio: AudioContext,
    ) -> Self {
        Self {
            config_manager,
            consent_manager,
            capture_paused,
            audio,
            audio_regate: Arc::new(tokio::sync::Notify::new()),
            vad_resume_pending: Arc::new(AtomicBool::new(false)),
        }
    }

    pub(crate) fn disabled(config_manager: ConfigManager) -> Self {
        // Seed `capture_paused` with `true` so the 4-term privacy gate in
        // `start_audio_capture` always returns false while this placeholder
        // state is installed. Without this, a future regression that attaches
        // a real `AudioContext` without also swapping the runtime state (via
        // `with_audio_runtime`) could pass the gate unchallenged.
        Self::new(
            config_manager,
            None,
            Arc::new(AtomicBool::new(true)),
            AudioContext::disabled(std::env::temp_dir().join("maekon-audio-models")),
        )
    }

    pub(crate) fn config_manager(&self) -> &ConfigManager {
        &self.config_manager
    }

    pub(crate) fn consent_manager(&self) -> Option<&Arc<dyn ConsentManagerPort>> {
        self.consent_manager.as_ref()
    }

    pub(crate) fn capture_paused(&self) -> &Arc<AtomicBool> {
        &self.capture_paused
    }

    pub(crate) fn audio_regate(&self) -> &Arc<tokio::sync::Notify> {
        &self.audio_regate
    }

    pub(crate) fn set_vad_resume_pending(&self, pending: bool) {
        self.vad_resume_pending
            .store(pending, std::sync::atomic::Ordering::Relaxed);
    }

    /// Read and clear the resume flag (one-shot).
    pub(crate) fn take_vad_resume_pending(&self) -> bool {
        self.vad_resume_pending
            .swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn audio(&self) -> &AudioContext {
        &self.audio
    }
}

/// E20-41 (#4833): Tauri managed state holding the ephemeral per-session
/// local-API auth token. Read by the `get_local_auth_token` IPC command — the
/// WebView's race-safe fallback when the injected `window.__MAEKON_LOCAL_AUTH__`
/// global is not yet set. IPC is a Tauri-internal channel (not HTTP), so exposing
/// the token here does not widen the multi-user-loopback attack surface.
pub struct LocalAuthTokenState(pub Arc<str>);

/// Feature-scoped Tauri managed state for config-backed IPC commands.
pub struct ConfigRuntimeState {
    config_manager: ConfigManager,
    web_port: Arc<AtomicU16>,
}

impl ConfigRuntimeState {
    pub(crate) fn new(config_manager: ConfigManager, web_port: Arc<AtomicU16>) -> Self {
        Self {
            config_manager,
            web_port,
        }
    }

    pub(crate) fn config_manager(&self) -> &ConfigManager {
        &self.config_manager
    }

    pub(crate) fn web_port(&self) -> u16 {
        self.web_port.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// RAII reservation held for the duration of a single `run_suggestion_action`
/// (T4.1 #7917). Reserve-then-execute (#6699 house pattern): the id is removed
/// from the in-flight set when this guard drops, whether the run succeeded,
/// errored, or panicked. No lock is held across an `.await`.
pub(crate) struct ActionReservation {
    in_flight: Arc<parking_lot::Mutex<std::collections::HashSet<String>>>,
    suggestion_id: String,
}

impl Drop for ActionReservation {
    fn drop(&mut self) {
        self.in_flight.lock().remove(&self.suggestion_id);
    }
}

/// Feature-scoped Tauri managed state for overlay suggestion IPC and shortcuts.
#[derive(Default)]
pub struct SuggestionRuntimeState {
    manager: Option<Arc<SuggestionManager>>,
    overlay: Option<MagicOverlayHandle>,
    /// #7600: shared cross-loop regime snapshot, read by the feedback IPC
    /// command to attach the live `regime_id` to `SuggestionFeedback` on
    /// accept/reject/defer. `None` only in test builders that skip wiring.
    shared_regime: Option<Arc<SharedRegimeState>>,
    /// #7917: suggestion_ids with a `run_suggestion_action` currently in flight.
    /// The command-level in-flight guard: a second concurrent run for the same
    /// id is refused, so a double-fire cannot execute a (side-effectful) preset
    /// twice under the Auto policy. The UI's disable-while-pending is a UX
    /// nicety; this is the correctness guarantee.
    in_flight: Arc<parking_lot::Mutex<std::collections::HashSet<String>>>,
}

impl SuggestionRuntimeState {
    pub(crate) fn new(
        manager: Option<Arc<SuggestionManager>>,
        overlay: Option<MagicOverlayHandle>,
    ) -> Self {
        Self {
            manager,
            overlay,
            shared_regime: None,
            in_flight: Arc::default(),
        }
    }

    /// Reserve `suggestion_id` for execution. Returns `None` if a run for the
    /// same id is already in flight (the caller must refuse). The returned guard
    /// releases the reservation on drop (T4.1 #7917).
    pub(crate) fn try_reserve_action(&self, suggestion_id: &str) -> Option<ActionReservation> {
        let mut set = self.in_flight.lock();
        if set.contains(suggestion_id) {
            return None;
        }
        set.insert(suggestion_id.to_string());
        Some(ActionReservation {
            in_flight: self.in_flight.clone(),
            suggestion_id: suggestion_id.to_string(),
        })
    }

    /// Attach the shared cross-loop regime snapshot (#7600, chainable).
    pub(crate) fn with_shared_regime(mut self, shared_regime: Arc<SharedRegimeState>) -> Self {
        self.shared_regime = Some(shared_regime);
        self
    }

    pub(crate) fn manager(&self) -> Option<Arc<SuggestionManager>> {
        self.manager.clone()
    }

    pub(crate) fn overlay(&self) -> Option<MagicOverlayHandle> {
        self.overlay.clone()
    }

    /// The regime the user is (or was, as of the last monitor-loop tick) in,
    /// read from the shared cross-loop snapshot (#7600). `None` when regime
    /// tracking is not wired (test builders) or no regime has been
    /// classified yet.
    pub(crate) fn current_regime_id(&self) -> Option<String> {
        self.shared_regime
            .as_ref()
            .and_then(|r| r.snapshot().regime_id)
    }
}

/// Feature-scoped Tauri managed state for embedding model hot-reloading.
///
/// #6266: like [`SyncRuntimeState`], the `ReloadableModel` (the
/// `LocalEmbeddingProvider` facet) is built ASYNCHRONOUSLY inside the agent
/// runtime's embedding setup, so it does not exist at app-launch time when this
/// managed state is registered. It therefore lives in a shared write-once slot:
/// the runtime populates it via `slot()` once the local embedding provider is
/// built, and the `reload_embedding_model` IPC reads it via `reloadable()`
/// (returning `service.unavailable` until populated — which is also the honest
/// steady state in builds without the `embedding` feature / a local provider).
/// Cloning shares the same `Arc<OnceLock<..>>`.
#[derive(Clone, Default)]
pub struct EmbeddingRuntimeState {
    reloadable:
        Arc<std::sync::OnceLock<Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>>>,
}

impl EmbeddingRuntimeState {
    pub(crate) fn reloadable(
        &self,
    ) -> Option<Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>> {
        self.reloadable.get().cloned()
    }

    /// Shared write-once slot handle for the agent runtime to populate once the
    /// reloadable embedding model has been built (#6266).
    pub(crate) fn slot(
        &self,
    ) -> Arc<std::sync::OnceLock<Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>>>
    {
        self.reloadable.clone()
    }
}

/// Feature-scoped Tauri managed state for automation (RPA) IPC commands.
#[derive(Default)]
pub struct AutomationRuntimeState {
    controller: Option<Arc<dyn maekon_core::ports::automation::AutomationPort>>,
}

// #5703: removed stale `#[allow(dead_code)]` — controller is now wired via
// `with_automation_runtime` in the ManagedStateBuilder chain.
impl AutomationRuntimeState {
    pub(crate) fn new(
        controller: Option<Arc<dyn maekon_core::ports::automation::AutomationPort>>,
    ) -> Self {
        Self { controller }
    }

    pub(crate) fn controller(
        &self,
    ) -> Option<Arc<dyn maekon_core::ports::automation::AutomationPort>> {
        self.controller.clone()
    }
}

/// Feature-scoped Tauri managed state for cross-device sync IPC commands.
///
/// #6264: the `SyncEngine` is built ASYNCHRONOUSLY inside the agent runtime's
/// background task (it needs the device identity from SQLite plus transport
/// setup), so it does not yet exist at app-launch time when this managed state
/// is registered. The engine therefore lives in a shared write-once slot: the
/// runtime populates it via `slot()` once built, and the 4 sync IPC commands
/// read it via `engine()` (returning `service.unavailable` until populated).
/// Cloning shares the same `Arc<OnceLock<..>>`, so the registered managed-state
/// instance and the runtime's populating handle observe the same cell.
#[derive(Clone, Default)]
pub struct SyncRuntimeState {
    engine: Arc<std::sync::OnceLock<Arc<maekon_core::sync_engine::SyncEngine>>>,
}

impl SyncRuntimeState {
    pub(crate) fn engine(&self) -> Option<Arc<maekon_core::sync_engine::SyncEngine>> {
        self.engine.get().cloned()
    }

    /// Shared write-once slot handle for the agent runtime to populate once the
    /// `SyncEngine` has been built (#6264).
    pub(crate) fn slot(
        &self,
    ) -> Arc<std::sync::OnceLock<Arc<maekon_core::sync_engine::SyncEngine>>> {
        self.engine.clone()
    }
}

/// Feature-scoped Tauri managed state for detection overlay IPC and shortcuts.
pub struct DetectionRuntimeState {
    active: Arc<AtomicBool>,
    scene_finder: SceneFinderSlot,
    overlay: Option<MagicOverlayHandle>,
}

impl Default for DetectionRuntimeState {
    fn default() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            scene_finder: Arc::new(std::sync::OnceLock::new()),
            overlay: None,
        }
    }
}

impl DetectionRuntimeState {
    pub(crate) fn with_scene_finder_slot(
        active: Arc<AtomicBool>,
        scene_finder: SceneFinderSlot,
        overlay: Option<MagicOverlayHandle>,
    ) -> Self {
        Self {
            active,
            scene_finder,
            overlay,
        }
    }

    pub(crate) fn set_active(&self, active: bool) {
        self.active
            .store(active, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn toggle_active(&self) -> bool {
        !self
            .active
            .fetch_xor(true, std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn scene_finder(&self) -> Option<Arc<dyn ElementFinder>> {
        self.scene_finder.get().cloned()
    }

    pub(crate) fn overlay(&self) -> Option<MagicOverlayHandle> {
        self.overlay.clone()
    }
}

/// Groups connectivity flags for server, LLM, and CLI connections.
pub struct ConnectionStatus {
    /// Server API connectivity (REST or gRPC).
    pub server_connected: Arc<AtomicBool>,
    /// Local LLM provider connectivity (Ollama, subprocess, etc.).
    pub llm_connected: Arc<AtomicBool>,
    /// CLI bridge / automation controller connectivity.
    pub cli_connected: Arc<AtomicBool>,
}

pub struct AppState {
    // #7719: no `State<AppState>`-based IPC command reads this back — the
    // startup sequence's own `LaunchContext`-style builders (update_runtime,
    // launch_resources, background_runtime, web_server_runtime) hold and use
    // their own runtime handle before `AppState` is even constructed. Kept
    // for whenever a command needs to spawn directly off managed state.
    #[allow(dead_code)]
    pub runtime_handle: tokio::runtime::Handle,
    // #7734: narrowed from `pub` — `ManagedBackgroundRuntime` itself is
    // `pub(crate)` (composition-root internal), and nothing outside this
    // crate ever read this field (private_interfaces lint fallout from the
    // `[lib]` target enabler; behavior-neutral).
    pub(crate) background_runtime: Arc<crate::bootstrap_runtime::ManagedBackgroundRuntime>,
    pub config: AppConfig,
    pub storage: Arc<SqliteStorage>,
    pub update_control: Option<UpdateControl>,
    pub update_action_tx: tokio::sync::mpsc::UnboundedSender<UpdateAction>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Shared flag for on-demand re-clustering requests from Tauri/REST.
    // #7719: the SAME `Arc<AtomicBool>` is also threaded through the
    // scheduler's tracking-schedule state (`scheduler/analysis_pipeline/*.rs`
    // access it as `ts.recluster_requested`) — that path is what's actually
    // read; nothing reads it off `AppState` directly today.
    #[allow(dead_code)]
    pub recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    /// MagicOverlay handle for transparent coaching overlay window.
    pub magic_overlay: Option<MagicOverlayHandle>,
    /// Coaching engine for proactive coaching messages (Phase 2 IPC access).
    /// Typed as `dyn CoachingPort` so Tauri IPC commands call through the port trait,
    /// keeping the binary crate decoupled from the concrete `CoachingEngine` type.
    pub coaching_engine: Option<Arc<dyn CoachingPort>>,
    /// Whether capture/monitoring is paused (toggled via tray or IPC).
    pub capture_paused: Arc<AtomicBool>,
    /// Whether the tracking indicator border is visible.
    pub indicator_visible: Arc<AtomicBool>,
    /// Connection status flags for server, LLM, and CLI.
    pub connection: ConnectionStatus,
    /// Focus mode state — transient, not persisted. Suppresses coaching + notifications.
    pub focus_mode: Arc<crate::focus_mode::FocusModeState>,
    /// Capture-related resources for IPC commands (A1, A2).
    pub capture: CaptureContext,
    /// Analysis provider health (None when no LLM configured).
    pub analysis_health: Option<AnalysisHealthFlags>,
    /// Regime state persistence port. Populated by the composition root
    /// (`app_runtime_launch.rs`). The save-guard in
    /// `main.rs::RunEvent::Exit` short-circuits on `None` — which only
    /// happens in test builders that skip wiring.
    pub regime_storage: Option<Arc<dyn maekon_core::ports::regime_storage::RegimeStoragePort>>,
    /// Shared handle to the `RegimeManager` so the shutdown path can read
    /// the current regime set and hand it to `regime_storage.save_all`.
    /// Populated by the composition root; `None` only in test builders.
    pub regime_manager_snapshot: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>>,
}

pub struct OAuthState(pub Option<Arc<dyn OAuthPort>>);

pub struct OAuthCoordinatorState(pub OAuthCoordinator);

#[derive(Debug, Clone, Serialize)]
pub struct SecretBackendCapabilities {
    pub os_secret_store_available: bool,
    pub oauth_available: bool,
    pub oauth_provider_ids: Vec<String>,
    pub default_backend_kind: String,
    pub byok_backend_kind: String,
    pub fallback_backend_kind: String,
}

pub struct SecretBackendState(pub SecretBackendCapabilities);

// #7719: `app.manage(self.integration_session_state)` registers this as
// Tauri-managed state, but no IPC command currently extracts
// `State<IntegrationSessionState>` — same "kept for state-registration
// symmetry" situation as `IntegrationAuthState` below (#7600).
#[allow(dead_code)]
pub struct IntegrationSessionState(pub Option<Arc<dyn IntegrationSessionPort>>);

// #7600: the IPC commands that read this (integration_auth_status,
// integration_start_device_authorization, integration_poll_device_authorization,
// integration_cancel_device_authorization, integration_reset_auth_state) were
// removed as dead duplicates — the frontend drives device-auth via the
// embedded HTTP API instead. The same `Arc<dyn IntegrationAuthPort>` value is
// still constructed and wired to `web_server_runtime` for that HTTP path;
// this Tauri-managed wrapper is kept for state-registration symmetry.
#[allow(dead_code)]
pub struct IntegrationAuthState(pub Option<Arc<dyn IntegrationAuthPort>>);

#[derive(Clone)]
pub(crate) struct ManagedStateCapabilityProfile {
    pub(crate) oauth_provider_ids: Vec<String>,
    pub(crate) provider_backend_kind: CredentialBackendKind,
    pub(crate) fallback_backend_kind: CredentialBackendKind,
}

impl Default for ManagedStateCapabilityProfile {
    fn default() -> Self {
        Self {
            oauth_provider_ids: Vec::new(),
            provider_backend_kind: CredentialBackendKind::Unavailable,
            fallback_backend_kind: CredentialBackendKind::Unavailable,
        }
    }
}

pub(crate) struct ManagedStateRegistration {
    pub(crate) app_state: AppState,
    pub(crate) ai_session_runtime_state: AiSessionRuntimeState,
    pub(crate) codex_approval_runtime_state: CodexApprovalRuntimeState,
    pub(crate) audio_runtime_state: AudioRuntimeState,
    pub(crate) config_runtime_state: ConfigRuntimeState,
    pub(crate) suggestion_runtime_state: SuggestionRuntimeState,
    pub(crate) detection_runtime_state: DetectionRuntimeState,
    pub(crate) sync_runtime_state: SyncRuntimeState,
    pub(crate) automation_runtime_state: AutomationRuntimeState,
    pub(crate) embedding_runtime_state: EmbeddingRuntimeState,
    pub(crate) oauth_state: OAuthState,
    pub(crate) oauth_coordinator_state: OAuthCoordinatorState,
    pub(crate) secret_backend_state: SecretBackendState,
    pub(crate) feature_capability_state: crate::feature_capabilities::FeatureCapabilityState,
    pub(crate) integration_auth_state: IntegrationAuthState,
    pub(crate) integration_session_state: IntegrationSessionState,
}

pub(crate) struct ManagedStateBuilder {
    app_state: AppState,
    ai_session_runtime_state: AiSessionRuntimeState,
    codex_approval_runtime_state: CodexApprovalRuntimeState,
    audio_runtime_state: AudioRuntimeState,
    config_runtime_state: ConfigRuntimeState,
    suggestion_runtime_state: SuggestionRuntimeState,
    detection_runtime_state: DetectionRuntimeState,
    sync_runtime_state: SyncRuntimeState,
    automation_runtime_state: AutomationRuntimeState,
    embedding_runtime_state: EmbeddingRuntimeState,
    oauth_state: OAuthState,
    oauth_coordinator_state: OAuthCoordinatorState,
    capability_profile: ManagedStateCapabilityProfile,
    integration_auth_state: IntegrationAuthState,
    integration_session_state: IntegrationSessionState,
}

impl ManagedStateBuilder {
    pub(crate) fn new(app_state: AppState, config_runtime_state: ConfigRuntimeState) -> Self {
        let audio_runtime_state =
            AudioRuntimeState::disabled(config_runtime_state.config_manager().clone());
        Self {
            app_state,
            ai_session_runtime_state: AiSessionRuntimeState::default(),
            codex_approval_runtime_state: CodexApprovalRuntimeState::default(),
            audio_runtime_state,
            config_runtime_state,
            suggestion_runtime_state: SuggestionRuntimeState::default(),
            detection_runtime_state: DetectionRuntimeState::default(),
            sync_runtime_state: SyncRuntimeState::default(),
            automation_runtime_state: AutomationRuntimeState::default(),
            embedding_runtime_state: EmbeddingRuntimeState::default(),
            oauth_state: OAuthState(None),
            oauth_coordinator_state: OAuthCoordinatorState(None),
            capability_profile: ManagedStateCapabilityProfile::default(),
            integration_auth_state: IntegrationAuthState(None),
            integration_session_state: IntegrationSessionState(None),
        }
    }

    pub(crate) fn with_ai_session_runtime(
        mut self,
        ai_session_runtime_state: AiSessionRuntimeState,
    ) -> Self {
        self.ai_session_runtime_state = ai_session_runtime_state;
        self
    }

    /// Install the Codex approval registry state (E21 #5044). The registry held
    /// here MUST be the SAME `Arc` the `CodexUiApprovalHook` writes to, so the
    /// `respond_codex_approval` command resolves the parked oneshot.
    pub(crate) fn with_codex_approval_runtime(
        mut self,
        codex_approval_runtime_state: CodexApprovalRuntimeState,
    ) -> Self {
        self.codex_approval_runtime_state = codex_approval_runtime_state;
        self
    }

    pub(crate) fn with_audio_runtime(mut self, audio_runtime_state: AudioRuntimeState) -> Self {
        self.audio_runtime_state = audio_runtime_state;
        self
    }

    pub(crate) fn with_suggestion_runtime(
        mut self,
        suggestion_runtime_state: SuggestionRuntimeState,
    ) -> Self {
        self.suggestion_runtime_state = suggestion_runtime_state;
        self
    }

    pub(crate) fn with_detection_runtime(
        mut self,
        detection_runtime_state: DetectionRuntimeState,
    ) -> Self {
        self.detection_runtime_state = detection_runtime_state;
        self
    }

    /// Wire the AutomationRuntimeState so all 7 automation IPC commands gain a live
    /// controller. Without this call the state defaults to `Default` (controller = None)
    /// and every IPC returns service.unavailable. (#5703)
    pub(crate) fn with_automation_runtime(
        mut self,
        automation_runtime_state: AutomationRuntimeState,
    ) -> Self {
        self.automation_runtime_state = automation_runtime_state;
        self
    }

    /// Wire the SyncRuntimeState so the 4 cross-device-sync IPC commands (status,
    /// trigger, peers) gain a live engine once the agent runtime populates the
    /// shared slot. Without this call the state defaults to an empty slot and
    /// every IPC returns service.unavailable. (#6264)
    pub(crate) fn with_sync_runtime(mut self, sync_runtime_state: SyncRuntimeState) -> Self {
        self.sync_runtime_state = sync_runtime_state;
        self
    }

    /// Wire the EmbeddingRuntimeState so `reload_embedding_model` reaches the live
    /// reloadable model once the agent runtime populates the shared slot (#6266).
    pub(crate) fn with_embedding_runtime(
        mut self,
        embedding_runtime_state: EmbeddingRuntimeState,
    ) -> Self {
        self.embedding_runtime_state = embedding_runtime_state;
        self
    }

    #[cfg(feature = "analysis")]
    pub(crate) fn with_oauth(
        mut self,
        oauth_port: Option<Arc<dyn OAuthPort>>,
        oauth_coordinator: OAuthCoordinator,
    ) -> Self {
        self.oauth_state = OAuthState(oauth_port);
        self.oauth_coordinator_state = OAuthCoordinatorState(oauth_coordinator);
        self
    }

    #[cfg(feature = "analysis")]
    pub(crate) fn with_secret_backend_profile(
        mut self,
        capability_profile: ManagedStateCapabilityProfile,
    ) -> Self {
        self.capability_profile = capability_profile;
        self
    }

    #[cfg(feature = "server")]
    pub(crate) fn with_integration(
        mut self,
        integration_auth: Option<Arc<dyn IntegrationAuthPort>>,
        integration_session: Option<Arc<dyn IntegrationSessionPort>>,
    ) -> Self {
        self.integration_auth_state = IntegrationAuthState(integration_auth);
        self.integration_session_state = IntegrationSessionState(integration_session);
        self
    }

    pub(crate) fn build(self) -> ManagedStateRegistration {
        let oauth_available = self.oauth_state.0.is_some();
        let secret_backend_state = SecretBackendState(secret_backend_capabilities(
            oauth_available,
            self.capability_profile.oauth_provider_ids,
            self.capability_profile.provider_backend_kind,
            self.capability_profile.fallback_backend_kind,
        ));
        let feature_capability_state =
            crate::feature_capabilities::FeatureCapabilityState(secret_backend_state.0.clone());

        ManagedStateRegistration {
            app_state: self.app_state,
            ai_session_runtime_state: self.ai_session_runtime_state,
            codex_approval_runtime_state: self.codex_approval_runtime_state,
            audio_runtime_state: self.audio_runtime_state,
            config_runtime_state: self.config_runtime_state,
            suggestion_runtime_state: self.suggestion_runtime_state,
            detection_runtime_state: self.detection_runtime_state,
            sync_runtime_state: self.sync_runtime_state,
            automation_runtime_state: self.automation_runtime_state,
            embedding_runtime_state: self.embedding_runtime_state,
            oauth_state: self.oauth_state,
            oauth_coordinator_state: self.oauth_coordinator_state,
            secret_backend_state,
            feature_capability_state,
            integration_auth_state: self.integration_auth_state,
            integration_session_state: self.integration_session_state,
        }
    }
}

impl ManagedStateRegistration {
    pub(crate) fn register_on(self, app: &mut App) {
        app.manage(self.app_state);
        app.manage(self.ai_session_runtime_state);
        app.manage(self.codex_approval_runtime_state);
        app.manage(self.audio_runtime_state);
        app.manage(self.config_runtime_state);
        app.manage(self.suggestion_runtime_state);
        app.manage(self.detection_runtime_state);
        app.manage(self.sync_runtime_state);
        app.manage(self.automation_runtime_state);
        app.manage(self.embedding_runtime_state);
        app.manage(self.oauth_state);
        app.manage(self.oauth_coordinator_state);
        app.manage(self.secret_backend_state);
        app.manage(self.feature_capability_state);
        app.manage(self.integration_auth_state);
        app.manage(self.integration_session_state);
    }
}

pub(crate) fn secret_backend_capabilities(
    oauth_available: bool,
    oauth_provider_ids: Vec<String>,
    provider_backend_kind: CredentialBackendKind,
    fallback_backend_kind: CredentialBackendKind,
) -> SecretBackendCapabilities {
    SecretBackendCapabilities {
        os_secret_store_available: oauth_available,
        oauth_available,
        oauth_provider_ids,
        default_backend_kind: credential_backend_kind_to_wire(provider_backend_kind).to_string(),
        byok_backend_kind: credential_backend_kind_to_wire(provider_backend_kind).to_string(),
        fallback_backend_kind: credential_backend_kind_to_wire(fallback_backend_kind).to_string(),
    }
}

fn credential_backend_kind_to_wire(value: CredentialBackendKind) -> &'static str {
    match value {
        CredentialBackendKind::OsSecretStore => "os_secret_store",
        CredentialBackendKind::FileSecretStore => "file_secret_store",
        CredentialBackendKind::Env => "env",
        CredentialBackendKind::BridgeManaged => "bridge_managed",
        CredentialBackendKind::Unavailable => "unavailable",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::models::intent::{ElementBounds, UiElement};
    use maekon_core::models::ui_scene::UiScene;
    use maekon_web::update_control::UpdateAction;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    struct TestElementFinder;

    #[async_trait]
    impl ElementFinder for TestElementFinder {
        async fn find_element(
            &self,
            _text: Option<&str>,
            _role: Option<&str>,
            _region: Option<&ElementBounds>,
        ) -> Result<Vec<UiElement>, CoreError> {
            Ok(Vec::new())
        }

        async fn analyze_scene(
            &self,
            _app_name: Option<&str>,
            _screen_id: Option<&str>,
        ) -> Result<UiScene, CoreError> {
            Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "test finder does not analyze scenes".to_string(),
            })
        }

        async fn analyze_scene_from_image(
            &self,
            _image_data: Vec<u8>,
            _image_format: String,
            app_name: Option<&str>,
            screen_id: Option<&str>,
        ) -> Result<UiScene, CoreError> {
            self.analyze_scene(app_name, screen_id).await
        }

        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn detection_runtime_state_observes_late_scene_finder_slot_population() {
        let active = Arc::new(AtomicBool::new(false));
        let slot = Arc::new(std::sync::OnceLock::new());
        let state = DetectionRuntimeState::with_scene_finder_slot(active, slot.clone(), None);
        assert!(
            state.scene_finder().is_none(),
            "scene finder starts unavailable before automation runtime is built"
        );

        let finder: Arc<dyn ElementFinder> = Arc::new(TestElementFinder);
        slot.set(finder.clone())
            .unwrap_or_else(|_| panic!("scene finder slot must be empty"));

        let observed = state
            .scene_finder()
            .expect("late-populated scene finder must be visible to state readers");
        assert!(
            Arc::ptr_eq(&observed, &finder),
            "state readers must observe the exact finder Arc published later"
        );
    }

    #[test]
    fn managed_state_builder_defaults_to_unavailable_secret_backend() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let handle = runtime.handle().clone();
        let temp_dir = TempDir::new().expect("temp dir");
        let storage = Arc::new(
            maekon_storage::sqlite::SqliteStorage::open(&temp_dir.path().join("state.db"), 1, None)
                .expect("db"),
        );
        let config_manager =
            ConfigManager::with_path(temp_dir.path().join("config.json")).expect("config manager");
        let (update_action_tx, _update_action_rx) = mpsc::unbounded_channel::<UpdateAction>();
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);

        let web_port = Arc::new(AtomicU16::new(0));
        let registration = ManagedStateBuilder::new(
            AppState {
                runtime_handle: handle,
                background_runtime: crate::bootstrap_runtime::spawn_background_runtime()
                    .expect("background runtime"),
                config: AppConfig::default_config(),
                storage,
                update_control: None,
                update_action_tx,
                shutdown_tx,
                recluster_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                magic_overlay: None,
                coaching_engine: None,
                capture_paused: Arc::new(AtomicBool::new(false)),
                indicator_visible: Arc::new(AtomicBool::new(true)),
                connection: ConnectionStatus {
                    server_connected: Arc::new(AtomicBool::new(false)),
                    llm_connected: Arc::new(AtomicBool::new(false)),
                    cli_connected: Arc::new(AtomicBool::new(false)),
                },
                focus_mode: Arc::new(crate::focus_mode::FocusModeState::new()),
                capture: CaptureContext {
                    frame_processor: None,
                    frame_storage: None,
                    activity_monitor: None,
                    accessibility_extractor: None,
                    consent_manager: None,
                    work_classifier: None,
                },
                analysis_health: None,
                regime_storage: None,
                regime_manager_snapshot: None,
            },
            ConfigRuntimeState::new(config_manager, web_port),
        )
        .build();

        assert_eq!(
            registration.secret_backend_state.0.byok_backend_kind,
            "unavailable"
        );
        assert!(!registration.secret_backend_state.0.oauth_available);
    }

    #[test]
    fn audio_runtime_state_shares_one_regate_notify() {
        // The pause handler and the VAD task must observe the SAME Notify instance.
        let temp = TempDir::new().expect("temp dir");
        let config_manager =
            ConfigManager::with_path(temp.path().join("config.json")).expect("config manager");
        let state = AudioRuntimeState::disabled(config_manager);
        assert!(
            Arc::ptr_eq(state.audio_regate(), state.audio_regate()),
            "audio_regate() must return the same shared Notify instance"
        );
    }

    #[test]
    fn vad_resume_pending_set_then_take_is_one_shot() {
        let temp = TempDir::new().expect("temp dir");
        let config_manager =
            ConfigManager::with_path(temp.path().join("config.json")).expect("config manager");
        let state = AudioRuntimeState::disabled(config_manager);

        assert!(!state.take_vad_resume_pending(), "defaults to false");
        state.set_vad_resume_pending(true);
        assert!(
            state.take_vad_resume_pending(),
            "take returns the set value"
        );
        assert!(
            !state.take_vad_resume_pending(),
            "take clears — a second take is false (one-shot)"
        );
    }

    #[test]
    fn audio_runtime_state_disabled_seeds_capture_paused_true() {
        // Regression guard: the `disabled()` placeholder state must seed
        // `capture_paused` with `true`. That value is consumed by the shared
        // 4-term privacy gate (`commands::audio::ensure_capture_permitted`,
        // used by BOTH `start_audio_capture` and `start_vad_listening`); a
        // `false` seed would bypass the pause veto if a future wiring change let
        // a real `AudioContext` reach this state without going through
        // `with_audio_runtime`.
        use std::sync::atomic::Ordering;

        let temp_dir = TempDir::new().expect("temp dir");
        let config_manager =
            ConfigManager::with_path(temp_dir.path().join("config.json")).expect("config manager");
        let state = AudioRuntimeState::disabled(config_manager);

        assert!(
            state.capture_paused().load(Ordering::Relaxed),
            "AudioRuntimeState::disabled() must seed capture_paused=true to block the privacy gate"
        );
        assert!(
            state.consent_manager().is_none(),
            "disabled() must not carry a real ConsentManager"
        );
    }
}
