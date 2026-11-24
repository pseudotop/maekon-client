use super::*;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::config::{PiiFilterLevel, TrackingWindow};
use maekon_core::config_manager::ConfigManager;
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::error::CoreError;
use maekon_core::error_codes::{InternalCode, StorageCode};
use maekon_core::models::ai_summary::AiSummaryArtifact;
use maekon_core::models::context::{ProcessInfo, UserContext, WindowBounds, WindowInfo};
use maekon_core::models::event::{ContextEvent, ProcessDetail, SystemEvent, SystemEventType};
use maekon_core::models::frame::ProcessedFrame;
use maekon_core::models::suggestion::Suggestion;
use maekon_core::models::system::SystemMetrics;
use maekon_core::models::tiered_memory::SegmentSummary;
use maekon_core::ports::batch_sink::{BatchSink, PendingEvent, QueuedUpload};
use maekon_core::ports::monitor::{ActivityMonitor, ProcessMonitor, SystemMonitor};
use maekon_core::ports::storage::StorageService;
use maekon_core::ports::vision::{CaptureRequest, CaptureTrigger, FrameProcessor};
use maekon_monitor::input_activity::InputActivityCollector;
use maekon_storage::sqlite::{storage_event_id, SqliteStorage};
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{watch, Notify};

use crate::focus_mode::FocusModeState;
use crate::scheduler::config::SchedulerConfig;
use crate::scheduler::shared_regime_state::SharedRegimeState;
use crate::scheduler::Scheduler;

const RAW_TITLE: &str = "Research document for employee@example.com - original-tail";

fn unused_port() -> CoreError {
    CoreError::Internal {
        code: InternalCode::Generic,
        message: "owned monitor fixture does not capture screens or call external services".into(),
    }
}

struct ExternalPorts;
#[async_trait]
impl SystemMonitor for ExternalPorts {
    async fn collect_metrics(&self) -> Result<SystemMetrics, CoreError> {
        Err(unused_port())
    }
}
#[async_trait]
impl ActivityMonitor for ExternalPorts {
    async fn collect_context(&self) -> Result<UserContext, CoreError> {
        Ok(UserContext {
            timestamp: Utc::now(),
            active_window: Some(WindowInfo {
                title: RAW_TITLE.into(),
                app_name: "PrivacyBFixture".into(),
                app_bundle_id: None,
                pid: 1,
                bounds: Some(WindowBounds {
                    x: 0,
                    y: 0,
                    width: 640,
                    height: 480,
                }),
            }),
            processes: Vec::new(),
        })
    }
}
#[async_trait]
impl ProcessMonitor for ExternalPorts {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        Ok(None)
    }
    async fn get_top_processes(&self, _limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
        Ok(Vec::new())
    }
    async fn get_detailed_processes(
        &self,
        _pid: Option<u32>,
        _limit: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        Ok(Vec::new())
    }
}
impl CaptureTrigger for ExternalPorts {
    fn should_capture(&self, _event: &ContextEvent) -> Option<CaptureRequest> {
        None
    }
}
#[async_trait]
impl FrameProcessor for ExternalPorts {
    async fn capture_and_process(
        &self,
        _request: &CaptureRequest,
    ) -> Result<ProcessedFrame, CoreError> {
        Err(unused_port())
    }
}

#[derive(Default)]
struct RecordingSink(Mutex<Vec<QueuedUpload>>);
#[async_trait]
impl BatchSink for RecordingSink {
    fn enqueue(&self, item: QueuedUpload) {
        self.0.lock().push(item);
    }
    fn enqueue_many(&self, items: Vec<QueuedUpload>) {
        self.0.lock().extend(items);
    }
    async fn flush(&self) -> Result<Vec<String>, CoreError> {
        panic!("monitor-only fixture must not run transport or shutdown flush")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Inserted(bool),
    Failed(Option<StorageCode>, String),
}
struct Saved {
    event: Event,
    proof: Option<UploadProvenance>,
    outcome: Outcome,
}
struct ObservedStorage {
    inner: Arc<SqliteStorage>,
    records: Mutex<Vec<Saved>>,
    completed: Notify,
    first: Notify,
    release: Notify,
    pause_first: bool,
}
#[async_trait]
impl StorageService for ObservedStorage {
    async fn save_event(&self, event: &Event) -> Result<(), CoreError> {
        self.inner.save_event(event).await
    }
    async fn save_event_with_upload_provenance(
        &self,
        event: &Event,
        proof: Option<&UploadProvenance>,
    ) -> Result<bool, CoreError> {
        let result = self
            .inner
            .save_event_with_upload_provenance(event, proof)
            .await;
        let outcome = match &result {
            Ok(inserted) => Outcome::Inserted(*inserted),
            Err(error) => Outcome::Failed(
                match error {
                    CoreError::Storage { code, .. } => Some(*code),
                    _ => None,
                },
                error.to_string(),
            ),
        };
        let count = {
            let mut records = self.records.lock();
            records.push(Saved {
                event: event.clone(),
                proof: proof.cloned(),
                outcome,
            });
            records.len()
        };
        if count == 1 && self.pause_first {
            self.first.notify_one();
            self.release.notified().await;
        }
        if count >= 2 {
            self.completed.notify_one();
        }
        result
    }
    async fn get_events(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<Event>, CoreError> {
        self.inner.get_events(from, to, limit).await
    }
    async fn get_pending_events(&self, limit: usize) -> Result<Vec<Event>, CoreError> {
        self.inner.get_pending_events(limit).await
    }
    async fn get_pending_events_with_upload_provenance(
        &self,
        limit: usize,
    ) -> Result<Vec<PendingEvent>, CoreError> {
        self.inner
            .get_pending_events_with_upload_provenance(limit)
            .await
    }
    async fn mark_as_sent(&self, ids: &[String]) -> Result<(), CoreError> {
        self.inner.mark_as_sent(ids).await
    }
    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.inner.enforce_retention().await
    }
    async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
        self.inner.save_suggestion(suggestion).await
    }
    async fn save_activity_segment(&self, summary: &SegmentSummary) -> Result<(), CoreError> {
        self.inner.save_activity_segment(summary).await
    }
    async fn update_segment_llm_summary(&self, id: &str, text: &str) -> Result<(), CoreError> {
        self.inner.update_segment_llm_summary(id, text).await
    }
    async fn update_segment_ai_summary(
        &self,
        id: &str,
        artifact: &AiSummaryArtifact,
    ) -> Result<(), CoreError> {
        self.inner.update_segment_ai_summary(id, artifact).await
    }
}
impl ObservedStorage {
    async fn two_saves(&self) {
        loop {
            let notified = self.completed.notified();
            if self.records.lock().len() >= 2 {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    Allow,
    Block,
    UnknownBlocked,
    UnknownAllowed,
    Defer,
    Duplicate,
    Erasing,
    Error,
    LateBlock,
    LateDefer,
}
impl Case {
    fn barrier(self) -> bool {
        matches!(self, Self::LateBlock | Self::LateDefer)
    }
    fn title_consent(self) -> bool {
        !matches!(self, Self::UnknownBlocked | Self::UnknownAllowed)
    }
}
struct ResetErasing(Arc<AtomicBool>);
impl Drop for ResetErasing {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn install_fixture(storage: &SqliteStorage, case: Case) {
    let sql = match case {
        Case::Duplicate => {
            "PRAGMA recursive_triggers=ON;
            CREATE TEMP TABLE privacy_b_seeded(kind TEXT PRIMARY KEY);
            CREATE TEMP TRIGGER privacy_b_duplicate BEFORE INSERT ON events
            WHEN (NEW.event_type='context_change' OR NEW.event_type LIKE 'window_%')
            AND NOT EXISTS(SELECT 1 FROM privacy_b_seeded WHERE kind=NEW.event_type)
            BEGIN
                INSERT INTO privacy_b_seeded VALUES(NEW.event_type);
                INSERT INTO events(event_id,event_type,timestamp,data,upload_provenance)
                VALUES(NEW.event_id,NEW.event_type,NEW.timestamp,
                    replace(NEW.data,'original-tail','winner-tail'),NULL);
            END;"
        }
        Case::Error => {
            "CREATE TEMP TRIGGER privacy_b_abort BEFORE INSERT ON events
            WHEN NEW.event_type='context_change' OR NEW.event_type LIKE 'window_%'
            BEGIN SELECT RAISE(ABORT,'privacy B owned insertion failure'); END;"
        }
        _ => return,
    };
    storage
        .connection_arc()
        .write_lock()
        .run((), |conn| conn.execute_batch(sql))
        .expect("owned fixture DDL");
}

struct Row {
    id: String,
    event: Event,
    proof: Option<UploadProvenance>,
    raw: String,
}
fn rows(storage: &SqliteStorage) -> Vec<Row> {
    let connection = storage.connection_arc();
    let tuples = connection
        .read_lock()
        .run::<_, Vec<(String, String, Option<String>)>, rusqlite::Error>(|conn| {
            let mut statement = conn.prepare(
                "SELECT event_id,data,upload_provenance FROM events
            WHERE event_type='context_change' OR event_type LIKE 'window_%' ORDER BY event_id",
            )?;
            let rows =
                statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            rows.collect()
        })
        .expect("independent rows");
    tuples
        .into_iter()
        .map(|(id, raw, proof)| Row {
            id,
            event: serde_json::from_str(&raw).expect("stored Event"),
            proof: proof.map(|value| serde_json::from_str(&value).expect("stored proof")),
            raw,
        })
        .collect()
}
fn title(event: &Event) -> &str {
    match event {
        Event::Context(value) => &value.window_title,
        Event::Window(value) => &value.window.window_title,
        _ => panic!("target producer"),
    }
}
fn ledger(storage: &SqliteStorage, disposition: &str) -> i64 {
    storage
        .connection_arc()
        .read_lock()
        .run::<_, i64, rusqlite::Error>(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM egress_ledger WHERE destination='server.batch_upload'
            AND disposition=?1",
                [disposition],
                |row| row.get(0),
            )
        })
        .expect("ledger")
}

async fn prove_sql_fixture_results() {
    for case in [Case::Allow, Case::Duplicate, Case::Error] {
        let storage = SqliteStorage::open_in_memory(30).expect("owned DB");
        install_fixture(&storage, case);
        let event = Event::Context(ContextEvent {
            app_name: "PrivacyBFixture".into(),
            window_title: sanitize_title_with_level(RAW_TITLE, PiiFilterLevel::Basic),
            ..Default::default()
        });
        let id = storage_event_id(&event);
        let proof = UploadProvenance::new(&id, &event, [1; 32], true, true).expect("bound proof");
        let result = storage
            .save_event_with_upload_provenance(&event, Some(&proof))
            .await;
        match case {
            Case::Allow => {
                assert!(result.expect("real insert"));
                let stored = rows(&storage);
                assert_eq!(stored.len(), 1);
                assert_eq!(stored[0].proof.as_ref(), Some(&proof));
            }
            Case::Duplicate => {
                assert!(!result.expect("outer INSERT OR IGNORE"));
                let stored = rows(&storage);
                assert_eq!(stored.len(), 1);
                assert_eq!(stored[0].id, id);
                assert_eq!(storage_event_id(&stored[0].event), id);
                assert!(title(&stored[0].event).contains("winner-tail"));
                assert!(stored[0].proof.is_none());
            }
            Case::Error => {
                let error = result.expect_err("real trigger failure");
                assert!(error
                    .to_string()
                    .contains("privacy B owned insertion failure"));
                assert!(matches!(
                    error,
                    CoreError::Storage {
                        code: StorageCode::Failed,
                        ..
                    }
                ));
                assert!(rows(&storage).is_empty());
            }
            _ => unreachable!(),
        }
    }
}

async fn monitor_case(case: Case) {
    if matches!(case, Case::Duplicate | Case::Error) {
        prove_sql_fixture_results().await;
    }
    let directory = tempfile::tempdir().expect("owned fixture");
    let sqlite = Arc::new(
        SqliteStorage::open(&directory.path().join("events.db"), 30, None).expect("owned file DB"),
    );
    install_fixture(&sqlite, case);
    let sentinel = Event::System(SystemEvent {
        event_id: uuid::Uuid::new_v4(),
        event_type: SystemEventType::MetricsUpdate,
        timestamp: Utc::now(),
        data: serde_json::json!({"privacy_B_sentinel":true}),
    });
    sqlite
        .save_event(&sentinel)
        .await
        .expect("sentinel before erasing");
    let erase_flag = sqlite.erasing();
    let _restore = ResetErasing(erase_flag.clone());
    if case == Case::Erasing {
        erase_flag.store(true, Ordering::SeqCst);
    }
    let config =
        ConfigManager::with_path(directory.path().join("config.json")).expect("owned config");
    config
        .update_with(|value| {
            value.vision.capture_enabled = true;
            value.schedule.active_hours_enabled = false;
            value.schedule.pause_on_battery_saver = false;
            value.tracking_schedule.enabled = false;
            value.monitor.upload_enabled = case != Case::Defer;
            value.privacy.auto_exclude_sensitive = false;
            value.privacy.excluded_apps.clear();
            value.privacy.excluded_app_patterns.clear();
            value.privacy.excluded_title_patterns.clear();
            value.privacy.pii_filter_level = PiiFilterLevel::Basic;
            value.ai_provider.external_data_policy = ExternalDataPolicy::AllowFiltered;
            if matches!(case, Case::Block | Case::UnknownBlocked) {
                value
                    .privacy
                    .excluded_title_patterns
                    .push("*employee@*".into());
            }
            Ok(())
        })
        .expect("capture allowed independently of upload policy");
    let consent = Arc::new(ConsentManager::new(directory.path().join("consent.json")));
    consent
        .grant_consent(
            ConsentPermissions {
                screen_capture: true,
                telemetry: true,
                window_title_collection: case.title_consent(),
                ..Default::default()
            },
            30,
        )
        .expect("owned consent");
    let tick = config.snapshot();
    let tick_fingerprint = PlatformEgressPolicy::title_policy_fingerprint(&tick.privacy);
    let scheduler_config = SchedulerConfig {
        upload_enabled: tick.monitor.upload_enabled,
        privacy_config: tick.privacy.clone(),
        external_data_policy: tick.ai_provider.external_data_policy,
        ..Default::default()
    };
    let policy = Arc::new(
        PlatformEgressPolicy::new(&scheduler_config)
            .with_config_manager(Some(config.clone()))
            .with_consent_manager(Some(consent.clone())),
    );
    let observed = Arc::new(ObservedStorage {
        inner: sqlite.clone(),
        records: Mutex::new(Vec::new()),
        completed: Notify::new(),
        first: Notify::new(),
        release: Notify::new(),
        pause_first: case.barrier(),
    });
    let sink = Arc::new(RecordingSink::default());
    let ports = Arc::new(ExternalPorts);
    let mut scheduler = Scheduler::new(
        scheduler_config,
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports,
        observed.clone(),
        sqlite.clone(),
        Some(sink.clone()),
        None,
    )
    .with_consent_manager(consent);
    scheduler.config_manager = Some(config.clone());
    let (shutdown, receiver) = watch::channel(false);
    let mut task = scheduler.spawn_monitor_loop(
        Duration::from_secs(3600),
        u64::MAX,
        "privacy-B-owned-session".into(),
        policy,
        Arc::new(InputActivityCollector::new()),
        None,
        Arc::new(SharedRegimeState::new()),
        Arc::new(FocusModeState::new()),
        receiver,
        None,
    );
    let observation: Result<(), String> = async {
        if case.barrier() {
            tokio::time::timeout(Duration::from_secs(10), observed.first.notified())
                .await
                .map_err(|e| e.to_string())?;
            config
                .update_with(|value| {
                    if case == Case::LateBlock {
                        value
                            .privacy
                            .excluded_title_patterns
                            .push("*new rule*".into());
                    } else {
                        value.monitor.upload_enabled = false;
                    }
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
            observed.release.notify_one();
        }
        tokio::time::timeout(Duration::from_secs(10), observed.two_saves())
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    .await;
    let _ = shutdown.send(true);
    observed.release.notify_one();
    match tokio::time::timeout(Duration::from_secs(10), &mut task).await {
        Ok(result) => result.expect("actual monitor tick must complete"),
        Err(error) => {
            task.abort();
            let _ = task.await;
            panic!("owned monitor task did not terminate: {error}; observation={observation:?}");
        }
    }
    observation
        .expect("both actual Window/Context saves must complete; timeout is not a privacy verdict");
    erase_flag.store(false, Ordering::SeqCst);
    let unchanged_sentinel = sqlite
        .get_events(
            Utc::now() - chrono::Duration::days(1),
            Utc::now() + chrono::Duration::days(1),
            100,
        )
        .await
        .expect("sentinel read");
    let records = observed.records.lock();
    assert_eq!(records.len(), 2, "exactly one real tick: {case:?}");
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r.event, Event::Window(_)))
            .count(),
        1
    );
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r.event, Event::Context(_)))
            .count(),
        1
    );
    for record in records.iter() {
        let expected = match case {
            Case::Duplicate | Case::Erasing => Outcome::Inserted(false),
            Case::Error => {
                assert!(matches!(
                    &record.outcome,
                    Outcome::Failed(Some(StorageCode::Failed), message)
                        if message.contains("privacy B owned insertion failure")
                ));
                continue;
            }
            _ => Outcome::Inserted(true),
        };
        assert_eq!(record.outcome, expected);
        if case.title_consent() {
            let proof = record.proof.as_ref().expect("capture evidence");
            assert!(proof.is_bound_to(&storage_event_id(&record.event), &record.event));
            assert!(proof.title_information_lost);
            assert_eq!(proof.title_allowed, case != Case::Block);
            assert_eq!(proof.title_policy_fingerprint, tick_fingerprint);
        } else {
            assert!(record.proof.is_none());
        }
    }
    let stored = rows(&sqlite);
    assert_eq!(
        stored.len(),
        if matches!(case, Case::Erasing | Case::Error) {
            0
        } else {
            2
        }
    );
    for row in &stored {
        assert!(!row.raw.contains("employee@example.com"));
        assert_eq!(storage_event_id(&row.event), row.id);
        if case == Case::Duplicate {
            assert!(title(&row.event).contains("winner-tail"));
            assert!(row.proof.is_none());
        } else if case.title_consent() {
            assert!(row
                .proof
                .as_ref()
                .expect("private persisted proof")
                .is_bound_to(&row.id, &row.event));
        } else {
            assert!(
                row.proof.is_none(),
                "missing source consent must stay Unknown"
            );
        }
    }
    let queued = sink.0.lock();
    let allowed = matches!(case, Case::Allow | Case::UnknownAllowed);
    assert_eq!(
        queued.len(),
        if allowed { 2 } else { 0 },
        "real producer admission: {case:?}"
    );
    if allowed {
        let mut stored_ids: Vec<_> = stored.iter().map(|row| row.id.clone()).collect();
        let mut queued_ids: Vec<_> = queued.iter().map(|item| item.storage_id.clone()).collect();
        stored_ids.sort();
        queued_ids.sort();
        assert_eq!(
            queued_ids, stored_ids,
            "both producers enqueue exactly once"
        );
    }
    for item in queued.iter() {
        let row = stored
            .iter()
            .find(|row| row.id == item.storage_id)
            .expect("original persisted ID");
        assert_eq!(
            serde_json::to_value(&item.event).expect("payload"),
            serde_json::to_value(&row.event).expect("stored")
        );
    }
    let blocked = matches!(case, Case::Block | Case::UnknownBlocked | Case::LateBlock);
    assert_eq!(ledger(&sqlite, "uploaded"), if allowed { 2 } else { 0 });
    assert_eq!(ledger(&sqlite, "blocked"), if blocked { 2 } else { 0 });
    let sentinel_row = unchanged_sentinel
        .iter()
        .find(|event| storage_event_id(event) == storage_event_id(&sentinel))
        .expect("sentinel row preserved");
    assert_eq!(
        serde_json::to_value(sentinel_row).expect("stored sentinel"),
        serde_json::to_value(&sentinel).expect("original sentinel")
    );
}

#[tokio::test]
async fn monitor_tick_insert_winners_bind_masked_rows_and_enqueue_original_ids() {
    monitor_case(Case::Allow).await;
}
#[tokio::test]
async fn monitor_tick_title_constraints_preserve_capture_loss_and_unknown() {
    for case in [Case::Block, Case::UnknownBlocked, Case::UnknownAllowed] {
        monitor_case(case).await;
    }
}
#[tokio::test]
async fn monitor_tick_real_duplicate_winners_never_promote_legacy_proof() {
    monitor_case(Case::Duplicate).await;
}
#[tokio::test]
async fn monitor_tick_erasure_skip_never_enqueues() {
    monitor_case(Case::Erasing).await;
}
#[tokio::test]
async fn monitor_tick_storage_failure_never_enqueues() {
    monitor_case(Case::Error).await;
}
#[tokio::test]
async fn monitor_tick_late_policy_block_and_defer_use_live_snapshot() {
    for case in [Case::Defer, Case::LateBlock, Case::LateDefer] {
        monitor_case(case).await;
    }
}

fn grant_telemetry(consent: &ConsentManager) {
    consent
        .grant_consent(
            ConsentPermissions {
                telemetry: true,
                ..Default::default()
            },
            30,
        )
        .expect("owned telemetry consent");
}

fn policy_fixture() -> (
    tempfile::TempDir,
    ConfigManager,
    Arc<ConsentManager>,
    PlatformEgressPolicy,
) {
    let directory = tempfile::tempdir().expect("owned policy directory");
    let manager = ConfigManager::with_path(directory.path().join("config.json")).expect("config");
    manager
        .update_with(|config| {
            config.monitor.upload_enabled = true;
            config.tracking_schedule.enabled = false;
            config.privacy.auto_exclude_sensitive = false;
            config.privacy.excluded_apps.clear();
            config.privacy.excluded_app_patterns.clear();
            config.privacy.excluded_title_patterns.clear();
            config.privacy.pii_filter_level = PiiFilterLevel::Basic;
            config.ai_provider.external_data_policy = ExternalDataPolicy::AllowFiltered;
            Ok(())
        })
        .expect("initial live policy");
    let consent = Arc::new(ConsentManager::new(directory.path().join("consent.json")));
    grant_telemetry(&consent);
    let policy = PlatformEgressPolicy::new(&SchedulerConfig::default())
        .with_config_manager(Some(manager.clone()))
        .with_consent_manager(Some(consent.clone()));
    (directory, manager, consent, policy)
}

fn bound_title(
    policy: &PlatformEgressPolicy,
    manager: &ConfigManager,
    raw: &str,
    title: &str,
) -> (Event, UploadProvenance) {
    let event = Event::Context(ContextEvent {
        app_name: "Editor".into(),
        window_title: title.into(),
        ..Default::default()
    });
    let snapshot = manager.snapshot();
    let evidence = policy.capture_title_evidence(Some(&snapshot.privacy), Some(raw), title);
    let proof = policy
        .bind_source_title("persisted", &event, evidence.as_ref())
        .expect("consented source evidence");
    (event, proof)
}

#[test]
fn live_policy_requires_each_temporary_gate_and_resumes() {
    let (_directory, manager, consent, policy) = policy_fixture();
    let (event, _) = bound_title(&policy, &manager, "Clear note", "Clear note");
    let verdict = || {
        policy
            .evaluate_with_provenance("persisted", &mut event.clone(), None)
            .0
    };
    assert_eq!(verdict(), UploadDecision::Allow);
    consent.revoke_consent().expect("owned revoke");
    assert_eq!(verdict(), UploadDecision::Defer);
    grant_telemetry(&consent);
    assert_eq!(verdict(), UploadDecision::Allow);
    manager
        .update_with(|config| {
            config.tracking_schedule.enabled = true;
            config.tracking_schedule.windows = vec![TrackingWindow {
                start: "09:00".into(),
                end: "10:00".into(),
                days_of_week: Vec::new(),
                label: "Never active on any day".into(),
            }];
            Ok(())
        })
        .expect("clock-independent schedule denial");
    assert_eq!(verdict(), UploadDecision::Defer);
    manager
        .update_with(|config| {
            config.tracking_schedule.enabled = false;
            Ok(())
        })
        .expect("restore unrestricted schedule");
    assert_eq!(verdict(), UploadDecision::Allow);
}

#[test]
fn live_policy_keeps_lost_known_allowed_under_same_title_rules() {
    let (_directory, manager, _consent, policy) = policy_fixture();
    manager
        .update_with(|config| {
            config.privacy.excluded_title_patterns = vec!["*confidential*".into()];
            Ok(())
        })
        .expect("active nonmatching title rule");
    let masked = sanitize_title_with_level(RAW_TITLE, PiiFilterLevel::Basic);
    assert_ne!(masked, RAW_TITLE);
    let (event, proof) = bound_title(&policy, &manager, RAW_TITLE, &masked);
    assert!(proof.title_information_lost);
    assert_eq!(
        policy
            .evaluate_with_provenance("persisted", &mut event.clone(), Some(&proof))
            .0,
        UploadDecision::Allow
    );
    assert_eq!(
        policy
            .evaluate_with_provenance("persisted", &mut event.clone(), None)
            .0,
        UploadDecision::Block
    );
}

#[test]
fn live_policy_evaluates_lossless_known_clear_title() {
    let (_directory, manager, _consent, policy) = policy_fixture();
    let (event, proof) = bound_title(&policy, &manager, "Clear research", "Clear research");
    assert!(!proof.title_information_lost);
    assert_eq!(
        policy
            .evaluate_with_provenance("persisted", &mut event.clone(), Some(&proof))
            .0,
        UploadDecision::Allow
    );
    manager
        .update_with(|config| {
            config.privacy.excluded_title_patterns = vec!["*research*".into()];
            Ok(())
        })
        .expect("exclude the still-visible title");
    assert_eq!(
        policy
            .evaluate_with_provenance("persisted", &mut event.clone(), Some(&proof))
            .0,
        UploadDecision::Block
    );
}

#[test]
fn live_policy_preserves_new_egress_loss_across_title_rule_changes() {
    let (_directory, manager, _consent, policy) = policy_fixture();
    manager
        .update_with(|config| {
            config.privacy.pii_filter_level = PiiFilterLevel::Off;
            Ok(())
        })
        .expect("capture without masking, egress retains its Basic floor");
    let (original, proof) = bound_title(&policy, &manager, RAW_TITLE, RAW_TITLE);
    assert!(!proof.title_information_lost);
    let mut masked = original.clone();
    let (decision, next) = policy.evaluate_with_provenance("persisted", &mut masked, Some(&proof));
    assert_eq!(decision, UploadDecision::Allow);
    let Event::Context(masked_context) = &masked else {
        panic!("owned fixture is a Context event");
    };
    assert_ne!(masked_context.window_title, RAW_TITLE);
    let next = next.expect("bound Known survives egress");
    let (decision, unknown) =
        policy.evaluate_with_provenance("persisted", &mut original.clone(), None);
    assert_eq!(decision, UploadDecision::Allow);
    assert!(unknown.is_none());
    manager
        .update_with(|config| {
            config.privacy.excluded_title_patterns = vec!["*employee@*".into()];
            Ok(())
        })
        .expect("new restriction requires the lost original title");
    assert_eq!(
        policy
            .evaluate_with_provenance("persisted", &mut masked, Some(&next))
            .0,
        UploadDecision::Block
    );
    assert!(next.title_information_lost);
    assert!(next.is_bound_to("persisted", &masked));
}
