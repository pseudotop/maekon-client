// OOS-TBD: ADR-013 file split — baselined past the 900-line giant
// threshold while growing for #9688; split per ADR-003 when next touched.
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
    /// Runtime shutdown signal, used to terminate the notification config
    /// watcher. Sender-drop cannot serve as its exit condition: the app keeps
    /// `ConfigManager` clones alive for the whole process (Tauri managed state
    /// among them), so the watcher must be told to stop.
    shutdown_rx: Option<tokio::sync::watch::Receiver<bool>>,
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
    /// #9459: the ONE shared `TokenManager` from the composition root — already
    /// keychain-restored and registered in the `TokenManagerState` IPC slot.
    /// `build_server_transports` adopts it so the upload/REST/SSE transports and
    /// the login command operate on a single session. `None` (standalone use, or
    /// a failed construction upstream) keeps the pre-#9459 local construction.
    #[cfg(feature = "server")]
    shared_token_manager: Option<Arc<TokenManager>>,
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
            shutdown_rx: None,
            consent_manager: None,
            offline_mode: false,
            breaker_registry: crate::breaker_registry::CircuitBreakerRegistry::new(),
            #[cfg(feature = "analysis")]
            provider_secret_stores: None,
            #[cfg(feature = "server")]
            shared_token_manager: None,
        }
    }

    /// #9459: inject the composition root's single shared `TokenManager` so the
    /// server transports built in [`build`] reuse the session the login IPC
    /// writes to, instead of constructing a second one that never sees a bearer
    /// token. `None` preserves the pre-#9459 local construction.
    ///
    /// [`build`]: AgentSupportContextBuilder::build
    #[cfg(feature = "server")]
    pub(crate) fn with_shared_token_manager(mut self, manager: Option<Arc<TokenManager>>) -> Self {
        self.shared_token_manager = manager;
        self
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

    pub(crate) fn with_shutdown_rx(mut self, rx: tokio::sync::watch::Receiver<bool>) -> Self {
        self.shutdown_rx = Some(rx);
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
        // #9639: keep a handle for the notification master-switch gate below
        // (config_manager itself is consumed by the transport wiring).
        let notifier_config_manager = config_manager.clone();
        #[cfg(feature = "server")]
        let (batch_sink_opt, api_client_opt, sse_client_opt, feature_perf_sink_opt) =
            server_transport_ports_for_mode(
                self.offline_mode,
                self.config,
                &session_id,
                config_manager,
                self.shared_token_manager.take(),
            )?;
        #[cfg(not(feature = "server"))]
        let (batch_sink_opt, api_client_opt, feature_perf_sink_opt) =
            server_transport_ports_for_mode(
                self.offline_mode,
                self.config,
                &session_id,
                config_manager,
            )?;

        let raw_notifier: Arc<dyn maekon_core::ports::notifier::DesktopNotifier> =
            if let Some(handle) = self.app_handle.clone() {
                Arc::new(TauriNotifier::new(handle))
            } else {
                Arc::new(LogOnlyNotifier)
            };
        // #9639: every injected-notifier consumer goes through the
        // notification.enabled master switch (see GatedNotifier). Without a
        // config manager (minimal/test wiring) the raw notifier stands.
        let notifier: Arc<dyn maekon_core::ports::notifier::DesktopNotifier> =
            match notifier_config_manager.clone() {
                Some(mgr) => Arc::new(GatedNotifier::new(raw_notifier, mgr)),
                None => raw_notifier,
            };
        let notification_manager = Arc::new(NotificationManager::new(
            self.config.notification.clone(),
            notifier.clone(),
        ));
        // #9639 follow-up: without this the manager keeps its BOOT config, so
        // enabling notifications after launch stayed invisible until a restart.
        // The watcher exits on the runtime shutdown signal — see
        // `spawn_notification_config_watcher` for why sender-drop cannot be its
        // exit condition here.
        if let Some(mgr) = notifier_config_manager {
            crate::notification_manager::spawn_notification_config_watcher(
                notification_manager.clone(),
                mgr,
                self.shutdown_rx.clone(),
            );
        }
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
                        // #10112: local storage is the client's system of record.
                        // Without this the receiver wrote only to the in-memory
                        // queue and server-pushed suggestions were lost on
                        // restart. `AgentRuntime::run` always calls
                        // `.with_storage(...)`, so this is Some in production.
                        self.storage.clone(),
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
    shared_token_manager: Option<Arc<TokenManager>>,
) -> Result<ServerTransportPorts> {
    if offline_mode {
        return Ok((None, None, None, None));
    }

    build_server_transports(config, session_id, config_manager, shared_token_manager)
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
) -> Option<SseClientPort> {
    if use_grpc_context {
        return Some(Arc::new(GrpcSseAdapter::new(unified.clone())) as SseClientPort);
    }
    // #10969: a transport that cannot be constructed degrades, it does not take
    // the runtime with it. This used to return `Err`, which `?` propagated all
    // the way out of `AgentRuntimeBundle::run()` — so monitoring, analysis and
    // suggestions all died because one optional stream could not be built. The
    // shipped defaults guarantee that failure (`base_url` is cleartext
    // `http://localhost:8000` while `TlsConfig::enabled` is true), so every
    // fresh install lost its agent while the desktop shell kept working and
    // looked healthy.
    //
    // `ServerTransportPorts` already models this as `Option<SseClientPort>`;
    // only the construction path disagreed. Mirrors the embedding fallback in
    // the same startup sequence, which warns and continues.
    match SseStreamClient::new_with_tls(server_base_url, token_manager, sse_max_retry_secs, tls) {
        Ok(sse_stream) => Some(Arc::new(sse_stream) as SseClientPort),
        Err(error) => {
            tracing::warn!(
                %error,
                server_base_url,
                "SSE client unavailable — continuing without server push (server-driven \
                 suggestions and context updates are degraded)"
            );
            None
        }
    }
}

#[cfg(feature = "server")]
fn build_server_transports(
    config: &AppConfig,
    session_id: &str,
    config_manager: Option<ConfigManager>,
    shared_token_manager: Option<Arc<TokenManager>>,
) -> Result<ServerTransportPorts> {
    // #9459: adopt the composition root's shared session when present. Building
    // a second manager here is what used to strand the login token: the IPC
    // `TokenManagerState` slot held one manager while every transport below
    // authenticated with another that no one ever signed in. The local
    // construction survives only as the fallback for a `None` upstream (a
    // `[tls]` config error there) and for standalone/test use.
    let token_manager = match shared_token_manager {
        Some(manager) => manager,
        None => Arc::new(
            TokenManager::new_with_tls(
                &config.server.base_url,
                &config.tls,
                Some(config.request_timeout()),
            )
            .map_err(|e| anyhow::anyhow!("failed to build TLS-aware TokenManager: {e}"))?,
        ),
    };

    // #5069: clone the shared TokenManager for the feature-perf sink BEFORE the
    // transport branches consume it (the non-grpc branch moves it into the SSE
    // client). Same `TokenManager` ⇒ same bearer JWT.
    let token_manager_for_perf = token_manager.clone();

    #[cfg(feature = "grpc")]
    let (api_client, sse_client): (ApiClientPort, Option<SseClientPort>) = {
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
        );

        (
            Arc::new(GrpcApiAdapter::new(unified, http_fallback)),
            sse_client,
        )
    };

    #[cfg(not(feature = "grpc"))]
    let (api_client, sse_client): (ApiClientPort, Option<SseClientPort>) = {
        let http_client = HttpApiClient::new_with_tls(
            &config.server.base_url,
            token_manager.clone(),
            config.request_timeout(),
            &config.tls,
        )?;
        // #10969: same degradation as the grpc branch above — an unbuildable
        // SSE stream must not abort the whole agent runtime.
        let sse_client = match SseStreamClient::new_with_tls(
            &config.server.base_url,
            token_manager,
            config.server.sse_max_retry_secs,
            &config.tls,
        ) {
            Ok(sse_stream) => Some(Arc::new(sse_stream) as SseClientPort),
            Err(error) => {
                tracing::warn!(
                    %error,
                    server_base_url = %config.server.base_url,
                    "SSE client unavailable — continuing without server push (server-driven \
                     suggestions and context updates are degraded)"
                );
                None
            }
        };
        (Arc::new(http_client), sse_client)
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

    // `sse_client` is already an Option (#10969): `None` means the stream could
    // not be built and the runtime continues without server push.
    Ok((
        Some(batch_uploader),
        Some(api_client),
        sse_client,
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

/// #9639: master-switch gate in front of ANY injected notifier.
///
/// `NotificationManager` honors `notification.enabled`, but four focus-
/// analyzer toasts (and any future direct consumer of the injected
/// `DesktopNotifier`) called the port directly and bypassed the switch —
/// turning notifications OFF in settings did not silence them. Gating at the
/// composition root covers every consumer without touching adapter crates.
/// NOTE: the launch path constructs its own FocusAnalyzer and overwrites the
/// support builder's instance, so it wraps its notifier with this gate too
/// (#9671 review B2) — a new injection site must do the same.
///
/// Suppression returns an ERROR (`service.unavailable`), not `Ok(())`
/// (#9671 review I2): every caller treats `Ok` as "shown" and commits state
/// on it (cooldown stamps, `mark_suggestion_shown_by_id`) — a silent `Ok`
/// would record deliveries that never happened. Two consequences of routing
/// a PERMANENT setting through the transient-failure paths (#9671 re-review
/// N1/N2, both accepted): callers other than `NotificationManager` log the
/// suppression at `warn!`, and the pattern-detected toast takes its
/// retry branch once per pattern before the save-dedup path re-applies the
/// cooldown — bounded, not a hot loop. The live-config read makes
/// OFF take effect immediately; ON-after-boot additionally depends on each
/// caller's own config handling (NotificationManager keeps a boot snapshot).
pub(crate) struct GatedNotifier {
    inner: Arc<dyn maekon_core::ports::notifier::DesktopNotifier>,
    config_manager: maekon_core::config_manager::ConfigManager,
}

impl GatedNotifier {
    pub fn new(
        inner: Arc<dyn maekon_core::ports::notifier::DesktopNotifier>,
        config_manager: maekon_core::config_manager::ConfigManager,
    ) -> Self {
        Self {
            inner,
            config_manager,
        }
    }

    fn enabled(&self) -> bool {
        // snapshot() = Arc borrow, no deep clone (this runs per notification).
        self.config_manager.snapshot().notification.enabled
    }

    fn suppressed() -> maekon_core::error::CoreError {
        maekon_core::error::CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "notifications disabled by user setting (notification.enabled=false)"
                .to_string(),
        }
    }
}

#[async_trait::async_trait]
impl maekon_core::ports::notifier::DesktopNotifier for GatedNotifier {
    async fn show_suggestion(
        &self,
        suggestion: &maekon_core::models::suggestion::Suggestion,
    ) -> Result<(), maekon_core::error::CoreError> {
        if !self.enabled() {
            return Err(Self::suppressed());
        }
        self.inner.show_suggestion(suggestion).await
    }

    async fn show_notification(
        &self,
        title: &str,
        body: &str,
    ) -> Result<(), maekon_core::error::CoreError> {
        if !self.enabled() {
            return Err(Self::suppressed());
        }
        self.inner.show_notification(title, body).await
    }

    async fn show_error(&self, message: &str) -> Result<(), maekon_core::error::CoreError> {
        if !self.enabled() {
            return Err(Self::suppressed());
        }
        self.inner.show_error(message).await
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
        if let Err(e) = crate::windows_notification_activation::show_actionable_notification(
            &self.app_handle,
            title,
            &body,
            crate::windows_notification_activation::DEFAULT_NOTIFICATION_ROUTE,
        ) {
            tracing::warn!("native notification failed, suppressing: {e}");
        }
        Ok(())
    }

    async fn show_notification(
        &self,
        title: &str,
        body: &str,
    ) -> Result<(), maekon_core::error::CoreError> {
        if let Err(e) = crate::windows_notification_activation::show_actionable_notification(
            &self.app_handle,
            title,
            body,
            crate::windows_notification_activation::DEFAULT_NOTIFICATION_ROUTE,
        ) {
            tracing::warn!("native notification failed, suppressing: {e}");
        }
        Ok(())
    }

    async fn show_error(&self, message: &str) -> Result<(), maekon_core::error::CoreError> {
        if let Err(e) = crate::windows_notification_activation::show_actionable_notification(
            &self.app_handle,
            "Maekon - Error",
            message,
            "/audit/entries",
        ) {
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
mod tests;
