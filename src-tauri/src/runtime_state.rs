use oneshim_core::config::{AppConfig, CredentialBackendKind};
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::consent::ConsentManager;
use oneshim_core::ports::accessibility::AccessibilityExtractor;
use oneshim_core::ports::audio_capture::AudioCapturePort;
use oneshim_core::ports::coaching::CoachingPort;
use oneshim_core::ports::element_finder::ElementFinder;
use oneshim_core::ports::frame_storage::FrameStoragePort;
use oneshim_core::ports::integration::{IntegrationAuthPort, IntegrationSessionPort};
use oneshim_core::ports::model_downloader::ModelDownloader;
use oneshim_core::ports::monitor::ActivityMonitor;
use oneshim_core::ports::oauth::OAuthPort;
use oneshim_core::ports::session_storage::SessionStoragePort;
use oneshim_core::ports::stt_provider::SttProvider;
use oneshim_core::ports::vision::FrameProcessor;
use oneshim_core::ports::work_classifier::WorkTypeClassifier;
use oneshim_storage::sqlite::SqliteStorage;
use oneshim_web::update_control::{UpdateAction, UpdateControl};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU16};
use std::sync::Arc;
use tauri::{App, Manager};

use crate::magic_overlay::MagicOverlayHandle;
use crate::session_manager::SessionManagerImpl;
use crate::suggestion_manager::SuggestionManager;

#[cfg(feature = "server")]
pub(crate) type OAuthCoordinator =
    Option<Arc<oneshim_network::oauth::refresh_coordinator::TokenRefreshCoordinator>>;
#[cfg(not(feature = "server"))]
pub(crate) type OAuthCoordinator = Option<()>;

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
    pub consent_manager: Option<Arc<ConsentManager>>,
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

/// Feature-scoped Tauri managed state for audio capture and STT commands.
pub struct AudioRuntimeState {
    config_manager: ConfigManager,
    /// ConsentManager for GDPR-compliant gate check in start_audio_capture (CONS-PC04 / D13).
    consent_manager: Option<Arc<ConsentManager>>,
    /// Tray-pause veto flag for start_audio_capture gate (CONS-PC02 / D13).
    capture_paused: Arc<AtomicBool>,
    audio: AudioContext,
}

impl AudioRuntimeState {
    pub(crate) fn new(
        config_manager: ConfigManager,
        consent_manager: Option<Arc<ConsentManager>>,
        capture_paused: Arc<AtomicBool>,
        audio: AudioContext,
    ) -> Self {
        Self {
            config_manager,
            consent_manager,
            capture_paused,
            audio,
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
            AudioContext::disabled(std::env::temp_dir().join("oneshim-audio-models")),
        )
    }

    pub(crate) fn config_manager(&self) -> &ConfigManager {
        &self.config_manager
    }

    pub(crate) fn consent_manager(&self) -> Option<&Arc<ConsentManager>> {
        self.consent_manager.as_ref()
    }

    pub(crate) fn capture_paused(&self) -> &Arc<AtomicBool> {
        &self.capture_paused
    }

    pub(crate) fn audio(&self) -> &AudioContext {
        &self.audio
    }
}

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

/// Feature-scoped Tauri managed state for overlay suggestion IPC and shortcuts.
#[derive(Default)]
pub struct SuggestionRuntimeState {
    manager: Option<Arc<SuggestionManager>>,
    overlay: Option<MagicOverlayHandle>,
}

impl SuggestionRuntimeState {
    pub(crate) fn new(
        manager: Option<Arc<SuggestionManager>>,
        overlay: Option<MagicOverlayHandle>,
    ) -> Self {
        Self { manager, overlay }
    }

    pub(crate) fn manager(&self) -> Option<Arc<SuggestionManager>> {
        self.manager.clone()
    }

    pub(crate) fn overlay(&self) -> Option<MagicOverlayHandle> {
        self.overlay.clone()
    }
}

/// Feature-scoped Tauri managed state for embedding model hot-reloading.
///
/// Holds a `ReloadableModel` so that `reload()` and `model_version()` are
/// accessible from IPC commands without depending on the concrete embedding
/// crate (which may be feature-gated).
#[derive(Default)]
pub struct EmbeddingRuntimeState {
    reloadable: Option<Arc<dyn oneshim_core::ports::embedding_provider::ReloadableModel>>,
}

#[allow(dead_code)] // wired when embedding provider is available
impl EmbeddingRuntimeState {
    pub(crate) fn new(
        reloadable: Option<Arc<dyn oneshim_core::ports::embedding_provider::ReloadableModel>>,
    ) -> Self {
        Self { reloadable }
    }

    pub(crate) fn reloadable(
        &self,
    ) -> Option<&Arc<dyn oneshim_core::ports::embedding_provider::ReloadableModel>> {
        self.reloadable.as_ref()
    }
}

/// Feature-scoped Tauri managed state for automation (RPA) IPC commands.
#[derive(Default)]
pub struct AutomationRuntimeState {
    controller: Option<Arc<dyn oneshim_core::ports::automation::AutomationPort>>,
}

#[allow(dead_code)] // wired when automation controller is available
impl AutomationRuntimeState {
    pub(crate) fn new(
        controller: Option<Arc<dyn oneshim_core::ports::automation::AutomationPort>>,
    ) -> Self {
        Self { controller }
    }

    pub(crate) fn controller(
        &self,
    ) -> Option<Arc<dyn oneshim_core::ports::automation::AutomationPort>> {
        self.controller.clone()
    }
}

/// Feature-scoped Tauri managed state for cross-device sync IPC commands.
#[derive(Default)]
pub struct SyncRuntimeState {
    engine: Option<Arc<crate::sync_engine::SyncEngine>>,
}

#[allow(dead_code)] // wired when sync feature enabled in agent_runtime
impl SyncRuntimeState {
    pub(crate) fn new(engine: Option<Arc<crate::sync_engine::SyncEngine>>) -> Self {
        Self { engine }
    }

    pub(crate) fn engine(&self) -> Option<Arc<crate::sync_engine::SyncEngine>> {
        self.engine.clone()
    }
}

/// Feature-scoped Tauri managed state for detection overlay IPC and shortcuts.
pub struct DetectionRuntimeState {
    active: Arc<AtomicBool>,
    scene_finder: Option<Arc<dyn ElementFinder>>,
    overlay: Option<MagicOverlayHandle>,
}

impl Default for DetectionRuntimeState {
    fn default() -> Self {
        Self {
            active: Arc::new(AtomicBool::new(false)),
            scene_finder: None,
            overlay: None,
        }
    }
}

impl DetectionRuntimeState {
    pub(crate) fn new(
        active: Arc<AtomicBool>,
        scene_finder: Option<Arc<dyn ElementFinder>>,
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
        self.scene_finder.clone()
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

#[allow(dead_code)] // runtime_handle/update_control stored for future scheduler access
pub struct AppState {
    pub runtime_handle: tokio::runtime::Handle,
    pub background_runtime: Arc<crate::bootstrap_runtime::ManagedBackgroundRuntime>,
    pub config: AppConfig,
    pub storage: Arc<SqliteStorage>,
    pub update_control: Option<UpdateControl>,
    pub update_action_tx: tokio::sync::mpsc::UnboundedSender<UpdateAction>,
    pub shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Shared flag for on-demand re-clustering requests from Tauri/REST.
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
    pub regime_storage: Option<Arc<dyn oneshim_core::ports::regime_storage::RegimeStoragePort>>,
    /// Shared handle to the `RegimeManager` so the shutdown path can read
    /// the current regime set and hand it to `regime_storage.save_all`.
    /// Populated by the composition root; `None` only in test builders.
    pub regime_manager_snapshot: Option<Arc<parking_lot::Mutex<oneshim_analysis::RegimeManager>>>,
}

pub struct OAuthState(pub Option<Arc<dyn OAuthPort>>);

#[allow(dead_code)] // Tauri managed state; inner accessed via pattern match in commands
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

#[allow(dead_code)] // Tauri managed state; inner accessed via pattern match in commands
pub struct IntegrationSessionState(pub Option<Arc<dyn IntegrationSessionPort>>);

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

    #[allow(dead_code)] // wired when embedding provider is available
    pub(crate) fn with_embedding_runtime(
        mut self,
        embedding_runtime_state: EmbeddingRuntimeState,
    ) -> Self {
        self.embedding_runtime_state = embedding_runtime_state;
        self
    }

    #[cfg(feature = "server")]
    pub(crate) fn with_oauth(
        mut self,
        oauth_port: Option<Arc<dyn OAuthPort>>,
        oauth_coordinator: OAuthCoordinator,
    ) -> Self {
        self.oauth_state = OAuthState(oauth_port);
        self.oauth_coordinator_state = OAuthCoordinatorState(oauth_coordinator);
        self
    }

    #[cfg(feature = "server")]
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
    use oneshim_web::update_control::UpdateAction;
    use tempfile::TempDir;
    use tokio::sync::mpsc;

    #[test]
    fn managed_state_builder_defaults_to_unavailable_secret_backend() {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        let handle = runtime.handle().clone();
        let temp_dir = TempDir::new().expect("temp dir");
        let storage = Arc::new(
            oneshim_storage::sqlite::SqliteStorage::open(
                &temp_dir.path().join("state.db"),
                1,
                None,
            )
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
    fn audio_runtime_state_disabled_seeds_capture_paused_true() {
        // Regression guard: the `disabled()` placeholder state must seed
        // `capture_paused` with `true`. That value is consumed by the 4-term
        // privacy gate in `commands::audio::start_audio_capture`; a `false`
        // seed would bypass the pause veto if a future wiring change let a
        // real `AudioContext` reach this state without going through
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
