// ## Lock Ordering
//
// Acquire locks in this order to prevent deadlocks:
//
// 1. deferred_suggestions   (tokio::sync::Mutex — async, held briefly)
// 2. suggestion_queue        (tokio::sync::Mutex — async, held briefly)
// 3. retry_queue             (tokio::sync::Mutex — async, held briefly)
// 4. shared_regime_state     (parking_lot::RwLock — sync, <1μs ops)
// 5. capture_context         (AppState sub-struct fields)
// 6. context_analyzer        (parking_lot::RwLock — sync, <1μs ops; #7652)
//
// Never acquire a lower-numbered lock while holding a higher-numbered one.
// `context_analyzer` is always acquired standalone (read-clone-drop or
// write-replace-drop) and never held across an `.await`, so it has no
// ordering interaction with the locks above.

mod analysis_pipeline;
mod config;
mod egress_policy;
/// GUI Activity Intelligence pipeline — wired into the monitor loop.
/// Called after `run_analysis_tick()` each cycle when `gui_intelligence.enabled`.
pub(crate) mod gui_pipeline;
pub(crate) mod heatmap;
mod loops;
pub(crate) mod required_deps;
pub(crate) mod schedule;
pub(crate) mod shared_regime_state;
pub(crate) mod trigger_state;

// ── Public re-exports (external API) ────────────────────────────────────────
pub use config::SchedulerConfig;
pub use required_deps::SchedulerRequiredDeps;
// #7731: `SchedulerStorage` relocated to `maekon-core` (Hexagonal Architecture
// port); re-exported here so existing `crate::scheduler::SchedulerStorage`
// call sites are unaffected.
pub use maekon_core::ports::scheduler_storage::SchedulerStorage;
pub use schedule::audio_capture_permitted_now;
pub use schedule::capture_permitted_now;
pub use schedule::set_battery_saver_active_for_scheduler;
#[cfg(feature = "server")]
pub(crate) use schedule::tracking_schedule_allows_capture;
// #7735 E-3: `should_run_now_with_time` re-export removed — its only consumer
// (`capture_permitted_now_inner`, formerly in `tracking_schedule_helper.rs`)
// moved into `maekon_core::capture_gate` and now calls the core-crate-local
// `should_run_now_with_time` directly (no `crate::scheduler::` path needed).
pub(crate) use trigger_state::AdaptiveTriggerState;

use maekon_analysis::focus_analyzer::FocusAnalyzer;
use maekon_core::config_manager::ConfigManager;
use maekon_core::models::activity::SessionStats;
use maekon_core::ports::accessibility::AccessibilityExtractor;
#[cfg(feature = "hnsw")]
use maekon_core::ports::ann_index::AnnIndex;
use maekon_core::ports::api_client::ApiClient;
use maekon_core::ports::batch_sink::BatchSink;
use maekon_core::ports::calibration_store::CalibrationReader;
use maekon_core::ports::coaching_storage::CoachingStoragePort;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::monitor::{ActivityMonitor, ProcessMonitor, SystemMonitor};
use maekon_core::ports::overlay_driver::OverlayDriver;
use maekon_core::ports::storage::StorageService;
use maekon_core::ports::vector_index::VectorIndex;
use maekon_core::ports::vision::{CaptureTrigger, FrameProcessor};
#[cfg(feature = "analysis")]
use maekon_network::oauth::refresh_coordinator::TokenRefreshCoordinator;
use maekon_web::RealtimeEvent;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::notification_manager::NotificationManager;
use crate::runtime_state::SceneFinderSlot;

// ── Scheduler struct ─────────────────────────────────────────────────────────

pub struct Scheduler {
    pub(super) config: SchedulerConfig,
    pub(super) system_monitor: Arc<dyn SystemMonitor>,
    pub(super) activity_monitor: Arc<dyn ActivityMonitor>,
    pub(super) process_monitor: Arc<dyn ProcessMonitor>,
    pub(super) capture_trigger: Arc<dyn CaptureTrigger>,
    pub(super) frame_processor: Arc<dyn FrameProcessor>,
    pub(super) storage: Arc<dyn StorageService>,
    pub(super) sqlite_storage: Arc<dyn SchedulerStorage>,
    pub(super) frame_storage: Option<Arc<dyn FrameStoragePort>>,
    pub(super) batch_sink: Option<Arc<dyn BatchSink>>,
    pub(super) api_client: Option<Arc<dyn ApiClient>>,
    pub(super) event_tx: Option<broadcast::Sender<RealtimeEvent>>,
    pub(super) notification_manager: Option<Arc<NotificationManager>>,
    pub(super) focus_analyzer: Option<Arc<FocusAnalyzer>>,
    #[cfg(feature = "analysis")]
    pub(super) oauth_coordinator: Option<Arc<TokenRefreshCoordinator>>,
    /// #7652: shared runtime slot for the LLM analysis pipeline, wrapped in a
    /// `parking_lot::RwLock` (not a plain `Option`) so `spawn_analysis_loop`
    /// can install/tear down the analyzer WITHOUT an app restart when
    /// `analysis.enabled` flips at runtime. Single-writer: only the analysis
    /// loop writes to this slot; `spawn_monitor_loop` only reads it (clone +
    /// drop the guard immediately, never held across an `.await`).
    pub(super) context_analyzer:
        Arc<parking_lot::RwLock<Option<Arc<maekon_analysis::ContextAnalyzer>>>>,
    /// #7652: reusable factory that (re)builds the analyzer from the CURRENT
    /// (live) config on demand — the mechanism the analysis loop uses to
    /// honor a runtime enable/BYOK-provider change without a restart.
    #[cfg(feature = "analysis")]
    pub(super) context_analyzer_factory:
        Option<crate::agent_runtime_support::ContextAnalyzerFactory>,
    pub(super) config_manager: Option<ConfigManager>,
    pub(super) vector_store: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>>,
    pub(super) embedding_provider:
        Option<Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>>,
    pub(super) vector_index: Option<Arc<dyn VectorIndex>>,
    pub(super) search_coordinator: Option<Arc<maekon_analysis::AdaptiveSearchCoordinator>>,
    #[cfg(feature = "hnsw")]
    pub(super) ann_index: Option<Arc<dyn AnnIndex>>,
    pub(super) adaptive_trigger: Mutex<Option<AdaptiveTriggerState>>,
    pub(super) sync_engine: Option<Arc<maekon_core::sync_engine::SyncEngine>>,
    pub(super) accessibility_extractor: Option<Arc<dyn AccessibilityExtractor>>,
    pub(super) consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// #5069: per-feature performance emitter (buffer + consent-gated flush).
    /// Concrete `Arc` (not a trait object) because the flush loop needs
    /// `flush()` while instrumentation needs the `FeaturePerfRecorder` seam —
    /// both come from this one handle. `None` in non-`analysis` builds.
    #[cfg(feature = "analysis")]
    pub(super) feature_perf:
        Option<Arc<maekon_network::feature_perf_uploader::FeaturePerfUploader>>,
    pub(super) coaching_engine: Option<Arc<maekon_analysis::CoachingEngine>>,
    pub(super) magic_overlay: Option<crate::magic_overlay::MagicOverlayHandle>,
    pub(super) overlay_driver: Option<Arc<dyn OverlayDriver>>,
    pub(super) analysis_provider:
        Option<Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider>>,
    pub(super) coaching_storage: Option<Arc<dyn CoachingStoragePort>>,
    pub(super) capture_paused: Arc<std::sync::atomic::AtomicBool>,
    pub(super) detection_active: Arc<std::sync::atomic::AtomicBool>,
    pub(super) scene_finder_slot: Option<SceneFinderSlot>,
    pub(super) server_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) llm_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) cli_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) server_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) llm_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) cli_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) tray_app_handle: Option<tauri::AppHandle>,
    #[cfg(feature = "server")]
    pub(super) suggestion_receiver: Option<Arc<maekon_suggestion::receiver::SuggestionReceiver>>,
    // E20-24 (#4816): local pipeline needs the manager (producer pushes into its
    // queue; maintenance resurfaces deferred) — gated on `local-suggestions`.
    #[cfg(feature = "local-suggestions")]
    pub(super) suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>>,
    pub(super) suggestions_enabled: bool,
    pub(super) focus_mode: Arc<crate::focus_mode::FocusModeState>,
    pub(super) shared_regime: Option<Arc<shared_regime_state::SharedRegimeState>>,
    /// ADR-023: local symbolic memory-graph store for digest-claim promotion.
    /// Shares the single `SqliteStorage` Arc (Port Instance Sharing guardrail).
    pub(super) memory_graph:
        Option<Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>>,
    /// ADR-023 Phase-2: LLM belief revision over the memory graph. Already
    /// local-LLM-gated + consent-built by the composition root; the aggregation
    /// loop additionally gates each run on consent + `belief_revision_enabled`.
    pub(super) belief_revision: Option<Arc<maekon_analysis::BeliefRevision>>,
    /// ADR-033 §7.5: vault mirror writer for the aggregation loop.
    pub(super) memory_vault_writer:
        Option<Arc<dyn maekon_core::ports::memory_vault_writer::MemoryVaultWriterPort>>,
    /// #5810: periodic regime checkpoint storage port.
    ///
    /// Shares the same `SqliteRegimeManagerStateStore` Arc as the shutdown
    /// path (main.rs RunEvent::Exit). The aggregation loop calls `save_all`
    /// every `REGIME_CHECKPOINT_INTERVAL_MINS` as a crash-durability supplement.
    /// `None` in builds that do not wire regime persistence.
    pub(super) regime_storage:
        Option<Arc<dyn maekon_core::ports::regime_storage::RegimeStoragePort>>,
    /// #5810: live regime manager Arc — same instance as AdaptiveTriggerState and
    /// AppState.regime_manager_snapshot so checkpoints reflect in-flight regimes.
    pub(super) regime_manager_arc: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>>,
    /// #7678 D4: calibration-log retention enforcement in the aggregation loop's
    /// housekeeping block. Shares the same underlying `SqliteStorage` instance as
    /// `AdaptiveTriggerState.calibration_reader`/`calibration_writer` (Port
    /// Instance Sharing guardrail — no second handle). `None` in builds that do
    /// not wire the calibration store.
    pub(super) calibration_reader: Option<Arc<dyn CalibrationReader>>,
}

// ── Builder methods ──────────────────────────────────────────────────────────

impl Scheduler {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: SchedulerConfig,
        system_monitor: Arc<dyn SystemMonitor>,
        activity_monitor: Arc<dyn ActivityMonitor>,
        process_monitor: Arc<dyn ProcessMonitor>,
        capture_trigger: Arc<dyn CaptureTrigger>,
        frame_processor: Arc<dyn FrameProcessor>,
        storage: Arc<dyn StorageService>,
        sqlite_storage: Arc<dyn SchedulerStorage>,
        batch_sink: Option<Arc<dyn BatchSink>>,
        api_client: Option<Arc<dyn ApiClient>>,
    ) -> Self {
        Self {
            config,
            system_monitor,
            activity_monitor,
            process_monitor,
            capture_trigger,
            frame_processor,
            storage,
            sqlite_storage,
            frame_storage: None,
            batch_sink,
            api_client,
            event_tx: None,
            notification_manager: None,
            focus_analyzer: None,
            #[cfg(feature = "analysis")]
            oauth_coordinator: None,
            context_analyzer: Arc::new(parking_lot::RwLock::new(None)),
            #[cfg(feature = "analysis")]
            context_analyzer_factory: None,
            config_manager: None,
            vector_store: None,
            embedding_provider: None,
            vector_index: None,
            search_coordinator: None,
            #[cfg(feature = "hnsw")]
            ann_index: None,
            adaptive_trigger: Mutex::new(None),
            sync_engine: None,
            accessibility_extractor: None,
            consent_manager: None,
            #[cfg(feature = "analysis")]
            feature_perf: None,
            coaching_engine: None,
            magic_overlay: None,
            overlay_driver: None,
            analysis_provider: None,
            coaching_storage: None,
            capture_paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            detection_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scene_finder_slot: None,
            server_health_flag: None,
            llm_health_flag: None,
            cli_health_flag: None,
            server_connected: None,
            llm_connected: None,
            cli_connected: None,
            tray_app_handle: None,
            #[cfg(feature = "server")]
            suggestion_receiver: None,
            #[cfg(feature = "local-suggestions")]
            suggestion_manager: None,
            suggestions_enabled: false,
            focus_mode: Arc::new(crate::focus_mode::FocusModeState::new()),
            shared_regime: None,
            memory_graph: None,
            belief_revision: None,
            memory_vault_writer: None,
            regime_storage: None,
            regime_manager_arc: None,
            calibration_reader: None,
        }
    }

    /// Consume the 9-field VERIFIED-unconditional dependency set (#7737 C1
    /// PR-2). Every field is destructured by name (no `..`), so a field
    /// added to `SchedulerRequiredDeps` without a matching line here fails
    /// to compile instead of silently staying unset. Mirrors
    /// `maekon_web::WebServer::with_required_deps` (#7738 D-2). CRITICAL:
    /// every one of `Scheduler`'s own fields stays `Option<Arc<T>>` exactly
    /// as before this split — only `SchedulerRequiredDeps` is non-Option.
    pub fn with_required_deps(mut self, deps: SchedulerRequiredDeps) -> Self {
        let SchedulerRequiredDeps {
            frame_storage,
            notification_manager,
            focus_analyzer,
            config_manager,
            memory_graph,
            calibration_reader,
            belief_revision,
            regime_storage,
            memory_vault_writer,
        } = deps;
        self.frame_storage = Some(frame_storage);
        self.notification_manager = Some(notification_manager);
        self.focus_analyzer = Some(focus_analyzer);
        self.config_manager = Some(config_manager);
        self.memory_graph = Some(memory_graph);
        self.calibration_reader = Some(calibration_reader);
        self.belief_revision = Some(belief_revision);
        self.regime_storage = Some(regime_storage);
        self.memory_vault_writer = Some(memory_vault_writer);
        self
    }

    /// #5810: inject the live regime manager Arc for periodic checkpoints.
    /// Must be the same Arc used in AdaptiveTriggerState and AppState so that
    /// in-flight regime updates are captured at checkpoint time.
    pub fn with_regime_manager_arc(
        mut self,
        manager: Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>,
    ) -> Self {
        self.regime_manager_arc = Some(manager);
        self
    }

    pub fn with_event_tx(mut self, event_tx: broadcast::Sender<RealtimeEvent>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    #[cfg(feature = "analysis")]
    pub fn with_oauth_coordinator(mut self, coordinator: Arc<TokenRefreshCoordinator>) -> Self {
        self.oauth_coordinator = Some(coordinator);
        self
    }

    pub fn with_context_analyzer(self, analyzer: Arc<maekon_analysis::ContextAnalyzer>) -> Self {
        *self.context_analyzer.write() = Some(analyzer);
        self
    }

    /// #7652: wire the runtime-rebuild factory so the analysis loop can honor
    /// an `analysis.enabled` flip (or a freshly-saved BYOK `ai_provider.llm_api`
    /// key) WITHOUT an app restart, even when `with_context_analyzer` above was
    /// never called (analysis disabled, or no provider configured, at startup).
    #[cfg(feature = "analysis")]
    pub fn with_context_analyzer_factory(
        mut self,
        factory: crate::agent_runtime_support::ContextAnalyzerFactory,
    ) -> Self {
        self.context_analyzer_factory = Some(factory);
        self
    }

    pub fn with_vector_store(
        mut self,
        store: Arc<dyn maekon_core::ports::vector_store::VectorStore>,
    ) -> Self {
        self.vector_store = Some(store);
        self
    }

    pub fn with_embedding_provider(
        mut self,
        provider: Arc<dyn maekon_core::ports::embedding_provider::EmbeddingProvider>,
    ) -> Self {
        self.embedding_provider = Some(provider);
        self
    }

    pub fn with_vector_index(mut self, index: Arc<dyn VectorIndex>) -> Self {
        self.vector_index = Some(index);
        self
    }

    pub fn with_search_coordinator(
        mut self,
        coordinator: Arc<maekon_analysis::AdaptiveSearchCoordinator>,
    ) -> Self {
        self.search_coordinator = Some(coordinator);
        self
    }

    #[cfg(feature = "hnsw")]
    pub fn with_ann_index(mut self, ann: Arc<dyn AnnIndex>) -> Self {
        self.ann_index = Some(ann);
        self
    }

    // #7734: narrowed from `pub` — `AdaptiveTriggerState` itself is
    // `pub(crate)` and every call site is internal to this crate
    // (private_interfaces lint fallout from the `[lib]` target enabler;
    // behavior-neutral).
    pub(crate) fn with_adaptive_trigger(self, state: AdaptiveTriggerState) -> Self {
        *self.adaptive_trigger.lock().unwrap_or_else(|poisoned| {
            warn!("adaptive trigger lock poisoned — recovering inner data");
            poisoned.into_inner()
        }) = Some(state);
        self
    }

    pub fn with_sync_engine(mut self, engine: Arc<maekon_core::sync_engine::SyncEngine>) -> Self {
        self.sync_engine = Some(engine);
        self
    }

    pub fn with_accessibility_extractor(
        mut self,
        extractor: Arc<dyn AccessibilityExtractor>,
    ) -> Self {
        self.accessibility_extractor = Some(extractor);
        self
    }

    pub fn with_consent_manager(mut self, consent_manager: Arc<dyn ConsentManagerPort>) -> Self {
        self.consent_manager = Some(consent_manager);
        self
    }

    /// #5069: wire the feature-performance emitter (analysis builds only).
    #[cfg(feature = "analysis")]
    pub fn with_feature_perf(
        mut self,
        feature_perf: Arc<maekon_network::feature_perf_uploader::FeaturePerfUploader>,
    ) -> Self {
        self.feature_perf = Some(feature_perf);
        self
    }

    pub fn with_coaching_engine(mut self, engine: Arc<maekon_analysis::CoachingEngine>) -> Self {
        self.coaching_engine = Some(engine);
        self
    }

    pub fn with_magic_overlay(mut self, overlay: crate::magic_overlay::MagicOverlayHandle) -> Self {
        self.magic_overlay = Some(overlay);
        self
    }

    pub fn with_overlay_driver(mut self, driver: Arc<dyn OverlayDriver>) -> Self {
        self.overlay_driver = Some(driver);
        self
    }

    pub fn with_analysis_provider(
        mut self,
        provider: Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider>,
    ) -> Self {
        self.analysis_provider = Some(provider);
        self
    }

    pub fn with_coaching_storage(mut self, storage: Arc<dyn CoachingStoragePort>) -> Self {
        self.coaching_storage = Some(storage);
        self
    }

    pub fn with_capture_paused(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.capture_paused = flag;
        self
    }

    pub fn with_detection_active(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.detection_active = flag;
        self
    }

    pub fn with_scene_finder(
        mut self,
        finder: Arc<dyn maekon_core::ports::element_finder::ElementFinder>,
    ) -> Self {
        let slot = Arc::new(std::sync::OnceLock::new());
        let _ = slot.set(finder);
        self.scene_finder_slot = Some(slot);
        self
    }

    /// #7817: observes the same scene_finder slot populated after automation builds.
    pub fn with_scene_finder_slot(mut self, slot: SceneFinderSlot) -> Self {
        self.scene_finder_slot = Some(slot);
        self
    }

    pub fn with_health_flags(
        mut self,
        server: Arc<std::sync::atomic::AtomicBool>,
        llm: Arc<std::sync::atomic::AtomicBool>,
        cli: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.server_health_flag = Some(server);
        self.llm_health_flag = Some(llm);
        self.cli_health_flag = Some(cli);
        self
    }

    pub fn with_connection_flags(
        mut self,
        server: Arc<std::sync::atomic::AtomicBool>,
        llm: Arc<std::sync::atomic::AtomicBool>,
        cli: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.server_connected = Some(server);
        self.llm_connected = Some(llm);
        self.cli_connected = Some(cli);
        self
    }

    pub fn with_tray_app_handle(mut self, handle: tauri::AppHandle) -> Self {
        self.tray_app_handle = Some(handle);
        self
    }

    #[cfg(feature = "server")]
    pub fn with_suggestion_receiver(
        mut self,
        receiver: Arc<maekon_suggestion::receiver::SuggestionReceiver>,
    ) -> Self {
        self.suggestion_receiver = Some(receiver);
        self
    }

    pub fn with_suggestions_enabled(mut self, enabled: bool) -> Self {
        self.suggestions_enabled = enabled;
        self
    }

    #[cfg(feature = "local-suggestions")]
    pub fn with_suggestion_manager(
        mut self,
        manager: Arc<crate::suggestion_manager::SuggestionManager>,
    ) -> Self {
        self.suggestion_manager = Some(manager);
        self
    }

    /// #7914: the shared `FeedbackScorer` handle used for uniform learned
    /// relevance gating of every LOCAL suggestion producer, or `None` when the
    /// local-suggestion pipeline is not compiled in (`--no-default-features`).
    /// Keeping the `local-suggestions` cfg fork here lets the LOC-capped monitor
    /// loop read the handle in a single, cfg-free line. #7913 (T2.1) will back
    /// this scorer with persisted state; this accessor stays the seam.
    pub(in crate::scheduler) fn relevance_scorer(
        &self,
    ) -> Option<Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>> {
        #[cfg(feature = "local-suggestions")]
        {
            self.suggestion_manager.as_ref().map(|m| m.scorer().clone())
        }
        #[cfg(not(feature = "local-suggestions"))]
        {
            None
        }
    }

    pub fn with_focus_mode(mut self, focus_mode: Arc<crate::focus_mode::FocusModeState>) -> Self {
        self.focus_mode = focus_mode;
        self
    }

    pub fn with_shared_regime(
        mut self,
        regime: Arc<shared_regime_state::SharedRegimeState>,
    ) -> Self {
        self.shared_regime = Some(regime);
        self
    }

    // --- Session management ---

    pub(super) async fn initialize_session(&self, session_id: &str) {
        let sqlite_init = self.sqlite_storage.clone();
        let session_stats = SessionStats::new(session_id.to_string());
        if let Err(e) = sqlite_init.upsert_session(&session_stats).await {
            warn!("session initialize failure: {e}");
        }
    }

    // --- Spawn orchestration ---

    pub async fn run(
        &self,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
        app_handle: Option<tauri::AppHandle>,
    ) {
        info!(
            monitor_poll_ms = self.config.poll_interval.as_millis() as u64,
            metrics_ms = self.config.metrics_interval.as_millis() as u64,
            process_ms = self.config.process_interval.as_millis() as u64,
            detailed_process_ms = self.config.detailed_process_interval.as_millis() as u64,
            input_activity_ms = self.config.input_activity_interval.as_millis() as u64,
            sync_ms = self.config.sync_interval.as_millis() as u64,
            heartbeat_ms = self.config.heartbeat_interval.as_millis() as u64,
            aggregation_ms = self.config.aggregation_interval.as_millis() as u64,
            health_check_secs = config::HEALTH_CHECK_INTERVAL_SECS,
            coaching_secs = config::COACHING_INTERVAL_SECS,
            sqlite_maintenance_mins = config::SQLITE_MAINTENANCE_INTERVAL_MINS,
            "scheduler loops starting"
        );
        self.run_scheduler_loops(shutdown_rx, app_handle).await;
    }
}

#[cfg(test)]
mod tests;
