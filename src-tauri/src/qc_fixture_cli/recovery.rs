//! `debug-*-qc-legacy-migration` isolated fixture — CJ-05-14 recovery journey.
//!
//! After the ADR-003 directory-module split (#8765), the legacy-migration
//! recovery fixture was relocated from the single `qc_fixture_cli.rs` into this
//! module. Behavior and contract are unchanged.

use std::fmt;
use std::path::Path;

use anyhow::{bail, Context, Result};
use maekon_core::config::{AppConfig, CloudSttPolicy};
use maekon_core::config_manager::ConfigManager;
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::sqlite::SqliteStorage;
use serde::Serialize;

use crate::storage_runtime::resolve_shared_master_key;

use super::{require_exact_gate, require_isolated_profile};

const LEGACY_MIGRATION_PREPARE_COMMAND: &str = "debug-prepare-qc-legacy-migration";
const LEGACY_MIGRATION_VERIFY_COMMAND: &str = "debug-verify-qc-legacy-migration";
const LEGACY_MIGRATION_GATE_ENV: &str = "MAEKON_DEBUG_QC_LEGACY_MIGRATION_FIXTURE";
const LEGACY_MIGRATION_CONFIRM_ENV: &str = "MAEKON_QC_LEGACY_MIGRATION_CONFIRM";
const LEGACY_MIGRATION_STATE_FILE: &str = "qc-legacy-migration-state.json";
const LEGACY_MIGRATION_MARKER_KEY: &str = "qc_legacy_migration_invariant";
const LEGACY_MIGRATION_MARKER_VALUE: &str = "synthetic-cj-05-14-v1";
const LEGACY_MIGRATION_SOURCE_VERSION: u32 = 47;
// #9083: the migrated-fixture target is whatever schema the product currently
// migrates to. Hardcoding 48 here silently broke when V49-V52 landed; track
// the runner's own constant instead so version advancement cannot desync.
const LEGACY_MIGRATION_TARGET_VERSION: u32 = maekon_storage::migration::CURRENT_VERSION;
const LEGACY_MIGRATION_CONFIRM_VALUE: &str = "prepare-v47";
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LegacyMigrationReport {
    data_dir: String,
    phase: &'static str,
    source_version: u32,
    target_version: u32,
    invariant_preserved: bool,
    backup_count: usize,
    egress_ledger_entries: u64,
}

#[derive(Debug, Serialize)]
struct LegacyMigrationState<'a> {
    schema: &'static str,
    phase: &'a str,
    source_schema_version: u32,
    target_schema_version: u32,
    invariant_preserved: bool,
    target_table_present: bool,
    product_open_observed: bool,
    pre_migration_backup_count: usize,
    external_egress_enabled: bool,
    egress_ledger_entries: u64,
    host_mutation: bool,
}

impl fmt::Display for LegacyMigrationReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "QC legacy migration fixture {}: data_dir={} source_version={} target_version={} invariant_preserved={} backup_count={} egress_ledger_entries={}",
            self.phase,
            self.data_dir,
            self.source_version,
            self.target_version,
            self.invariant_preserved,
            self.backup_count,
            self.egress_ledger_entries
        )
    }
}

pub(crate) fn legacy_migration_prepare_command_requested<'a>(
    args: impl Iterator<Item = &'a str>,
) -> bool {
    args.into_iter().next() == Some(LEGACY_MIGRATION_PREPARE_COMMAND)
}

pub(crate) fn legacy_migration_verify_command_requested<'a>(
    args: impl Iterator<Item = &'a str>,
) -> bool {
    args.into_iter().next() == Some(LEGACY_MIGRATION_VERIFY_COMMAND)
}

fn require_exact_value(name: &str, expected: &str) -> Result<()> {
    if !exact_value_enabled(std::env::var(name).ok().as_deref(), expected) {
        bail!("{name}={expected} is required")
    }
    Ok(())
}

fn exact_value_enabled(value: Option<&str>, expected: &str) -> bool {
    value == Some(expected)
}

pub(crate) fn run_legacy_migration_prepare_from_env() -> Result<LegacyMigrationReport> {
    require_isolated_profile()?;
    require_exact_gate(LEGACY_MIGRATION_GATE_ENV)?;
    require_exact_value(LEGACY_MIGRATION_CONFIRM_ENV, LEGACY_MIGRATION_CONFIRM_VALUE)?;

    let config = ConfigManager::new()
        .context("initialize isolated config")?
        .update_with(|config| {
            configure_recovery_fixture(config);
            Ok(())
        })
        .context("persist isolated QC recovery config")?;
    ensure_recovery_fixture_isolated(&config)?;
    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    std::fs::create_dir_all(&data_dir).context("create isolated data directory")?;
    let encryption_key =
        resolve_shared_master_key(&data_dir).context("resolve isolated profile encryption key")?;

    prepare_legacy_migration_fixture(&data_dir, encryption_key, config.storage.retention_days)
}

pub(crate) fn run_legacy_migration_verify_from_env() -> Result<LegacyMigrationReport> {
    require_isolated_profile()?;
    require_exact_gate(LEGACY_MIGRATION_GATE_ENV)?;

    let config = ConfigManager::new()
        .context("initialize isolated config")?
        .get();
    ensure_recovery_fixture_isolated(&config)?;
    let data_dir = ConfigManager::data_dir().context("resolve isolated data directory")?;
    let encryption_key =
        resolve_shared_master_key(&data_dir).context("resolve isolated profile encryption key")?;

    verify_legacy_migration_fixture(&data_dir, encryption_key, config.storage.retention_days)
}

fn prepare_legacy_migration_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
) -> Result<LegacyMigrationReport> {
    let db_path = data_dir.join(maekon_storage::encryption::SQLCIPHER_DB_FILENAME);
    if db_path.exists() {
        bail!(
            "legacy migration preparation requires a fresh isolated profile; discard this qc-*/tc-* profile and retry"
        )
    }

    // #9083: open the database migrated only up to v47 so the fixture is a
    // REAL legacy database. The previous approach migrated fully and then
    // deleted v48 artifacts, which silently broke once V49-V52 advanced
    // CURRENT_VERSION past the hardcoded seal.
    let storage = SqliteStorage::open_with_max_schema_version(
        &db_path,
        retention_days,
        Some(&encryption_key),
        Some(LEGACY_MIGRATION_SOURCE_VERSION),
    )
    .context("create isolated encrypted v47 QC database")?;
    storage
        .set_meta_checked(LEGACY_MIGRATION_MARKER_KEY, LEGACY_MIGRATION_MARKER_VALUE)
        .context("seed legacy migration invariant")?;
    storage
        .set_meta_checked("onboarding_completed", "true")
        .context("mark isolated fixture onboarding complete")?;

    let (schema_version, invariant_preserved, target_table_present, egress_ledger_entries) =
        inspect_legacy_migration_state(&storage)?;
    if schema_version != LEGACY_MIGRATION_SOURCE_VERSION
        || !invariant_preserved
        || target_table_present
        || egress_ledger_entries != 0
    {
        bail!(
            "legacy fixture invariant mismatch: version={schema_version} invariant_preserved={invariant_preserved} target_table_present={target_table_present} egress_ledger_entries={egress_ledger_entries}"
        )
    }
    drop(storage);

    let backup_count = legacy_migration_backup_count(data_dir)?;
    if backup_count != 0 {
        bail!("fresh legacy fixture unexpectedly has a v47 migration backup")
    }
    write_legacy_migration_state(
        data_dir,
        LegacyMigrationState {
            schema: "maekon.qc-legacy-migration-state.v1",
            phase: "prepared",
            source_schema_version: LEGACY_MIGRATION_SOURCE_VERSION,
            target_schema_version: LEGACY_MIGRATION_TARGET_VERSION,
            invariant_preserved,
            target_table_present,
            product_open_observed: false,
            pre_migration_backup_count: backup_count,
            external_egress_enabled: false,
            egress_ledger_entries,
            host_mutation: false,
        },
    )?;

    Ok(LegacyMigrationReport {
        data_dir: data_dir.display().to_string(),
        phase: "prepared",
        source_version: LEGACY_MIGRATION_SOURCE_VERSION,
        target_version: LEGACY_MIGRATION_TARGET_VERSION,
        invariant_preserved,
        backup_count,
        egress_ledger_entries,
    })
}

fn verify_legacy_migration_fixture(
    data_dir: &Path,
    encryption_key: EncryptionKey,
    retention_days: u32,
) -> Result<LegacyMigrationReport> {
    let backup_count = legacy_migration_backup_count(data_dir)?;
    if backup_count == 0 {
        bail!(
            "no v47 pre-migration backup exists; launch and cleanly stop the isolated Maekon app before verification"
        )
    }

    let db_path = data_dir.join(maekon_storage::encryption::SQLCIPHER_DB_FILENAME);
    let storage = SqliteStorage::open(&db_path, retention_days, Some(&encryption_key))
        .context("reopen migrated isolated QC database")?;
    let (schema_version, invariant_preserved, target_table_present, egress_ledger_entries) =
        inspect_legacy_migration_state(&storage)?;
    if schema_version != LEGACY_MIGRATION_TARGET_VERSION
        || !invariant_preserved
        || !target_table_present
        || egress_ledger_entries != 0
    {
        bail!(
            "migrated fixture invariant mismatch: version={schema_version} invariant_preserved={invariant_preserved} target_table_present={target_table_present} egress_ledger_entries={egress_ledger_entries}"
        )
    }

    write_legacy_migration_state(
        data_dir,
        LegacyMigrationState {
            schema: "maekon.qc-legacy-migration-state.v1",
            phase: "verified",
            source_schema_version: LEGACY_MIGRATION_SOURCE_VERSION,
            target_schema_version: schema_version,
            invariant_preserved,
            target_table_present,
            product_open_observed: true,
            pre_migration_backup_count: backup_count,
            external_egress_enabled: false,
            egress_ledger_entries,
            host_mutation: false,
        },
    )?;

    Ok(LegacyMigrationReport {
        data_dir: data_dir.display().to_string(),
        phase: "verified",
        source_version: LEGACY_MIGRATION_SOURCE_VERSION,
        target_version: schema_version,
        invariant_preserved,
        backup_count,
        egress_ledger_entries,
    })
}

fn inspect_legacy_migration_state(storage: &SqliteStorage) -> Result<(u32, bool, bool, u64)> {
    let connection = storage.connection_arc();
    let connection = connection.read_lock();
    let schema_version = connection
        .conn()
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get::<_, u32>(0),
        )
        .context("read fixture schema version")?;
    let marker = connection
        .conn()
        .query_row(
            "SELECT value FROM app_meta WHERE key = ?1",
            [LEGACY_MIGRATION_MARKER_KEY],
            |row| row.get::<_, String>(0),
        )
        .context("read legacy migration invariant")?;
    let target_table_present = connection
        .conn()
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'pomodoro_state'",
            [],
            |row| row.get::<_, bool>(0),
        )
        .context("inspect v48 target table")?;
    let egress_ledger_entries = connection
        .conn()
        .query_row("SELECT COUNT(*) FROM egress_ledger", [], |row| {
            row.get::<_, u64>(0)
        })
        .context("count external egress ledger entries")?;
    Ok((
        schema_version,
        marker == LEGACY_MIGRATION_MARKER_VALUE,
        target_table_present,
        egress_ledger_entries,
    ))
}

fn legacy_migration_backup_count(data_dir: &Path) -> Result<usize> {
    let prefix = format!("maekon.backup.v{}.", LEGACY_MIGRATION_SOURCE_VERSION);
    let count = std::fs::read_dir(data_dir)
        .context("enumerate isolated migration backups")?
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .count();
    Ok(count)
}

fn write_legacy_migration_state(data_dir: &Path, state: LegacyMigrationState<'_>) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&state).context("serialize migration fixture state")?;
    std::fs::write(data_dir.join(LEGACY_MIGRATION_STATE_FILE), bytes)
        .context("write migration fixture state")
}

pub(super) fn configure_recovery_fixture(config: &mut AppConfig) {
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

fn ensure_recovery_fixture_isolated(config: &AppConfig) -> Result<()> {
    let mut enabled_fields = Vec::new();
    for (field, enabled) in [
        ("vision.capture_enabled", config.vision.capture_enabled),
        ("audio.enabled", config.audio.enabled),
        (
            "audio.cloud_api_key_present",
            !config.audio.cloud_api_key.is_empty(),
        ),
        (
            "audio.cloud_stt_policy_external",
            config.audio.cloud_stt_policy != CloudSttPolicy::Disabled,
        ),
        ("sync.enabled", config.sync.enabled),
        ("integration.enabled", config.integration.enabled),
        ("telemetry.enabled", config.telemetry.enabled),
        ("telemetry.crash_reports", config.telemetry.crash_reports),
        (
            "telemetry.usage_analytics",
            config.telemetry.usage_analytics,
        ),
        (
            "telemetry.performance_metrics",
            config.telemetry.performance_metrics,
        ),
        ("web.allow_external", config.web.allow_external),
        ("external_grpc.enabled", config.external_grpc.enabled),
        ("automation.enabled", config.automation.enabled),
        ("update.enabled", config.update.enabled),
        ("update.auto_install", config.update.auto_install),
    ] {
        if enabled {
            enabled_fields.push(field);
        }
    }

    if !enabled_fields.is_empty() {
        bail!(
            "isolated QC recovery config was not preserved: {}",
            enabled_fields.join(", ")
        )
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_migration_command_flags_are_explicit() {
        assert!(legacy_migration_prepare_command_requested(
            [LEGACY_MIGRATION_PREPARE_COMMAND].into_iter()
        ));
        assert!(legacy_migration_verify_command_requested(
            [LEGACY_MIGRATION_VERIFY_COMMAND].into_iter()
        ));
        assert!(!legacy_migration_prepare_command_requested(
            [LEGACY_MIGRATION_VERIFY_COMMAND].into_iter()
        ));
        assert!(!legacy_migration_verify_command_requested(
            [LEGACY_MIGRATION_PREPARE_COMMAND].into_iter()
        ));
        assert!(exact_value_enabled(Some("prepare-v47"), "prepare-v47"));
        assert!(!exact_value_enabled(None, "prepare-v47"));
        assert!(!exact_value_enabled(Some("prepare-v48"), "prepare-v47"));
        assert!(!exact_value_enabled(Some(" prepare-v47"), "prepare-v47"));
    }

    #[test]
    fn recovery_fixture_is_local_only_and_egress_closed() {
        let mut config = AppConfig::default_config();
        config.vision.capture_enabled = true;
        config.audio.enabled = true;
        config.audio.cloud_api_key = "synthetic-secret-must-be-cleared".into();
        config.sync.enabled = true;
        config.integration.enabled = true;
        config.telemetry.enabled = true;
        config.web.allow_external = true;
        config.external_grpc.enabled = true;
        config.automation.enabled = true;
        config.update.enabled = true;
        config.update.auto_install = true;

        configure_recovery_fixture(&mut config);

        ensure_recovery_fixture_isolated(&config).expect("fixture must remain isolated");
        assert!(!config.vision.capture_enabled);
        assert!(!config.audio.enabled);
        assert!(config.audio.cloud_api_key.is_empty());
        assert_eq!(config.audio.cloud_stt_policy, CloudSttPolicy::Disabled);
        assert!(!config.sync.enabled);
        assert!(!config.integration.enabled);
        assert!(!config.telemetry.enabled);
        assert!(!config.web.allow_external);
        assert!(!config.external_grpc.enabled);
        assert!(!config.automation.enabled);
        assert!(!config.update.enabled);
        assert!(!config.update.auto_install);
    }

    #[test]
    fn legacy_migration_fixture_requires_product_open_before_verification() {
        let temp = tempfile::tempdir().expect("temp dir");
        let key = EncryptionKey::from_bytes([0x87; 32]);
        let prepared = prepare_legacy_migration_fixture(temp.path(), key.clone(), 30)
            .expect("prepare v47 fixture");
        assert_eq!(prepared.phase, "prepared");
        assert_eq!(prepared.source_version, 47);
        assert_eq!(prepared.target_version, LEGACY_MIGRATION_TARGET_VERSION);
        assert!(prepared.invariant_preserved);
        assert_eq!(prepared.backup_count, 0);
        assert_eq!(prepared.egress_ledger_entries, 0);

        let error = verify_legacy_migration_fixture(temp.path(), key, 30)
            .expect_err("verification must not perform the product migration itself");
        assert!(error.to_string().contains("launch and cleanly stop"));
    }

    #[test]
    fn legacy_migration_fixture_exercises_production_open_and_preserves_invariant() {
        let temp = tempfile::tempdir().expect("temp dir");
        let key = EncryptionKey::from_bytes([0x88; 32]);
        prepare_legacy_migration_fixture(temp.path(), key.clone(), 30)
            .expect("prepare v47 fixture");

        let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
            .expect("simulate production app open");
        drop(storage);

        let verified =
            verify_legacy_migration_fixture(temp.path(), key, 30).expect("verify migrated fixture");
        assert_eq!(verified.phase, "verified");
        assert_eq!(verified.source_version, 47);
        assert_eq!(verified.target_version, LEGACY_MIGRATION_TARGET_VERSION);
        assert!(verified.invariant_preserved);
        assert_eq!(verified.backup_count, 1);
        assert_eq!(verified.egress_ledger_entries, 0);

        let state = std::fs::read_to_string(temp.path().join(LEGACY_MIGRATION_STATE_FILE))
            .expect("read state file");
        assert!(state.contains("\"phase\": \"verified\""));
        assert!(state.contains("\"product_open_observed\": true"));
        assert!(state.contains("\"external_egress_enabled\": false"));
        assert!(state.contains("\"egress_ledger_entries\": 0"));
        assert!(state.contains("\"host_mutation\": false"));
    }

    #[test]
    fn recovery_fixture_guard_names_unsafe_fields_without_secret_values() {
        let mut config = AppConfig::default_config();
        configure_recovery_fixture(&mut config);
        config.audio.cloud_stt_policy = CloudSttPolicy::Allow;
        config.audio.cloud_api_key = "must-not-appear-in-diagnostics".into();
        config.update.enabled = true;

        let error = ensure_recovery_fixture_isolated(&config)
            .expect_err("unsafe recovery settings must be rejected")
            .to_string();

        assert!(error.contains("audio.cloud_stt_policy_external"));
        assert!(error.contains("audio.cloud_api_key_present"));
        assert!(error.contains("update.enabled"));
        assert!(!error.contains("must-not-appear-in-diagnostics"));
    }
}
