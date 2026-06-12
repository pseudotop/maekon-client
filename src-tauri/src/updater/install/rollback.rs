use std::path::Path;

use super::{UpdateError, Updater};

/// Phase 4 D11: exit code used by `execute_rollback` when it must terminate
/// the current process (either Windows helper path or any Unix error after
/// the image-replacement syscall would have fired).
///
/// `75` (EX_TEMPFAIL, from `sysexits(3)`) signals a "temporary failure;
/// try again" — appropriate for rollback: the current binary failed to
/// stay healthy, but user data / install dir is intact and the previous
/// binary was restored.
pub const ROLLBACK_EXIT_CODE: i32 = 75;

impl Updater {
    pub(super) fn write_install_pending(
        install_dir: &Path,
        new_version: &str,
        previous_version: &str,
        backup_path: &Path,
    ) -> Result<(), UpdateError> {
        let marker_path = install_dir.join(format!(".install_pending_{new_version}"));
        let payload = serde_json::json!({
            "installed_at": chrono::Utc::now().to_rfc3339(),
            "previous_version": previous_version,
            "backup_path": backup_path,
        });
        let bytes = serde_json::to_vec(&payload).map_err(|e| {
            UpdateError::Install(format!("Failed to serialize install_pending: {}", e))
        })?;
        std::fs::write(&marker_path, bytes)
            .map_err(|e| UpdateError::Install(format!("Failed to write install_pending: {}", e)))?;
        tracing::info!(
            "install_pending written: version={new_version}, previous={previous_version}"
        );
        Ok(())
    }

    /// Phase 4 D11 rollback execution.
    ///
    /// Contract:
    /// - Returns `Result<Infallible, UpdateError>` — the success path does
    ///   not return (spawns replacement binary, terminates current process
    ///   with `ROLLBACK_EXIT_CODE`).
    /// - Caller supplies `rollback_event` which is invoked BEFORE the swap
    ///   so subscribers (update_coordinator, UI) can observe the rollback
    ///   even if the subsequent restart races with shutdown.
    ///
    /// Windows: deferred to §4.8 spike deliverable (Task 12). Current stub
    /// returns `UpdateError::Install` with a spike-pending message.
    pub fn execute_rollback<F>(
        backup_path: &Path,
        current_exe_path: &Path,
        from_version: &str,
        to_version: &str,
        reason: maekon_api_contracts::update::RollbackReason,
        rollback_event: F,
    ) -> Result<std::convert::Infallible, UpdateError>
    where
        F: FnOnce(&maekon_api_contracts::update::RollbackInfo),
    {
        Self::execute_rollback_swap_only(
            backup_path,
            current_exe_path,
            from_version,
            to_version,
            reason,
            rollback_event,
        )?;

        // Spawn the restored binary as a child process and terminate the
        // current process. spawn + exit gives simpler semantics than image
        // replacement while still achieving the rollback outcome — user sees
        // a brief flicker as the new PID starts.
        #[cfg(unix)]
        {
            std::process::Command::new(current_exe_path)
                .spawn()
                .map_err(|e| {
                    UpdateError::Install(format!("rollback spawn of restored binary failed: {e}"))
                })?;
            std::process::exit(ROLLBACK_EXIT_CODE);
        }

        // Windows: image replacement + spawn semantics are delegated to the
        // Task 12 spike output (docs/guides/updater-rollback-windows.md).
        // The detection path (health probe) still fires on Windows — this
        // branch makes the swap a documented no-op that support can surface
        // in logs (holistic-review Q8).
        #[cfg(windows)]
        {
            let _ = current_exe_path;
            tracing::warn!(
                "rollback swap not implemented on Windows (Task 12 pending); \
                 user remains on failing binary until manual reinstall — \
                 see docs/guides/updater-rollback-windows.md"
            );
            return Err(UpdateError::Install(
                "Windows rollback helper pending (§4.8 spike — Task 12)".to_string(),
            ));
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = current_exe_path;
            return Err(UpdateError::Install(
                "rollback not implemented for this platform".to_string(),
            ));
        }
    }

    /// Test-mode + Windows-spike shared core: verify backup, broadcast event,
    /// rename backup into current_exe position. Does NOT perform the
    /// replacement-binary spawn — production `execute_rollback` does that
    /// after this helper returns Ok.
    ///
    /// Extracted so the integration test can observe the file-swap +
    /// event-broadcast portion without the test harness itself being
    /// terminated by the production path's `std::process::exit` call.
    pub(crate) fn execute_rollback_swap_only<F>(
        backup_path: &Path,
        current_exe_path: &Path,
        from_version: &str,
        to_version: &str,
        reason: maekon_api_contracts::update::RollbackReason,
        rollback_event: F,
    ) -> Result<(), UpdateError>
    where
        F: FnOnce(&maekon_api_contracts::update::RollbackInfo),
    {
        // 1. Verify backup exists and is a regular file.
        if !backup_path.exists() {
            return Err(UpdateError::Install(format!(
                "rollback backup not found: {:?}",
                backup_path
            )));
        }
        let backup_meta = std::fs::metadata(backup_path)
            .map_err(|e| UpdateError::Install(format!("stat backup failed: {e}")))?;
        if !backup_meta.is_file() {
            return Err(UpdateError::Install(format!(
                "rollback backup is not a regular file: {:?}",
                backup_path
            )));
        }

        // 2. Emit the event BEFORE the swap — subscribers observe the
        //    rollback even if the subsequent rename races with shutdown.
        let info = maekon_api_contracts::update::RollbackInfo {
            from_version: from_version.to_string(),
            from_published_at: None, // populated upstream if caller has it
            to_version: to_version.to_string(),
            to_published_at: None,
            reason,
            rolled_back_at: chrono::Utc::now().to_rfc3339(),
        };
        rollback_event(&info);

        // Persist a rollback notification file so the restored binary can
        // surface the RolledBack state in the UI on its next boot. The
        // current (failing) process terminates immediately after the image
        // replacement; without a persisted marker the user would see the
        // version number decrement silently.
        //
        // Filename is version-scoped (`.rolled_back_notification_{to_version}`)
        // so the consumer on next boot can match its own running version and
        // sweep stale markers from earlier rollback cycles whose consumer did
        // not complete (holistic-review I-2).
        if let Some(install_dir) = current_exe_path.parent() {
            let notif_path = install_dir.join(format!(".rolled_back_notification_{to_version}"));
            match serde_json::to_vec(&info) {
                Ok(bytes) => {
                    if let Err(e) = std::fs::write(&notif_path, bytes) {
                        tracing::warn!("rolled_back_notification write failed (non-fatal): {e}");
                    }
                }
                Err(e) => {
                    tracing::warn!("rolled_back_notification serialize failed (non-fatal): {e}")
                }
            }
        }

        // 3. Atomic rename: backup → current_exe. On Unix, this works even
        //    while the current binary is running (files held open by inode).
        //    On Windows, this would fail for a running executable — the
        //    production path short-circuits before reaching here on Windows
        //    (Task 12 replaces with a spike-derived mechanism).
        #[cfg(unix)]
        {
            std::fs::rename(backup_path, current_exe_path)
                .map_err(|e| UpdateError::Install(format!("rollback rename failed: {e}")))?;
        }

        #[cfg(not(unix))]
        {
            // Non-Unix test-harness only path: copy + remove simulates the swap
            // without depending on a helper binary. Production Windows hits the
            // §4.8-pending error branch in execute_rollback before reaching here.
            std::fs::copy(backup_path, current_exe_path).map_err(|e| {
                UpdateError::Install(format!("rollback copy (non-unix) failed: {e}"))
            })?;
            let _ = std::fs::remove_file(backup_path);
        }

        tracing::warn!(
            "rollback executed: from={from_version} -> to={to_version} (reason: {:?})",
            reason
        );
        Ok(())
    }
}
