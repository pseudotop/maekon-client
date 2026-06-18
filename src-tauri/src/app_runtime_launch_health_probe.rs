//! Health probe + rollback startup phase for `app_runtime_launch::build_and_spawn`.
//!
//! Runs BEFORE any scheduler loop spawns. If the probe escalates to
//! `RollbackRequired` (two consecutive failed boots on this version without a
//! self-healthy marker), `execute_rollback` spawns the restored binary and
//! this process terminates. On any error, logs and returns `None` — the
//! current (failing) binary keeps running; the next boot retries.
//!
//! Extracted from `app_runtime_launch.rs` to keep the runtime orchestrator
//! small and testable under the client ADR-002 GUI/runtime boundary.

use std::path::PathBuf;

use maekon_web::update_control::UpdateControl;

use crate::updater::{HealthProbe, RollbackReason, StartupAction, Updater, CURRENT_VERSION};

/// Execute the startup health probe + spawn the rolled-back-notification scan.
///
/// Returns the live `HealthProbe` handle (kept alive until the scheduler is
/// ready to call `spawn_healthy_writer`). `None` on any error in the probe
/// setup chain — downstream code must handle the no-probe case.
pub(crate) fn execute_startup_probe(
    handle: &tokio::runtime::Handle,
    update_control: UpdateControl,
) -> Option<HealthProbe> {
    // #5988: surface the Windows auto-rollback limitation once per boot so operators
    // see it proactively — not only at the point a crash-loop rollback is attempted
    // (where execute_rollback also logs at error level). Full Windows rollback is Task 12.
    #[cfg(windows)]
    tracing::warn!(
        "update auto-rollback is NOT implemented on Windows (Task 12 / #5988): if this \
         version repeatedly fails to start, the agent cannot self-restore the previous \
         version — manual reinstall or backup restore would be required."
    );

    let current_exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("health probe skipped: std::env::current_exe() failed: {e}");
            return None;
        }
    };
    let install_dir = match current_exe.parent().map(std::path::Path::to_path_buf) {
        Some(dir) => dir,
        None => {
            tracing::warn!("health probe skipped: current_exe has no parent");
            return None;
        }
    };

    // Fire-and-forget: consume `.rolled_back_notification_<version>` markers
    // written by the previous (failing) binary. The scan must run AFTER
    // `update_control` exists (it's the consumer of set_rolled_back).
    spawn_rolled_back_scan(handle, install_dir.clone(), update_control);

    let probe = HealthProbe::new(install_dir, CURRENT_VERSION.to_string());
    match probe.check_startup_state() {
        StartupAction::Normal => {
            tracing::debug!("health probe: Normal — proceeding with startup");
            Some(probe)
        }
        StartupAction::RollbackRequired {
            from_version,
            to_version,
            backup_path,
            reason,
        } => {
            tracing::error!(
                "health probe escalated to rollback: {from_version} -> {to_version} ({:?})",
                reason
            );
            let contract_reason = match reason {
                RollbackReason::RepeatedStartupFailure => {
                    maekon_api_contracts::update::RollbackReason::RepeatedStartupFailure
                }
            };
            match Updater::execute_rollback(
                &backup_path,
                &current_exe,
                &from_version,
                &to_version,
                contract_reason,
                |info| {
                    tracing::warn!(
                        "rollback event: {} -> {} ({:?})",
                        info.from_version,
                        info.to_version,
                        info.reason
                    );
                },
            ) {
                Ok(_never) => unreachable!("Infallible success path"),
                Err(e) => {
                    tracing::error!("rollback failed: {e}");
                    // Leave user on the failing binary; next boot retries.
                    None
                }
            }
        }
    }
}

/// Scan for `.rolled_back_notification_<version>` markers written by a
/// previous (failing) binary just before the rollback swap. The restored
/// binary surfaces the RolledBack state in UI on next boot.
///
/// Holistic-review I-2: files whose `to_version` matches the current binary
/// are OUR rollback — consume + delete. Files whose `to_version` does not
/// match are stale from a prior rollback cycle whose consumer never
/// completed — delete without surfacing UI so unrelated launches don't
/// re-render a stale banner.
fn spawn_rolled_back_scan(
    handle: &tokio::runtime::Handle,
    install_dir: PathBuf,
    update_control: UpdateControl,
) {
    handle.spawn(async move {
        let entries = match tokio::task::spawn_blocking({
            let install_dir = install_dir.clone();
            move || std::fs::read_dir(&install_dir)
        })
        .await
        {
            Ok(Ok(it)) => it,
            Ok(Err(e)) => {
                tracing::warn!(
                    "rolled_back_notification scan failed ({:?}): {e}",
                    install_dir
                );
                return;
            }
            Err(e) => {
                tracing::warn!("rolled_back_notification scan task failed: {e}");
                return;
            }
        };
        let current_version = CURRENT_VERSION;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with(".rolled_back_notification_") {
                continue;
            }
            let path = entry.path();
            match tokio::fs::read(&path).await {
                Ok(bytes) => {
                    match serde_json::from_slice::<maekon_api_contracts::update::RollbackInfo>(
                        &bytes,
                    ) {
                        Ok(info) => {
                            if info.to_version == current_version {
                                tracing::warn!(
                                    "consuming rolled_back_notification: {} -> {}",
                                    info.from_version,
                                    info.to_version
                                );
                                let _ = update_control.set_rolled_back(info).await;
                            } else {
                                tracing::debug!(
                                    "sweeping stale rolled_back_notification (to_version={}, current={})",
                                    info.to_version,
                                    current_version
                                );
                            }
                        }
                        Err(e) => tracing::warn!("rolled_back_notification parse failed: {e}"),
                    }
                }
                Err(e) => tracing::warn!("rolled_back_notification read failed: {e}"),
            }
            let _ = tokio::fs::remove_file(&path).await;
        }
    });
}

/// Spawn the self-healthy marker writer. After `healthy_threshold` (default
/// 30s) of continuous wall-clock uptime without a crash, the writer records
/// `.self_healthy_{VERSION}`, deletes `.install_pending_{VERSION}` + all
/// `.boot_count_pid_{VERSION}_*` per-PID markers (and any legacy
/// `.boot_count_{VERSION}` single-file residual), and cleans sibling
/// rollback backups.
///
/// Called from the orchestrator after the scheduler is fully up. Fire-and-
/// forget — the JoinHandle is intentionally dropped.
pub(crate) fn spawn_healthy_writer(probe: Option<&HealthProbe>, handle: &tokio::runtime::Handle) {
    if let Some(probe) = probe {
        let _join_handle = probe.spawn_healthy_writer(handle);
        tracing::debug!("health probe: spawn_healthy_writer dispatched");
    }
}
