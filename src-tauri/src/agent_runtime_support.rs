use anyhow::Result;
use maekon_analysis::focus_analyzer::FocusAnalyzer;
use maekon_core::config::AppConfig;
use maekon_core::config_manager::ConfigManager;
use maekon_core::ports::accessibility::AccessibilityExtractor;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::focus_storage::FocusStorage;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::monitor::{ActivityMonitor, ProcessMonitor};
#[cfg(feature = "server")]
use maekon_network::auth::TokenManager;
#[cfg(feature = "server")]
use maekon_network::batch_uploader::BatchUploader;
#[cfg(feature = "grpc")]
use maekon_network::grpc::{GrpcApiAdapter, GrpcConfig, GrpcSseAdapter, UnifiedClient};
#[cfg(feature = "server")]
use maekon_network::http_client::HttpApiClient;
// #7668: needed in both the grpc and non-grpc `feature = "server"` builds — the
// grpc-feature branch of `build_server_transports` now falls back to the REST
// SSE client when `use_grpc_context` is disabled (the shipped default).
#[cfg(feature = "server")]
use maekon_network::sse_client::SseStreamClient;
use maekon_vision::trigger::SmartCaptureTrigger;
use std::path::Path;
#[cfg(feature = "analysis")]
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[cfg(feature = "analysis")]
use std::future::Future;
#[cfg(feature = "analysis")]
use std::pin::Pin;
use std::sync::atomic::AtomicBool;

use crate::capture_services::SharedCaptureServices;
use crate::notification_manager::NotificationManager;
#[cfg(feature = "analysis")]
use crate::provider_adapters::ExternalOcrPrivacyGuard;
use crate::scheduler::SchedulerConfig;

pub(crate) struct AgentSupportContext {
    pub(crate) frame_storage: Arc<dyn FrameStoragePort>,
    pub(crate) system_monitor: Arc<maekon_monitor::system::SysInfoMonitor>,
    pub(crate) process_monitor: Arc<dyn ProcessMonitor>,
    pub(crate) activity_monitor: Arc<dyn ActivityMonitor>,
    pub(crate) capture_trigger: Arc<dyn maekon_core::ports::vision::CaptureTrigger>,
    pub(crate) frame_processor: Arc<dyn maekon_core::ports::vision::FrameProcessor>,
    pub(crate) accessibility_extractor: Option<Arc<dyn AccessibilityExtractor>>,
    pub(crate) scheduler_config: SchedulerConfig,
    pub(crate) batch_sink_opt: Option<Arc<dyn maekon_core::ports::batch_sink::BatchSink>>,
    pub(crate) api_client_opt: Option<Arc<dyn maekon_core::ports::api_client::ApiClient>>,
    /// Dedicated REST sink for the #5069 feature-performance emitter (None in
    /// non-`server` builds — nothing to flush to). Shares the same `TokenManager`
    /// as the main api client so it reuses the bearer JWT.
    ///
    /// Populated unconditionally by `server_transport_ports_for_mode` (both the
    /// `server` and `not(server)` arms return the 3/4-tuple that includes this
    /// slot), but only READ by `agent_runtime::AgentRuntimeBundle::run`'s
    /// `#[cfg(feature = "analysis")]` feature-perf-uploader wiring — under
    /// `--no-default-features` (analysis off) nothing reads it (#7743 ctd-W3
    /// A2b follow-up).
    #[cfg_attr(not(feature = "analysis"), allow(dead_code))]
    pub(crate) feature_perf_sink_opt: Option<FeaturePerfSinkPort>,
    pub(crate) notification_manager: Arc<NotificationManager>,
    pub(crate) focus_analyzer: Arc<FocusAnalyzer>,
    pub(crate) context_analyzer: Option<Arc<maekon_analysis::ContextAnalyzer>>,
    /// #7652: reusable factory that (re)builds a `ContextAnalyzer` from the
    /// CURRENT (live) config on demand. Always `Some` in `analysis` builds
    /// regardless of the startup-time `analysis.enabled`/provider state, so
    /// the scheduler's analysis loop can honor a runtime enable (or a BYOK
    /// key saved after boot) WITHOUT an app restart. `None` (field absent) in
    /// non-`analysis` builds — nothing to rebuild there.
    #[cfg(feature = "analysis")]
    pub(crate) context_analyzer_factory: Option<ContextAnalyzerFactory>,
    // Only read in `agent_runtime/mod.rs`'s `#[cfg(feature = "server")]`
    // suggestion-reception wiring — always `None` and unread without that
    // feature.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub(crate) suggestion_receiver: Option<Arc<maekon_suggestion::receiver::SuggestionReceiver>>,
}

/// #7652: signature for the runtime analyzer-rebuild factory. Takes a live
/// `AppConfig` snapshot (so a freshly-saved BYOK `ai_provider.llm_api` key is
/// honored) and resolves to `Some(analyzer)` when `analysis.enabled` is true
/// AND a usable LLM provider is configured, `None` otherwise (still disabled,
/// or no provider yet — the caller should retry on a later tick).
#[cfg(feature = "analysis")]
pub(crate) type ContextAnalyzerFactory = Arc<
    dyn Fn(
            Arc<AppConfig>,
        )
            -> Pin<Box<dyn Future<Output = Option<Arc<maekon_analysis::ContextAnalyzer>>> + Send>>
        + Send
        + Sync,
>;

/// #7652: dependencies needed to (re)construct a `ContextAnalyzer`, captured
/// once (cheap `Arc` clones) so the runtime factory closure can rebuild the
/// analyzer on demand without holding a borrow of the (by-then consumed)
/// `AgentSupportContextBuilder`.
#[cfg(feature = "analysis")]
#[derive(Clone)]
struct ContextAnalyzerDeps {
    storage: Option<Arc<dyn maekon_core::ports::storage::StorageService>>,
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    process_monitor: Arc<dyn ProcessMonitor>,
    provider_secret_stores: Option<maekon_core::ports::secret_store::SecretStoreSet>,
    analysis_health_flag: Option<Arc<AtomicBool>>,
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    few_shot_storage: Option<Arc<dyn maekon_core::ports::few_shot_storage::FewShotStorage>>,
    data_dir: PathBuf,
}

type BatchSinkPort = Arc<dyn maekon_core::ports::batch_sink::BatchSink>;
type ApiClientPort = Arc<dyn maekon_core::ports::api_client::ApiClient>;
type FeaturePerfSinkPort = Arc<dyn maekon_core::ports::feature_perf::FeaturePerfSink>;
#[cfg(feature = "server")]
type SseClientPort = Arc<dyn maekon_core::ports::api_client::SseClient>;
#[cfg(feature = "server")]
type ServerTransportPorts = (
    Option<BatchSinkPort>,
    Option<ApiClientPort>,
    Option<SseClientPort>,
    Option<FeaturePerfSinkPort>,
);
#[cfg(not(feature = "server"))]
type ServerTransportPorts = (
    Option<BatchSinkPort>,
    Option<ApiClientPort>,
    Option<FeaturePerfSinkPort>,
);

pub(crate) struct AgentSupportContextBuilder<'a> {
    #[cfg(feature = "analysis")]
    data_dir: &'a Path,
    config: &'a AppConfig,
    focus_storage: Arc<dyn FocusStorage>,
    storage: Option<Arc<dyn maekon_core::ports::storage::StorageService>>,
    app_handle: Option<tauri::AppHandle>,
    /// Pre-created shared suggestion queue from SuggestionManager.
    /// When set, the SuggestionReceiver will use this queue instead of creating its own.
    shared_suggestion_queue:
        Option<Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>>,
    shared_scorer: Option<Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>>,
    shared_capture_services: Option<Arc<SharedCaptureServices>>,
    few_shot_storage: Option<Arc<dyn maekon_core::ports::few_shot_storage::FewShotStorage>>,
    /// Pre-created health flag shared with AppState. When set, `build_context_analyzer`
    /// wires it into the FallbackAnalysisProvider so the IPC `get_analysis_health`
    /// command reflects the actual provider health.
    analysis_health_flag: Option<Arc<AtomicBool>>,
    /// ConfigManager shared with the composition root. When set, the BatchUploader
    /// suppression predicate uses `snapshot()` to gate uploads outside allowed windows.
    config_manager: Option<ConfigManager>,
    /// Shared consent authority from capture wiring. External AI guards must use
    /// this instance instead of reloading consent from disk.
    consent_manager: Option<Arc<dyn ConsentManagerPort>>,
    /// When true, do not construct server-backed transports. The GUI still boots
    /// with local capture/analysis wiring, but upload, REST, SSE, and feature
    /// performance egress stay disconnected.
    offline_mode: bool,
    /// D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    /// registry from the composition root, used by the `ContextAnalyzer` analysis
    /// provider. Defaults to a fresh registry for standalone use; the composition
    /// root overrides it with the shared Arc via `with_breaker_registry`.
    breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    /// Provider secret stores for BYOK credential resolution.  Threaded from the
    /// composition root (ProviderRuntimeContext) so that OS-keychain-backed keys
    /// are resolved at request time via `CredentialSource::StoredSecret`.
    /// Only meaningful in `analysis` builds; ignored otherwise.
    #[cfg(feature = "analysis")]
    provider_secret_stores: Option<maekon_core::ports::secret_store::SecretStoreSet>,
}

impl<'a> AgentSupportContextBuilder<'a> {
    pub(crate) fn new(
        _data_dir: &'a Path,
        config: &'a AppConfig,
        focus_storage: Arc<dyn FocusStorage>,
    ) -> Self {
        Self {
            #[cfg(feature = "analysis")]
            data_dir: _data_dir,
            config,
            focus_storage,
            storage: None,
            app_handle: None,
            shared_suggestion_queue: None,
            shared_scorer: None,
            shared_capture_services: None,
            few_shot_storage: None,
            analysis_health_flag: None,
            config_manager: None,
            consent_manager: None,
            offline_mode: false,
            breaker_registry: crate::breaker_registry::CircuitBreakerRegistry::new(),
            #[cfg(feature = "analysis")]
            provider_secret_stores: None,
        }
    }

    /// Inject the single shared workspace-wide circuit-breaker registry from the
    /// composition root (D7 #4812 / E20-20). The `ContextAnalyzer` analysis
    /// provider built in `build_context_analyzer` clones this one Arc.
    pub(crate) fn with_breaker_registry(
        mut self,
        registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    ) -> Self {
        self.breaker_registry = registry;
        self
    }

    pub(crate) fn with_storage(
        mut self,
        storage: Arc<dyn maekon_core::ports::storage::StorageService>,
    ) -> Self {
        self.storage = Some(storage);
        self
    }

    pub(crate) fn with_app_handle(mut self, handle: tauri::AppHandle) -> Self {
        self.app_handle = Some(handle);
        self
    }

    pub(crate) fn with_shared_suggestion_queue(
        mut self,
        queue: Arc<tokio::sync::Mutex<maekon_suggestion::queue::SuggestionQueue>>,
    ) -> Self {
        self.shared_suggestion_queue = Some(queue);
        self
    }

    pub(crate) fn with_shared_scorer(
        mut self,
        scorer: Arc<tokio::sync::Mutex<maekon_suggestion::scorer::FeedbackScorer>>,
    ) -> Self {
        self.shared_scorer = Some(scorer);
        self
    }

    pub(crate) fn with_shared_capture_services(
        mut self,
        services: Arc<SharedCaptureServices>,
    ) -> Self {
        self.shared_capture_services = Some(services);
        self
    }

    pub(crate) fn with_few_shot_storage(
        mut self,
        storage: Arc<dyn maekon_core::ports::few_shot_storage::FewShotStorage>,
    ) -> Self {
        self.few_shot_storage = Some(storage);
        self
    }

    pub(crate) fn with_analysis_health_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.analysis_health_flag = Some(flag);
        self
    }

    /// Wire the ConfigManager so the BatchUploader suppression predicate can call
    /// `snapshot()` to check the tracking schedule on every flush (O(1) Arc-clone,
    /// per CONS-PI13 — not a deep-clone of all 37 config sections).
    pub(crate) fn with_config_manager(mut self, mgr: ConfigManager) -> Self {
        self.config_manager = Some(mgr);
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

    /// Wire the provider secret stores from `ProviderRuntimeContext` so the
    /// `ContextAnalyzer` LLM provider resolves OS-keychain-backed BYOK keys at
    /// request time.  No-op in non-`analysis` builds.
    #[cfg(feature = "analysis")]
    pub(crate) fn with_provider_secret_stores(
        mut self,
        stores: maekon_core::ports::secret_store::SecretStoreSet,
    ) -> Self {
        self.provider_secret_stores = Some(stores);
        self
    }

    #[cfg(feature = "analysis")]
    fn build_context_analyzer(
        &self,
        process_monitor: Arc<dyn ProcessMonitor>,
    ) -> Option<Arc<maekon_analysis::ContextAnalyzer>> {
        let deps = self.context_analyzer_deps(process_monitor);
        build_context_analyzer_sync(self.config, &deps)
    }

    /// #7652: capture the dependencies `build_context_analyzer` needs as cheap
    /// `Arc` clones, independent of the (short-lived) builder borrow. Shared by
    /// both the startup path (`build_context_analyzer`) and the runtime-rebuild
    /// factory (`context_analyzer_factory`) so there is exactly one place that
    /// knows how to assemble a `ContextAnalyzer`.
    #[cfg(feature = "analysis")]
    fn context_analyzer_deps(
        &self,
        process_monitor: Arc<dyn ProcessMonitor>,
    ) -> ContextAnalyzerDeps {
        ContextAnalyzerDeps {
            storage: self.storage.clone(),
            consent_manager: self.consent_manager.clone(),
            process_monitor,
            provider_secret_stores: self.provider_secret_stores.clone(),
            analysis_health_flag: self.analysis_health_flag.clone(),
            breaker_registry: self.breaker_registry.clone(),
            few_shot_storage: self.few_shot_storage.clone(),
            data_dir: self.data_dir.to_path_buf(),
        }
    }

    /// #7652: build a factory closure that (re)constructs the `ContextAnalyzer`
    /// from a LIVE `AppConfig` snapshot on demand. Installed into the Scheduler
    /// unconditionally (in `analysis` builds) so the analysis loop can honor a
    /// runtime `analysis.enabled` flip (or a BYOK `ai_provider.llm_api` key
    /// saved after boot) without an app restart, even when the startup-time
    /// analyzer was `None`.
    #[cfg(feature = "analysis")]
    fn context_analyzer_factory(
        &self,
        process_monitor: Arc<dyn ProcessMonitor>,
    ) -> ContextAnalyzerFactory {
        let deps = self.context_analyzer_deps(process_monitor);
        Arc::new(move |config: Arc<AppConfig>| {
            let deps = deps.clone();
            Box::pin(async move { build_context_analyzer_async(&config, &deps).await })
        })
    }

    #[cfg(not(feature = "analysis"))]
    fn build_context_analyzer(
        &self,
        _process_monitor: Arc<dyn ProcessMonitor>,
    ) -> Option<Arc<maekon_analysis::ContextAnalyzer>> {
        None
    }

    pub(crate) async fn build(mut self) -> Result<AgentSupportContext> {
        // #8039: `shared_capture_services` used to be treated as optional here,
        // with a silent fallback to a keyless `FrameFileStorage::new()` (an
        // UNENCRYPTED frame store reaching the scheduler) whenever it was
        // absent. The sole production caller (`agent_runtime::run`) now always
        // wires it — fed by `build_capture_wiring`'s fail-closed
        // `SharedCaptureServices::build` (#8039) — so a missing value here is
        // no longer a legitimate degraded-but-running state; it means this
        // builder was used incorrectly. Fail closed instead of constructing an
        // encryption-key-less frame store: there is no "run capture
        // unencrypted" mode.
        let Some(shared) = self.shared_capture_services.as_ref() else {
            anyhow::bail!(
                "capture services not wired; refusing to build an unencrypted frame store \
                 (frame_storage would reach the scheduler without at-rest encryption) — #8039"
            );
        };
        let (
            frame_storage,
            process_monitor,
            activity_monitor,
            frame_processor,
            accessibility_extractor,
        ) = (
            shared.frame_storage.clone(),
            shared.process_monitor.clone(),
            shared.activity_monitor.clone(),
            shared.frame_processor.clone(),
            shared.accessibility_extractor.clone(),
        );

        let system_monitor = Arc::new(maekon_monitor::system::SysInfoMonitor::new());
        let capture_trigger: Arc<dyn maekon_core::ports::vision::CaptureTrigger> = Arc::new(
            SmartCaptureTrigger::new(self.config.vision.capture_throttle_ms),
        );

        let session_id = generate_session_id();
        // Extract config_manager before any later borrows of `self` to avoid
        // partial-move conflicts (build_context_analyzer borrows self below).
        let config_manager = self.config_manager.take();
        #[cfg(feature = "server")]
        let (batch_sink_opt, api_client_opt, sse_client_opt, feature_perf_sink_opt) =
            server_transport_ports_for_mode(
                self.offline_mode,
                self.config,
                &session_id,
                config_manager,
            )?;
        #[cfg(not(feature = "server"))]
        let (batch_sink_opt, api_client_opt, feature_perf_sink_opt) =
            server_transport_ports_for_mode(
                self.offline_mode,
                self.config,
                &session_id,
                config_manager,
            )?;

        let notifier: Arc<dyn maekon_core::ports::notifier::DesktopNotifier> =
            if let Some(handle) = self.app_handle.clone() {
                Arc::new(TauriNotifier::new(handle))
            } else {
                Arc::new(LogOnlyNotifier)
            };
        let notification_manager = Arc::new(NotificationManager::new(
            self.config.notification.clone(),
            notifier.clone(),
        ));
        let focus_analyzer = Arc::new(FocusAnalyzer::with_defaults(
            self.focus_storage.clone(),
            notifier.clone(),
        ));

        let context_analyzer = self.build_context_analyzer(process_monitor.clone());

        // Wire few-shot storage into the analyzer for personalized prompts.
        if let (Some(ref analyzer), Some(ref fs_storage)) =
            (&context_analyzer, &self.few_shot_storage)
        {
            analyzer.set_few_shot_storage(fs_storage.clone()).await;
        }

        // #7652: install the runtime-rebuild factory REGARDLESS of the
        // startup-time analyzer state — this is what lets the scheduler's
        // analysis loop honor a later `analysis.enabled` flip (or a BYOK
        // `ai_provider.llm_api` key saved after boot) without an app restart.
        #[cfg(feature = "analysis")]
        let context_analyzer_factory = Some(self.context_analyzer_factory(process_monitor.clone()));

        // Build SuggestionReceiver when SSE client is available and suggestions enabled.
        // When a shared_suggestion_queue is provided (from SuggestionManager), the receiver
        // uses it so SSE-received suggestions are visible in IPC queries.
        #[cfg(feature = "server")]
        let suggestion_receiver = if let Some(sse_client) = sse_client_opt {
            if self.config.suggestions.enabled {
                let queue = self.shared_suggestion_queue.unwrap_or_else(|| {
                    Arc::new(tokio::sync::Mutex::new(
                        maekon_suggestion::queue::SuggestionQueue::new(
                            self.config.analysis.max_suggestions,
                        ),
                    ))
                });
                let scorer = self.shared_scorer.unwrap_or_else(|| {
                    Arc::new(tokio::sync::Mutex::new(
                        maekon_suggestion::scorer::FeedbackScorer::new(),
                    ))
                });
                Some(Arc::new(
                    maekon_suggestion::receiver::SuggestionReceiver::new(
                        sse_client,
                        Some(notifier),
                        queue,
                        scorer,
                    ),
                ))
            } else {
                None
            }
        } else {
            None
        };
        #[cfg(not(feature = "server"))]
        let suggestion_receiver = None;

        let scheduler_config = SchedulerConfig {
            poll_interval: Duration::from_millis(self.config.monitor.poll_interval_ms),
            metrics_interval: Duration::from_secs(5),
            process_interval: Duration::from_secs(10),
            detailed_process_interval: Duration::from_secs(30),
            input_activity_interval: Duration::from_secs(30),
            sync_interval: Duration::from_millis(self.config.monitor.sync_interval_ms),
            heartbeat_interval: Duration::from_millis(self.config.monitor.heartbeat_interval_ms),
            aggregation_interval: Duration::from_secs(3600),
            session_id,
            external_data_policy: self.config.ai_provider.external_data_policy,
            privacy_config: self.config.privacy.clone(),
            idle_threshold_secs: 300,
            upload_enabled: self.config.monitor.upload_enabled,
            analysis_config: self.config.analysis.clone(),
            cross_device_sync_interval: Duration::from_secs(300), // 5 min default
        };

        // #6442 F10: surface an incoherent egress-privacy pairing (AllowFiltered + PII
        // filtering Off) ONCE, loudly, at config load — replacing #5992's silent per-call
        // upgrade warn with explicit user feedback. Egress proceeds safely: both the
        // window-title and OCR-image paths floor to Basic via
        // ExternalDataPolicy::effective_egress_pii_level.
        if scheduler_config.has_incoherent_egress_privacy() {
            tracing::error!(
                "Incoherent privacy config: external_data_policy=AllowFiltered with PII \
                 filter level Off. AllowFiltered means 'egress, but filter PII', so Off is \
                 contradictory. Egress (window titles AND OCR images) is filtered at the \
                 Basic floor; set a PII filter level of at least Basic, or change the data \
                 policy, to resolve this. (#6442 F10)"
            );
        }

        Ok(AgentSupportContext {
            frame_storage,
            system_monitor,
            process_monitor,
            activity_monitor,
            capture_trigger,
            frame_processor,
            accessibility_extractor,
            scheduler_config,
            batch_sink_opt,
            api_client_opt,
            feature_perf_sink_opt,
            notification_manager,
            focus_analyzer,
            context_analyzer,
            #[cfg(feature = "analysis")]
            context_analyzer_factory,
            suggestion_receiver,
        })
    }
}

/// #7652: the actual analyzer-construction rules, shared by both the startup
/// path (`AgentSupportContextBuilder::build_context_analyzer`) and the
/// runtime-rebuild factory (`context_analyzer_factory`). Takes `config`
/// explicitly (instead of reading `self.config`) so it can be called with
/// either the startup snapshot OR a later LIVE `ConfigManager` snapshot.
#[cfg(feature = "analysis")]
fn build_context_analyzer_sync(
    config: &AppConfig,
    deps: &ContextAnalyzerDeps,
) -> Option<Arc<maekon_analysis::ContextAnalyzer>> {
    if !config.analysis.enabled {
        return None;
    }

    let storage = match deps.storage.as_ref() {
        Some(s) => s.clone(),
        None => {
            tracing::warn!("analysis enabled but no storage available");
            return None;
        }
    };

    let analysis_provider: Arc<dyn maekon_core::ports::analysis_provider::AnalysisProvider> =
        if let Some((provider, _health)) =
            crate::agent_runtime::analysis_helpers::build_analysis_provider_with_flag(
                &config.ai_provider,
                config.privacy.pii_filter_level,
                Some(ExternalOcrPrivacyGuard::new(
                    deps.consent_manager.clone().unwrap_or_else(|| {
                        Arc::new(maekon_core::consent::ConsentManager::new(
                            deps.data_dir.join("consent.json"),
                        ))
                    }),
                    config.privacy.pii_filter_level,
                    config.ai_provider.external_data_policy,
                    config.privacy.clone(),
                    deps.process_monitor.clone(),
                    None,
                )),
                deps.provider_secret_stores.as_ref(),
                deps.analysis_health_flag.clone(),
                deps.breaker_registry.clone(),
            )
        {
            provider
        } else {
            tracing::warn!("analysis enabled but no LLM provider configured");
            return None;
        };

    let pattern_miner = maekon_analysis::PatternMiner::new();
    let pii_level = config.privacy.pii_filter_level;
    let context_assembler = maekon_analysis::ContextAssembler::new(Box::new(move |text| {
        maekon_vision::privacy::sanitize_title_with_level(text, pii_level)
    }));
    let few_shot_pii_filter: Box<dyn Fn(&str) -> String + Send + Sync> =
        Box::new(move |text| maekon_vision::privacy::sanitize_title_with_level(text, pii_level));

    Some(Arc::new(maekon_analysis::ContextAnalyzer::with_pii_filter(
        storage,
        analysis_provider,
        pattern_miner,
        context_assembler,
        config.analysis.clone(),
        few_shot_pii_filter,
    )))
}

/// #7652: async wrapper around `build_context_analyzer_sync` that also wires
/// few-shot storage (mirrors the two-step startup sequence in `build()`, but
/// self-contained since the runtime factory has no separate follow-up step).
#[cfg(feature = "analysis")]
async fn build_context_analyzer_async(
    config: &AppConfig,
    deps: &ContextAnalyzerDeps,
) -> Option<Arc<maekon_analysis::ContextAnalyzer>> {
    let analyzer = build_context_analyzer_sync(config, deps)?;
    if let Some(fs_storage) = deps.few_shot_storage.as_ref() {
        analyzer.set_few_shot_storage(fs_storage.clone()).await;
    }
    Some(analyzer)
}

#[cfg(feature = "server")]
fn server_transport_ports_for_mode(
    offline_mode: bool,
    config: &AppConfig,
    session_id: &str,
    config_manager: Option<ConfigManager>,
) -> Result<ServerTransportPorts> {
    if offline_mode {
        return Ok((None, None, None, None));
    }

    build_server_transports(config, session_id, config_manager)
}

#[cfg(not(feature = "server"))]
fn server_transport_ports_for_mode(
    _offline_mode: bool,
    config: &AppConfig,
    session_id: &str,
    config_manager: Option<ConfigManager>,
) -> Result<ServerTransportPorts> {
    build_server_transports(config, session_id, config_manager)
}

/// #7668: select the SSE transport for the suggestion stream based on the
/// resolved gRPC context mode.
///
/// `GrpcSseAdapter::connect` calls `UnifiedClient::subscribe_suggestions`,
/// which hard-errors immediately when `use_grpc_context` is false — the
/// shipped default (`GrpcConfig::default` in
/// maekon-core::config::sections::network). Before this fix, the `--features
/// grpc` build (the shipped build) always constructed `GrpcSseAdapter`
/// regardless of `use_grpc_context`, so with the default config the
/// suggestion SSE loop's escalating backoff / give-up / respawn cycle
/// (scheduler/loops/suggestions.rs) spun forever without ever delivering a
/// suggestion: the only SSE client ever constructed required gRPC context
/// that is never enabled. This function selects the REST `SseStreamClient`
/// fallback when gRPC context is disabled, using the same
/// TokenManager/TLS/retry config as the REST `ApiClient` path (non-grpc
/// branch below), so auth is identical.
///
/// Extracted as a standalone function (rather than inlined in
/// `build_server_transports`) so the selection itself is unit-testable
/// without needing the full server-transport wiring — see
/// `tests::grpc_disabled_selects_rest_sse_client_and_delivers_suggestion`.
#[cfg(feature = "grpc")]
fn select_sse_client(
    use_grpc_context: bool,
    unified: &Arc<UnifiedClient>,
    server_base_url: &str,
    token_manager: Arc<TokenManager>,
    sse_max_retry_secs: u64,
    tls: &maekon_core::config::TlsConfig,
) -> Result<SseClientPort> {
    if use_grpc_context {
        Ok(Arc::new(GrpcSseAdapter::new(unified.clone())) as SseClientPort)
    } else {
        let sse_stream =
            SseStreamClient::new_with_tls(server_base_url, token_manager, sse_max_retry_secs, tls)
                .map_err(|e| anyhow::anyhow!("failed to build SSE client: {e}"))?;
        Ok(Arc::new(sse_stream) as SseClientPort)
    }
}

#[cfg(feature = "server")]
fn build_server_transports(
    config: &AppConfig,
    session_id: &str,
    config_manager: Option<ConfigManager>,
) -> Result<ServerTransportPorts> {
    let token_manager = Arc::new(
        TokenManager::new_with_tls(
            &config.server.base_url,
            &config.tls,
            Some(config.request_timeout()),
        )
        .map_err(|e| anyhow::anyhow!("failed to build TLS-aware TokenManager: {e}"))?,
    );

    // #5069: clone the shared TokenManager for the feature-perf sink BEFORE the
    // transport branches consume it (the non-grpc branch moves it into the SSE
    // client). Same `TokenManager` ⇒ same bearer JWT.
    let token_manager_for_perf = token_manager.clone();

    #[cfg(feature = "grpc")]
    let (api_client, sse_client): (ApiClientPort, SseClientPort) = {
        let grpc_config =
            GrpcConfig::from_core_with_rest_tls(&config.grpc, &config.server.base_url, &config.tls);
        let unified = Arc::new(UnifiedClient::new(grpc_config, token_manager.clone())?);
        let http_fallback = HttpApiClient::new_with_tls(
            &config.server.base_url,
            token_manager.clone(),
            config.request_timeout(),
            &config.tls,
        )?;

        let sse_client = select_sse_client(
            config.grpc.use_grpc_context,
            &unified,
            &config.server.base_url,
            token_manager.clone(),
            config.server.sse_max_retry_secs,
            &config.tls,
        )?;

        (
            Arc::new(GrpcApiAdapter::new(unified, http_fallback)),
            sse_client,
        )
    };

    #[cfg(not(feature = "grpc"))]
    let (api_client, sse_client): (ApiClientPort, SseClientPort) = {
        let http_client = HttpApiClient::new_with_tls(
            &config.server.base_url,
            token_manager.clone(),
            config.request_timeout(),
            &config.tls,
        )?;
        let sse_stream = SseStreamClient::new_with_tls(
            &config.server.base_url,
            token_manager,
            config.server.sse_max_retry_secs,
            &config.tls,
        )
        .map_err(|e| anyhow::anyhow!("failed to build SSE client: {e}"))?;
        (Arc::new(http_client), Arc::new(sse_stream) as SseClientPort)
    };

    // Build the suppression predicate: uploads are allowed only inside an
    // enabled, configured tracking schedule window. Disabled/empty schedules
    // preserve unrestricted-by-schedule behavior.
    // Uses snapshot() (O(1) Arc-clone) instead of get() (deep-clone of 37 sections)
    // per CONS-PI13 — the predicate is called on every flush, so hot-path cost matters.
    let mut uploader = BatchUploader::new(api_client.clone(), session_id.to_string(), 100, 3);
    if let Some(mgr) = config_manager {
        let pred: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || !crate::scheduler::tracking_schedule_allows_capture(&mgr.snapshot()));
        uploader = uploader.with_suppression_predicate(pred);
    }
    let batch_uploader = Arc::new(uploader);

    // #5069: a dedicated REST `HttpApiClient` for the feature-performance emitter.
    // Built regardless of the gRPC switch (the perf contract is REST-only) and
    // sharing `token_manager` so it reuses the same bearer JWT. Coerced to
    // `FeaturePerfSink` (POST /api/v1/system/features/{key}/performance).
    let feature_perf_sink: FeaturePerfSinkPort = Arc::new(HttpApiClient::new_with_tls(
        &config.server.base_url,
        token_manager_for_perf,
        config.request_timeout(),
        &config.tls,
    )?);

    Ok((
        Some(batch_uploader),
        Some(api_client),
        Some(sse_client),
        Some(feature_perf_sink),
    ))
}

#[cfg(not(feature = "server"))]
fn build_server_transports(
    _config: &AppConfig,
    _session_id: &str,
    _config_manager: Option<ConfigManager>,
) -> Result<ServerTransportPorts> {
    Ok((None, None, None))
}

pub(crate) fn generate_session_id() -> String {
    use std::hash::{Hash, Hasher};

    let ts = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    let rand_part = hasher.finish() as u32;
    format!("sess_{ts}_{rand_part:08x}")
}

/// Notifier that bridges to Tauri's native notification plugin.
pub(crate) struct TauriNotifier {
    app_handle: tauri::AppHandle,
}

impl TauriNotifier {
    pub fn new(app_handle: tauri::AppHandle) -> Self {
        Self { app_handle }
    }
}

#[async_trait::async_trait]
impl maekon_core::ports::notifier::DesktopNotifier for TauriNotifier {
    async fn show_suggestion(
        &self,
        suggestion: &maekon_core::models::suggestion::Suggestion,
    ) -> Result<(), maekon_core::error::CoreError> {
        let title = match suggestion.priority {
            maekon_core::models::suggestion::Priority::Critical => "Maekon - Urgent",
            maekon_core::models::suggestion::Priority::High => "Maekon - Important",
            maekon_core::models::suggestion::Priority::Medium => "Maekon",
            maekon_core::models::suggestion::Priority::Low => "Maekon - Info",
        };
        let body = suggestion.content.chars().take(200).collect::<String>();
        if let Err(e) = tauri_plugin_notification::NotificationExt::notification(&self.app_handle)
            .builder()
            .title(title)
            .body(&body)
            .show()
        {
            tracing::warn!("native notification failed, suppressing: {e}");
        }
        Ok(())
    }

    async fn show_notification(
        &self,
        title: &str,
        body: &str,
    ) -> Result<(), maekon_core::error::CoreError> {
        if let Err(e) = tauri_plugin_notification::NotificationExt::notification(&self.app_handle)
            .builder()
            .title(title)
            .body(body)
            .show()
        {
            tracing::warn!("native notification failed, suppressing: {e}");
        }
        Ok(())
    }

    async fn show_error(&self, message: &str) -> Result<(), maekon_core::error::CoreError> {
        if let Err(e) = tauri_plugin_notification::NotificationExt::notification(&self.app_handle)
            .builder()
            .title("Maekon - Error")
            .body(message)
            .show()
        {
            tracing::warn!("native error notification failed, suppressing: {e}");
        }
        Ok(())
    }
}

/// Fallback notifier for headless/test mode.
struct LogOnlyNotifier;

#[async_trait::async_trait]
impl maekon_core::ports::notifier::DesktopNotifier for LogOnlyNotifier {
    async fn show_suggestion(
        &self,
        suggestion: &maekon_core::models::suggestion::Suggestion,
    ) -> Result<(), maekon_core::error::CoreError> {
        tracing::debug!(id = %suggestion.suggestion_id, "suggestion notification (headless mode)");
        Ok(())
    }

    async fn show_notification(
        &self,
        title: &str,
        body: &str,
    ) -> Result<(), maekon_core::error::CoreError> {
        // Log digests only — title and body may carry suggestion content derived
        // from the user's active window; the log file persists outside the
        // consent/erasure path (#6006).
        tracing::debug!(
            title = %maekon_monitor::log_privacy::title_digest(title),
            body = %maekon_monitor::log_privacy::content_digest(body),
            "notification (headless mode)"
        );
        Ok(())
    }

    async fn show_error(&self, message: &str) -> Result<(), maekon_core::error::CoreError> {
        tracing::debug!(message, "error notification (headless mode)");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_session_id_format() {
        let id = generate_session_id();
        assert!(id.starts_with("sess_"));
        assert!(id.len() > 20);
    }

    #[test]
    fn tauri_notifier_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TauriNotifier>();
        assert_send_sync::<LogOnlyNotifier>();
    }

    #[test]
    fn offline_mode_disables_server_transport_wiring() {
        let config = AppConfig::default_config();
        let ports =
            server_transport_ports_for_mode(true, &config, "sess_test_offline", None).unwrap();

        assert!(ports.0.is_none());
        assert!(ports.1.is_none());
        assert!(ports.2.is_none());
        #[cfg(feature = "server")]
        assert!(ports.3.is_none());
    }

    /// #7668 regression: with `use_grpc_context=false` (the shipped default),
    /// `select_sse_client` must pick the REST `SseStreamClient`, not the
    /// gRPC-only `GrpcSseAdapter`. Proven end-to-end: log in against a stub
    /// REST server, then confirm the *selected* client's `connect()` actually
    /// delivers a suggestion pushed over the REST SSE endpoint.
    ///
    /// Before the fix, `GrpcSseAdapter` was selected unconditionally in the
    /// `--features grpc` build. Its `connect()` calls
    /// `UnifiedClient::subscribe_suggestions`, which returns
    /// `Err(CoreError::Network { .. "Suggestion streaming is available only
    /// in gRPC mode. Set use_grpc_context=true." .. })` immediately — no
    /// request would ever reach the stub server below, so this test would
    /// time out waiting on `rx.recv()` and fail (fails-before evidence).
    #[cfg(feature = "grpc")]
    #[tokio::test]
    async fn grpc_disabled_selects_rest_sse_client_and_delivers_suggestion() {
        use maekon_core::config::TlsConfig;
        use maekon_core::ports::api_client::SseEvent;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://127.0.0.1:{}", addr.port());

        // Stub server: 1) respond to the login POST with a valid token, 2)
        // respond to the SSE GET with a single `suggestion` event. Mirrors the
        // stub-server pattern in maekon-network::sse_client::tests.
        let server_task = tokio::spawn(async move {
            // login (POST /api/v1/auth/tokens)
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let body = r#"{"access_token":"tok","refresh_token":"ref","expires_in":3600}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            }
            // SSE stream (GET /user_context/sessions/stream) → one suggestion event
            if let Ok((mut socket, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let suggestion_json = r#"{"suggestion_id":"sug_7668","suggestion_type":"WORK_GUIDANCE","content":"REST-SSE fallback delivered","priority":"HIGH","confidence_score":0.9,"relevance_score":0.9,"is_actionable":true,"created_at":"2026-01-28T10:00:00Z"}"#;
                let sse_body = format!("event: suggestion\ndata: {suggestion_json}\n\n");
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{sse_body}"
                );
                let _ = socket.write_all(resp.as_bytes()).await;
                let _ = socket.flush().await;
            }
        });

        let tls = TlsConfig {
            enabled: false,
            allow_self_signed: false,
        };
        let token_manager = Arc::new(
            TokenManager::new_with_tls(&base, &tls, None)
                .expect("TokenManager must build for a loopback http base_url"),
        );
        // Runtime-built password fixture — a string literal at the `login()`
        // call site trips CodeQL `rust/hard-coded-cryptographic-value`;
        // mirrors `maekon_network::sse_client::tests::primary_password`.
        let password = String::from_utf8(vec![b'x'; 16]).expect("password fixture must be UTF-8");
        token_manager
            .login("user@example.com", &password)
            .await
            .expect("login against the stub REST server must succeed");

        // A minimal UnifiedClient — required by `select_sse_client`'s signature
        // even though the REST branch never touches it. Construction performs
        // no network I/O.
        let unified = Arc::new(
            UnifiedClient::new(GrpcConfig::default(), token_manager.clone())
                .expect("UnifiedClient must build without network I/O"),
        );

        let sse_client = select_sse_client(
            false, // use_grpc_context — the shipped default
            &unified,
            &base,
            token_manager.clone(),
            30,
            &tls,
        )
        .expect("select_sse_client must build the REST fallback when use_grpc_context is false");

        let (tx, mut rx) = tokio::sync::mpsc::channel::<SseEvent>(8);
        let connect_task = tokio::spawn(async move {
            let _ = sse_client.connect("sess_7668_fallback", tx).await;
        });

        let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect(
                "REST SSE fallback must deliver an event before the timeout — the pre-fix \
                 GrpcSseAdapter selection would fail immediately with 'Suggestion streaming is \
                 available only in gRPC mode' instead of ever reaching this stub server",
            )
            .expect("event channel must not close before the suggestion arrives");

        assert!(
            matches!(event, SseEvent::Suggestion(ref s) if s.suggestion_id == "sug_7668"),
            "expected a Suggestion event delivered via the REST SseStreamClient fallback, got: {event:?}"
        );

        connect_task.abort();
        server_task.abort();
    }
}
