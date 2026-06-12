use super::*;
use chrono::Duration as ChronoDuration;
use tempfile::tempdir;

fn write_pending(dir: &Path, version: &str, installed_at: &str, previous: &str, backup: &Path) {
    let pending = InstallPending {
        installed_at: installed_at.to_string(),
        previous_version: previous.to_string(),
        backup_path: backup.to_path_buf(),
    };
    let bytes = serde_json::to_vec(&pending).unwrap();
    std::fs::write(dir.join(format!(".install_pending_{version}")), bytes).unwrap();
}

fn write_boot_count(dir: &Path, version: &str, count: u32) {
    // Legacy single-file format — used only by tests that exercise migration cleanup.
    std::fs::write(
        dir.join(format!(".boot_count_{version}")),
        count.to_string(),
    )
    .unwrap();
}

fn write_boot_count_pid_marker(dir: &Path, version: &str, pid: u32) {
    std::fs::write(dir.join(format!(".boot_count_pid_{version}_{pid}")), b"").unwrap();
}

fn write_boot_count_pids(dir: &Path, version: &str, count: u32) {
    for i in 0..count {
        write_boot_count_pid_marker(dir, version, 10000 + i);
    }
}

fn write_self_healthy(dir: &Path, version: &str) {
    std::fs::write(
        dir.join(format!(".self_healthy_{version}")),
        Utc::now().to_rfc3339(),
    )
    .unwrap();
}

#[test]
fn check_startup_no_pending_install_is_normal() {
    let dir = tempdir().unwrap();
    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
    assert_eq!(probe.check_startup_state(), StartupAction::Normal);
}

#[test]
fn check_startup_with_healthy_marker_is_normal() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("maekon.rollback.1");
    std::fs::write(&backup, b"backup-bytes").unwrap();
    write_pending(
        dir.path(),
        "0.5.0",
        &Utc::now().to_rfc3339(),
        "0.4.39",
        &backup,
    );
    write_self_healthy(dir.path(), "0.5.0");

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
    assert_eq!(probe.check_startup_state(), StartupAction::Normal);
}

#[test]
fn check_startup_below_failed_boot_threshold_is_normal() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("maekon.rollback.1");
    std::fs::write(&backup, b"backup-bytes").unwrap();
    write_pending(
        dir.path(),
        "0.5.0",
        &Utc::now().to_rfc3339(),
        "0.4.39",
        &backup,
    );

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
    assert_eq!(probe.check_startup_state(), StartupAction::Normal);

    assert_eq!(probe.boot_count().unwrap(), 1);
}

#[test]
fn check_startup_at_failed_boot_threshold_triggers_rollback() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("maekon.rollback.1");
    std::fs::write(&backup, b"backup-bytes").unwrap();
    write_pending(
        dir.path(),
        "0.5.0",
        &Utc::now().to_rfc3339(),
        "0.4.39",
        &backup,
    );
    write_boot_count_pids(dir.path(), "0.5.0", 2);

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
    match probe.check_startup_state() {
        StartupAction::RollbackRequired {
            from_version,
            to_version,
            backup_path,
            reason,
        } => {
            assert_eq!(from_version, "0.5.0");
            assert_eq!(to_version, "0.4.39");
            assert_eq!(backup_path, backup);
            assert_eq!(reason, RollbackReason::RepeatedStartupFailure);
        }
        other => panic!("Expected RollbackRequired, got {:?}", other),
    }

    assert_eq!(probe.boot_count().unwrap(), 2);
}

#[test]
fn stale_install_pending_older_than_24h_returns_normal_without_rollback() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("maekon.rollback.1");
    std::fs::write(&backup, b"backup-bytes").unwrap();
    let old_ts = (Utc::now() - ChronoDuration::hours(25)).to_rfc3339();
    write_pending(dir.path(), "0.5.0", &old_ts, "0.4.39", &backup);
    write_boot_count(dir.path(), "0.5.0", 5);

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
    assert_eq!(probe.check_startup_state(), StartupAction::Normal);

    assert!(!probe.install_pending_path().exists());
    assert_eq!(probe.boot_count().unwrap(), 0);
    assert!(
        !probe.legacy_boot_count_path().exists(),
        "legacy single-file must be deleted during staleness cleanup"
    );
}

#[tokio::test]
async fn spawn_healthy_writer_sets_marker_after_injected_short_delay() {
    let dir = tempdir().unwrap();
    let backup = dir.path().join("maekon.rollback.1");
    std::fs::write(&backup, b"backup-bytes").unwrap();
    write_pending(
        dir.path(),
        "0.5.0",
        &Utc::now().to_rfc3339(),
        "0.4.39",
        &backup,
    );
    write_boot_count_pids(dir.path(), "0.5.0", 2);

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into())
        .with_threshold(std::time::Duration::from_millis(50));

    let runtime_handle = tokio::runtime::Handle::current();
    let join = probe.spawn_healthy_writer(&runtime_handle);
    join.await.unwrap();

    let marker = dir.path().join(".self_healthy_0.5.0");
    assert!(marker.exists(), "healthy marker should have been written");
    assert!(!probe.install_pending_path().exists());
    assert_eq!(probe.boot_count().unwrap(), 0);
    assert!(backup.exists(), "canonical backup should remain");
}

/// Regression test for the v0.4.40-rc.1 macOS launch panic
/// ("there is no reactor running") when the Tauri `setup` callback
/// invokes `spawn_healthy_writer` synchronously before the tokio runtime
/// is entered. The explicit `&Handle` parameter fixes this.
#[test]
fn spawn_healthy_writer_does_not_panic_outside_async_context() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let handle = runtime.handle().clone();

    // lint:allow-is-err-hedge — TryCurrentError is a unit struct with no payload; the error fact itself is the precondition
    assert!(
        tokio::runtime::Handle::try_current().is_err(),
        "test must run outside any tokio runtime context"
    );

    let dir = tempdir().unwrap();
    write_pending(
        dir.path(),
        "0.5.0",
        &Utc::now().to_rfc3339(),
        "0.4.39",
        &dir.path().join("maekon.rollback.1"),
    );

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into())
        .with_threshold(std::time::Duration::from_millis(50));

    let join = probe.spawn_healthy_writer(&handle);

    runtime.block_on(async {
        join.await.unwrap();
    });

    let marker = dir.path().join(".self_healthy_0.5.0");
    assert!(marker.exists(), "healthy marker should have been written");
}

#[test]
fn healthy_writer_cleanup_sweeps_foreign_version_state_files() {
    let dir = tempdir().unwrap();
    let current_version = "0.5.0";

    write_pending(
        dir.path(),
        current_version,
        &Utc::now().to_rfc3339(),
        "0.4.40",
        &dir.path().join("nonexistent-backup"),
    );

    std::fs::write(dir.path().join(".install_pending_0.4.40"), "stale-content").unwrap();
    std::fs::write(dir.path().join(".boot_count_0.4.40"), "2").unwrap();
    write_boot_count_pid_marker(dir.path(), "0.4.40", 100);
    write_boot_count_pid_marker(dir.path(), "0.4.40", 200);
    std::fs::write(
        dir.path().join(".self_healthy_0.4.40"),
        Utc::now().to_rfc3339(),
    )
    .unwrap();

    std::fs::write(dir.path().join("unrelated.txt"), "keep me").unwrap();

    write_self_healthy_and_cleanup(dir.path(), current_version).unwrap();

    assert!(dir.path().join(".self_healthy_0.5.0").exists());
    assert!(!dir.path().join(".install_pending_0.5.0").exists());
    assert!(!dir.path().join(".boot_count_0.5.0").exists());

    assert!(!dir.path().join(".install_pending_0.4.40").exists());
    assert!(!dir.path().join(".boot_count_0.4.40").exists());
    assert!(!dir.path().join(".boot_count_pid_0.4.40_100").exists());
    assert!(!dir.path().join(".boot_count_pid_0.4.40_200").exists());
    assert!(!dir.path().join(".self_healthy_0.4.40").exists());

    assert!(dir.path().join("unrelated.txt").exists());
}

#[test]
fn probe_io_error_is_non_fatal() {
    let dir = tempdir().unwrap();
    let pending_path = dir.path().join(".install_pending_0.5.0");
    std::fs::write(&pending_path, b"NOT VALID JSON {{{").unwrap();

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
    assert_eq!(probe.check_startup_state(), StartupAction::Normal);
}

#[test]
fn concurrent_boot_count_no_undercount() {
    let dir = tempdir().unwrap();
    let version = "0.5.0";

    write_boot_count_pid_marker(dir.path(), version, 100);
    write_boot_count_pid_marker(dir.path(), version, 200);

    let probe = HealthProbe::new(dir.path().to_path_buf(), version.into());
    assert_eq!(probe.boot_count().unwrap(), 2);
}

#[test]
fn cleanup_boot_count_markers_removes_per_pid_and_legacy_files() {
    let dir = tempdir().unwrap();
    let version = "0.5.0";

    write_boot_count_pids(dir.path(), version, 3);
    write_boot_count(dir.path(), version, 7);

    let probe = HealthProbe::new(dir.path().to_path_buf(), version.into());
    assert_eq!(probe.boot_count().unwrap(), 3);

    probe.cleanup_boot_count_markers().unwrap();

    assert_eq!(probe.boot_count().unwrap(), 0);
    assert!(
        !probe.legacy_boot_count_path().exists(),
        "legacy single-file must be removed"
    );
}

#[test]
fn legacy_single_file_removed_by_startup_migration() {
    // count=99 would trigger rollback if mistakenly read.
    let dir = tempdir().unwrap();
    let backup = dir.path().join("maekon.rollback.1");
    std::fs::write(&backup, b"backup-bytes").unwrap();
    write_pending(
        dir.path(),
        "0.5.0",
        &Utc::now().to_rfc3339(),
        "0.4.39",
        &backup,
    );
    write_boot_count(dir.path(), "0.5.0", 99);

    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
    assert_eq!(
        probe.check_startup_state(),
        StartupAction::Normal,
        "migration must discard the legacy count, not parse it"
    );

    assert!(
        !probe.legacy_boot_count_path().exists(),
        "legacy single-file must be removed during migration"
    );
    assert_eq!(
        probe.boot_count().unwrap(),
        1,
        "this boot is recorded via the new per-PID format"
    );
}
