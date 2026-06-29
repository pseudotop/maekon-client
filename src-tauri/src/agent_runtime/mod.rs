pub(crate) mod analysis_helpers;
mod analysis_setup;
mod embedding_setup;
mod sync_setup;

use anyhow::Result;
use maekon_core::config::AppConfig;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use maekon_core::ports::coaching_storage::CoachingStoragePort;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::storage::StorageService;
#[cfg(feature = "analysis")]
use maekon_network::oauth::refresh_coordinator::TokenRefreshCoordinator;
use maekon_web::RealtimeEvent;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::AppHandle;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, watch};
use tracing::{error, info};

use crate::agent_runtime_support::AgentSupportContextBuilder;
use crate::capture_services::SharedCaptureServices;
use crate::focus_analyzer::FocusStorage;
use crate::scheduler::shared_regime_state::SharedRegimeState;
use crate::scheduler::{Scheduler, SchedulerStorage};

#[derive(Clone)]
pub(crate) struct AgentRuntimeBundle {
    storage: Arc<dyn StorageService>,
    scheduler_storage: Arc<dyn SchedulerStorage>,
    focus_storage: Arc<dyn FocusStorage>,
    calibration_writer: Option<Arc<dyn CalibrationWriter>>,
    calibration_reader: Option<Arc<dyn CalibrationReader>>,
    override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    /// Shared flag for on-demand re-clustering requests from Tauri/REST.
    recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    /// Shared flag set by the web "Delete all data" endpoint so a local GDPR
    /// erasure propagates a device-wide DeletionEvent to LAN sync peers (#4478 G3).
    erasure_requested: Arc<std::sync::atomic::AtomicBool>,
    /// Pre-built VectorStore for the embedding pipeline (None if embedding disabled).
    vector_store: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>>,
    data_dir: PathBuf,
    config: AppConfig,
    config_manager: ConfigManager,
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// Concrete SQLite storage for sync engine wiring.
    sqlite_storage_concrete: Arc<maekon_storage::sqlite::SqliteStorage>,
    /// #6264: shared write-once slot the runtime populates with the built
    /// `SyncEngine` so the 4 cross-device-sync IPC commands gain a live engine.
    /// `None` when no SyncRuntimeState was wired (e.g. tests).
    sync_runtime_slot: Option<Arc<std::sync::OnceLock<Arc<crate::sync_engine::SyncEngine>>>>,
    /// #6266: shared write-once slot the runtime populates with the local
    /// embedding provider's `ReloadableModel` facet so `reload_embedding_model`
    /// IPC gains a live target. `None` when no EmbeddingRuntimeState was wired.
    embedding_runtime_slot: Option<
        Arc<std::sync::OnceLock<Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>>>,
    >,
    offline_mode: bool,
    event_tx: Option<broadcast::Sender<RealtimeEvent>>,
    #[cfg(feature = "analysis")]
    oauth_coordinator: Option<Arc<TokenRefreshCoordinator>>,
    /// Provider secret stores for BYOK credential resolution at request time.
    ///
    /// Threaded from `ProviderRuntimeContext` so analysis and embedding
    /// adapters can resolve OS-keychain-backed keys via
    /// `CredentialSource::StoredSecret`.  Gated on `analysis` (same as
    /// `oauth_coordinator`) because the secret-store infrastructure lives in
    /// `maekon-network` (analysis feature gate).
    #[cfg(feature = "analysis")]
    provider_secret_stores: Option<maekon_core::ports::secret_store::SecretStoreSet>,
    app_handle: AppHandle,
    coaching_engine: Option<Arc<maekon_analysis::CoachingEngine>>,
    coaching_storage: Option<Arc<dyn CoachingStoragePort>>,
    magic_overlay: Option<crate::magic_overlay::MagicOverlayHandle>,
    overlay_driver: Option<Arc<dyn maekon_core::ports::overlay_driver::OverlayDriver>>,
    capture_paused: Option<Arc<std::sync::atomic::AtomicBool>>,
    detection_active: Option<Arc<std::sync::atomic::AtomicBool>>,
    server_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    llm_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    cli_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    server_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    llm_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    cli_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    tray_app_handle: Option<tauri::AppHandle>,
    #[cfg(feature = "server")]
    suggestion_receiver: Option<Arc<maekon_suggestion::receiver::SuggestionReceiver>>,
    /// Suggestion manager — provides deferred/retry queue access for maintenance loop.
    /// E20-24 (#4816): gated on `local-suggestions` so the OSS local pipeline gets it.
    #[cfg(feature = "local-suggestions")]
    suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>>,
    suggestions_enabled: bool,
    focus_mode: Option<Arc<crate::focus_mode::FocusModeState>>,
    shared_capture_services: Option<Arc<SharedCaptureServices>>,
    /// Shared suggestion queue — SAME Arc as SuggestionManager's queue,
    /// so SSE-received suggestions are visible in IPC queries.
    shared_suggestion_queue:
        Option<Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>>,
    shared_scorer: Option<Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>>,
    /// SharedRegimeState passed through to the Scheduler so it shares the same
    /// instance as the SessionManager's context assembler.
    shared_regime: Option<Arc<SharedRegimeState>>,
    /// Pre-created health flag for the primary analysis provider, shared with AppState
    /// so the `get_analysis_health` IPC command reflects actual provider health.
    analysis_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Pre-constructed `RegimeManager` handle shared with AppState. When
    /// present, `build_analysis_pipeline` uses this instead of constructing
    /// a fresh one — lets the composition root both (a) hydrate from
    /// persisted state and (b) hand the same Arc to AppState for the
    /// shutdown save guard.
    regime_manager: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>>,
    /// Pre-constructed `RegimeClassifier` handle shared with the composition
    /// root so `CompositeFeedbackSink` can fan accept/reject signals into it.
    regime_classifier: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>>,
    /// D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    /// registry threaded from the composition root. All network adapters built
    /// inside `run()` (remote embedding, analysis primary/fallback, LLM-summary,
    /// WorkType refiner, enrichment) clone this one Arc, so adapters targeting
    /// the same endpoint converge on a single breaker (iter-011 consolidation).
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
}

impl AgentRuntimeBundle {
    pub(crate) fn spawn_on(&self, handle: &Handle, shutdown_rx: watch::Receiver<bool>) {
        let bundle = self.clone();
        handle.spawn(async move {
            if let Err(error) = bundle.run(shutdown_rx).await {
                error!(error = %error, "Agent error");
            }
        });
    }

    async fn run(self, shutdown_rx: watch::Receiver<bool>) -> Result<()> {
        info!("Agent initializing");
        let mut builder = AgentSupportContextBuilder::new(
            &self.data_dir,
            &self.config,
            self.focus_storage.clone(),
        )
        .with_storage(self.storage.clone())
        .with_app_handle(self.app_handle.clone());
        if let Some(ref capture_services) = self.shared_capture_services {
            builder = builder.with_shared_capture_services(capture_services.clone());
        }
        if let Some(ref shared_queue) = self.shared_suggestion_queue {
            builder = builder.with_shared_suggestion_queue(shared_queue.clone());
        }
        if let Some(ref shared_scorer) = self.shared_scorer {
            builder = builder.with_shared_scorer(shared_scorer.clone());
        }
        builder = builder.with_few_shot_storage(Arc::clone(&self.sqlite_storage_concrete)
            as Arc<dyn maekon_core::ports::few_shot_storage::FewShotStorage>);
        if let Some(ref flag) = self.analysis_health_flag {
            builder = builder.with_analysis_health_flag(flag.clone());
        }
        // D7 (#4812 / E20-20): hand the SAME shared breaker registry to the
        // support context so its `ContextAnalyzer` analysis provider converges on
        // the same breaker as every other adapter built in this runtime.
        builder = builder.with_breaker_registry(self.breaker_registry.clone());
        // Clone before the move into Scheduler::with_config_manager below.
        builder = builder.with_config_manager(self.config_manager.clone());
        if let Some(ref cm) = self.consent_manager {
            builder = builder.with_consent_manager(cm.clone());
        }
        // Wire provider secret stores so the ContextAnalyzer analysis provider
        // resolves OS-keychain-backed BYOK keys at request time.
        #[cfg(feature = "analysis")]
        if let Some(ref stores) = self.provider_secret_stores {
            builder = builder.with_provider_secret_stores(stores.clone());
        }
        let support = builder.build().await?;
        let accessibility_extractor = support.accessibility_extractor.clone();
        let external_llm_privacy_guard = crate::provider_adapters::ExternalOcrPrivacyGuard::new(
            self.consent_manager.clone().unwrap_or_else(|| {
                Arc::new(maekon_core::consent::ConsentManager::new(
                    self.data_dir.join("consent.json"),
                ))
            }),
            self.config.privacy.pii_filter_level,
            self.config.ai_provider.external_data_policy,
            self.config.privacy.clone(),
            support.process_monitor.clone(),
            None,
        );

        let mut scheduler = Scheduler::new(
            support.scheduler_config,
            support.system_monitor,
            support.activity_monitor,
            support.process_monitor,
            support.capture_trigger,
            support.frame_processor,
            self.storage,
            self.scheduler_storage,
            Some(support.frame_storage),
            support.batch_sink_opt,
            support.api_client_opt,
        )
        .with_config_manager(self.config_manager)
        .with_notification_manager(support.notification_manager)
        .with_focus_analyzer(support.focus_analyzer);

        // ADR-023: share the single SqliteStorage Arc as the MemoryGraphPort so the
        // aggregation loop can promote daily-digest content into durable claims +
        // evidence edges (Port Instance Sharing guardrail — no second handle).
        scheduler = scheduler.with_memory_graph(Arc::clone(&self.sqlite_storage_concrete)
            as Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>);

        // ADR-023 Phase-2: belief revision. The enrichment provider is LOCAL-LLM-
        // gated by construction (non-loopback endpoints → NoOp); a per-claim PII
        // masker is applied at the enrichment boundary (MG-PII-03). The aggregation
        // loop additionally gates each run on memory_graph_enrichment consent + the
        // belief_revision_enabled flag.
        {
            let enrichment_provider =
                crate::agent_runtime::analysis_helpers::build_enrichment_provider(
                    &self.config.ai_provider,
                    self.config.privacy.pii_filter_level,
                    self.breaker_registry.clone(),
                );
            let mask_level = self.config.privacy.pii_filter_level;
            let belief_pii_filter: maekon_analysis::BeliefPiiFilter =
                Arc::new(move |text: &str| {
                    maekon_vision::privacy::sanitize_title_with_level(text, mask_level)
                });
            let belief_revision = maekon_analysis::BeliefRevision::new(
                enrichment_provider,
                Arc::clone(&self.sqlite_storage_concrete)
                    as Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>,
                belief_pii_filter,
                self.config.analysis.supersede_confidence_threshold,
                self.config.analysis.belief_revision_enabled,
            );
            scheduler = scheduler.with_belief_revision(Arc::new(belief_revision));
        }

        if let Some(ref analyzer) = support.context_analyzer {
            scheduler = scheduler.with_context_analyzer(analyzer.clone());
        }

        #[cfg(feature = "analysis")]
        if let Some(coordinator) = self.oauth_coordinator {
            scheduler = scheduler.with_oauth_coordinator(coordinator);
        }

        if let Some(event_tx) = self.event_tx {
            scheduler = scheduler.with_event_tx(event_tx);
        }

        // --- Layer 2: Build embedding + LLM summary pipeline ---
        let mut embedding = embedding_setup::build_embedding_components(
            &self.config,
            self.vector_store.clone(),
            Some(external_llm_privacy_guard.clone()),
            #[cfg(feature = "analysis")]
            self.provider_secret_stores.as_ref(),
            // #6830: the local SQLite store is the egress ledger sink — audits each
            // external embedding upload (loopback uploads are gated out downstream).
            #[cfg(feature = "analysis")]
            Some(Arc::clone(&self.sqlite_storage_concrete)
                as Arc<
                    dyn maekon_core::ports::egress_ledger::EgressLedgerSink,
                >),
            self.breaker_registry.clone(),
        );

        // #6266: publish the reloadable embedding model to the shared IPC slot so
        // `reload_embedding_model` operates on the SAME model the pipeline uses,
        // instead of returning service.unavailable. set() is a no-op if already
        // populated; only the local provider yields a reloadable facet.
        if let (Some(slot), Some(reloadable)) = (
            &self.embedding_runtime_slot,
            embedding.reloadable_model.clone(),
        ) {
            let _ = slot.set(reloadable);
        }

        // Wire embedding provider + vector store into scheduler if available.
        if let (Some(ref vs), Some(ref ep)) =
            (&embedding.vector_store, &embedding.embedding_provider)
        {
            scheduler = scheduler
                .with_vector_store(vs.clone())
                .with_embedding_provider(ep.clone());
        }

        // --- Layer 2b: Vector index + adaptive search coordinator ---
        let vector_index: Option<Arc<dyn maekon_core::ports::vector_index::VectorIndex>> =
            if embedding.vector_store.is_some() {
                let conn = self.sqlite_storage_concrete.connection_arc();
                Some(Arc::new(
                    maekon_storage::sqlite::vector_index_impl::SqliteVectorIndex::new(conn),
                ))
            } else {
                None
            };

        let embedding_config = &self.config.analysis.embedding;
        let search_coordinator: Option<Arc<maekon_analysis::AdaptiveSearchCoordinator>> = if let (
            Some(ref vs),
            Some(ref vi),
        ) =
            (&embedding.vector_store, &vector_index)
        {
            let sc = maekon_analysis::SearchConfig {
                brute_force_threshold: 10_000,
                ivf_threshold: 100_000,
                hnsw_threshold: 5_000,
                oversample_factor: embedding_config.binary_oversample_factor,
                default_nprobe: embedding_config.ivf_nprobe,
                forced_strategy: match embedding_config.index_strategy.as_str() {
                    "auto" => None,
                    s @ ("brute_force" | "ivf" | "ivf_binary") => Some(s.to_string()),
                    // "hnsw" is meaningful only when compiled with the feature
                    // AND an AnnIndex is wired (not the case in production).
                    #[cfg(feature = "hnsw")]
                    "hnsw" => Some("hnsw".to_string()),
                    other => {
                        // review4 F9: an unrecognized index_strategy previously
                        // degraded silently to a full brute-force scan. Warn once
                        // at startup and fall back to auto strategy selection.
                        tracing::warn!(
                            index_strategy = %other,
                            "unrecognized embedding.index_strategy; using auto strategy selection"
                        );
                        None
                    }
                },
            };
            Some(Arc::new(maekon_analysis::AdaptiveSearchCoordinator::new(
                vs.clone(),
                vi.clone(),
                sc,
            )))
        } else {
            None
        };

        if let Some(ref vi) = vector_index {
            scheduler = scheduler.with_vector_index(vi.clone());
        }
        if let Some(ref coord) = search_coordinator {
            scheduler = scheduler.with_search_coordinator(coord.clone());
        }

        // Inject VectorRetriever into ContextAnalyzer (post-hoc, since analyzer
        // is built before embedding components are available).
        if let Some(ref analyzer) = support.context_analyzer {
            if let (Some(ref ep), Some(ref vs)) =
                (&embedding.embedding_provider, &embedding.vector_store)
            {
                let pii_level = self.config.privacy.pii_filter_level;
                let pii_filter: maekon_analysis::PiiFilter = Box::new(move |text: &str| {
                    maekon_vision::privacy::sanitize_title_with_level(text, pii_level)
                });
                let retriever = if let Some(ref coord) = search_coordinator {
                    maekon_analysis::VectorRetriever::with_coordinator(
                        ep.clone(),
                        vs.clone(),
                        pii_filter,
                        embedding_config.max_search_results,
                        embedding_config.time_decay_hours,
                        embedding_config.quantization_enabled,
                        coord.clone(),
                    )
                } else {
                    maekon_analysis::VectorRetriever::new(
                        ep.clone(),
                        vs.clone(),
                        pii_filter,
                        embedding_config.max_search_results,
                        embedding_config.time_decay_hours,
                        embedding_config.quantization_enabled,
                    )
                };
                analyzer.set_vector_retriever(retriever).await;
            }
        }

        // --- Layer 3: Tiered-memory analysis pipeline ---
        let analysis = analysis_setup::build_analysis_pipeline(
            &self.config,
            &self.consent_manager,
            self.calibration_writer,
            self.calibration_reader,
            self.override_store.clone(),
            self.recluster_requested.clone(),
            &mut embedding,
            Some(external_llm_privacy_guard.clone()),
            self.regime_manager.clone(),
            self.regime_classifier.clone(),
            self.breaker_registry.clone(),
            #[cfg(feature = "analysis")]
            self.provider_secret_stores.as_ref(),
        );
        if let Some(state) = analysis.adaptive_trigger_state {
            scheduler = scheduler.with_adaptive_trigger(state);
        }

        // --- Cross-device sync engine ---
        let sync = sync_setup::build_sync_engine(
            &self.config,
            &self.data_dir,
            &self.sqlite_storage_concrete,
            self.consent_manager.clone(),
            Some(self.erasure_requested.clone()),
        )
        .await;
        if let Some(sync_engine) = sync.sync_engine {
            // #6264: publish the built engine to the shared IPC slot so the
            // cross-device-sync Tauri commands (status/trigger/peers) operate on
            // the SAME engine the scheduler drives, instead of returning
            // service.unavailable. set() is a no-op if already populated.
            if let Some(slot) = &self.sync_runtime_slot {
                let _ = slot.set(sync_engine.clone());
            }
            scheduler = scheduler.with_sync_engine(sync_engine);
        }

        // --- Phase 3: Wire ConsentManager into scheduler for runtime consent checks ---
        if let Some(ref cm) = self.consent_manager {
            scheduler = scheduler.with_consent_manager(cm.clone());
        }

        // --- #5069: feature-performance emitter ---
        // Needs all three: the REST sink (analysis/server builds only), the
        // consent authority (telemetry gate), and the egress ledger sink (the
        // shared SqliteStorage — #4803 audit). Absent any one ⇒ recorder stays
        // None and every `time_feature(..)` wrap is a zero-overhead pass-through.
        #[cfg(feature = "analysis")]
        if let (Some(sink), Some(cm)) = (
            support.feature_perf_sink_opt.clone(),
            self.consent_manager.clone(),
        ) {
            let egress: Arc<dyn maekon_core::ports::egress_ledger::EgressLedgerSink> =
                self.sqlite_storage_concrete.clone();
            let uploader = Arc::new(
                maekon_network::feature_perf_uploader::FeaturePerfUploader::new(
                    sink,
                    cm,
                    Some(egress),
                ),
            );
            scheduler = scheduler.with_feature_perf(uploader);
        }

        // --- Coaching engine + storage + overlay wiring ---
        if let Some(engine) = self.coaching_engine {
            scheduler = scheduler.with_coaching_engine(engine);
        }
        if let Some(coaching_storage) = self.coaching_storage {
            scheduler = scheduler.with_coaching_storage(coaching_storage);
        }
        // Clone before move — needed to wire on_new callback for SuggestionReceiver below.
        #[cfg(feature = "server")]
        let magic_overlay_for_suggestions = self.magic_overlay.clone();
        if let Some(overlay) = self.magic_overlay {
            scheduler = scheduler.with_magic_overlay(overlay);
        }
        if let Some(driver) = self.overlay_driver {
            scheduler = scheduler.with_overlay_driver(driver);
        }
        if let Some(capture_paused) = self.capture_paused {
            scheduler = scheduler.with_capture_paused(capture_paused);
        }
        if let Some(detection_active) = self.detection_active {
            scheduler = scheduler.with_detection_active(detection_active);
        }

        // --- Focus mode state for coaching/notification suppression (A4) ---
        if let Some(focus_mode) = self.focus_mode {
            scheduler = scheduler.with_focus_mode(focus_mode);
        }

        // --- SharedRegimeState: thread through to scheduler for single-instance sharing ---
        if let Some(shared_regime) = self.shared_regime {
            scheduler = scheduler.with_shared_regime(shared_regime);
        }

        // #5810: wire regime crash-durability checkpoint — same store as the
        // shutdown path (built from the shared sqlite_storage_concrete connection)
        // and the same regime_manager Arc (injected by the composition root).
        {
            let regime_store: Arc<dyn maekon_core::ports::regime_storage::RegimeStoragePort> =
                Arc::new(
                    maekon_storage::regime_manager_state_store::SqliteRegimeManagerStateStore::new(
                        self.sqlite_storage_concrete.connection_arc(),
                    ),
                );
            scheduler = scheduler.with_regime_storage(regime_store);
        }
        if let Some(rm) = self.regime_manager.clone() {
            scheduler = scheduler.with_regime_manager_arc(rm);
        }

        // --- Analysis provider for coaching LLM personalization ---
        #[cfg(feature = "analysis")]
        if let Some((provider, _health)) = analysis_helpers::build_analysis_provider(
            &self.config.ai_provider,
            self.config.privacy.pii_filter_level,
            Some(external_llm_privacy_guard.clone()),
            self.provider_secret_stores.as_ref(),
            self.breaker_registry.clone(),
        ) {
            scheduler = scheduler.with_analysis_provider(provider);
        }

        // --- Health check flags ---
        if let (Some(s), Some(l), Some(c)) = (
            self.server_health_flag,
            self.llm_health_flag,
            self.cli_health_flag,
        ) {
            scheduler = scheduler.with_health_flags(s, l, c);
        }
        if let (Some(s), Some(l), Some(c)) = (
            self.server_connected,
            self.llm_connected,
            self.cli_connected,
        ) {
            scheduler = scheduler.with_connection_flags(s, l, c);
        }
        if let Some(handle) = self.tray_app_handle {
            scheduler = scheduler.with_tray_app_handle(handle);
        }

        // --- Suggestion reception ---
        scheduler = scheduler.with_suggestions_enabled(self.suggestions_enabled);
        // Prefer receiver from support context (built with SSE client);
        // fall back to externally-injected receiver if present.
        // SSE receiver (server-sourced suggestions) — server feature only.
        #[cfg(feature = "server")]
        {
            let receiver = support.suggestion_receiver.or(self.suggestion_receiver);
            if let Some(ref receiver) = receiver {
                // Wire on_new callback so SSE-received suggestions notify the overlay.
                if let Some(overlay) = magic_overlay_for_suggestions {
                    let overlay_clone = overlay.clone();
                    receiver
                        .set_on_new(Arc::new(move |count| {
                            overlay_clone.emit_suggestions_changed(count);
                        }))
                        .await;
                    info!("SuggestionReceiver on_new callback wired to MagicOverlay");
                }
            }
            if let Some(receiver) = receiver {
                scheduler = scheduler.with_suggestion_receiver(receiver);
            }
        }
        // E20-24 (#4816): the local SuggestionManager is wired in OSS builds too —
        // it is the live queue the local analysis producer pushes into and the IPC
        // accept/reject path reads. Gated on `local-suggestions`, separate from the
        // server-only SSE receiver above.
        #[cfg(feature = "local-suggestions")]
        if let Some(mgr) = self.suggestion_manager {
            scheduler = scheduler.with_suggestion_manager(mgr);
        }

        // --- Phase 2: Accessibility extractor (gated by config + consent) ---
        {
            let text_config = self.config.analysis.text_intelligence.clone();
            let ax_consent_ok = self
                .consent_manager
                .as_ref()
                .and_then(|cm| cm.current_consent())
                .map(|c| c.permissions.activity_pattern_learning)
                .unwrap_or(false);

            if text_config.enabled && text_config.accessibility_extraction && ax_consent_ok {
                if let Some(extractor) =
                    accessibility_extractor.or_else(maekon_vision::accessibility::create_extractor)
                {
                    info!(
                        name = extractor.name(),
                        "Accessibility extractor enabled (Phase 2)"
                    );
                    scheduler = scheduler.with_accessibility_extractor(extractor);
                }
            }
        }

        info!("Agent started (offline={})", self.offline_mode);
        scheduler.run(shutdown_rx, Some(self.app_handle)).await;
        info!("Agent ended");
        Ok(())
    }
}

pub(crate) struct AgentRuntimeBuilder<'a> {
    storage: Arc<dyn StorageService>,
    scheduler_storage: Arc<dyn SchedulerStorage>,
    focus_storage: Arc<dyn FocusStorage>,
    calibration_writer: Option<Arc<dyn CalibrationWriter>>,
    calibration_reader: Option<Arc<dyn CalibrationReader>>,
    override_store: Option<Arc<dyn maekon_core::ports::override_store::OverrideStore>>,
    recluster_requested: Arc<std::sync::atomic::AtomicBool>,
    erasure_requested: Arc<std::sync::atomic::AtomicBool>,
    vector_store: Option<Arc<dyn maekon_core::ports::vector_store::VectorStore>>,
    data_dir: &'a Path,
    config: &'a AppConfig,
    config_manager: ConfigManager,
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// Concrete SQLite storage for sync engine wiring.
    sqlite_storage_concrete: Arc<maekon_storage::sqlite::SqliteStorage>,
    /// #6264: shared write-once slot for the built `SyncEngine` — see the
    /// matching field on `AgentRuntimeBundle`.
    sync_runtime_slot: Option<Arc<std::sync::OnceLock<Arc<crate::sync_engine::SyncEngine>>>>,
    /// #6266: shared write-once slot for the reloadable embedding model — see the
    /// matching field on `AgentRuntimeBundle`.
    embedding_runtime_slot: Option<
        Arc<std::sync::OnceLock<Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>>>,
    >,
    offline_mode: bool,
    event_tx: Option<broadcast::Sender<RealtimeEvent>>,
    #[cfg(feature = "analysis")]
    oauth_coordinator: Option<Arc<TokenRefreshCoordinator>>,
    /// Provider secret stores — see matching field on `AgentRuntimeBundle`.
    #[cfg(feature = "analysis")]
    provider_secret_stores: Option<maekon_core::ports::secret_store::SecretStoreSet>,
    app_handle: AppHandle,
    coaching_engine: Option<Arc<maekon_analysis::CoachingEngine>>,
    coaching_storage: Option<Arc<dyn CoachingStoragePort>>,
    magic_overlay: Option<crate::magic_overlay::MagicOverlayHandle>,
    overlay_driver: Option<Arc<dyn maekon_core::ports::overlay_driver::OverlayDriver>>,
    capture_paused: Option<Arc<std::sync::atomic::AtomicBool>>,
    detection_active: Option<Arc<std::sync::atomic::AtomicBool>>,
    server_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    llm_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    cli_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    server_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    llm_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    cli_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    tray_app_handle: Option<tauri::AppHandle>,
    #[cfg(feature = "server")]
    suggestion_receiver: Option<Arc<maekon_suggestion::receiver::SuggestionReceiver>>,
    #[cfg(feature = "local-suggestions")]
    suggestion_manager: Option<Arc<crate::suggestion_manager::SuggestionManager>>,
    suggestions_enabled: bool,
    focus_mode: Option<Arc<crate::focus_mode::FocusModeState>>,
    shared_capture_services: Option<Arc<SharedCaptureServices>>,
    /// Shared suggestion queue — passed through to AgentSupportContextBuilder
    /// so the SuggestionReceiver uses the same queue as SuggestionManager.
    shared_suggestion_queue:
        Option<Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>>,
    shared_scorer: Option<Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>>,
    /// SharedRegimeState — passed through to the Scheduler so it shares the same
    /// instance as the SessionManager's context assembler.
    shared_regime: Option<Arc<SharedRegimeState>>,
    /// Pre-created health flag for the primary analysis provider, shared with AppState.
    analysis_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Pre-constructed RegimeManager + RegimeClassifier handles — see
    /// matching fields on `AgentRuntimeBundle` for the rationale.
    regime_manager: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>>,
    regime_classifier: Option<Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>>,
    /// D7 (#4812 / E20-20): shared workspace-wide circuit-breaker registry from
    /// the composition root. Defaults to a fresh registry so this builder works
    /// standalone (e.g. in tests); the composition root overrides it with the
    /// single shared Arc via `with_breaker_registry`.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
}

impl<'a> AgentRuntimeBuilder<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        storage: Arc<dyn StorageService>,
        scheduler_storage: Arc<dyn SchedulerStorage>,
        focus_storage: Arc<dyn FocusStorage>,
        sqlite_storage_concrete: Arc<maekon_storage::sqlite::SqliteStorage>,
        data_dir: &'a Path,
        config: &'a AppConfig,
        config_manager: ConfigManager,
        recluster_requested: Arc<std::sync::atomic::AtomicBool>,
        erasure_requested: Arc<std::sync::atomic::AtomicBool>,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            storage,
            scheduler_storage,
            focus_storage,
            calibration_writer: None,
            calibration_reader: None,
            override_store: None,
            recluster_requested,
            erasure_requested,
            vector_store: None,
            sqlite_storage_concrete,
            sync_runtime_slot: None,
            embedding_runtime_slot: None,
            data_dir,
            config,
            config_manager,
            consent_manager: None,
            offline_mode: false,
            event_tx: None,
            #[cfg(feature = "analysis")]
            oauth_coordinator: None,
            #[cfg(feature = "analysis")]
            provider_secret_stores: None,
            app_handle,
            coaching_engine: None,
            coaching_storage: None,
            magic_overlay: None,
            overlay_driver: None,
            capture_paused: None,
            detection_active: None,
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
            focus_mode: None,
            shared_capture_services: None,
            shared_suggestion_queue: None,
            shared_scorer: None,
            shared_regime: None,
            analysis_health_flag: None,
            regime_manager: None,
            regime_classifier: None,
            breaker_registry: crate::breaker_registry::CircuitBreakerRegistry::new(),
        }
    }

    /// Inject the single shared workspace-wide circuit-breaker registry from the
    /// composition root (D7 #4812 / E20-20). All network adapters this runtime
    /// builds will clone this one Arc.
    pub(crate) fn with_breaker_registry(
        mut self,
        registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    ) -> Self {
        self.breaker_registry = registry;
        self
    }

    /// Inject pre-constructed RegimeManager + RegimeClassifier handles so the
    /// analysis pipeline uses the same instances AppState keeps for hydrate /
    /// shutdown save / feedback fan-out.
    pub(crate) fn with_regime_handles(
        mut self,
        regime_manager: Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>,
        regime_classifier: Arc<parking_lot::Mutex<maekon_analysis::RegimeClassifier>>,
    ) -> Self {
        self.regime_manager = Some(regime_manager);
        self.regime_classifier = Some(regime_classifier);
        self
    }

    pub(crate) fn with_focus_mode(
        mut self,
        focus_mode: Arc<crate::focus_mode::FocusModeState>,
    ) -> Self {
        self.focus_mode = Some(focus_mode);
        self
    }

    pub(crate) fn with_shared_capture_services(
        mut self,
        services: Arc<SharedCaptureServices>,
    ) -> Self {
        self.shared_capture_services = Some(services);
        self
    }

    pub(crate) fn with_calibration_writer(mut self, writer: Arc<dyn CalibrationWriter>) -> Self {
        self.calibration_writer = Some(writer);
        self
    }

    pub(crate) fn with_calibration_reader(mut self, reader: Arc<dyn CalibrationReader>) -> Self {
        self.calibration_reader = Some(reader);
        self
    }

    pub(crate) fn with_override_store(
        mut self,
        store: Arc<dyn maekon_core::ports::override_store::OverrideStore>,
    ) -> Self {
        self.override_store = Some(store);
        self
    }

    pub(crate) fn with_vector_store(
        mut self,
        store: Arc<dyn maekon_core::ports::vector_store::VectorStore>,
    ) -> Self {
        self.vector_store = Some(store);
        self
    }

    pub(crate) fn with_consent_manager(mut self, cm: Arc<dyn ConsentManagerPort>) -> Self {
        self.consent_manager = Some(cm);
        self
    }

    pub(crate) fn with_offline_mode(mut self, offline_mode: bool) -> Self {
        self.offline_mode = offline_mode;
        self
    }

    pub(crate) fn with_event_tx(
        mut self,
        event_tx: Option<broadcast::Sender<RealtimeEvent>>,
    ) -> Self {
        self.event_tx = event_tx;
        self
    }

    #[cfg(feature = "analysis")]
    pub(crate) fn with_oauth_coordinator(
        mut self,
        oauth_coordinator: Option<Arc<TokenRefreshCoordinator>>,
    ) -> Self {
        self.oauth_coordinator = oauth_coordinator;
        self
    }

    /// Inject the provider secret store set so analysis and embedding adapters
    /// can resolve OS-keychain-backed keys at request time.
    ///
    /// Called by `ProviderRuntimeContext::configure_agent_builder`; gated on
    /// `analysis` (same as `oauth_coordinator`).
    #[cfg(feature = "analysis")]
    pub(crate) fn with_provider_secret_stores(
        mut self,
        stores: maekon_core::ports::secret_store::SecretStoreSet,
    ) -> Self {
        self.provider_secret_stores = Some(stores);
        self
    }

    pub(crate) fn with_coaching_engine(
        mut self,
        engine: Arc<maekon_analysis::CoachingEngine>,
    ) -> Self {
        self.coaching_engine = Some(engine);
        self
    }

    pub(crate) fn with_coaching_storage(mut self, storage: Arc<dyn CoachingStoragePort>) -> Self {
        self.coaching_storage = Some(storage);
        self
    }

    pub(crate) fn with_magic_overlay(
        mut self,
        overlay: crate::magic_overlay::MagicOverlayHandle,
    ) -> Self {
        self.magic_overlay = Some(overlay);
        self
    }

    pub(crate) fn with_overlay_driver(
        mut self,
        driver: Arc<dyn maekon_core::ports::overlay_driver::OverlayDriver>,
    ) -> Self {
        self.overlay_driver = Some(driver);
        self
    }

    /// #6264: provide the shared write-once slot the runtime populates with the
    /// built `SyncEngine`, so the cross-device-sync IPC commands gain a live
    /// engine. Shares the `Arc<OnceLock<..>>` held by the registered
    /// `SyncRuntimeState`.
    pub(crate) fn with_sync_runtime_slot(
        mut self,
        slot: Arc<std::sync::OnceLock<Arc<crate::sync_engine::SyncEngine>>>,
    ) -> Self {
        self.sync_runtime_slot = Some(slot);
        self
    }

    /// #6266: provide the shared write-once slot the runtime populates with the
    /// reloadable embedding model, so `reload_embedding_model` IPC gains a live
    /// target. Shares the `Arc<OnceLock<..>>` held by the registered
    /// `EmbeddingRuntimeState`.
    pub(crate) fn with_embedding_runtime_slot(
        mut self,
        slot: Arc<
            std::sync::OnceLock<Arc<dyn maekon_core::ports::embedding_provider::ReloadableModel>>,
        >,
    ) -> Self {
        self.embedding_runtime_slot = Some(slot);
        self
    }

    pub(crate) fn with_capture_paused(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.capture_paused = Some(flag);
        self
    }

    pub(crate) fn with_detection_active(
        mut self,
        flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.detection_active = Some(flag);
        self
    }

    pub(crate) fn with_health_flags(
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

    pub(crate) fn with_connection_flags(
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

    pub(crate) fn with_tray_app_handle(mut self, handle: tauri::AppHandle) -> Self {
        self.tray_app_handle = Some(handle);
        self
    }

    #[cfg(feature = "server")]
    #[allow(dead_code)] // retained for external injection; support context is the primary path
    pub(crate) fn with_suggestion_receiver(
        mut self,
        receiver: Arc<maekon_suggestion::receiver::SuggestionReceiver>,
    ) -> Self {
        self.suggestion_receiver = Some(receiver);
        self
    }

    pub(crate) fn with_suggestions_enabled(mut self, enabled: bool) -> Self {
        self.suggestions_enabled = enabled;
        self
    }

    #[cfg(feature = "local-suggestions")]
    pub(crate) fn with_suggestion_manager(
        mut self,
        manager: Arc<crate::suggestion_manager::SuggestionManager>,
    ) -> Self {
        self.suggestion_manager = Some(manager);
        self
    }

    // E20-24 (#4816): invoked under `local-suggestions` (default-on); the shared
    // queue/scorer it sets are consumed only by the server SSE receiver
    // (agent_runtime_support.rs, cfg server), so the value terminates unused in
    // OSS builds — `allow(dead_code)` keeps --no-default-features quiet.
    #[allow(dead_code)]
    pub(crate) fn with_shared_suggestion_queue(
        mut self,
        queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    ) -> Self {
        self.shared_suggestion_queue = Some(queue);
        self
    }

    // E20-24 (#4816): see with_shared_suggestion_queue — set under local-suggestions,
    // read only by the server SSE receiver, so unused in OSS builds.
    #[allow(dead_code)]
    pub(crate) fn with_shared_scorer(
        mut self,
        scorer: Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>,
    ) -> Self {
        self.shared_scorer = Some(scorer);
        self
    }

    pub(crate) fn with_shared_regime(mut self, regime: Arc<SharedRegimeState>) -> Self {
        self.shared_regime = Some(regime);
        self
    }

    pub(crate) fn with_analysis_health_flag(
        mut self,
        flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.analysis_health_flag = Some(flag);
        self
    }

    pub(crate) fn build(self) -> AgentRuntimeBundle {
        AgentRuntimeBundle {
            storage: self.storage,
            scheduler_storage: self.scheduler_storage,
            focus_storage: self.focus_storage,
            calibration_writer: self.calibration_writer,
            calibration_reader: self.calibration_reader,
            override_store: self.override_store,
            recluster_requested: self.recluster_requested,
            erasure_requested: self.erasure_requested,
            vector_store: self.vector_store,
            data_dir: self.data_dir.to_path_buf(),
            config: self.config.clone(),
            config_manager: self.config_manager,
            consent_manager: self.consent_manager,
            sqlite_storage_concrete: self.sqlite_storage_concrete,
            sync_runtime_slot: self.sync_runtime_slot,
            embedding_runtime_slot: self.embedding_runtime_slot,
            offline_mode: self.offline_mode,
            event_tx: self.event_tx,
            #[cfg(feature = "analysis")]
            oauth_coordinator: self.oauth_coordinator,
            #[cfg(feature = "analysis")]
            provider_secret_stores: self.provider_secret_stores,
            app_handle: self.app_handle,
            coaching_engine: self.coaching_engine,
            coaching_storage: self.coaching_storage,
            magic_overlay: self.magic_overlay,
            overlay_driver: self.overlay_driver,
            capture_paused: self.capture_paused,
            detection_active: self.detection_active,
            server_health_flag: self.server_health_flag,
            llm_health_flag: self.llm_health_flag,
            cli_health_flag: self.cli_health_flag,
            server_connected: self.server_connected,
            llm_connected: self.llm_connected,
            cli_connected: self.cli_connected,
            tray_app_handle: self.tray_app_handle,
            #[cfg(feature = "server")]
            suggestion_receiver: self.suggestion_receiver,
            #[cfg(feature = "local-suggestions")]
            suggestion_manager: self.suggestion_manager,
            suggestions_enabled: self.suggestions_enabled,
            focus_mode: self.focus_mode,
            shared_capture_services: self.shared_capture_services,
            shared_suggestion_queue: self.shared_suggestion_queue,
            shared_scorer: self.shared_scorer,
            shared_regime: self.shared_regime,
            analysis_health_flag: self.analysis_health_flag,
            regime_manager: self.regime_manager,
            regime_classifier: self.regime_classifier,
            breaker_registry: self.breaker_registry,
        }
    }
}
