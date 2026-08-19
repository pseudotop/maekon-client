pub(crate) mod analysis_helpers;
mod analysis_setup;
// #8059: `pub(crate)` so the web semantic-search wiring
// (`app_runtime_launch::web_server_wiring`) can call
// `embedding_setup::build_web_search_components`, tapping the same embedding
// source as this scheduler ingestion path.
pub(crate) mod embedding_setup;
mod sync_setup;

use anyhow::Result;
use maekon_analysis::focus_analyzer::FocusAnalyzer;
use maekon_core::config::AppConfig;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::calibration_store::{CalibrationReader, CalibrationWriter};
use maekon_core::ports::coaching_storage::CoachingStoragePort;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::focus_storage::FocusStorage;
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
use crate::runtime_state::SceneFinderSlot;
use crate::scheduler::shared_regime_state::SharedRegimeState;
use crate::scheduler::{Scheduler, SchedulerStorage};

/// Upper bound on how many previously-unindexed segments a single startup FTS
/// backfill pass indexes (#8051). Bounds startup cost for heavy-history users;
/// a larger backlog is drained across subsequent restarts (see
/// `SqliteStorage::backfill_unindexed_segments_fts`).
const FTS_BACKFILL_MAX_SEGMENTS: usize = 5_000;

#[derive(Clone)]
pub(crate) struct AgentRuntimeBundle {
    storage: Arc<dyn StorageService>,
    scheduler_storage: Arc<dyn SchedulerStorage>,
    focus_storage: Arc<dyn FocusStorage>,
    /// Pre-built FocusAnalyzer shared with Tauri consent commands. Production
    /// wiring installs one instance so runtime consent narrowing can clear its
    /// in-memory pattern state immediately.
    focus_analyzer: Option<Arc<FocusAnalyzer>>,
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
    sync_runtime_slot: Option<Arc<std::sync::OnceLock<Arc<maekon_core::sync_engine::SyncEngine>>>>,
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
    scene_finder_slot: Option<SceneFinderSlot>,
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
    /// #9459: the ONE shared `TokenManager` from the composition root, forwarded
    /// to `AgentSupportContextBuilder` so the server transports built inside
    /// `run()` authenticate with the same session the login IPC writes to.
    #[cfg(feature = "server")]
    shared_token_manager: Option<Arc<maekon_network::auth::TokenManager>>,
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
        .with_app_handle(self.app_handle.clone())
        // #9639 review I1: the notification config watcher needs a real exit
        // signal — sender-drop is unreachable in this app.
        .with_shutdown_rx(shutdown_rx.clone());
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
        // #9459: same rationale, applied to the login session — the support
        // context's server transports must authenticate with the composition
        // root's manager, not a second one built inside `build()`.
        #[cfg(feature = "server")]
        {
            builder = builder.with_shared_token_manager(self.shared_token_manager.clone());
        }
        // Clone before the move into Scheduler::with_config_manager below.
        builder = builder.with_config_manager(self.config_manager.clone());
        builder = builder.with_offline_mode(self.offline_mode);
        if let Some(ref cm) = self.consent_manager {
            builder = builder.with_consent_manager(cm.clone());
        }
        // Wire provider secret stores so the ContextAnalyzer analysis provider
        // resolves OS-keychain-backed BYOK keys at request time.
        #[cfg(feature = "analysis")]
        if let Some(ref stores) = self.provider_secret_stores {
            builder = builder.with_provider_secret_stores(stores.clone());
        }
        let mut support = builder.build().await?;
        if let Some(ref focus_analyzer) = self.focus_analyzer {
            support.focus_analyzer = focus_analyzer.clone();
        }
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

        // #7737 C1 PR-2: assemble the VERIFIED-unconditional dependency set as
        // ONE struct literal — every field named, no `Default`/`..` escape
        // (mirrors #7738 D-2's `WebServerRequiredDeps`). Each is either an
        // unconditional Port Instance Sharing cast off `sqlite_storage_concrete`
        // or an unconditional concrete value built right here — see
        // `scheduler::required_deps` for the per-field verified-unconditional
        // source citations.
        // ADR-033: vault mirror writer over the same shared SqliteStorage Arc
        // (stateless — see vault_wiring). Built before the deps literal so the
        // config_manager move below stays intact.
        let memory_vault_writer = crate::vault_wiring::build_vault_writer(
            Arc::clone(&self.sqlite_storage_concrete),
            self.consent_manager.clone(),
            self.config_manager.clone(),
        );
        let scheduler_deps = crate::scheduler::SchedulerRequiredDeps {
            frame_storage: support.frame_storage,
            memory_vault_writer,
            notification_manager: support.notification_manager,
            focus_analyzer: support.focus_analyzer,
            config_manager: self.config_manager,
            // ADR-023: share the single SqliteStorage Arc as the MemoryGraphPort
            // so the aggregation loop can promote daily-digest content into
            // durable claims + evidence edges (Port Instance Sharing guardrail —
            // no second handle).
            memory_graph: Arc::clone(&self.sqlite_storage_concrete)
                as Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>,
            // #7678 D4: share the single SqliteStorage Arc as the
            // CalibrationReader so the aggregation loop's housekeeping block can
            // bound calibration_log growth via the configured retention
            // window/row-cap (previously parsed+persisted but never enforced —
            // Port Instance Sharing guardrail).
            calibration_reader: Arc::clone(&self.sqlite_storage_concrete)
                as Arc<dyn maekon_core::ports::calibration_store::CalibrationReader>,
            // ADR-023 Phase-2: belief revision. The enrichment provider is
            // LOCAL-LLM-gated by construction (non-loopback endpoints → NoOp); a
            // per-claim PII masker is applied at the enrichment boundary
            // (MG-PII-03). The aggregation loop additionally gates each run on
            // memory_graph_enrichment consent + the belief_revision_enabled flag.
            belief_revision: {
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
                Arc::new(maekon_analysis::BeliefRevision::new(
                    enrichment_provider,
                    Arc::clone(&self.sqlite_storage_concrete)
                        as Arc<dyn maekon_core::ports::memory_graph_port::MemoryGraphPort>,
                    belief_pii_filter,
                    self.config.analysis.supersede_confidence_threshold,
                    self.config.analysis.belief_revision_enabled,
                ))
            },
            // #5810: same store as the shutdown path (built from the shared
            // sqlite_storage_concrete connection) — periodic crash-durability
            // checkpoint target for the aggregation loop.
            regime_storage: Arc::new(
                maekon_storage::regime_manager_state_store::SqliteRegimeManagerStateStore::new(
                    self.sqlite_storage_concrete.connection_arc(),
                ),
            )
                as Arc<dyn maekon_core::ports::regime_storage::RegimeStoragePort>,
        };

        let mut scheduler = Scheduler::new(
            support.scheduler_config,
            support.system_monitor,
            support.activity_monitor,
            support.process_monitor,
            support.capture_trigger,
            support.frame_processor,
            self.storage,
            self.scheduler_storage,
            support.batch_sink_opt,
            support.api_client_opt,
        )
        .with_required_deps(scheduler_deps);

        if let Some(ref analyzer) = support.context_analyzer {
            scheduler = scheduler.with_context_analyzer(analyzer.clone());
        }
        // #7652: wire the runtime-rebuild factory REGARDLESS of the startup-time
        // analyzer state — it is what lets the analysis loop honor a later
        // `analysis.enabled` flip (or a BYOK key saved after boot) without a
        // restart, even when `support.context_analyzer` above was `None`.
        #[cfg(feature = "analysis")]
        if let Some(factory) = support.context_analyzer_factory {
            scheduler = scheduler.with_context_analyzer_factory(factory);
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
        let search_coordinator: Option<Arc<maekon_analysis::AdaptiveSearchCoordinator>> =
            if let (Some(ref vs), Some(ref vi)) = (&embedding.vector_store, &vector_index) {
                // #8059: SearchConfig construction moved to the shared
                // `embedding_setup::search_config_from` so the scheduler ingestion
                // coordinator and the web search coordinator stay in lock-step.
                let sc = embedding_setup::search_config_from(embedding_config);
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
            // #8051: share the single SqliteStorage Arc as the FTS content
            // indexer so closed segments become keyword-searchable.
            Some(self.sqlite_storage_concrete.clone()
                as Arc<dyn maekon_core::ports::text_search::TextSearchProvider>),
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
        if let Some(scene_finder_slot) = self.scene_finder_slot {
            scheduler = scheduler.with_scene_finder_slot(scene_finder_slot);
        }

        // --- Focus mode state for coaching/notification suppression (A4) ---
        if let Some(focus_mode) = self.focus_mode {
            scheduler = scheduler.with_focus_mode(focus_mode);
        }

        // --- SharedRegimeState: thread through to scheduler for single-instance sharing ---
        if let Some(shared_regime) = self.shared_regime {
            scheduler = scheduler.with_shared_regime(shared_regime);
        }

        // #5810 / #7737 C1 PR-2: the regime crash-durability checkpoint STORE is
        // now assembled above in `scheduler_deps.regime_storage` (same
        // sqlite_storage_concrete-backed wrapper). Only the live regime manager
        // Arc — genuinely conditional (injected only when the composition root
        // pre-constructed one) — is wired here.
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

        // --- #8051: one-shot FTS content backfill ---
        //
        // Segments persisted before the live segment-close FTS wiring existed
        // are absent from `search_fts`, so keyword search returns nothing for
        // them. Index them once at startup as a detached, bounded, non-fatal
        // task. It is a single `NOT IN` batch (capped at
        // `FTS_BACKFILL_MAX_SEGMENTS`) offloaded to `spawn_blocking` via
        // `with_conn`, so it never touches the scheduler hot path; a larger
        // backlog is drained across restarts. Not a long-running loop, so no
        // shutdown wiring is required.
        {
            let storage = self.sqlite_storage_concrete.clone();
            tokio::spawn(async move {
                match storage
                    .backfill_unindexed_segments_fts(FTS_BACKFILL_MAX_SEGMENTS)
                    .await
                {
                    Ok(0) => {}
                    Ok(n) => info!("FTS backfill: indexed {n} previously-unindexed segment(s)"),
                    Err(e) => tracing::warn!(
                        "FTS backfill failed (non-fatal; keyword search may miss older \
                         segments until the next start): {e}"
                    ),
                }
            });
        }

        // --- #8686 AC7 (ADR-028): one-shot durable-task reconcile at startup ---
        //
        // Expires PROPOSED candidates past their TTL (clearing their content)
        // and counts confirmed-candidate/to-do integrity drift. The store
        // persists a monotonic reconcile floor, so replays across restarts are
        // idempotent. Previously this contract existed only in tests — nothing
        // invoked it at launch, so stale proposals survived restarts with
        // their content intact. Detached, bounded, non-fatal (same shape as
        // the #8051 FTS backfill above).
        {
            use maekon_core::ports::task_store::TaskCommandPort as _;
            let storage = self.sqlite_storage_concrete.clone();
            tokio::spawn(async move {
                match storage.reconcile_tasks(chrono::Utc::now()).await {
                    Ok(report) => {
                        if report.expired_candidates > 0 || report.integrity_errors > 0 {
                            info!(
                                expired = report.expired_candidates,
                                integrity_errors = report.integrity_errors,
                                "durable-task reconcile: expired stale candidates at startup"
                            );
                        }
                    }
                    Err(e) => tracing::warn!(
                        "durable-task reconcile failed (non-fatal; stale proposed candidates \
                         keep their content until the next start): {e}"
                    ),
                }
            });
        }

        info!("Agent started (offline={})", self.offline_mode);
        scheduler.run(shutdown_rx, Some(self.app_handle)).await;
        info!("Agent ended");
        Ok(())
    }
}

impl AgentRuntimeBundle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        storage: Arc<dyn StorageService>,
        scheduler_storage: Arc<dyn SchedulerStorage>,
        focus_storage: Arc<dyn FocusStorage>,
        sqlite_storage_concrete: Arc<maekon_storage::sqlite::SqliteStorage>,
        data_dir: &Path,
        config: &AppConfig,
        config_manager: ConfigManager,
        recluster_requested: Arc<std::sync::atomic::AtomicBool>,
        erasure_requested: Arc<std::sync::atomic::AtomicBool>,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            storage,
            scheduler_storage,
            focus_storage,
            focus_analyzer: None,
            calibration_writer: None,
            calibration_reader: None,
            override_store: None,
            recluster_requested,
            erasure_requested,
            vector_store: None,
            sqlite_storage_concrete,
            sync_runtime_slot: None,
            embedding_runtime_slot: None,
            data_dir: data_dir.to_path_buf(),
            config: config.clone(),
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
            focus_mode: None,
            shared_capture_services: None,
            shared_suggestion_queue: None,
            shared_scorer: None,
            shared_regime: None,
            analysis_health_flag: None,
            regime_manager: None,
            regime_classifier: None,
            breaker_registry: crate::breaker_registry::CircuitBreakerRegistry::new(),
            #[cfg(feature = "server")]
            shared_token_manager: None,
        }
    }

    /// #9459: forward the composition root's single shared `TokenManager` to the
    /// support-context builder, so the transports built inside `run()` share the
    /// session the login IPC writes through `TokenManagerState`. `None` keeps the
    /// pre-#9459 behavior of a locally-constructed manager.
    #[cfg(feature = "server")]
    pub(crate) fn with_shared_token_manager(
        mut self,
        manager: Option<Arc<maekon_network::auth::TokenManager>>,
    ) -> Self {
        self.shared_token_manager = manager;
        self
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

    pub(crate) fn with_focus_analyzer(mut self, focus_analyzer: Arc<FocusAnalyzer>) -> Self {
        self.focus_analyzer = Some(focus_analyzer);
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
        slot: Arc<std::sync::OnceLock<Arc<maekon_core::sync_engine::SyncEngine>>>,
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

    /// #7817: passes a shared scene_finder slot to the scheduler. The real
    /// finder is populated exactly once after the automation controller builds.
    pub(crate) fn with_scene_finder_slot(mut self, slot: SceneFinderSlot) -> Self {
        self.scene_finder_slot = Some(slot);
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

    // #7719: no call site sets this builder's `suggestion_receiver` via this
    // setter — it stays `None` and is forwarded as such (line ~1053). The
    // receiver actually used by the scheduler comes from a different setter
    // of the same name on the scheduler's own builder (`scheduler/mod.rs`).
    // Kept in case this builder's copy is wired up later.
    #[cfg(feature = "server")]
    #[allow(dead_code)]
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

    // E20-24 (#4816): the ONLY call site is
    // `app_runtime_launch/mod.rs`'s `#[cfg(feature = "local-suggestions")]`
    // wiring block. `local-suggestions` is default-on, so this compiles in
    // normal builds, but under `--no-default-features` that call site
    // disappears and the setter itself becomes dead code — gate it to match
    // (#7743 ctd-W3 A2b follow-up; a prior `allow(dead_code)` comment here
    // predated the actual attribute and had drifted, misattributing the gate
    // to `server` instead of `local-suggestions`).
    #[cfg(feature = "local-suggestions")]
    pub(crate) fn with_shared_suggestion_queue(
        mut self,
        queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    ) -> Self {
        self.shared_suggestion_queue = Some(queue);
        self
    }

    // E20-24 (#4816): see with_shared_suggestion_queue — same single call site,
    // same `local-suggestions` gate.
    #[cfg(feature = "local-suggestions")]
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
}
