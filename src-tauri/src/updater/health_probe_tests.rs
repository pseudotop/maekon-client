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
fn macos_app_executable_uses_mutable_data_dir_for_health_state() {
    let root = tempdir().unwrap();
    let app_bundle = root.path().join("Maekon Dev.app");
    let current_exe = app_bundle.join("Contents/MacOS/maekon");
    let app_data_dir = root.path().join("profile/data");

    let state_dir = resolve_health_state_dir(&current_exe, Some(&app_data_dir)).unwrap();

    assert_eq!(state_dir, app_data_dir.join(HEALTH_STATE_DIR_NAME));
    assert!(
        !state_dir.starts_with(&app_bundle),
        "mutable updater state must never be placed inside a signed app bundle"
    );
}

#[test]
fn macos_app_executable_without_data_dir_fails_closed() {
    let current_exe = Path::new("/Applications/Maekon.app/Contents/MacOS/maekon");
    let error = resolve_health_state_dir(current_exe, None).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn loose_binary_keeps_adjacent_health_state_layout() {
    let current_exe = Path::new("/opt/maekon/bin/maekon");
    let state_dir = resolve_health_state_dir(current_exe, None).unwrap();

    assert_eq!(state_dir, Path::new("/opt/maekon/bin"));
}

#[test]
fn macos_app_detector_rejects_near_miss_directory_layouts() {
    assert!(!is_macos_app_executable(Path::new(
        "/Applications/Maekon.app/NotContents/MacOS/maekon"
    )));
    assert!(!is_macos_app_executable(Path::new(
        "/Applications/Maekon.app/Contents/NotMacOS/maekon"
    )));
}

#[test]
fn health_probe_uses_versioned_state_file_names() {
    let dir = tempdir().unwrap();
    let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());

    assert_eq!(
        probe.legacy_boot_count_path(),
        dir.path().join(".boot_count_0.5.0")
    );
    assert_eq!(
        probe.self_healthy_path(),
        dir.path().join(".self_healthy_0.5.0")
    );
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
fn clean_shutdown_before_healthy_threshold_disarms_boot_marker() {
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

    probe.mark_clean_shutdown().unwrap();

    assert!(dir.path().join(".self_healthy_0.5.0").exists());
    assert!(!probe.install_pending_path().exists());
    assert_eq!(
        probe.boot_count().unwrap(),
        0,
        "a clean sub-threshold exit must not count as a failed boot on next launch"
    );
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

#[test]
fn healthy_writer_cleans_executable_backups_when_state_is_elsewhere() {
    let state_dir = tempdir().unwrap();
    let install_dir = tempdir().unwrap();
    let keep_backup = install_dir.path().join("maekon.rollback.keep");
    let stale_backup = install_dir.path().join("maekon.rollback.stale");
    std::fs::write(&keep_backup, b"keep").unwrap();
    std::fs::write(&stale_backup, b"stale").unwrap();
    write_pending(
        state_dir.path(),
        "0.5.0",
        &Utc::now().to_rfc3339(),
        "0.4.39",
        &keep_backup,
    );

    write_self_healthy_and_cleanup(state_dir.path(), install_dir.path(), "0.5.0").unwrap();

    assert!(
        keep_backup.exists(),
        "canonical rollback backup must remain"
    );
    assert!(
        !stale_backup.exists(),
        "stale sibling backup must be removed"
    );
    assert!(state_dir.path().join(".self_healthy_0.5.0").exists());
}

#[test]
fn healthy_writer_cleans_stale_backups_without_pending_marker() {
    let state_dir = tempdir().unwrap();
    let install_dir = tempdir().unwrap();
    let stale_backup = install_dir.path().join("maekon.rollback.stale");
    std::fs::write(&stale_backup, b"stale").unwrap();

    write_self_healthy_and_cleanup(state_dir.path(), install_dir.path(), "0.5.0").unwrap();

    assert!(state_dir.path().join(".self_healthy_0.5.0").exists());
    assert!(
        !stale_backup.exists(),
        "historical backup cleanup must remain active when probe state moves"
    );
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

    write_self_healthy_and_cleanup(dir.path(), dir.path(), current_version).unwrap();

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
