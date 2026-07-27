// OOS-TBD: ADR-013 file split — crossed 900 LOC with the #7946 re-prime
// helper + its tests; split candidates: sync/heartbeat loop separation.
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};

use super::super::Scheduler;
use crate::scheduler::egress_policy::PlatformEgressPolicy;
// Only referenced by the #[cfg(test)] helper/ctors below (SchedulerRequiredDeps
// is a test-construction payload type, never touched by the loop-spawning code
// in this file's non-test scope) — gate the import to avoid an unused-import
// error in non-test builds.
#[cfg(test)]
use super::super::SchedulerRequiredDeps;

/// Fixed, code-defined upload-channel authority label for the batch-upload
/// metrics (E20-43 #4835). Cardinality = 1 and contains NO user-identifying
/// data; it names the upload channel, never a user/host derived from input.
const BATCH_UPLOAD_AUTHORITY: &str = "batch-sink";

/// Upper bound on events re-primed into the upload queue per startup (#7946).
/// Keeps a pathological backlog from stalling loop start; the remainder is
/// picked up by subsequent restarts (or ages out via retention).
const REPRIME_LIMIT: usize = 1_000;

/// Re-prime persisted-but-unsent events into the upload queue at sync-loop
/// start (#7946). Applies the egress policy to each pending event exactly like
/// the live producers do; events the policy blocks NOW are retired via
/// `mark_as_sent` (the `is_sent` column's only consumer is the pending query,
/// their egress disposition was already ledgered at the original attempt, and
/// leaving them pending would re-prime them forever). Returns (enqueued,
/// retired).
async fn reprime_pending_uploads(
    storage: &std::sync::Arc<dyn maekon_core::ports::storage::StorageService>,
    sink: &std::sync::Arc<dyn maekon_core::ports::batch_sink::BatchSink>,
    egress: &PlatformEgressPolicy,
) -> (usize, usize) {
    let pending = match storage.get_pending_events(REPRIME_LIMIT).await {
        Ok(pending) => pending,
        Err(e) => {
            warn!(err.code = %e.code(), "re-prime: pending-event load failed: {e}");
            return (0, 0);
        }
    };
    if pending.is_empty() {
        return (0, 0);
    }

    let mut enqueued = 0usize;
    let mut retired: Vec<String> = Vec::new();
    for event in pending {
        // The storage id MUST come from the original persisted event —
        // egress filtering can change id-relevant fields (#7946).
        let storage_id = maekon_storage::sqlite::storage_event_id(&event);
        match egress.prepare_event_for_upload(event) {
            Some(upload_event) => {
                sink.enqueue(maekon_core::ports::batch_sink::QueuedUpload {
                    storage_id,
                    event: upload_event,
                });
                enqueued += 1;
            }
            None => retired.push(storage_id),
        }
    }
    if !retired.is_empty() {
        if let Err(e) = storage.mark_as_sent(&retired).await {
            warn!(err.code = %e.code(), "re-prime: retire of policy-blocked events failed: {e}");
        }
    }
    (enqueued, retired.len())
}

impl Scheduler {
    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_sync_loop(
        &self,
        sync_interval: Duration,
        egress_policy: Arc<PlatformEgressPolicy>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let uploader4 = self.batch_sink.clone();
        let storage4 = self.storage.clone();
        let frame_storage4 = self.frame_storage.clone();
        let egress4 = egress_policy;
        // #8050: a successful batch upload definitively proves the server is
        // reachable, so it is a valid POSITIVE confirmation for the server-health
        // flag. It is a secondary signal to the heartbeat: only successes are
        // recorded here. Upload failures are deliberately NOT written — a flush
        // error can be data/validation-specific (not a connectivity fault) and
        // would flap against the heartbeat, which owns the authoritative negative
        // signal.
        let server_health_flag = self.server_health_flag.clone();

        tokio::spawn(async move {
            let mut interval = super::intervals::coalescing_interval(sync_interval);

            // #7946: re-prime persisted-but-unsent events into the upload queue.
            // Events dropped from the in-memory queue (drop-oldest) or left
            // unflushed at last shutdown keep is_sent=0 in storage; without
            // this they were silently lost (the old time-based bulk stamp
            // marked them sent without ever uploading them).
            if let Some(ref sink) = uploader4 {
                if egress4.is_enabled() {
                    let (enqueued, retired) =
                        reprime_pending_uploads(&storage4, sink, &egress4).await;
                    if enqueued > 0 || retired > 0 {
                        info!(enqueued, retired, "upload spool re-primed from storage");
                    }
                }
            }

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if let Some(ref sink) = uploader4 {
                            if egress4.is_enabled() {
                                match sink.flush().await {
                                    Ok(sent_ids) => {
                                        if !sent_ids.is_empty() {
                                            let count = sent_ids.len();
                                            debug!("batch: {count}items sent");
                                            // #8050: a non-empty upload confirms
                                            // the server is reachable — positive
                                            // health confirmation only (see the
                                            // `server_health_flag` note at the top
                                            // of this loop for why failures are
                                            // left to the heartbeat).
                                            if let Some(ref flag) = server_health_flag {
                                                flag.store(
                                                    true,
                                                    std::sync::atomic::Ordering::Relaxed,
                                                );
                                            }
                                            // E20-43 #4835: NON-PII upload outcome. Label is the
                                            // bounded, code-defined upload-channel authority only —
                                            // no per-user/per-event data. The `BatchSink` port does
                                            // not surface the concrete server host, so we record the
                                            // fixed channel authority (cardinality = 1) here.
                                            crate::telemetry::metrics::record_batch_upload_success(
                                                BATCH_UPLOAD_AUTHORITY,
                                            );
                                            // #7946: mark EXACTLY the uploaded rows as sent —
                                            // never a time-based bulk stamp (it falsely retired
                                            // dropped/unflushed events).
                                            if let Err(e) = storage4.mark_as_sent(&sent_ids).await {
                                                warn!(err.code = %e.code(), "mark sent failure: {e}");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        crate::telemetry::metrics::record_batch_upload_failure(
                                            BATCH_UPLOAD_AUTHORITY,
                                        );
                                        warn!(err.code = %e.code(), "batch failure: {e}");
                                    }
                                }
                            }
                        }

                        // Log events dropped during this flush cycle
                        if let Some(ref sink) = uploader4 {
                            let dropped = sink.take_dropped_since_last();
                            if dropped > 0 {
                                warn!(count = dropped, "events dropped during flush cycle");
                            }
                        }

                        if let Err(e) = storage4.enforce_retention().await {
                            warn!(err.code = %e.code(), "event policy failure: {e}");
                        }

                        if let Some(ref fs) = frame_storage4 {
                            if let Err(e) = fs.enforce_retention().await {
                                warn!(err.code = %e.code(), "frame policy failure: {e}");
                            }
                            if let Err(e) = fs.enforce_storage_limit().await {
                                warn!(err.code = %e.code(), "frame failure: {e}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        // Flush remaining events before shutdown
                        if let Some(ref sink) = uploader4 {
                            if egress4.is_enabled() {
                                loop {
                                    match sink.flush().await {
                                        Ok(sent_ids) if sent_ids.is_empty() => break,
                                        Ok(sent_ids) => {
                                            info!("shutdown flush: {} events sent", sent_ids.len());
                                            // #7946: precise marking on the shutdown path too —
                                            // unmarked-but-uploaded events would re-prime next
                                            // start (accepted at-least-once duplicate).
                                            if let Err(e) = storage4.mark_as_sent(&sent_ids).await {
                                                warn!(err.code = %e.code(), "shutdown mark sent failure: {e}");
                                            }
                                        }
                                        Err(e) => {
                                            warn!(err.code = %e.code(), "shutdown flush failed: {e}");
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        info!("ended");
                        break;
                    }
                }
            }
        })
    }

    #[tracing::instrument(skip_all)]
    pub(in crate::scheduler) fn spawn_heartbeat_loop(
        &self,
        heartbeat_interval: Duration,
        session_id: String,
        egress_policy: Arc<PlatformEgressPolicy>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let api = self.api_client.clone();
        let sid = session_id;
        // #8050: the heartbeat is the authoritative "server reachable" signal —
        // a dedicated periodic liveness RPC that runs every interval independent
        // of whether there is user data to upload, so its success/failure is the
        // most direct and consistent proxy for server connectivity. Store the
        // outcome so the health-check loop can surface it in the tray.
        let server_health_flag = self.server_health_flag.clone();

        tokio::spawn(async move {
            let api = match api {
                Some(a) => a,
                None => {
                    let _ = shutdown_rx.changed().await;
                    return;
                }
            };

            let mut interval = super::intervals::coalescing_interval(heartbeat_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if egress_policy.is_enabled() {
                            // E20-43 #4835: NON-PII liveness signal. `record_*` is a
                            // no-op when the telemetry feature is off and a noop-meter
                            // drop when telemetry.enabled is false at runtime, so this
                            // is safe after the egress consent gate passes.
                            crate::telemetry::metrics::record_heartbeat();
                            crate::telemetry::metrics::record_loop_iteration("heartbeat");
                            let outcome = api.send_heartbeat(&sid).await;
                            if let Some(ref flag) = server_health_flag {
                                // #8050: authoritative server-connectivity write —
                                // `true` on a reachable server, `false` on a failed
                                // heartbeat (both directions).
                                flag.store(outcome.is_ok(), std::sync::atomic::Ordering::Relaxed);
                            }
                            if let Err(e) = outcome {
                                warn!(err.code = %e.code(), "heartbeat failure: {e}");
                            }
                        }
                    }
                    _ = shutdown_rx.changed() => {
                        info!("heartbeat ended");
                        break;
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::SchedulerConfig;
    use async_trait::async_trait;
    use chrono::Utc;
    use chrono::{Datelike, Duration as ChronoDuration};
    use maekon_core::config::{
        ExternalDataPolicy, TrackingScheduleConfig, TrackingWindow, Weekday,
    };
    use maekon_core::config_manager::ConfigManager;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::error::CoreError;
    use maekon_core::error_codes::InternalCode;
    use maekon_core::models::context::{ProcessInfo, UserContext, WindowInfo};
    use maekon_core::models::event::{Event, EventBatch, ProcessDetail};
    use maekon_core::models::frame::ProcessedFrame;
    use maekon_core::models::suggestion::SuggestionFeedback;
    use maekon_core::models::system::SystemMetrics;
    use maekon_core::models::tiered_memory::{Regime, RegimeFeatures, RegimeStatus, TriggerParams};
    use maekon_core::ports::api_client::{ApiClient, SessionCreateResponse};
    use maekon_core::ports::batch_sink::BatchSink;
    use maekon_core::ports::monitor::{ActivityMonitor, ProcessMonitor, SystemMonitor};
    use maekon_core::ports::regime_storage::RegimeStoragePort;
    use maekon_core::ports::vision::{CaptureRequest, CaptureTrigger, FrameProcessor};
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::watch;

    fn unused_port_error(port: &str) -> CoreError {
        CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("{port} test double should not be called"),
        }
    }

    struct NoopSystemMonitor;

    #[async_trait]
    impl SystemMonitor for NoopSystemMonitor {
        async fn collect_metrics(&self) -> Result<SystemMetrics, CoreError> {
            Err(unused_port_error("system monitor"))
        }
    }

    struct NoopActivityMonitor;

    #[async_trait]
    impl ActivityMonitor for NoopActivityMonitor {
        async fn collect_context(&self) -> Result<UserContext, CoreError> {
            Err(unused_port_error("activity monitor"))
        }
    }

    struct NoopProcessMonitor;

    #[async_trait]
    impl ProcessMonitor for NoopProcessMonitor {
        async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
            Ok(None)
        }

        async fn get_top_processes(&self, _limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
            Ok(Vec::new())
        }

        async fn get_detailed_processes(
            &self,
            _foreground_pid: Option<u32>,
            _top_n: usize,
        ) -> Result<Vec<ProcessDetail>, CoreError> {
            Ok(Vec::new())
        }
    }

    struct NoopCaptureTrigger;

    impl CaptureTrigger for NoopCaptureTrigger {
        fn should_capture(
            &self,
            _event: &maekon_core::models::event::ContextEvent,
        ) -> Option<CaptureRequest> {
            None
        }
    }

    struct NoopFrameProcessor;

    #[async_trait]
    impl FrameProcessor for NoopFrameProcessor {
        async fn capture_and_process(
            &self,
            _capture_request: &CaptureRequest,
        ) -> Result<ProcessedFrame, CoreError> {
            Err(unused_port_error("frame processor"))
        }
    }

    #[derive(Default)]
    struct CountingApiClient {
        heartbeats: AtomicUsize,
        /// #8050: when set, `send_heartbeat` returns `Err` so the server-health
        /// write path can be exercised in both directions. Defaults to `false`
        /// (Ok), keeping every existing test unchanged.
        fail_heartbeat: std::sync::atomic::AtomicBool,
    }

    #[derive(Default)]
    struct CountingBatchSink {
        flushes: AtomicUsize,
    }

    #[async_trait]
    impl BatchSink for CountingBatchSink {
        fn enqueue(&self, _item: maekon_core::ports::batch_sink::QueuedUpload) {}

        fn enqueue_many(&self, _items: Vec<maekon_core::ports::batch_sink::QueuedUpload>) {}

        async fn flush(&self) -> Result<Vec<String>, CoreError> {
            self.flushes.fetch_add(1, Ordering::SeqCst);
            Ok(vec!["counting-sink-evt".to_string()])
        }
    }

    #[async_trait]
    impl ApiClient for CountingApiClient {
        async fn create_session(
            &self,
            _client_id: &str,
        ) -> Result<SessionCreateResponse, CoreError> {
            Err(unused_port_error("create_session"))
        }

        async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
            Err(unused_port_error("end_session"))
        }

        async fn upload_batch(&self, _batch: &EventBatch) -> Result<(), CoreError> {
            Err(unused_port_error("upload_batch"))
        }

        async fn send_feedback(&self, _feedback: &SuggestionFeedback) -> Result<(), CoreError> {
            Err(unused_port_error("send_feedback"))
        }

        async fn send_heartbeat(&self, _session_id: &str) -> Result<(), CoreError> {
            self.heartbeats.fetch_add(1, Ordering::SeqCst);
            if self.fail_heartbeat.load(Ordering::SeqCst) {
                return Err(CoreError::Internal {
                    code: InternalCode::Generic,
                    message: "heartbeat: injected failure".to_string(),
                });
            }
            Ok(())
        }
    }

    /// #7737 C1 PR-2: shared literal-construction helper for the 3 test ctors
    /// below that build `Scheduler` directly — every `SchedulerRequiredDeps`
    /// field is non-Option (no `Default`/`..` escape on that struct, by
    /// design), so each needs all 8 fields filled with a cheap-but-real
    /// value. Storage-derived ports reuse the SAME in-memory `SqliteStorage`
    /// Arc (Port Instance Sharing, mirrors the production wiring in
    /// `agent_runtime/mod.rs`); `notification_manager`/`focus_analyzer`/
    /// `belief_revision` get manual stub notifiers/providers (project
    /// convention: no mockall). Mirrors `maekon-web`'s own
    /// `full_required_deps` test helper (#7738 D-2).
    fn minimal_required_deps(storage: &Arc<SqliteStorage>) -> SchedulerRequiredDeps {
        use maekon_core::models::suggestion::Suggestion;
        use maekon_core::ports::calibration_store::CalibrationReader;
        use maekon_core::ports::focus_storage::FocusStorage;
        use maekon_core::ports::frame_storage::FrameStoragePort;
        use maekon_core::ports::memory_graph_port::MemoryGraphPort;
        use maekon_core::ports::notifier::DesktopNotifier;
        use std::path::{Path, PathBuf};

        struct NoopNotifier;
        #[async_trait]
        impl DesktopNotifier for NoopNotifier {
            async fn show_suggestion(&self, _suggestion: &Suggestion) -> Result<(), CoreError> {
                Ok(())
            }

            async fn show_notification(&self, _title: &str, _body: &str) -> Result<(), CoreError> {
                Ok(())
            }

            async fn show_error(&self, _message: &str) -> Result<(), CoreError> {
                Ok(())
            }
        }

        struct NoopFrameStorage;
        #[async_trait]
        impl FrameStoragePort for NoopFrameStorage {
            async fn save_frame(
                &self,
                _timestamp: chrono::DateTime<Utc>,
                _data: &[u8],
            ) -> Result<PathBuf, CoreError> {
                Err(unused_port_error("frame storage"))
            }

            async fn save_frames_batch(
                &self,
                _frames: Vec<(chrono::DateTime<Utc>, Vec<u8>)>,
            ) -> Vec<Result<PathBuf, CoreError>> {
                Vec::new()
            }

            async fn load_frame(&self, _relative_path: &Path) -> Result<Vec<u8>, CoreError> {
                Err(unused_port_error("frame storage"))
            }

            async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, CoreError> {
                Ok(None)
            }

            async fn enforce_retention(&self) -> Result<usize, CoreError> {
                Ok(0)
            }

            async fn enforce_storage_limit(&self) -> Result<usize, CoreError> {
                Ok(0)
            }

            async fn delete_all_frames(&self) -> Result<usize, CoreError> {
                Ok(0)
            }
        }

        let notifier: Arc<dyn DesktopNotifier> = Arc::new(NoopNotifier);
        let notification_manager = Arc::new(crate::notification_manager::NotificationManager::new(
            maekon_core::config::NotificationConfig::default(),
            notifier.clone(),
        ));
        let focus_analyzer = Arc::new(
            maekon_analysis::focus_analyzer::FocusAnalyzer::with_defaults(
                storage.clone() as Arc<dyn FocusStorage>,
                notifier,
            ),
        );
        // #7737: intentionally leaked test-only tempdir (`.keep()`, not the
        // default drop-deletes `TempDir` guard) — `ConfigManager` caches its
        // config in memory after construction (only `update_with` touches
        // disk again), but nothing in this test module calls that, so the
        // leak is a no-op in practice; keeping the backing file alive avoids
        // any risk of a stray re-read racing the guard's drop.
        let config_dir = tempfile::tempdir().expect("tempdir").keep();
        let config_manager =
            ConfigManager::with_path(config_dir.join("config.json")).expect("ConfigManager");
        let belief_pii_filter: maekon_analysis::BeliefPiiFilter =
            Arc::new(|text: &str| text.to_string());
        let belief_revision = Arc::new(maekon_analysis::BeliefRevision::new(
            Arc::new(maekon_analysis::NoOpAnalysisProvider),
            storage.clone() as Arc<dyn MemoryGraphPort>,
            belief_pii_filter,
            0.9,
            false,
        ));

        SchedulerRequiredDeps {
            frame_storage: Arc::new(NoopFrameStorage),
            notification_manager,
            focus_analyzer,
            config_manager,
            memory_graph: storage.clone() as Arc<dyn MemoryGraphPort>,
            calibration_reader: storage.clone() as Arc<dyn CalibrationReader>,
            belief_revision,
            regime_storage: Arc::new(
                maekon_storage::regime_manager_state_store::SqliteRegimeManagerStateStore::new(
                    storage.connection_arc(),
                ),
            ) as Arc<dyn RegimeStoragePort>,
        }
    }

    #[test]
    fn with_required_deps_populates_all_eight_fields_and_shares_arcs() {
        // #7737 C1 PR-2: standalone regression coverage for
        // `with_required_deps` in isolation (the 3 ctors above/below also
        // exercise it, but only implicitly through their own loop-behavior
        // assertions). Mirrors #7738 D-2's
        // `memory_graph_binding_is_applied_to_core_state` — the SAME Arc
        // (not a copy) must land on the Scheduler (Port Instance Sharing).
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let deps = minimal_required_deps(&storage);
        let expected_memory_graph = deps.memory_graph.clone();
        let expected_regime_storage = deps.regime_storage.clone();
        let scheduler = Scheduler::new(
            SchedulerConfig::default(),
            Arc::new(NoopSystemMonitor),
            Arc::new(NoopActivityMonitor),
            Arc::new(NoopProcessMonitor),
            Arc::new(NoopCaptureTrigger),
            Arc::new(NoopFrameProcessor),
            storage.clone(),
            storage,
            None,
            None,
        )
        .with_required_deps(deps);

        assert!(scheduler.frame_storage.is_some());
        assert!(scheduler.notification_manager.is_some());
        assert!(scheduler.focus_analyzer.is_some());
        assert!(scheduler.config_manager.is_some());
        assert!(scheduler.belief_revision.is_some());
        let applied_memory_graph = scheduler
            .memory_graph
            .expect("memory_graph must be Some after with_required_deps");
        assert!(
            Arc::ptr_eq(&applied_memory_graph, &expected_memory_graph),
            "with_required_deps must transfer the SAME memory_graph Arc, not a copy"
        );
        let applied_regime_storage = scheduler
            .regime_storage
            .expect("regime_storage must be Some after with_required_deps");
        assert!(
            Arc::ptr_eq(&applied_regime_storage, &expected_regime_storage),
            "with_required_deps must transfer the SAME regime_storage Arc, not a copy"
        );
    }

    fn scheduler_with_api(api_client: Arc<CountingApiClient>) -> Scheduler {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let deps = minimal_required_deps(&storage);
        Scheduler::new(
            SchedulerConfig::default(),
            Arc::new(NoopSystemMonitor),
            Arc::new(NoopActivityMonitor),
            Arc::new(NoopProcessMonitor),
            Arc::new(NoopCaptureTrigger),
            Arc::new(NoopFrameProcessor),
            storage.clone(),
            storage,
            None,
            Some(api_client),
        )
        .with_required_deps(deps)
    }

    fn scheduler_with_batch_sink(batch_sink: Arc<CountingBatchSink>) -> Scheduler {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let deps = minimal_required_deps(&storage);
        Scheduler::new(
            SchedulerConfig::default(),
            Arc::new(NoopSystemMonitor),
            Arc::new(NoopActivityMonitor),
            Arc::new(NoopProcessMonitor),
            Arc::new(NoopCaptureTrigger),
            Arc::new(NoopFrameProcessor),
            storage.clone(),
            storage,
            Some(batch_sink),
            None,
        )
        .with_required_deps(deps)
    }

    fn weekday_from_chrono(day: chrono::Weekday) -> Weekday {
        match day {
            chrono::Weekday::Mon => Weekday::Mon,
            chrono::Weekday::Tue => Weekday::Tue,
            chrono::Weekday::Wed => Weekday::Wed,
            chrono::Weekday::Thu => Weekday::Thu,
            chrono::Weekday::Fri => Weekday::Fri,
            chrono::Weekday::Sat => Weekday::Sat,
            chrono::Weekday::Sun => Weekday::Sun,
        }
    }

    fn config_manager_with_current_tracking_mute() -> (ConfigManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mgr = ConfigManager::with_path(dir.path().join("config.json")).expect("ConfigManager");
        let now = chrono::Local::now();
        let start = now - ChronoDuration::minutes(30);
        let end = now + ChronoDuration::minutes(30);
        mgr.update_with(|cfg| {
            cfg.tracking_schedule = TrackingScheduleConfig {
                enabled: true,
                windows: vec![TrackingWindow {
                    start: start.format("%H:%M").to_string(),
                    end: end.format("%H:%M").to_string(),
                    days_of_week: vec![weekday_from_chrono(start.weekday())],
                    label: "test mute".to_string(),
                }],
                timezone: "Local".to_string(),
            };
            Ok(())
        })
        .expect("update config");
        (mgr, dir)
    }

    fn upload_policy_with_telemetry_consent(
        consent_manager: Arc<ConsentManager>,
    ) -> Arc<PlatformEgressPolicy> {
        let config = SchedulerConfig {
            upload_enabled: true,
            external_data_policy: ExternalDataPolicy::PiiFilterStrict,
            ..Default::default()
        };
        Arc::new(PlatformEgressPolicy::new(&config).with_consent_manager(Some(consent_manager)))
    }

    fn upload_policy_with_tracking_schedule(
        consent_manager: Arc<ConsentManager>,
        config_manager: ConfigManager,
    ) -> Arc<PlatformEgressPolicy> {
        let config = SchedulerConfig {
            upload_enabled: true,
            external_data_policy: ExternalDataPolicy::PiiFilterStrict,
            ..Default::default()
        };
        Arc::new(
            PlatformEgressPolicy::new(&config)
                .with_consent_manager(Some(consent_manager))
                .with_config_manager(Some(config_manager)),
        )
    }

    #[tokio::test(start_paused = true)]
    async fn batch_flush_respects_tracking_schedule_mute() {
        let dir = tempfile::tempdir().expect("tempdir");
        let consent_manager = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        consent_manager
            .grant_consent(
                ConsentPermissions {
                    telemetry: true,
                    ..Default::default()
                },
                30,
            )
            .expect("grant telemetry consent");

        let (config_manager, _config_dir) = config_manager_with_current_tracking_mute();
        let batch_sink = Arc::new(CountingBatchSink::default());
        let scheduler = scheduler_with_batch_sink(batch_sink.clone());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = scheduler.spawn_sync_loop(
            Duration::from_secs(30),
            upload_policy_with_tracking_schedule(consent_manager, config_manager),
            shutdown_rx,
        );

        tokio::task::yield_now().await;
        assert_eq!(
            batch_sink.flushes.load(Ordering::SeqCst),
            0,
            "batch upload flush must not leave the device during tracking-schedule mute"
        );

        shutdown_tx.send(true).expect("send shutdown");
        handle.await.expect("sync loop join");
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_loop_picks_up_telemetry_consent_grant_and_revoke_per_tick() {
        let dir = tempfile::tempdir().expect("tempdir");
        let consent_manager = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        consent_manager
            .grant_consent(
                ConsentPermissions {
                    telemetry: false,
                    ..Default::default()
                },
                30,
            )
            .expect("seed telemetry consent off");

        let api_client = Arc::new(CountingApiClient::default());
        let scheduler = scheduler_with_api(api_client.clone());
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = scheduler.spawn_heartbeat_loop(
            Duration::from_secs(30),
            "session-test".to_string(),
            upload_policy_with_telemetry_consent(consent_manager.clone()),
            shutdown_rx,
        );

        tokio::task::yield_now().await;
        assert_eq!(
            api_client.heartbeats.load(Ordering::SeqCst),
            0,
            "startup telemetry=false must not emit a heartbeat"
        );

        consent_manager
            .grant_consent(
                ConsentPermissions {
                    telemetry: true,
                    ..Default::default()
                },
                30,
            )
            .expect("grant telemetry consent");
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            api_client.heartbeats.load(Ordering::SeqCst),
            1,
            "heartbeat loop must keep running and observe telemetry grant on a later tick"
        );

        consent_manager
            .revoke_consent()
            .expect("revoke telemetry consent");
        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            api_client.heartbeats.load(Ordering::SeqCst),
            1,
            "heartbeat loop must observe telemetry revoke on a later tick"
        );

        handle.abort();
    }

    /// Drive the heartbeat interval under the paused clock until at least
    /// `target` heartbeats have been sent, then return. Robust against executor
    /// scheduling: each iteration advances one full interval and yields so the
    /// spawned loop can process the due tick.
    async fn advance_until_heartbeats(api: &CountingApiClient, target: usize) {
        for _ in 0..100 {
            if api.heartbeats.load(Ordering::SeqCst) >= target {
                return;
            }
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::task::yield_now().await;
        }
        panic!(
            "heartbeat count never reached {target} (stuck at {})",
            api.heartbeats.load(Ordering::SeqCst)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn heartbeat_writes_server_health_flag_both_directions() {
        // #8050: the heartbeat is the authoritative server-connectivity writer.
        // A success flips the flag healthy, a failure flips it unhealthy, and a
        // recovery flips it back — proving the adapter is no longer a dead writer.
        let dir = tempfile::tempdir().expect("tempdir");
        let consent_manager = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        consent_manager
            .grant_consent(
                ConsentPermissions {
                    telemetry: true,
                    ..Default::default()
                },
                30,
            )
            .expect("grant telemetry consent");

        let api_client = Arc::new(CountingApiClient::default());
        // Start the flag DELIBERATELY unhealthy so the first successful heartbeat
        // must actively flip it true — a dead writer would leave it false.
        let server = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let llm = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let cli = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let scheduler = scheduler_with_api(api_client.clone()).with_health_flags(
            server.clone(),
            llm.clone(),
            cli.clone(),
        );
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = scheduler.spawn_heartbeat_loop(
            Duration::from_secs(30),
            "session-test".to_string(),
            upload_policy_with_telemetry_consent(consent_manager.clone()),
            shutdown_rx,
        );

        advance_until_heartbeats(&api_client, 1).await;
        assert!(
            server.load(std::sync::atomic::Ordering::Relaxed),
            "a successful heartbeat must flip server health healthy"
        );

        // Inject a failure — the next heartbeat must flip the flag unhealthy.
        api_client.fail_heartbeat.store(true, Ordering::SeqCst);
        let before = api_client.heartbeats.load(Ordering::SeqCst);
        advance_until_heartbeats(&api_client, before + 1).await;
        assert!(
            !server.load(std::sync::atomic::Ordering::Relaxed),
            "a failed heartbeat must flip server health unhealthy"
        );

        // Recover — a subsequent success must flip it healthy again.
        api_client.fail_heartbeat.store(false, Ordering::SeqCst);
        let before = api_client.heartbeats.load(Ordering::SeqCst);
        advance_until_heartbeats(&api_client, before + 1).await;
        assert!(
            server.load(std::sync::atomic::Ordering::Relaxed),
            "a recovered heartbeat must flip server health healthy again"
        );

        // The heartbeat writer is scoped to server health only.
        assert!(
            llm.load(std::sync::atomic::Ordering::Relaxed),
            "heartbeat must not touch the llm flag"
        );
        assert!(
            cli.load(std::sync::atomic::Ordering::Relaxed),
            "heartbeat must not touch the cli flag"
        );

        handle.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn successful_batch_upload_confirms_server_health() {
        // #8050: a successful batch upload is a positive server-reachability
        // confirmation, so it flips the server flag healthy even when no
        // heartbeat has run yet.
        let dir = tempfile::tempdir().expect("tempdir");
        let consent_manager = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        consent_manager
            .grant_consent(
                ConsentPermissions {
                    telemetry: true,
                    ..Default::default()
                },
                30,
            )
            .expect("grant telemetry consent");

        let batch_sink = Arc::new(CountingBatchSink::default());
        // Start unhealthy so a successful flush must actively flip it true.
        let server = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let llm = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let cli = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let scheduler = scheduler_with_batch_sink(batch_sink.clone()).with_health_flags(
            server.clone(),
            llm.clone(),
            cli.clone(),
        );
        // NOTE: no shutdown path here — `CountingBatchSink::flush` always returns a
        // non-empty id vec, so the sync loop's shutdown drain (flush-until-empty)
        // would never terminate against this test double. `handle.abort()` stops
        // the loop after the assertion, mirroring the heartbeat test.
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = scheduler.spawn_sync_loop(
            Duration::from_secs(30),
            upload_policy_with_telemetry_consent(consent_manager.clone()),
            shutdown_rx,
        );

        for _ in 0..100 {
            if batch_sink.flushes.load(Ordering::SeqCst) >= 1 {
                break;
            }
            tokio::time::advance(Duration::from_secs(30)).await;
            tokio::task::yield_now().await;
        }
        assert!(
            batch_sink.flushes.load(Ordering::SeqCst) >= 1,
            "sync loop must flush at least once under granted telemetry consent"
        );
        assert!(
            server.load(std::sync::atomic::Ordering::Relaxed),
            "a successful batch upload must confirm the server reachable"
        );

        handle.abort();
    }

    // ── #7574 regression: regime checkpoint sub-tick timer ─────────────────
    //
    // `spawn_aggregation_loop` lives in `system.rs`, but it is
    // `pub(in crate::scheduler)` and this module already carries the full
    // Noop-port scaffold + `SchedulerConfig`/`Scheduler` test wiring, so the
    // regression test is co-located here rather than duplicating that
    // scaffold in `system.rs`.

    /// Manual mock (project convention: no mockall) counting `save_all` calls
    /// so the test can observe checkpoint cadence without inspecting SQLite.
    #[derive(Default)]
    struct CountingRegimeStorage {
        save_all_calls: AtomicUsize,
    }

    #[async_trait]
    impl RegimeStoragePort for CountingRegimeStorage {
        async fn load_all(&self) -> Result<Vec<Regime>, CoreError> {
            Ok(Vec::new())
        }

        async fn save_all(&self, _regimes: &[Regime]) -> Result<(), CoreError> {
            self.save_all_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn test_regime() -> Regime {
        Regime {
            regime_id: "regime-test".to_string(),
            name: None,
            auto_label: "test".to_string(),
            centroid: RegimeFeatures::default(),
            optimal_params: TriggerParams::default(),
            sample_count: 1,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status: RegimeStatus::Active,
        }
    }

    /// Waits until `counter` reaches `expected`, robust under paused virtual time
    /// and heavy parallel `--workspace` load. A plain `yield_now()` loop stalled
    /// at count 1 under full-suite load (the public #138 flake): it busy-spins
    /// this current-thread runtime, and two things then race against that spin —
    /// (1) the spawned loop's first aggregation tick parks in real-OS-thread
    /// `spawn_blocking` housekeeping (FTS-optimize / log-cleanup in `system.rs`)
    /// and cannot return to its `select!` to deliver the next checkpoint tick
    /// until that thread finishes, and (2) the checkpoint `tokio::time::interval`
    /// tick, though due after the caller's `advance`, is only delivered once the
    /// time driver is re-processed. So each iteration nudges the paused clock
    /// (re-drives the due tick — item 2) and hands off to the blocking pool
    /// (parks this runtime so it releases its core, letting the housekeeping
    /// thread finish — item 1). Neither is a fixed real-time sleep; the sub-second
    /// nudge is orders of magnitude below the 30-min cadence, so it cannot itself
    /// manufacture an extra checkpoint, and the assertion is unchanged.
    async fn wait_for_count(counter: &AtomicUsize, expected: usize) {
        for _ in 0..500 {
            if counter.load(Ordering::SeqCst) >= expected {
                return;
            }
            // (2) Re-drive the time driver so an already-due interval tick fires.
            tokio::time::advance(Duration::from_millis(1)).await;
            // (1) Park on a trivial blocking-pool hand-off (NOT a fixed sleep) so
            // this runtime releases its core and the in-flight housekeeping thread
            // can finish, letting the spawned loop return to its `select!`.
            let _ = tokio::task::spawn_blocking(|| ()).await;
        }
    }

    fn scheduler_with_regime_checkpoint(
        aggregation_interval: Duration,
        regime_storage: Arc<CountingRegimeStorage>,
        regime_manager_arc: Arc<parking_lot::Mutex<maekon_analysis::RegimeManager>>,
    ) -> Scheduler {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let mut deps = minimal_required_deps(&storage);
        // Override the stub regime_storage with the counting mock this test
        // asserts against.
        deps.regime_storage = regime_storage as Arc<dyn RegimeStoragePort>;
        Scheduler::new(
            SchedulerConfig {
                aggregation_interval,
                ..Default::default()
            },
            Arc::new(NoopSystemMonitor),
            Arc::new(NoopActivityMonitor),
            Arc::new(NoopProcessMonitor),
            Arc::new(NoopCaptureTrigger),
            Arc::new(NoopFrameProcessor),
            storage.clone(),
            storage,
            None,
            None,
        )
        .with_required_deps(deps)
        .with_regime_manager_arc(regime_manager_arc)
    }

    /// #7574: before the fix, the regime crash-durability checkpoint only ran
    /// nested inside the `interval.tick()` (aggregation) branch of
    /// `spawn_aggregation_loop`, so the documented ">= 30 min" gate could only
    /// ever be (re-)evaluated once per `aggregation_interval` (default 60 min
    /// in production). This test uses a 2-hour `aggregation_interval` — far
    /// longer than the fixed 30-minute checkpoint cadence — and advances the
    /// paused clock by 31 minutes. A second `save_all` call in that window can
    /// only be explained by a checkpoint timer that is independent of
    /// `aggregation_interval`; before the fix, the count would still read 1
    /// here (the aggregation tick would not fire again for another ~89 min).
    #[tokio::test(start_paused = true)]
    async fn regime_checkpoint_fires_independent_of_long_aggregation_interval() {
        let regime_storage = Arc::new(CountingRegimeStorage::default());
        let mut regime_manager = maekon_analysis::RegimeManager::new(
            &maekon_core::config::TieredMemoryConfig::default(),
        );
        regime_manager.hydrate_from(vec![test_regime()]);
        let regime_manager_arc = Arc::new(parking_lot::Mutex::new(regime_manager));

        let aggregation_interval = Duration::from_secs(2 * 3600); // 2 hours
        let scheduler = scheduler_with_regime_checkpoint(
            aggregation_interval,
            regime_storage.clone(),
            regime_manager_arc,
        );
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let handle = scheduler.spawn_aggregation_loop(aggregation_interval, None, shutdown_rx);

        wait_for_count(&regime_storage.save_all_calls, 1).await;
        assert_eq!(
            regime_storage.save_all_calls.load(Ordering::SeqCst),
            1,
            "the first tick (t=0) must checkpoint immediately (last=None → due now)"
        );

        // Past the 30-min checkpoint cadence, but well short of the 2-hour
        // aggregation interval.
        tokio::time::advance(Duration::from_secs(31 * 60)).await;
        wait_for_count(&regime_storage.save_all_calls, 2).await;
        assert_eq!(
            regime_storage.save_all_calls.load(Ordering::SeqCst),
            2,
            "checkpoint must fire again on its own ~30-min timer even though \
             the 2-hour aggregation interval has not elapsed"
        );

        handle.abort();
    }

    // ── #7946: startup re-prime ─────────────────────────────────────────

    use maekon_core::ports::storage::StorageService;

    struct RecordingSink {
        items: parking_lot::Mutex<Vec<maekon_core::ports::batch_sink::QueuedUpload>>,
    }

    #[async_trait]
    impl BatchSink for RecordingSink {
        fn enqueue(&self, item: maekon_core::ports::batch_sink::QueuedUpload) {
            self.items.lock().push(item);
        }

        fn enqueue_many(&self, items: Vec<maekon_core::ports::batch_sink::QueuedUpload>) {
            self.items.lock().extend(items);
        }

        async fn flush(&self) -> Result<Vec<String>, CoreError> {
            Ok(Vec::new())
        }
    }

    fn reprime_egress_policy(
        dir: &tempfile::TempDir,
    ) -> crate::scheduler::egress_policy::PlatformEgressPolicy {
        let cm = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        cm.grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent");
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        crate::scheduler::egress_policy::PlatformEgressPolicy::new(&config)
            .with_consent_manager(Some(cm))
    }

    /// Pending (persisted, unsent) uploadable events are re-primed into the
    /// sink with the ORIGINAL storage ids, and stay pending until a flush
    /// actually confirms them (re-prime itself never marks sent).
    #[tokio::test]
    async fn reprime_enqueues_pending_events_with_storage_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let event = Event::Context(maekon_core::models::event::ContextEvent {
            app_name: "reprime-app".to_string(),
            window_title: "Reprime".to_string(),
            prev_app_name: None,
            timestamp: Utc::now(),
            ..Default::default()
        });
        let expected_id = maekon_storage::sqlite::storage_event_id(&event);
        StorageService::save_event(storage.as_ref(), &event)
            .await
            .expect("save_event");

        let sink = Arc::new(RecordingSink {
            items: parking_lot::Mutex::new(Vec::new()),
        });
        let egress = reprime_egress_policy(&dir);

        let storage_dyn: Arc<dyn StorageService> = storage.clone();
        let sink_dyn: Arc<dyn BatchSink> = sink.clone();
        let (enqueued, retired) =
            super::reprime_pending_uploads(&storage_dyn, &sink_dyn, &egress).await;

        assert_eq!((enqueued, retired), (1, 0));
        // Scope the parking_lot guard in a block so it is provably dropped
        // before the next `.await` (clippy::await_holding_lock does not treat
        // an explicit `drop()` as releasing the guard for its analysis).
        {
            let items = sink.items.lock();
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].storage_id, expected_id);
        }

        // Not yet marked sent — only a confirmed flush marks rows.
        let still_pending = storage.get_pending_events(10).await.expect("pending");
        assert_eq!(still_pending.len(), 1);
    }

    /// Events the egress policy blocks (clipboard is fail-closed) are RETIRED
    /// at re-prime — marked sent so they stop re-priming forever — and never
    /// reach the sink.
    #[tokio::test]
    async fn reprime_retires_policy_blocked_events() {
        let dir = tempfile::tempdir().expect("tempdir");
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let event = Event::Clipboard(maekon_core::models::event::ClipboardEvent {
            timestamp: Utc::now(),
            content_type: maekon_core::models::event::ClipboardContentType::Text,
            char_count: 42,
            preview: None,
        });
        StorageService::save_event(storage.as_ref(), &event)
            .await
            .expect("save_event");

        let sink = Arc::new(RecordingSink {
            items: parking_lot::Mutex::new(Vec::new()),
        });
        let egress = reprime_egress_policy(&dir);

        let storage_dyn: Arc<dyn StorageService> = storage.clone();
        let sink_dyn: Arc<dyn BatchSink> = sink.clone();
        let (enqueued, retired) =
            super::reprime_pending_uploads(&storage_dyn, &sink_dyn, &egress).await;

        assert_eq!((enqueued, retired), (0, 1));
        assert_eq!(sink.items.lock().len(), 0);

        // Retired: no longer pending, so it will not re-prime next start.
        let still_pending = storage.get_pending_events(10).await.expect("pending");
        assert_eq!(still_pending.len(), 0);
    }
}
