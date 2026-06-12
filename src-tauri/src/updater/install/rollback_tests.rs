// ── Loop 3 iter 1 fix (I-1): direct in-bin coverage of execute_rollback_swap_only ──
//
// Previously the only test exercising the swap + event + notification
// write was the src-tauri/tests/ integration test, which re-implemented
// the behavior locally. That left the production helper — including the
// new .rolled_back_notification_{to_version} write — with zero
// coverage against actual code. Regressions in the write position (e.g.,
// on the Unix path, moving it after the rename) would ship silently.

use super::{UpdateError, Updater};
use maekon_api_contracts::update::{RollbackInfo, RollbackReason};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn execute_rollback_swap_only_swaps_binary_and_emits_event_and_writes_notification() {
    let dir = tempdir().unwrap();
    let current_exe = dir.path().join("maekon-current");
    let backup = dir.path().join("maekon-current.rollback.42");

    let current_content = b"CURRENT-v0.5.0".to_vec();
    let backup_content = b"BACKUP-v0.4.40".to_vec();
    std::fs::write(&current_exe, &current_content).unwrap();
    std::fs::write(&backup, &backup_content).unwrap();

    let captured: Arc<Mutex<Option<RollbackInfo>>> = Arc::new(Mutex::new(None));
    let captured_clone = Arc::clone(&captured);

    let result = Updater::execute_rollback_swap_only(
        &backup,
        &current_exe,
        "0.5.0",
        "0.4.40",
        RollbackReason::RepeatedStartupFailure,
        move |info| {
            *captured_clone.lock().unwrap() = Some(info.clone());
        },
    );
    result.expect("execute_rollback_swap_only must succeed when backup exists at the given path");

    // (a) Binary swap landed — current now holds backup bytes.
    let post = std::fs::read(&current_exe).unwrap();
    assert_eq!(post, backup_content, "binary should be replaced by backup");

    // (b) Backup path is gone after rename/copy.
    assert!(!backup.exists(), "backup should be renamed/removed");

    // (c) Event callback fired with correct fields.
    let info = captured.lock().unwrap().clone().expect("event emitted");
    assert_eq!(info.from_version, "0.5.0");
    assert_eq!(info.to_version, "0.4.40");
    assert_eq!(info.reason, RollbackReason::RepeatedStartupFailure);

    // (d) Persistent notification file was written with valid JSON — the
    //     new binary's boot consumes this to surface RolledBack in UI.
    //     Filename is version-scoped per holistic-review I-2: the
    //     restored binary only consumes markers matching its own
    //     to_version, and sweeps stragglers from prior rollback cycles.
    let notif = dir.path().join(".rolled_back_notification_0.4.40");
    assert!(
        notif.exists(),
        "version-scoped notification file should be written"
    );
    assert!(
        !dir.path().join(".rolled_back_notification").exists(),
        "legacy unversioned filename must not be used"
    );
    let bytes = std::fs::read(&notif).unwrap();
    let persisted: RollbackInfo = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(persisted.from_version, "0.5.0");
    assert_eq!(persisted.to_version, "0.4.40");
    assert_eq!(persisted.reason, RollbackReason::RepeatedStartupFailure);
}

#[test]
fn execute_rollback_swap_only_fails_when_backup_missing() {
    let dir = tempdir().unwrap();
    let current_exe = dir.path().join("maekon-current");
    let missing_backup = dir.path().join("does-not-exist.rollback.0");
    std::fs::write(&current_exe, b"current").unwrap();

    let result = Updater::execute_rollback_swap_only(
        &missing_backup,
        &current_exe,
        "0.5.0",
        "0.4.40",
        RollbackReason::RepeatedStartupFailure,
        |_| panic!("event should NOT fire when backup is missing"),
    );
    assert!(
        matches!(result, Err(UpdateError::Install(_))),
        "missing backup should error"
    );
    // current_exe content unchanged (no partial swap).
    let post = std::fs::read(&current_exe).unwrap();
    assert_eq!(post, b"current");
}
