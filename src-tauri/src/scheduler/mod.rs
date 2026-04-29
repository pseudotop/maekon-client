// ## Lock Ordering
//
// Acquire locks in this order to prevent deadlocks:
//
// 1. deferred_suggestions   (tokio::sync::Mutex — async, held briefly)
// 2. suggestion_queue        (tokio::sync::Mutex — async, held briefly)
// 3. retry_queue             (tokio::sync::Mutex — async, held briefly)
// 4. shared_regime_state     (parking_lot::RwLock — sync, <1μs ops)
// 5. capture_context         (AppState sub-struct fields)
//
// Never acquire a lower-numbered lock while holding a higher-numbered one.

mod analysis_pipeline;
mod config;
/// GUI Activity Intelligence pipeline — wired into the monitor loop.
/// Called after `run_analysis_tick()` each cycle when `gui_intelligence.enabled`.
pub(crate) mod gui_pipeline;
pub(crate) mod heatmap;
mod loops;
pub(crate) mod shared_regime_state;

// ── Public re-exports (external API) ────────────────────────────────
pub use config::{SchedulerConfig, SchedulerStorage};
pub(crate) use loops::record_to_segment_summary;

use chrono::{Datelike, Timelike};
use oneshim_core::app_registry::AppRegistry;
use oneshim_core::config::{AppConfig, Weekday};
use oneshim_core::config_manager::ConfigManager;
use oneshim_core::consent::ConsentManager;
use oneshim_core::models::activity::SessionStats;
use oneshim_core::models::tiered_memory::ResolvedParams;
use oneshim_core::ports::accessibility::AccessibilityExtractor;
#[cfg(feature = "hnsw")]
use oneshim_core::ports::ann_index::AnnIndex;
use oneshim_core::ports::api_client::ApiClient;
use oneshim_core::ports::batch_sink::BatchSink;
use oneshim_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use oneshim_core::ports::coaching_storage::CoachingStoragePort;
use oneshim_core::ports::frame_storage::FrameStoragePort;
use oneshim_core::ports::monitor::{ActivityMonitor, ProcessMonitor, SystemMonitor};
use oneshim_core::ports::overlay_driver::OverlayDriver;
use oneshim_core::ports::storage::StorageService;
use oneshim_core::ports::vector_index::VectorIndex;
use oneshim_core::ports::vision::{CaptureTrigger, FrameProcessor};
#[cfg(feature = "server")]
use oneshim_network::oauth::refresh_coordinator::TokenRefreshCoordinator;
use oneshim_web::RealtimeEvent;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tracing::{info, warn};

use crate::focus_analyzer::FocusAnalyzer;
use crate::notification_manager::NotificationManager;

// --- Struct definition ---

/// Wraps all components needed for the adaptive tiered-memory pipeline.
/// Kept as owned (non-Arc) so the monitor loop can mutate the components
/// without interior-mutability overhead.
pub(crate) struct AdaptiveTriggerState {
    // --- Base analysis pipeline ---
    pub trigger: oneshim_analysis::AdaptiveTrigger,
    pub segment_buffer: oneshim_analysis::SegmentBuffer,
    pub calibration_buffer: oneshim_analysis::CalibrationBuffer,
    pub title_bar_parser: oneshim_analysis::TitleBarParser,
    pub work_type_classifier: oneshim_analysis::WorkTypeClassifier,
    pub content_tracker: oneshim_analysis::ContentTracker,
    pub segment_summarizer: oneshim_analysis::SegmentSummarizer,
    pub params: ResolvedParams,
    pub calibration_writer: Arc<dyn CalibrationWriter>,

    // --- Regime-aware pipeline ---
    //
    // Wrapped in `Arc<parking_lot::Mutex<_>>` so the composition root
    // can share handles with `AppState` for (a) startup hydration from
    // `RegimeStoragePort::load_all`, (b) shutdown save via the guard in
    // `main.rs::RunEvent::Exit`, and (c) `CompositeFeedbackSink` fan-out
    // (feedback_sink.rs). At runtime the scheduler has de-facto
    // exclusive access — the shutdown save guard fires only after
    // `shutdown_tx → shutdown_blocking()` drains the scheduler loops —
    // so scheduler-vs-save contention is absent. This says nothing
    // about the separate connection-mutex story in main.rs (see the
    // WAL-checkpoint-before-save note in `RunEvent::Exit`).
    pub regime_classifier: Arc<parking_lot::Mutex<oneshim_analysis::RegimeClassifier>>,
    pub regime_manager: Arc<parking_lot::Mutex<oneshim_analysis::RegimeManager>>,
    pub regime_detector: oneshim_analysis::RegimeDetector,
    pub param_resolver: oneshim_analysis::ParamResolver,
    pub calibration_reader: Arc<dyn CalibrationReader>,
    /// ID of the current active regime (for transition detection).
    pub current_regime_id: Option<String>,
    /// Last time regime detection (k-means) was run.
    pub last_detection_time: Option<chrono::DateTime<chrono::Utc>>,

    // --- Auto-tuning ---
    /// Per-category EMA statistics tracker for auto-tuning trigger params.
    pub ema_tracker: oneshim_analysis::auto_tuner::EmaStatsTracker,
    /// EWMA-based drift detector for regime shift detection.
    pub drift_detector: oneshim_analysis::auto_tuner::DriftDetector,
    /// Tick counter for periodic auto-tune override generation.
    pub auto_tune_tick_count: u64,
    /// Regime analysis facade for constrained re-clustering.
    pub regime_analysis: Option<oneshim_analysis::RegimeAnalysisFacade>,
    /// Override store for loading user overrides during re-clustering.
    pub override_store: Option<Arc<dyn oneshim_core::ports::override_store::OverrideStore>>,
    /// Flag set by REST/Tauri to request on-demand re-clustering.
    pub recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    /// Configurable interval between automatic regime re-detection (hours).
    pub regime_detection_interval_hours: i64,
    /// Flag: last drift observation result. Set by analysis pipeline,
    /// read-and-cleared by coaching evaluation in the monitor loop.
    pub last_drift_detected: Arc<std::sync::atomic::AtomicBool>,

    // --- LLM/embedding pipeline ---
    pub(crate) llm_summarizer: Option<Arc<oneshim_analysis::LlmSegmentSummarizer>>,
    pub(crate) embedding_pipeline: Option<Arc<oneshim_analysis::EmbeddingPipeline>>,

    // --- GUI Activity Intelligence ---
    pub(crate) gui_pipeline_state: Option<gui_pipeline::GuiPipelineState>,
    pub(crate) gui_work_type_refiner: oneshim_analysis::GuiWorkTypeRefiner,

    /// Optional LLM-based work type refinement. When present, refines rule-based
    /// classification using AnalysisProvider. Background prefetch + LRU cache.
    pub(crate) llm_work_type_refiner: Option<Arc<oneshim_analysis::LlmWorkTypeRefiner>>,

    // --- Application classification ---
    /// Centralized app registry for subcategory classification.
    /// Replaces the hardcoded `infer_subcategory()` fallback.
    pub(crate) app_registry: Arc<AppRegistry>,

    // --- Heatmap ---
    /// Mouse interaction heatmap aggregator for overlay visualization.
    pub(crate) heatmap_aggregator: heatmap::HeatmapAggregator,
}

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
    #[cfg(feature = "server")]
    pub(super) oauth_coordinator: Option<Arc<TokenRefreshCoordinator>>,
    pub(super) context_analyzer: Option<Arc<oneshim_analysis::ContextAnalyzer>>,
    pub(super) config_manager: Option<ConfigManager>,
    pub(super) vector_store: Option<Arc<dyn oneshim_core::ports::vector_store::VectorStore>>,
    pub(super) embedding_provider:
        Option<Arc<dyn oneshim_core::ports::embedding_provider::EmbeddingProvider>>,
    pub(super) vector_index: Option<Arc<dyn VectorIndex>>,
    pub(super) search_coordinator: Option<Arc<oneshim_analysis::AdaptiveSearchCoordinator>>,
    /// Optional HNSW ANN index for approximate nearest neighbor search.
    /// Only present when the `hnsw` feature is enabled and configured.
    #[cfg(feature = "hnsw")]
    pub(super) ann_index: Option<Arc<dyn AnnIndex>>,
    /// Wrapped in Mutex so `run_scheduler_loops(&self)` can take ownership
    /// and move it into the monitor loop's async block.
    pub(super) adaptive_trigger: Mutex<Option<AdaptiveTriggerState>>,
    /// Cross-device sync engine (P3 Phase 3a-2). Optional — only present
    /// when sync is enabled AND configured (folder + passphrase).
    pub(super) sync_engine: Option<Arc<crate::sync_engine::SyncEngine>>,
    /// Accessibility API extractor for focused element context (Phase 2).
    /// `None` when `text_intelligence.accessibility_extraction` is disabled
    /// or platform does not support it.
    pub(super) accessibility_extractor: Option<Arc<dyn AccessibilityExtractor>>,
    /// ConsentManager for runtime consent checks (e.g., full_text_extraction).
    /// Wrapped in Arc for shared access across async blocks.
    pub(super) consent_manager: Option<Arc<ConsentManager>>,
    /// Coaching engine for proactive coaching messages (Phase 1).
    /// `None` when coaching is not configured. The engine checks `enabled`
    /// internally and returns `None` from `evaluate()` when disabled.
    pub(super) coaching_engine: Option<Arc<oneshim_analysis::CoachingEngine>>,
    /// MagicOverlay handle for coaching message delivery (Phase 2).
    pub(super) magic_overlay: Option<crate::magic_overlay::MagicOverlayHandle>,
    /// OverlayDriver port for rendering focus highlights on screen elements.
    /// Uses `show_highlights()` / `clear_highlights()` to draw bounding boxes
    /// around the currently focused accessibility element.
    pub(super) overlay_driver: Option<Arc<dyn OverlayDriver>>,
    /// Analysis provider for LLM personalization of coaching messages (Phase 2).
    pub(super) analysis_provider:
        Option<Arc<dyn oneshim_core::ports::analysis_provider::AnalysisProvider>>,
    /// Focused coaching event persistence port (Phase 2).
    pub(super) coaching_storage: Option<Arc<dyn CoachingStoragePort>>,
    /// Shared flag: when `true` the monitor loop skips capture/frame processing.
    pub(super) capture_paused: Arc<std::sync::atomic::AtomicBool>,
    /// Whether detection overlay is active. When `true`, the monitor loop
    /// re-triggers scene analysis on window focus changes.
    pub(super) detection_active: Arc<std::sync::atomic::AtomicBool>,
    /// Element finder for detection overlay re-analysis on window change.
    /// `None` when automation controller is not configured or has no scene_finder.
    pub(super) scene_finder: Option<Arc<dyn oneshim_core::ports::element_finder::ElementFinder>>,
    /// Adapter-side health flag for server (BatchUploader / HttpApiClient).
    pub(super) server_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Adapter-side health flag for LLM provider (RemoteLlmProvider).
    pub(super) llm_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Adapter-side health flag for CLI / automation controller.
    pub(super) cli_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// UI-facing connection status flag for server.
    pub(super) server_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// UI-facing connection status flag for LLM.
    pub(super) llm_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// UI-facing connection status flag for CLI.
    pub(super) cli_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// App handle for tray icon sync from the health check loop.
    pub(super) tray_app_handle: Option<tauri::AppHandle>,
    /// Suggestion receiver for real-time SSE suggestion reception.
    #[cfg(feature = "server")]
    pub(super) suggestion_receiver: Option<Arc<oneshim_suggestion::receiver::SuggestionReceiver>>,
    /// Suggestion manager -- provides deferred/retry queue access for maintenance loop.
    #[cfg(feature = "server")]
    pub(super) suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>>,
    /// Whether suggestion reception is enabled (from SuggestionConfig).
    pub(super) suggestions_enabled: bool,
    /// Focus mode state — shared with IPC commands and scheduler loops (A4).
    /// When active, coaching evaluation and notifications are suppressed,
    /// and capture importance threshold is elevated.
    pub(super) focus_mode: Arc<crate::focus_mode::FocusModeState>,
    /// SharedRegimeState injected from app_runtime — shares a single instance
    /// with SessionManager's context assembler. Falls back to a local instance
    /// in run_scheduler_loops if not provided.
    pub(super) shared_regime: Option<Arc<shared_regime_state::SharedRegimeState>>,
}

// --- Builder methods ---

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
        frame_storage: Option<Arc<dyn FrameStoragePort>>,
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
            frame_storage,
            batch_sink,
            api_client,
            event_tx: None,
            notification_manager: None,
            focus_analyzer: None,
            #[cfg(feature = "server")]
            oauth_coordinator: None,
            context_analyzer: None,
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
            coaching_engine: None,
            magic_overlay: None,
            overlay_driver: None,
            analysis_provider: None,
            coaching_storage: None,
            capture_paused: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            detection_active: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            scene_finder: None,
            server_health_flag: None,
            llm_health_flag: None,
            cli_health_flag: None,
            server_connected: None,
            llm_connected: None,
            cli_connected: None,
            tray_app_handle: None,
            #[cfg(feature = "server")]
            suggestion_receiver: None,
            #[cfg(feature = "server")]
            suggestion_manager: None,
            suggestions_enabled: false,
            focus_mode: Arc::new(crate::focus_mode::FocusModeState::new()),
            shared_regime: None,
        }
    }

    pub fn with_config_manager(mut self, config_manager: ConfigManager) -> Self {
        self.config_manager = Some(config_manager);
        self
    }

    pub fn with_event_tx(mut self, event_tx: broadcast::Sender<RealtimeEvent>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    pub fn with_notification_manager(mut self, manager: Arc<NotificationManager>) -> Self {
        self.notification_manager = Some(manager);
        self
    }

    pub fn with_focus_analyzer(mut self, analyzer: Arc<FocusAnalyzer>) -> Self {
        self.focus_analyzer = Some(analyzer);
        self
    }

    #[cfg(feature = "server")]
    pub fn with_oauth_coordinator(mut self, coordinator: Arc<TokenRefreshCoordinator>) -> Self {
        self.oauth_coordinator = Some(coordinator);
        self
    }

    pub fn with_context_analyzer(
        mut self,
        analyzer: Arc<oneshim_analysis::ContextAnalyzer>,
    ) -> Self {
        self.context_analyzer = Some(analyzer);
        self
    }

    pub fn with_vector_store(
        mut self,
        store: Arc<dyn oneshim_core::ports::vector_store::VectorStore>,
    ) -> Self {
        self.vector_store = Some(store);
        self
    }

    pub fn with_embedding_provider(
        mut self,
        provider: Arc<dyn oneshim_core::ports::embedding_provider::EmbeddingProvider>,
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
        coordinator: Arc<oneshim_analysis::AdaptiveSearchCoordinator>,
    ) -> Self {
        self.search_coordinator = Some(coordinator);
        self
    }

    /// Attach an HNSW ANN index for approximate nearest neighbor search.
    #[cfg(feature = "hnsw")]
    #[allow(dead_code)] // Builder method; wired when hnsw feature is enabled
    pub fn with_ann_index(mut self, ann: Arc<dyn AnnIndex>) -> Self {
        self.ann_index = Some(ann);
        self
    }

    pub fn with_adaptive_trigger(self, state: AdaptiveTriggerState) -> Self {
        *self.adaptive_trigger.lock().unwrap_or_else(|poisoned| {
            warn!("adaptive trigger lock poisoned — recovering inner data");
            poisoned.into_inner()
        }) = Some(state);
        self
    }

    pub fn with_sync_engine(mut self, engine: Arc<crate::sync_engine::SyncEngine>) -> Self {
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

    pub fn with_consent_manager(mut self, consent_manager: Arc<ConsentManager>) -> Self {
        self.consent_manager = Some(consent_manager);
        self
    }

    pub fn with_coaching_engine(mut self, engine: Arc<oneshim_analysis::CoachingEngine>) -> Self {
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
        provider: Arc<dyn oneshim_core::ports::analysis_provider::AnalysisProvider>,
    ) -> Self {
        self.analysis_provider = Some(provider);
        self
    }

    #[allow(dead_code)] // Builder method; wired when coaching engine storage is configured
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

    #[allow(dead_code)] // wired when automation controller provides a scene_finder
    pub fn with_scene_finder(
        mut self,
        finder: Arc<dyn oneshim_core::ports::element_finder::ElementFinder>,
    ) -> Self {
        self.scene_finder = Some(finder);
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
        receiver: Arc<oneshim_suggestion::receiver::SuggestionReceiver>,
    ) -> Self {
        self.suggestion_receiver = Some(receiver);
        self
    }

    pub fn with_suggestions_enabled(mut self, enabled: bool) -> Self {
        self.suggestions_enabled = enabled;
        self
    }

    #[cfg(feature = "server")]
    pub fn with_suggestion_manager(
        mut self,
        manager: Arc<crate::suggestion_manager::SuggestionManager>,
    ) -> Self {
        self.suggestion_manager = Some(manager);
        self
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
            health_check_secs = 60,
            coaching_secs = config::COACHING_INTERVAL_SECS,
            sqlite_maintenance_mins = config::SQLITE_MAINTENANCE_INTERVAL_MINS,
            "scheduler loops starting"
        );
        self.run_scheduler_loops(shutdown_rx, app_handle).await;
    }
}

/// Time-injectable core of the active-hours gate.
///
/// Accepts an explicit `now: DateTime<Local>` so callers in tests can drive
/// deterministic scenarios (including the overnight wrap covered by CONS-C05).
/// Production call-sites should use [`should_run_now`] which calls
/// `chrono::Local::now()` internally.
///
/// # Overnight wrap (CONS-C05)
///
/// When `active_end_hour < active_start_hour` the window wraps midnight, e.g.
/// `22:00 – 06:00`.  For the hour-in-range check the rule is:
/// - Non-wrapping (`end > start`): `hour ∈ [start, end)` on `now.weekday()`.
/// - Wrapping (`end < start`): `hour ≥ start` OR `hour < end`.
///   - If `hour ≥ start`: check `now.weekday()` is in `active_days`.
///   - If `hour < end`:  check the *previous* weekday is in `active_days`
///     (because the window was opened last night).
/// - Equal (`end == start`): treated as empty window → returns `false`.
pub(crate) fn should_run_now_with_time(
    config: &AppConfig,
    now: chrono::DateTime<chrono::Local>,
) -> bool {
    let schedule = &config.schedule;
    if !schedule.active_hours_enabled {
        return true;
    }

    let hour = now.hour() as u8;
    let weekday = match now.weekday() {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    };

    let start = schedule.active_start_hour;
    let end = schedule.active_end_hour;

    if end > start {
        // Non-wrapping window: e.g. 09:00–17:00.
        // Active when hour ∈ [start, end) on a configured weekday.
        if !schedule.active_days.contains(&weekday) {
            return false;
        }
        hour >= start && hour < end
    } else if end < start {
        // Overnight (wrapping) window: e.g. 22:00–06:00.
        // The window opens at `start` on the "start-day" and closes at
        // `end` on the next calendar day.
        if hour >= start {
            // We are in the evening portion — check today's weekday.
            schedule.active_days.contains(&weekday)
        } else if hour < end {
            // We are in the early-morning carry-over portion — check yesterday.
            let yesterday = weekday_pred(weekday);
            schedule.active_days.contains(&yesterday)
        } else {
            // hour == end exactly — window is half-open [start, end), so end is excluded.
            false
        }
    } else {
        // start == end: empty / degenerate window → inactive.
        false
    }
}

/// Returns `true` when the current wall-clock time falls within the configured
/// active-hours window (or active_hours is disabled).
// A.7 removed the last non-test call-site (monitor.rs now uses capture_permitted_now).
// Retained for tests and potential future callers (e.g. A.9 loop gating helpers).
#[allow(dead_code)]
pub fn should_run_now(config: &AppConfig) -> bool {
    should_run_now_with_time(config, chrono::Local::now())
}

/// Returns `true` when the current instant falls inside any configured
/// tracking-schedule mute window.
///
/// Delegates to the time-injectable helper; uses `chrono::Local::now()`.
// A.7/A.9 call-sites will consume this; allow until wired.
#[allow(dead_code)]
pub fn tracking_schedule_active(config: &AppConfig) -> bool {
    loops::tracking_schedule_helper::tracking_schedule_active(config, chrono::Local::now())
}

/// Full 4-term privacy gate composite — use this at all gate sites rather than
/// piecemeal checks.
///
/// ```text
/// capture_permitted_now =
///     consent.screen_capture              // consent top-authority (CONS-PC02)
///     AND should_run_now(cfg)             // active_hours gate
///     AND !tracking_schedule_active(cfg)  // tracking-schedule mute gate
///     AND !capture_paused                 // user tray-toggle veto
/// ```
///
/// Callers must supply a [`ConsentPermissions`] snapshot and the current
/// `capture_paused` atomic read (A.7 / A.9 / A.12 / A.14 will thread these
/// through scheduler loops and IPC commands).
pub fn capture_permitted_now(
    config: &AppConfig,
    consent: &oneshim_core::consent::ConsentPermissions,
    capture_paused: bool,
) -> bool {
    loops::tracking_schedule_helper::capture_permitted_now(
        config,
        consent,
        capture_paused,
        chrono::Local::now(),
    )
}

/// Returns the predecessor (previous) weekday.
///
/// Used by [`should_run_now_with_time`] for overnight window carry-over checks.
fn weekday_pred(day: Weekday) -> Weekday {
    match day {
        Weekday::Mon => Weekday::Sun,
        Weekday::Tue => Weekday::Mon,
        Weekday::Wed => Weekday::Tue,
        Weekday::Thu => Weekday::Wed,
        Weekday::Fri => Weekday::Thu,
        Weekday::Sat => Weekday::Fri,
        Weekday::Sun => Weekday::Sat,
    }
}

#[cfg(test)]
mod tests {
    use self::config::{PlatformEgressPolicy, REDACTED_WINDOW_TITLE};
    use super::*;
    use oneshim_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
    use oneshim_core::models::event::{ContextEvent, Event};
    use std::time::Duration;

    #[test]
    fn should_run_when_disabled() {
        let config = AppConfig::default_config();
        assert!(should_run_now(&config));
    }

    #[test]
    fn scheduler_config_default() {
        let config = SchedulerConfig::default();
        assert_eq!(config.poll_interval, Duration::from_secs(1));
        assert_eq!(config.metrics_interval, Duration::from_secs(5));
        assert_eq!(config.idle_threshold_secs, 300);
    }

    #[test]
    fn platform_sync_is_disabled_in_current_ai_runtime() {
        let config = SchedulerConfig {
            ..SchedulerConfig::default()
        };

        let policy = PlatformEgressPolicy::new(&config);
        assert!(!policy.is_enabled());

        let policy = PlatformEgressPolicy::new(&config);
        assert!(!policy.is_enabled());
    }

    #[test]
    fn strict_policy_redacts_window_title() {
        let config = SchedulerConfig {
            upload_enabled: true,
            external_data_policy: ExternalDataPolicy::PiiFilterStrict,
            ..SchedulerConfig::default()
        };
        let policy = PlatformEgressPolicy::new(&config);
        let event = Event::Context(ContextEvent {
            app_name: "Chrome".to_string(),
            window_title: "Inbox user@example.com".to_string(),
            prev_app_name: None,
            timestamp: chrono::Utc::now(),
            ..Default::default()
        });

        let uploaded = policy.prepare_event_for_upload(event);
        let Some(Event::Context(ctx)) = uploaded else {
            panic!("context event should be uploadable");
        };
        assert_eq!(ctx.window_title, REDACTED_WINDOW_TITLE);
    }

    #[test]
    fn allow_filtered_policy_uses_pii_filter() {
        let privacy = PrivacyConfig {
            pii_filter_level: PiiFilterLevel::Basic,
            ..PrivacyConfig::default()
        };
        let config = SchedulerConfig {
            upload_enabled: true,
            external_data_policy: ExternalDataPolicy::AllowFiltered,
            privacy_config: privacy,
            ..SchedulerConfig::default()
        };
        let policy = PlatformEgressPolicy::new(&config);
        let event = Event::Context(ContextEvent {
            app_name: "Chrome".to_string(),
            window_title: "Inbox user@example.com".to_string(),
            prev_app_name: None,
            timestamp: chrono::Utc::now(),
            ..Default::default()
        });

        let uploaded = policy.prepare_event_for_upload(event);
        let Some(Event::Context(ctx)) = uploaded else {
            panic!("context event should be uploadable");
        };
        assert!(ctx.window_title.contains("[EMAIL]"));
        assert!(!ctx.window_title.contains('@'));
    }

    #[test]
    fn sensitive_apps_are_skipped_from_upload() {
        let config = SchedulerConfig {
            ..SchedulerConfig::default()
        };
        let policy = PlatformEgressPolicy::new(&config);
        let event = Event::Context(ContextEvent {
            app_name: "Bitwarden".to_string(),
            window_title: "Vault".to_string(),
            prev_app_name: None,
            timestamp: chrono::Utc::now(),
            ..Default::default()
        });

        assert!(policy.prepare_event_for_upload(event).is_none());
    }

    // ── Overnight active_hours wrap tests (CONS-C05) ─────────────────────────

    /// Build a `DateTime<Local>` for a known weekday at HH:MM.
    /// Wall-clock hour and weekday match the literal values on any machine,
    /// because `Local` interprets the naive datetime as local time.
    ///
    /// - 2024-01-10 = Wednesday
    /// - 2024-01-11 = Thursday
    /// - 2024-01-13 = Saturday
    fn fixed_at_local(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
    ) -> chrono::DateTime<chrono::Local> {
        use chrono::{NaiveDate, TimeZone as _};
        let naive = NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap();
        // Interpret the naive datetime as local wall-clock time so that
        // now.hour() == hour and now.weekday() == the date's weekday on any machine.
        chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
    }

    /// Build an `AppConfig` with an overnight active_hours window 22:00–06:00
    /// on Mon–Fri.
    fn overnight_cfg() -> AppConfig {
        let mut cfg = AppConfig::default_config();
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 22;
        cfg.schedule.active_end_hour = 6; // end < start → overnight wrap
        cfg.schedule.active_days = vec![
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
        ];
        cfg
    }

    /// Overnight wrap: active_hours 22:00–06:00 on Mon–Fri.
    ///
    /// Tests four instants (all via `should_run_now_with_time`):
    /// - Wed 23:00: inside (Wed in active_days, hour >= 22) → true
    /// - Thu 01:00: carry-over from Wed night (hour < 6, Wed in active_days) → true
    /// - Thu 05:59: still carry-over (end 06:00 is exclusive) → true
    /// - Thu 06:01: outside (past end 06, not carry-over) → false
    ///
    /// Note: Sat 00:01 IS inside the window (Fri opened at 22:00, carries to Sat 06:00).
    #[test]
    fn should_run_now_handles_overnight_range() {
        let cfg = overnight_cfg();

        // Wed 23:00 — evening portion on Wed (in active_days).
        let wed_23 = fixed_at_local(2024, 1, 10, 23, 0);
        assert!(
            should_run_now_with_time(&cfg, wed_23),
            "Wed 23:00 must be inside overnight window 22-06 (CONS-C05)"
        );

        // Thu 01:00 — carry-over from Wed night.
        let thu_01 = fixed_at_local(2024, 1, 11, 1, 0);
        assert!(
            should_run_now_with_time(&cfg, thu_01),
            "Thu 01:00 must be inside carry-over from Wed night (CONS-C05)"
        );

        // Thu 05:59 — still carry-over (end is exclusive at 06:00).
        let thu_0559 = fixed_at_local(2024, 1, 11, 5, 59);
        assert!(
            should_run_now_with_time(&cfg, thu_0559),
            "Thu 05:59 must be inside carry-over (end 06:00 is exclusive) (CONS-C05)"
        );

        // Thu 06:01 — outside window; Thu is a start-day but hour 6 == end → excluded.
        // 06:01 is also past end, so outside carry-over too.
        let thu_0601 = fixed_at_local(2024, 1, 11, 6, 1);
        assert!(
            !should_run_now_with_time(&cfg, thu_0601),
            "Thu 06:01 must be outside the window (past end hour 06) (CONS-C05)"
        );
    }

    /// Overnight wrap midnight: explicit Wed 23:00 → Thu 01:00 → Thu 05:59 →
    /// Thu 06:01 → Sat 00:01 sequence mirrors CONS-C05 pseudocode.
    #[test]
    fn should_run_now_wraps_midnight_thu_01() {
        let cfg = overnight_cfg();

        // Sat 00:01 — Sat is NOT in active_days as start, but Fri IS and
        // 00:01 < end_hour (6), so this is carry-over from Fri night → true.
        let sat_0001 = fixed_at_local(2024, 1, 13, 0, 1);
        assert!(
            should_run_now_with_time(&cfg, sat_0001),
            "Sat 00:01 must be inside carry-over from Fri night \
             (Fri is in active_days, hour 0 < end 6) (CONS-C05)"
        );

        // Sat 06:01 — past the carry-over end, and Sat is not in active_days.
        let sat_0601 = fixed_at_local(2024, 1, 13, 6, 1);
        assert!(
            !should_run_now_with_time(&cfg, sat_0601),
            "Sat 06:01 must be outside (past end 06, Sat not in active_days) (CONS-C05)"
        );

        // Wed 21:59 — before window opens on Wed.
        let wed_2159 = fixed_at_local(2024, 1, 10, 21, 59);
        assert!(
            !should_run_now_with_time(&cfg, wed_2159),
            "Wed 21:59 must be outside (before start hour 22 on Wed) (CONS-C05)"
        );
    }

    // ── Hoist migration tests: capture_permitted_now composite gate (A.6) ───
    //
    // These tests migrate the schedule-logic coverage from
    // `oneshim-vision::trigger` (marked #[ignore] in A.6, deleted in A.7) into
    // the scheduler where the schedule gate now lives.  They use the 4-arg
    // `capture_permitted_now(cfg, consent, capture_paused, now)` helper
    // directly so that `now` can be injected deterministically.

    /// Build a `ConsentPermissions` with `screen_capture` set to the given bool
    /// and all other fields defaulted to `false`.
    fn capture_consent(granted: bool) -> oneshim_core::consent::ConsentPermissions {
        oneshim_core::consent::ConsentPermissions {
            screen_capture: granted,
            ..Default::default()
        }
    }

    /// `capture_permitted_now` must return `false` when `now` is outside the
    /// configured active_hours window.
    ///
    /// Config: active_hours_enabled=true, 09:00–17:00, Mon only.
    /// now = Mon 20:00 → outside window → gate rejects capture.
    #[test]
    fn scheduler_blocks_capture_outside_active_hours() {
        let mut cfg = AppConfig::default_config();
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 9;
        cfg.schedule.active_end_hour = 17;
        cfg.schedule.active_days = vec![oneshim_core::config::Weekday::Mon];

        let consent = capture_consent(true);
        // 2024-01-08 is a Monday; 20:00 is outside the 09:00–17:00 window.
        let now = fixed_at_local(2024, 1, 8, 20, 0);

        assert!(
            !loops::tracking_schedule_helper::capture_permitted_now(&cfg, &consent, false, now),
            "Mon 20:00 must be blocked when active_hours is 09-17 (Mon only)"
        );
    }

    /// `capture_permitted_now` must return `true` when `active_hours_enabled`
    /// is `false`, regardless of the current time or weekday.
    ///
    /// Config: active_hours_enabled=false (default AppConfig); tracking_schedule
    /// disabled; consent granted; capture_paused=false.
    /// SmartCaptureTrigger is now schedule-free (A.7 hoist intent), so the
    /// gate always passes when scheduling is disabled.
    #[test]
    fn scheduler_allows_capture_when_schedule_disabled() {
        // Default AppConfig has active_hours_enabled=false and tracking_schedule
        // disabled, so any instant must be permitted.
        let cfg = AppConfig::default_config();
        let consent = capture_consent(true);

        // Sunday midnight — would be blocked by any typical Mon-Fri 09-17 window,
        // but schedule is disabled so it must be allowed.
        let now = fixed_at_local(2024, 1, 7, 0, 0); // 2024-01-07 = Sunday

        assert!(
            loops::tracking_schedule_helper::capture_permitted_now(&cfg, &consent, false, now),
            "capture must be permitted when active_hours_enabled=false (any time, any day)"
        );
    }

    /// `capture_permitted_now` handles overnight active_hours windows (end < start)
    /// correctly via pred-weekday carry-over (interpretation B, approved deviation
    /// from plan §3.3 A.6 original text which said interpretation A).
    ///
    /// Config: active_hours_enabled=true, 22:00–06:00, Mon–Fri.
    ///
    /// Sequence verified:
    /// - Wed 23:00 → true  (Wed in active_days, hour >= 22)
    /// - Thu 01:00 → true  (carry-over from Wed night, hour < 6)
    /// - Thu 05:59 → true  (still carry-over; end 06:00 is exclusive)
    /// - Thu 06:01 → false (past end; no carry-over applies)
    /// - Sat 00:01 → TRUE (Fri opened at 22:00, carries into Sat morning;
    ///   pred-weekday of Sat is Fri which IS in active_days and 00:01 < end_hour 06 —
    ///   interpretation B approved: Fri night is a single "Fri shift" spanning Saturday)
    #[test]
    fn scheduler_handles_overnight_active_hours() {
        let mut cfg = AppConfig::default_config();
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 22;
        cfg.schedule.active_end_hour = 6; // end < start → overnight wrap
        cfg.schedule.active_days = vec![
            oneshim_core::config::Weekday::Mon,
            oneshim_core::config::Weekday::Tue,
            oneshim_core::config::Weekday::Wed,
            oneshim_core::config::Weekday::Thu,
            oneshim_core::config::Weekday::Fri,
        ];

        let consent = capture_consent(true);
        let permit = |now| {
            loops::tracking_schedule_helper::capture_permitted_now(&cfg, &consent, false, now)
        };

        // 2024-01-10 = Wednesday, 2024-01-11 = Thursday, 2024-01-13 = Saturday.
        let wed_23 = fixed_at_local(2024, 1, 10, 23, 0);
        assert!(permit(wed_23), "Wed 23:00 must be inside window (CONS-C05)");

        let thu_01 = fixed_at_local(2024, 1, 11, 1, 0);
        assert!(
            permit(thu_01),
            "Thu 01:00 must be inside (carry-over from Wed night) (CONS-C05)"
        );

        let thu_0559 = fixed_at_local(2024, 1, 11, 5, 59);
        assert!(
            permit(thu_0559),
            "Thu 05:59 must be inside (end 06:00 is exclusive) (CONS-C05)"
        );

        let thu_0601 = fixed_at_local(2024, 1, 11, 6, 1);
        assert!(
            !permit(thu_0601),
            "Thu 06:01 must be outside (past end hour 06) (CONS-C05)"
        );

        // Sat 00:01 — interpretation B (pred-weekday carry-over):
        // Fri opened at 22:00 and the overnight window carries into Sat morning.
        // pred(Sat) == Fri, Fri IS in active_days, and 00:01 < end_hour 6
        // → the gate must pass (TRUE), not reject as plan text originally said.
        // Approved deviation: matches real-world shift scheduling semantics.
        let sat_0001 = fixed_at_local(2024, 1, 13, 0, 1);
        assert!(
            permit(sat_0001),
            "Sat 00:01 must be inside (Fri carry-over, interpretation B — \
             pred-weekday check against active_days) (CONS-C05)"
        );
    }
}
