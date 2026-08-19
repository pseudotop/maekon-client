//! Debug-only upload-spool interruption and re-prime fixture.
//!
//! The fixture uses the production SQLite adapter and `BatchUploader`, but a
//! synthetic in-process API client. It never opens a socket. Preparation
//! persists two events, forces an upload failure, and exits with both rows
//! still pending. Verification is a separate process step that reloads the
//! pending rows, confirms the exact storage IDs, and only then marks them sent.

use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{bail, ensure, Context, Result};
use chrono::{TimeZone, Utc};
use maekon_core::config::{AppConfig, CloudSttPolicy};
use maekon_core::config_manager::ConfigManager;
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::event::{ContextEvent, Event, EventBatch};
use maekon_core::models::suggestion::SuggestionFeedback;
use maekon_core::ports::api_client::{ApiClient, SessionCreateResponse};
use maekon_core::ports::batch_sink::QueuedUpload;
use maekon_core::ports::storage::StorageService;
use maekon_network::batch_uploader::BatchUploader;
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::sqlite::{storage_event_id, SqliteStorage};
use serde::{Deserialize, Serialize};

use crate::storage_runtime::resolve_shared_master_key;

const PREPARE_COMMAND: &str = "debug-prepare-qc-upload-spool";
const VERIFY_COMMAND: &str = "debug-verify-qc-upload-spool";
const DEBUG_GATE_ENV: &str = "MAEKON_DEBUG_QC_FIXTURE_CLI";
const ISOLATED_GATE_ENV: &str = "MAEKON_TC_ISOLATED_PROFILE";
const FIXTURE_GATE_ENV: &str = "MAEKON_DEBUG_QC_UPLOAD_SPOOL_FIXTURE";
const CONFIRM_ENV: &str = "MAEKON_QC_UPLOAD_SPOOL_CONFIRM";
const CONFIRM_VALUE: &str = "interrupt-and-reprime";
const FLAVOR_ENV: &str = "MAEKON_APP_FLAVOR";
const STATE_FILE: &str = "qc-upload-spool-state.json";
const STATE_SCHEMA: &str = "maekon.qc-upload-spool-state.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UploadSpoolReport {
    data_dir: String,
    phase: &'static str,
    seeded: usize,
    confirmed: usize,
    pending: usize,
    upload_attempts: usize,
    egress_ledger_entries: u64,
}

impl fmt::Display for UploadSpoolReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "QC upload-spool fixture {}: data_dir={} seeded={} confirmed={} pending={} upload_attempts={} egress_ledger_entries={}",
            self.phase,
            self.data_dir,
            self.seeded,
            self.confirmed,
            self.pending,
            self.upload_attempts,
            self.egress_ledger_entries
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct UploadSpoolState {
    schema: String,
    phase: String,
    expected_storage_ids: Vec<String>,
    confirmed_storage_ids: Vec<String>,
    pending_storage_ids: Vec<String>,
    upload_attempts: usize,
    sent_markers_written_after_success: bool,
    synthetic_transport: bool,
    external_egress_enabled: bool,
    egress_ledger_entries: u64,
    host_mutation: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct UploadSpoolStatus {
    phase: &'static str,
    pending_count: usize,
    attempt_count: usize,
    reprime_count: usize,
    sent_marker_count: usize,
    storage_id_preserved: bool,
    network_disabled: bool,
    real_account_used: bool,
    last_error: Option<&'static str>,
}

struct SyntheticUploadClient {
    fail_upload: bool,
    attempts: AtomicUsize,
}

impl SyntheticUploadClient {
    fn new(fail_upload: bool) -> Self {
        Self {
            fail_upload,
            attempts: AtomicUsize::new(0),
        }
    }

    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl ApiClient for SyntheticUploadClient {
    async fn create_session(&self, client_id: &str) -> Result<SessionCreateResponse, CoreError> {
        Ok(SessionCreateResponse {
            session_id: "qc-upload-spool-session".to_string(),
            user_id: "qc-synthetic-user".to_string(),
            client_id: client_id.to_string(),
            capabilities: Vec::new(),
        })
    }

    async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn upload_batch(&self, batch: &EventBatch) -> Result<(), CoreError> {
        self.attempts.fetch_add(1, Ordering::Relaxed);
        if batch.events.is_empty() {
            return Err(CoreError::Internal {
                code: InternalCode::Generic,
                message: "synthetic upload batch must not be empty".to_string(),
            });
        }
        if self.fail_upload {
            return Err(CoreError::Internal {
                code: InternalCode::Generic,
                message: "synthetic interrupted upload".to_string(),
            });
        }
        Ok(())
    }

    async fn send_feedback(&self, _feedback: &SuggestionFeedback) -> Result<(), CoreError> {
        Ok(())
    }

    async fn send_heartbeat(&self, _session_id: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

pub(crate) fn prepare_command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(PREPARE_COMMAND)
}

pub(crate) fn verify_command_requested<'a>(args: impl Iterator<Item = &'a str>) -> bool {
    args.into_iter().next() == Some(VERIFY_COMMAND)
}

pub(crate) fn run_prepare_from_env() -> Result<UploadSpoolReport> {
    require_fixture_gates()?;

    let config = ConfigManager::new()
        .context("initialize isolated upload-spool config")?
        .update_with(|config| {
            configure_isolated_profile(config);
            Ok(())
        })
        .context("persist isolated upload-spool config")?;
    let data_dir = ConfigManager::data_dir().context("resolve isolated upload-spool data dir")?;
    std::fs::create_dir_all(&data_dir).context("create isolated upload-spool data dir")?;
    let encryption_key = resolve_shared_master_key(&data_dir)
        .context("resolve isolated upload-spool encryption key")?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create upload-spool preparation runtime")?;
    runtime.block_on(prepare_fixture(
        &data_dir,
        encryption_key,
        config.storage.retention_days,
    ))
}

pub(crate) fn run_verify_from_env() -> Result<UploadSpoolReport> {
    require_fixture_gates()?;

    let config = ConfigManager::new()
        .context("initialize isolated upload-spool config")?
        .get();
    ensure_isolated_profile(&config)?;
    let data_dir = ConfigManager::data_dir().context("resolve isolated upload-spool data dir")?;
    let encryption_key = resolve_shared_master_key(&data_dir)
        .context("resolve isolated upload-spool encryption key")?;

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("create upload-spool verification runtime")?;
    runtime.block_on(verify_fixture(
        &data_dir,
        encryption_key,
        config.storage.retention_days,
    ))
}

pub(crate) async fn status_from_env(
    storage: &Arc<SqliteStorage>,
    data_dir: &Path,
) -> std::result::Result<Option<UploadSpoolStatus>, CoreError> {
    if !fixture_enabled() {
        return Ok(None);
    }
    ensure_runtime_isolated_profile().map_err(to_core_error)?;

    let state = read_state(data_dir).map_err(to_core_error)?;
    status_from_state(storage, &state)
        .await
        .map(Some)
        .map_err(to_core_error)
}

pub(crate) async fn run_step_from_env(
    storage: &Arc<SqliteStorage>,
    data_dir: &Path,
) -> std::result::Result<UploadSpoolStatus, CoreError> {
    require_fixture_gates().map_err(to_core_error)?;
    ensure_runtime_isolated_profile().map_err(to_core_error)?;
    let state = read_state(data_dir).map_err(to_core_error)?;

    match state.phase.as_str() {
        "interrupted" => {
            verify_fixture_with_storage(data_dir, Arc::clone(storage))
                .await
                .map_err(to_core_error)?;
        }
        "verified" => {}
        phase => {
            return Err(to_core_error(anyhow::anyhow!(
                "unsupported upload-spool fixture phase: {phase}"
            )));
        }
    }

    let state = read_state(data_dir).map_err(to_core_error)?;
    status_from_state(storage, &state)
        .await
        .map_err(to_core_error)
}

async fn prepare_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
) -> Result<UploadSpoolReport> {
    let state_path = data_dir.join(STATE_FILE);
    if state_path.exists() {
        bail!(
            "upload-spool state already exists; discard the dedicated qc-*/tc-* profile before preparing again"
        )
    }

    let db_path = data_dir.join(maekon_storage::encryption::SQLCIPHER_DB_FILENAME);
    if db_path.exists() {
        bail!("upload-spool preparation requires a fresh dedicated qc-*/tc-* profile")
    }

    let storage = Arc::new(
        SqliteStorage::open(&db_path, retention_days, Some(&encryption_key))
            .context("create isolated upload-spool database")?,
    );
    let events = synthetic_events();
    for event in &events {
        storage
            .save_event(event)
            .await
            .context("persist synthetic upload-spool event")?;
    }

    let expected_ids = event_ids(&events);
    let pending_before = storage
        .get_pending_events(10)
        .await
        .context("read persisted upload-spool events")?;
    ensure_exact_ids(
        &expected_ids,
        &event_ids(&pending_before),
        "seeded pending rows",
    )?;

    let client = Arc::new(SyntheticUploadClient::new(true));
    let uploader = BatchUploader::new(client.clone(), "qc-upload-spool-session".to_string(), 10, 0);
    uploader.enqueue_many(queued_uploads(&pending_before));
    ensure!(
        uploader.flush().await.is_err(),
        "interruption phase must force the synthetic upload to fail"
    );
    ensure!(
        uploader.queue_size() == events.len(),
        "failed upload must remain queued until process interruption"
    );
    ensure!(
        uploader.failed_batches() == 1 && uploader.total_dropped() == 0,
        "failed synthetic batch must be requeued without drops"
    );

    // Dropping the uploader simulates process interruption: the volatile queue
    // disappears, while the authoritative SQLite rows must remain pending.
    drop(uploader);
    let pending_after = storage
        .get_pending_events(10)
        .await
        .context("re-read pending rows after interrupted upload")?;
    let pending_ids = event_ids(&pending_after);
    ensure_exact_ids(
        &expected_ids,
        &pending_ids,
        "post-interruption pending rows",
    )?;
    let egress_ledger_entries = egress_ledger_count(&storage)?;
    ensure!(
        egress_ledger_entries == 0,
        "synthetic upload-spool fixture must not write external egress rows"
    );

    write_state(
        data_dir,
        &UploadSpoolState {
            schema: STATE_SCHEMA.to_string(),
            phase: "interrupted".to_string(),
            expected_storage_ids: expected_ids,
            confirmed_storage_ids: Vec::new(),
            pending_storage_ids: pending_ids,
            upload_attempts: client.attempts(),
            sent_markers_written_after_success: false,
            synthetic_transport: true,
            external_egress_enabled: false,
            egress_ledger_entries,
            host_mutation: false,
        },
    )?;

    Ok(UploadSpoolReport {
        data_dir: data_dir.display().to_string(),
        phase: "interrupted",
        seeded: events.len(),
        confirmed: 0,
        pending: pending_after.len(),
        upload_attempts: client.attempts(),
        egress_ledger_entries,
    })
}

async fn verify_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
) -> Result<UploadSpoolReport> {
    let storage = Arc::new(
        SqliteStorage::open(
            &data_dir.join(maekon_storage::encryption::SQLCIPHER_DB_FILENAME),
            retention_days,
            Some(&encryption_key),
        )
        .context("reopen interrupted upload-spool database")?,
    );
    verify_fixture_with_storage(data_dir, storage).await
}

async fn verify_fixture_with_storage(
    data_dir: &Path,
    storage: Arc<SqliteStorage>,
) -> Result<UploadSpoolReport> {
    let prior = read_state(data_dir)?;
    ensure!(
        prior.schema == STATE_SCHEMA && prior.phase == "interrupted",
        "verification requires an interrupted v1 upload-spool state"
    );
    ensure!(
        prior.confirmed_storage_ids.is_empty()
            && !prior.sent_markers_written_after_success
            && !prior.external_egress_enabled
            && prior.egress_ledger_entries == 0
            && !prior.host_mutation,
        "interrupted upload-spool state violates the fail-closed boundary"
    );

    let all_pending = storage
        .get_pending_events(10_000)
        .await
        .context("re-prime persisted upload-spool events")?;
    let pending = matching_events(&all_pending, &prior.expected_storage_ids);
    let pending_ids = event_ids(&pending);
    ensure_exact_ids(
        &prior.expected_storage_ids,
        &pending_ids,
        "re-primed pending rows",
    )?;

    let client = Arc::new(SyntheticUploadClient::new(false));
    let uploader = BatchUploader::new(client.clone(), "qc-upload-spool-session".to_string(), 10, 0);
    uploader.enqueue_many(queued_uploads(&pending));
    let confirmed_ids = uploader
        .flush()
        .await
        .context("confirm re-primed synthetic upload")?;
    ensure_exact_ids(
        &prior.expected_storage_ids,
        &confirmed_ids,
        "confirmed upload IDs",
    )?;

    // The upload adapter returns exact confirmed IDs but never mutates storage.
    // Assert the rows remain pending until this post-success marker write.
    let pending_before_mark = storage
        .get_pending_events(10_000)
        .await
        .context("verify sent markers are absent before confirmation write")?;
    let target_pending_before_mark =
        matching_events(&pending_before_mark, &prior.expected_storage_ids);
    ensure_exact_ids(
        &prior.expected_storage_ids,
        &event_ids(&target_pending_before_mark),
        "pre-marker pending rows",
    )?;

    storage
        .mark_as_sent(&confirmed_ids)
        .await
        .context("mark exactly the confirmed upload rows as sent")?;
    let pending_after_mark = storage
        .get_pending_events(10_000)
        .await
        .context("verify confirmed rows leave the pending spool")?;
    let target_pending_after_mark =
        matching_events(&pending_after_mark, &prior.expected_storage_ids);
    ensure!(
        target_pending_after_mark.is_empty(),
        "confirmed upload rows must be absent from the pending spool"
    );
    let egress_ledger_entries = egress_ledger_count(&storage)?;
    ensure!(
        egress_ledger_entries == 0,
        "synthetic upload-spool verification must not write external egress rows"
    );

    let mut confirmed_sorted = confirmed_ids;
    confirmed_sorted.sort();
    write_state(
        data_dir,
        &UploadSpoolState {
            schema: STATE_SCHEMA.to_string(),
            phase: "verified".to_string(),
            expected_storage_ids: prior.expected_storage_ids,
            confirmed_storage_ids: confirmed_sorted,
            pending_storage_ids: Vec::new(),
            upload_attempts: prior.upload_attempts + client.attempts(),
            sent_markers_written_after_success: true,
            synthetic_transport: true,
            external_egress_enabled: false,
            egress_ledger_entries,
            host_mutation: false,
        },
    )?;

    Ok(UploadSpoolReport {
        data_dir: data_dir.display().to_string(),
        phase: "verified",
        seeded: pending.len(),
        confirmed: pending.len(),
        pending: 0,
        upload_attempts: prior.upload_attempts + client.attempts(),
        egress_ledger_entries,
    })
}

async fn status_from_state(
    storage: &Arc<SqliteStorage>,
    state: &UploadSpoolState,
) -> Result<UploadSpoolStatus> {
    ensure!(
        state.schema == STATE_SCHEMA,
        "unsupported upload-spool state schema"
    );
    ensure!(
        state.synthetic_transport
            && !state.external_egress_enabled
            && state.egress_ledger_entries == 0
            && !state.host_mutation,
        "upload-spool state violates the fail-closed boundary"
    );

    let all_pending = storage
        .get_pending_events(10_000)
        .await
        .context("read upload-spool status from SQLite")?;
    let pending = matching_events(&all_pending, &state.expected_storage_ids);
    let pending_ids = event_ids(&pending);
    let (phase, storage_id_preserved, last_error) = match state.phase.as_str() {
        "interrupted" => (
            "interrupted",
            exact_ids(&state.expected_storage_ids, &pending_ids)
                && exact_ids(&state.pending_storage_ids, &pending_ids)
                && state.confirmed_storage_ids.is_empty()
                && !state.sent_markers_written_after_success,
            Some("synthetic_transport_interrupted"),
        ),
        "verified" => (
            "recovered",
            pending_ids.is_empty()
                && state.pending_storage_ids.is_empty()
                && exact_ids(&state.expected_storage_ids, &state.confirmed_storage_ids)
                && state.sent_markers_written_after_success,
            None,
        ),
        phase => bail!("unsupported upload-spool fixture phase: {phase}"),
    };

    Ok(UploadSpoolStatus {
        phase,
        pending_count: pending.len(),
        attempt_count: state.upload_attempts,
        reprime_count: state.upload_attempts,
        sent_marker_count: state.confirmed_storage_ids.len(),
        storage_id_preserved,
        network_disabled: !state.external_egress_enabled && state.synthetic_transport,
        real_account_used: false,
        last_error,
    })
}

fn synthetic_events() -> Vec<Event> {
    [0_i64, 1_i64]
        .into_iter()
        .map(|offset| {
            Event::Context(ContextEvent {
                app_name: "maekon-qc-upload-spool".to_string(),
                window_title: format!("Synthetic interrupted upload {offset}"),
                prev_app_name: None,
                timestamp: Utc
                    .timestamp_opt(1_768_780_800 + offset, 0)
                    .single()
                    .expect("fixed QC timestamp must be valid"),
                ..Default::default()
            })
        })
        .collect()
}

fn queued_uploads(events: &[Event]) -> Vec<QueuedUpload> {
    events
        .iter()
        .cloned()
        .map(|event| QueuedUpload {
            storage_id: storage_event_id(&event),
            event,
        })
        .collect()
}

fn event_ids(events: &[Event]) -> Vec<String> {
    let mut ids: Vec<String> = events.iter().map(storage_event_id).collect();
    ids.sort();
    ids
}

fn matching_events(events: &[Event], expected_ids: &[String]) -> Vec<Event> {
    events
        .iter()
        .filter(|event| expected_ids.contains(&storage_event_id(event)))
        .cloned()
        .collect()
}

fn exact_ids(expected: &[String], actual: &[String]) -> bool {
    let mut expected = expected.to_vec();
    let mut actual = actual.to_vec();
    expected.sort();
    actual.sort();
    actual == expected
}

fn ensure_exact_ids(expected: &[String], actual: &[String], label: &str) -> Result<()> {
    let mut expected = expected.to_vec();
    let mut actual = actual.to_vec();
    expected.sort();
    actual.sort();
    ensure!(
        actual == expected,
        "{label} mismatch: expected={expected:?} actual={actual:?}"
    );
    Ok(())
}

fn egress_ledger_count(storage: &SqliteStorage) -> Result<u64> {
    let connection = storage.connection_arc();
    let connection = connection.read_lock();
    connection
        .conn()
        .query_row("SELECT COUNT(*) FROM egress_ledger", [], |row| {
            row.get::<_, u64>(0)
        })
        .context("count external egress ledger entries")
}

fn write_state(data_dir: &Path, state: &UploadSpoolState) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(state).context("serialize upload-spool state")?;
    std::fs::write(data_dir.join(STATE_FILE), bytes).context("write upload-spool state")
}

fn read_state(data_dir: &Path) -> Result<UploadSpoolState> {
    let bytes =
        std::fs::read(data_dir.join(STATE_FILE)).context("read interrupted upload-spool state")?;
    serde_json::from_slice(&bytes).context("parse interrupted upload-spool state")
}

fn require_fixture_gates() -> Result<()> {
    require_exact_gate(DEBUG_GATE_ENV)?;
    require_exact_gate(ISOLATED_GATE_ENV)?;
    require_exact_gate(FIXTURE_GATE_ENV)?;
    let confirm = std::env::var(CONFIRM_ENV).ok();
    ensure!(
        confirm.as_deref() == Some(CONFIRM_VALUE),
        "{CONFIRM_ENV}={CONFIRM_VALUE} is required"
    );
    let flavor = std::env::var(FLAVOR_ENV).context("MAEKON_APP_FLAVOR is required")?;
    ensure!(
        is_isolated_flavor(&flavor),
        "MAEKON_APP_FLAVOR must be a dedicated qc-* or tc-* flavor"
    );
    Ok(())
}

fn fixture_enabled() -> bool {
    let debug_gate = std::env::var(DEBUG_GATE_ENV).ok();
    let isolated_gate = std::env::var(ISOLATED_GATE_ENV).ok();
    let fixture_gate = std::env::var(FIXTURE_GATE_ENV).ok();
    let confirmation = std::env::var(CONFIRM_ENV).ok();
    let flavor = std::env::var(FLAVOR_ENV).ok();
    fixture_enabled_from(
        debug_gate.as_deref(),
        isolated_gate.as_deref(),
        fixture_gate.as_deref(),
        confirmation.as_deref(),
        flavor.as_deref(),
    )
}

fn fixture_enabled_from(
    debug_gate: Option<&str>,
    isolated_gate: Option<&str>,
    fixture_gate: Option<&str>,
    confirmation: Option<&str>,
    flavor: Option<&str>,
) -> bool {
    debug_gate == Some("1")
        && isolated_gate == Some("1")
        && fixture_gate == Some("1")
        && confirmation == Some(CONFIRM_VALUE)
        && flavor.is_some_and(is_isolated_flavor)
}

fn to_core_error(error: anyhow::Error) -> CoreError {
    CoreError::Internal {
        code: InternalCode::Generic,
        message: format!("isolated QC upload-spool fixture: {error:#}"),
    }
}

fn require_exact_gate(name: &str) -> Result<()> {
    let value = std::env::var(name).ok();
    ensure!(value.as_deref() == Some("1"), "{name}=1 is required");
    Ok(())
}

fn is_isolated_flavor(flavor: &str) -> bool {
    let flavor = flavor.trim();
    (flavor.starts_with("qc-") || flavor.starts_with("tc-"))
        && flavor.len() > 3
        && flavor
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn configure_isolated_profile(config: &mut AppConfig) {
    config.vision.capture_enabled = false;
    config.audio.enabled = false;
    config.audio.cloud_api_key.clear();
    config.audio.cloud_stt_policy = CloudSttPolicy::Disabled;
    config.sync.enabled = false;
    config.integration.enabled = false;
    config.telemetry.enabled = false;
    config.telemetry.crash_reports = false;
    config.telemetry.usage_analytics = false;
    config.telemetry.performance_metrics = false;
    config.web.allow_external = false;
    config.external_grpc.enabled = false;
    config.automation.enabled = false;
    config.update.enabled = false;
    config.update.auto_install = false;
}

fn ensure_isolated_profile(config: &AppConfig) -> Result<()> {
    ensure!(
        !config.vision.capture_enabled
            && !config.audio.enabled
            && config.audio.cloud_api_key.is_empty()
            && config.audio.cloud_stt_policy == CloudSttPolicy::Disabled
            && !config.sync.enabled
            && !config.integration.enabled
            && !config.telemetry.enabled
            && !config.telemetry.crash_reports
            && !config.telemetry.usage_analytics
            && !config.telemetry.performance_metrics
            && !config.web.allow_external
            && !config.external_grpc.enabled
            && !config.automation.enabled
            && !config.update.enabled
            && !config.update.auto_install,
        "isolated upload-spool profile has an egress, capture, sync, or automation capability enabled"
    );
    Ok(())
}

fn ensure_runtime_isolated_profile() -> Result<()> {
    let config = ConfigManager::new()
        .context("reload isolated upload-spool config")?
        .get();
    ensure_isolated_profile(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_exact_and_flavor_is_bounded() {
        assert!(prepare_command_requested([PREPARE_COMMAND].into_iter()));
        assert!(verify_command_requested([VERIFY_COMMAND].into_iter()));
        assert!(!prepare_command_requested(
            ["debug-prepare-qc-upload-spool-extra"].into_iter()
        ));
        assert!(is_isolated_flavor("qc-8568-upload-spool"));
        assert!(is_isolated_flavor("tc-upload_spool"));
        assert!(!is_isolated_flavor("dev"));
        assert!(!is_isolated_flavor("qc-../../real-profile"));
    }

    #[test]
    fn ui_surface_requires_every_exact_fixture_gate() {
        let enabled = [
            Some("1"),
            Some("1"),
            Some("1"),
            Some(CONFIRM_VALUE),
            Some("qc-8568-upload-spool-ui"),
        ];
        assert!(fixture_enabled_from(
            enabled[0], enabled[1], enabled[2], enabled[3], enabled[4]
        ));

        for missing in 0..enabled.len() {
            let mut values = enabled;
            values[missing] = None;
            assert!(
                !fixture_enabled_from(values[0], values[1], values[2], values[3], values[4]),
                "fixture unexpectedly enabled with gate {missing} missing"
            );
        }
        assert!(!fixture_enabled_from(
            Some("true"),
            Some("1"),
            Some("1"),
            Some(CONFIRM_VALUE),
            Some("qc-8568-upload-spool-ui")
        ));
        assert!(!fixture_enabled_from(
            Some("1"),
            Some("1"),
            Some("1"),
            Some("yes"),
            Some("qc-8568-upload-spool-ui")
        ));
        assert!(!fixture_enabled_from(
            Some("1"),
            Some("1"),
            Some("1"),
            Some(CONFIRM_VALUE),
            Some("production")
        ));
    }

    #[test]
    fn isolated_profile_disables_sensitive_capabilities() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = true;
        config.audio.enabled = true;
        config.audio.cloud_api_key = "synthetic-secret".to_string();
        config.sync.enabled = true;
        config.integration.enabled = true;
        config.telemetry.enabled = true;
        config.telemetry.crash_reports = true;
        config.telemetry.usage_analytics = true;
        config.telemetry.performance_metrics = true;
        config.web.allow_external = true;
        config.external_grpc.enabled = true;
        config.automation.enabled = true;
        config.update.enabled = true;
        config.update.auto_install = true;

        configure_isolated_profile(&mut config);

        ensure_isolated_profile(&config).expect("profile must be fail-closed");
        assert!(!config.update.enabled);
        assert!(!config.update.auto_install);
    }

    #[tokio::test]
    async fn interruption_then_reprime_marks_only_confirmed_ids() {
        let temp = tempfile::tempdir().expect("temp dir");
        let key = EncryptionKey::from_bytes([0x89; 32]);

        let interrupted = prepare_fixture(temp.path(), key.clone(), 30)
            .await
            .expect("prepare interrupted spool");
        assert_eq!(interrupted.phase, "interrupted");
        assert_eq!(interrupted.seeded, 2);
        assert_eq!(interrupted.confirmed, 0);
        assert_eq!(interrupted.pending, 2);
        assert_eq!(interrupted.upload_attempts, 1);
        assert_eq!(interrupted.egress_ledger_entries, 0);

        let interrupted_state = read_state(temp.path()).expect("read interrupted state");
        assert!(interrupted_state.confirmed_storage_ids.is_empty());
        assert!(!interrupted_state.sent_markers_written_after_success);
        assert_eq!(interrupted_state.pending_storage_ids.len(), 2);

        let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
            .expect("reopen interrupted spool for unrelated metric");
        storage
            .save_event(&Event::Context(ContextEvent {
                app_name: "maekon-qc-os-metric".to_string(),
                window_title: "Synthetic unrelated OS metric".to_string(),
                timestamp: Utc
                    .timestamp_opt(1_768_780_900, 0)
                    .single()
                    .expect("fixed QC timestamp must be valid"),
                ..Default::default()
            }))
            .await
            .expect("persist unrelated metric");
        drop(storage);

        let verified = verify_fixture(temp.path(), key.clone(), 30)
            .await
            .expect("verify re-primed spool");
        assert_eq!(verified.phase, "verified");
        assert_eq!(verified.seeded, 2);
        assert_eq!(verified.confirmed, 2);
        assert_eq!(verified.pending, 0);
        assert_eq!(verified.upload_attempts, 2);
        assert_eq!(verified.egress_ledger_entries, 0);

        let verified_state = read_state(temp.path()).expect("read verified state");
        assert!(verified_state.sent_markers_written_after_success);
        assert_eq!(verified_state.confirmed_storage_ids.len(), 2);
        assert!(verified_state.pending_storage_ids.is_empty());
        assert!(!verified_state.external_egress_enabled);
        assert!(!verified_state.host_mutation);

        let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
            .expect("reopen verified spool");
        let remaining = storage
            .get_pending_events(10)
            .await
            .expect("read unrelated pending metric");
        assert_eq!(remaining.len(), 1);
        assert_eq!(
            event_ids(&remaining),
            event_ids(&[Event::Context(ContextEvent {
                app_name: "maekon-qc-os-metric".to_string(),
                window_title: "Synthetic unrelated OS metric".to_string(),
                timestamp: Utc
                    .timestamp_opt(1_768_780_900, 0)
                    .single()
                    .expect("fixed QC timestamp must be valid"),
                ..Default::default()
            })])
        );
    }
}
