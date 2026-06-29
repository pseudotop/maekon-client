//! Install orchestration: binary swap, rollback, pending marker, restart.
// OOS-TBD: ADR-013 file split applied — install.rs (1077L) split into
// download.rs / verification.rs / url.rs / archive.rs + this mod.rs.
#![allow(dead_code)] // Install helpers called from updater apply/verify paths

mod archive;
mod download;
mod url;
mod verification;

use std::path::{Path, PathBuf};

use super::{UpdateError, Updater};
// #6941: re-export the capped aux-body reader + cap so the releases-JSON fetch in
// updater/mod.rs shares the same OOM guard as the .sig/.sha256 reads here.
pub(crate) use download::{read_body_capped_update, MAX_AUX_UPDATE_BYTES};
#[cfg(test)]
pub(crate) use verification::SignatureKeySource;

/// Phase 4 D11: exit code used by `execute_rollback` when it must terminate
/// the current process.
///
/// `75` (EX_TEMPFAIL, from `sysexits(3)`) signals "temporary failure; try again".
pub const ROLLBACK_EXIT_CODE: i32 = 75;

impl Updater {
    pub(super) fn backup_path_for(current_exe: &Path) -> Result<PathBuf, UpdateError> {
        let parent = current_exe.parent().ok_or_else(|| {
            UpdateError::Install(
                "Failed to locate parent directory of current executable".to_string(),
            )
        })?;

        let file_name = current_exe
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("maekon")
            .to_string();
        let ts = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        Ok(parent.join(format!("{}.rollback.{}", file_name, ts)))
    }

    pub(super) fn install_and_restart_with_ops<FReplace, FRestart>(
        &self,
        downloaded_path: &Path,
        current_exe: &Path,
        new_version: Option<&str>,
        mut replace_binary: FReplace,
        mut restart_app: FRestart,
    ) -> Result<(), UpdateError>
    where
        FReplace: FnMut(&Path) -> Result<(), UpdateError>,
        FRestart: FnMut() -> Result<(), UpdateError>,
    {
        tracing::info!("Starting update installation: {:?}", downloaded_path);

        let backup_path = Self::backup_path_for(current_exe)?;
        std::fs::copy(current_exe, &backup_path)?;

        let file_name = downloaded_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        let binary_path = match Self::extract_if_archive(self, downloaded_path, file_name) {
            Ok(p) => p,
            Err(e) => {
                // Task 6 D11: orphan-backup cleanup on extract failure.
                let _ = std::fs::remove_file(&backup_path);
                return Err(e);
            }
        };

        if let Err(e) = replace_binary(&binary_path) {
            let _ = std::fs::remove_file(&backup_path);
            return Err(e);
        }

        // Task 6 D11: write .install_pending_{NEW_VERSION} immediately after
        // replace_binary succeeds and BEFORE restart_app.
        if let Some(new_ver) = new_version {
            let current_exe_parent = current_exe.parent().ok_or_else(|| {
                UpdateError::Install(
                    "current_exe has no parent directory for install_pending".to_string(),
                )
            })?;
            if let Err(e) = Self::write_install_pending(
                current_exe_parent,
                new_ver,
                super::CURRENT_VERSION,
                &backup_path,
            ) {
                tracing::error!("write_install_pending failed: {e}");
                tracing::warn!(
                    "D11 probe for this install is disabled (pending marker absent). \
                     Backup retained at {:?}",
                    backup_path
                );
                return Err(e);
            }
        }

        tracing::info!("Update installation completed, restarting application...");

        match restart_app() {
            Ok(()) => Ok(()),
            Err(restart_err) => {
                tracing::error!(
                    "Restart failed, attempting rollback: backup={:?}, error={}",
                    backup_path,
                    restart_err
                );

                match replace_binary(&backup_path) {
                    Ok(()) => Err(UpdateError::Install(format!(
                        "Rollback completed after restart failure: {}",
                        restart_err
                    ))),
                    Err(rollback_err) => Err(UpdateError::Install(format!(
                        "Restart failed and rollback failed: restart={}, rollback={}",
                        restart_err, rollback_err
                    ))),
                }
            }
        }
    }

    /// Decompress archive or return path as-is for loose binaries.
    fn extract_if_archive(
        updater: &Self,
        downloaded_path: &Path,
        file_name: &str,
    ) -> Result<PathBuf, UpdateError> {
        if file_name.ends_with(".tar.gz") || file_name.ends_with(".tgz") {
            updater.extract_tar_gz(downloaded_path)
        } else if file_name.ends_with(".zip") {
            updater.extract_zip(downloaded_path)
        } else {
            Ok(downloaded_path.to_path_buf())
        }
    }

    /// Write `.install_pending_{NEW_VERSION}` JSON marker in the install directory.
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

    /// Phase 4 D11 rollback execution. Success path does not return.
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
        // #5988: Windows in-place rollback (swapping the RUNNING .exe) is NOT yet
        // implemented. `execute_rollback_swap_only`'s `std::fs::copy(backup, running_exe)`
        // would fail with an opaque ERROR_SHARING_VIOLATION on the live executable, so the
        // health-probe crash-loop recovery hit an obscure copy error rather than a clear
        // signal. Fail LOUD and EARLY here — before the doomed swap — with an actionable
        // error and an error-level log. The verified backup is left in place so the user
        // (or a future Windows-verified Task 12 self_replace implementation) can recover.
        #[cfg(windows)]
        {
            let _ = (
                current_exe_path,
                from_version,
                to_version,
                reason,
                rollback_event,
            );
            tracing::error!(
                backup_path = %backup_path.display(),
                "Windows update rollback is NOT implemented (Task 12 / #5988): the agent \
                 cannot self-restore the previous version after repeated startup failures. \
                 The verified backup is preserved at the path above — recover by reinstalling \
                 the previous version or restoring that backup manually."
            );
            Err(UpdateError::Install(format!(
                "Windows rollback not implemented (#5988 / Task 12); verified backup preserved at {}",
                backup_path.display()
            )))
        }

        #[cfg(not(windows))]
        {
            Self::execute_rollback_swap_only(
                backup_path,
                current_exe_path,
                from_version,
                to_version,
                reason,
                rollback_event,
            )?;

            #[cfg(unix)]
            {
                std::process::Command::new(current_exe_path)
                    .spawn()
                    .map_err(|e| {
                        UpdateError::Install(format!(
                            "rollback spawn of restored binary failed: {e}"
                        ))
                    })?;
                std::process::exit(ROLLBACK_EXIT_CODE);
            }

            #[cfg(not(any(unix, windows)))]
            {
                let _ = current_exe_path;
                return Err(UpdateError::Install(
                    "rollback not implemented for this platform".to_string(),
                ));
            }
        }
    }

    /// Swap-only core: verify backup, broadcast event, rename backup into place.
    /// Does NOT spawn the replacement binary.
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

        let info = maekon_api_contracts::update::RollbackInfo {
            from_version: from_version.to_string(),
            from_published_at: None,
            to_version: to_version.to_string(),
            to_published_at: None,
            reason,
            rolled_back_at: chrono::Utc::now().to_rfc3339(),
        };
        rollback_event(&info);

        // Version-scoped notification file for the restored binary's next boot.
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

        #[cfg(unix)]
        {
            std::fs::rename(backup_path, current_exe_path)
                .map_err(|e| UpdateError::Install(format!("rollback rename failed: {e}")))?;
        }

        #[cfg(not(unix))]
        {
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

    // #6258: the version-dropping `install_and_restart(path)` convenience was
    // removed. It always passed `None`, so any caller that reached for it would
    // silently disarm the D11 `.install_pending_{ver}` crash-loop marker — the
    // exact bug this fixes. Callers must use `install_and_restart_versioned`
    // (the `UpdateExecutor::install_and_restart` trait method threads the
    // pending version through), making the version a required, visible argument.

    /// Install-and-restart, writing the D11 health-probe pending marker when
    /// `new_version` is `Some`.
    pub fn install_and_restart_versioned(
        &self,
        downloaded_path: &Path,
        new_version: Option<&str>,
    ) -> Result<(), UpdateError> {
        let current_exe = std::env::current_exe()?;
        self.install_and_restart_with_ops(
            downloaded_path,
            &current_exe,
            new_version,
            |candidate| {
                self_replace::self_replace(candidate)
                    .map_err(|e| UpdateError::Install(format!("Failed to replace binary: {}", e)))
            },
            || self.restart_app(),
        )
    }

    pub(super) fn restart_app(&self) -> Result<(), UpdateError> {
        let current_exe = std::env::current_exe()?;
        let args: Vec<String> = std::env::args().skip(1).collect();

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            let err = std::process::Command::new(&current_exe).args(&args).exec();
            Err(UpdateError::Install(format!("Restart failed: {}", err)))
        }

        #[cfg(windows)]
        {
            std::process::Command::new(&current_exe)
                .args(&args)
                .spawn()
                .map_err(|e| UpdateError::Install(format!("Restart failed: {}", e)))?;
            std::process::exit(0);
        }

        #[cfg(not(any(unix, windows)))]
        {
            Err(UpdateError::UnsupportedPlatform(
                "Restart is not supported on this platform".to_string(),
            ))
        }
    }
}

// ── Loop 3 iter 1 fix (I-1): direct in-bin coverage of execute_rollback_swap_only ──

#[cfg(test)]
mod rollback_tests {
    use super::*;
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
        result
            .expect("execute_rollback_swap_only must succeed when backup exists at the given path");

        // The byte contents of current_exe must equal the backup bytes — not just
        // "something different" but exactly the prior known-good bytes.
        let post = std::fs::read(&current_exe).unwrap();
        assert_eq!(post, backup_content, "binary should be replaced by backup");
        assert!(!backup.exists(), "backup should be renamed/removed");

        let info = captured.lock().unwrap().clone().expect("event emitted");
        assert_eq!(info.from_version, "0.5.0");
        assert_eq!(info.to_version, "0.4.40");
        assert_eq!(info.reason, RollbackReason::RepeatedStartupFailure);

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
        let post = std::fs::read(&current_exe).unwrap();
        assert_eq!(post, b"current");
    }
}

#[cfg(test)]
mod body_cap_tests {
    //! #7000: regression for `read_body_capped_update` — the body-cap helper that
    //! `download_update` (main artifact) plus the `.sig`/`.sha256` aux reads rely on.
    //! Confirms it reads within-cap bodies fully, rejects an oversized *declared*
    //! Content-Length early, and — the critical case — aborts mid-stream when a
    //! chunked body with NO Content-Length exceeds the cap (the forged/absent-CL
    //! OOM vector a declared-length pre-check cannot catch).
    use super::read_body_capped_update;
    use crate::updater::UpdateError;
    use std::io::Write;

    #[tokio::test]
    async fn read_body_capped_update_reads_within_cap() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(200)
            .with_body("0123456789") // 10 bytes
            .create_async()
            .await;
        let resp = reqwest::Client::new()
            .get(server.url())
            .send()
            .await
            .expect("mock request must succeed");
        let body = read_body_capped_update(resp, 1024)
            .await
            .expect("body within cap must read");
        assert_eq!(body, b"0123456789", "capped read must return the full body");
    }

    #[tokio::test]
    async fn read_body_capped_update_rejects_oversized_declared_length() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(200)
            .with_body("x".repeat(100)) // Content-Length: 100
            .create_async()
            .await;
        let resp = reqwest::Client::new()
            .get(server.url())
            .send()
            .await
            .expect("mock request must succeed");
        // Declared length 100 > cap 10 → early-reject before buffering.
        let err = read_body_capped_update(resp, 10)
            .await
            .expect_err("oversized declared length must be rejected");
        assert!(matches!(err, UpdateError::Download(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn read_body_capped_update_aborts_chunked_body_over_cap_midstream() {
        // Chunked body with NO Content-Length — the forged/absent-CL vector that a
        // declared-length pre-check cannot catch. Must abort mid-stream, not OOM.
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/")
            .with_status(200)
            .with_chunked_body(|w| w.write_all(&vec![b'x'; 100]))
            .create_async()
            .await;
        let resp = reqwest::Client::new()
            .get(server.url())
            .send()
            .await
            .expect("mock request must succeed");
        let err = read_body_capped_update(resp, 10)
            .await
            .expect_err("chunked body over cap must abort mid-stream");
        assert!(matches!(err, UpdateError::Download(_)), "got {err:?}");
    }
}
