//! Post-install self-healthy probe with 2-failed-boot automatic rollback.
//!
//! See `docs/reviews/2026-04-18-phase4-updater-hardening-design.md` §4 for
//! the full design.
//!
//! State-machine summary:
//! - On every startup, `check_startup_state` inspects `.install_pending_{VERSION}`,
//!   `.boot_count_pid_{VERSION}_{PID}` (per-PID markers; aggregate count is the
//!   number of such files), and `.self_healthy_{VERSION}` in the install directory.
//! - If the aggregate boot count reaches `failed_boot_threshold` (default 2)
//!   without a self-healthy marker, returns `RollbackRequired` with the backup
//!   path recorded at install.
//! - Otherwise returns `Normal`; the scheduler later calls `spawn_healthy_writer`
//!   which writes the self-healthy marker after `healthy_threshold` (default 30s)
//!   of continuous wall-clock uptime.
//! - Staleness rule (§4.3): an `.install_pending_{VERSION}` that is > 24h old
//!   with no healthy marker is treated as abandoned (same-version manual
//!   reinstall or long-idle device). Probe deletes state and returns Normal
//!   without triggering rollback.
//!
//! Ownership (spec Amendment 1 — applied in Task 1): both `check_startup_state`
//! and `spawn_healthy_writer` take `&self` so a single probe instance can be
//! created in `app_runtime_launch.rs`, used for the startup check, then shared
//! via `Arc` into the scheduler for the healthy-writer spawn.
//!
//! # Counter semantics
//!
//! - **Boot-counter ordering**: `failed_boot_threshold = 2` triggers
//!   rollback on the THIRD boot that fails to reach a self-healthy marker
//!   (boot 1 creates marker, count becomes 1; boot 2 creates marker,
//!   count becomes 2; boot 3 reads count=2 ≥ 2 and rolls back). The "2"
//!   refers to the maximum retry count, not the total boot count.
//!
//! - **Concurrent-process safety**: each boot creates one
//!   `.boot_count_pid_{VERSION}_{PID}` marker file via `create_new`
//!   (atomic). The count is derived by listing the directory at
//!   read-time. No read-modify-write sequence exists — concurrent boots
//!   of the same version each record independently. PID reuse across
//!   the lifetime of the install_pending window (< 24h per staleness
//!   rule) is possible but rare; the second `create_new` returns
//!   AlreadyExists and we treat that as "already recorded" (conservative
//!   undercount by 1 in the extreme case).

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Staleness cutoff for an `.install_pending_{VERSION}` marker (§4.3).
/// Entries older than this age without a self-healthy marker are treated as
/// abandoned — probe deletes them without triggering rollback.
const STALENESS_CUTOFF: Duration = Duration::from_secs(24 * 60 * 60);

/// Persistent content of `.install_pending_{VERSION}`.
///
/// Written by `install.rs::write_install_pending` (Task 6) after a successful
/// `replace_binary` and before `restart_app`. Consumed by the probe on the
/// next startup to determine rollback eligibility and backup selection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct InstallPending {
    /// ISO-8601 UTC timestamp at which the install completed.
    pub installed_at: String,
    /// The semver string of the version that was installed BEFORE this one
    /// — the rollback target.
    pub previous_version: String,
    /// Absolute filesystem path to the backup binary created by
    /// `install.rs::backup_path_for` before the binary swap. On rollback, the
    /// probe reads this field and the caller swaps it back into place.
    pub backup_path: PathBuf,
}

/// Outcome of a startup probe check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupAction {
    /// Proceed with normal startup.
    Normal,
    /// Boot counter reached the failed-boot threshold without a self-healthy
    /// marker; caller should invoke `execute_rollback` with the enclosed
    /// metadata.
    RollbackRequired {
        from_version: String,
        to_version: String,
        backup_path: PathBuf,
        reason: RollbackReason,
    },
}

/// Why the probe escalated to `RollbackRequired`.
///
/// Additive enum — new reasons can be added without breaking existing
/// consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackReason {
    /// The current version failed to reach the self-healthy threshold on
    /// `failed_boot_threshold` consecutive startups (default 2).
    RepeatedStartupFailure,
}

/// Errors raised by the internal probe implementation. The public
/// `check_startup_state` catches all of these and returns `Normal` after
/// logging a warning — probe I/O failures must never block user startup.
#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("install_pending file malformed: {0}")]
    InstallPendingParse(String),

    #[error("filesystem error in health probe: {0}")]
    Io(#[from] std::io::Error),
}

/// Post-install health probe. Constructed once per process at
/// `app_runtime_launch.rs`, shared via `Arc` into the scheduler for the
/// healthy-writer spawn.
#[derive(Debug, Clone)]
pub struct HealthProbe {
    install_dir: PathBuf,
    current_version: String,
    healthy_threshold: Duration,
    failed_boot_threshold: u8,
}

impl HealthProbe {
    /// Default thresholds: 30s healthy + 2 failed boots before rollback.
    pub fn new(install_dir: PathBuf, current_version: String) -> Self {
        Self {
            install_dir,
            current_version,
            healthy_threshold: Duration::from_secs(30),
            failed_boot_threshold: 2,
        }
    }

    /// Builder: override the healthy-threshold. Primarily for tests
    /// (inject a short duration so `spawn_healthy_writer` fires quickly).
    pub fn with_threshold(mut self, threshold: Duration) -> Self {
        self.healthy_threshold = threshold;
        self
    }

    fn install_pending_path(&self) -> PathBuf {
        self.install_dir
            .join(format!(".install_pending_{}", self.current_version))
    }

    /// Legacy single-file path — retained only for migration cleanup.
    fn legacy_boot_count_path(&self) -> PathBuf {
        self.install_dir
            .join(format!(".boot_count_{}", self.current_version))
    }

    /// Prefix used by per-PID boot-count marker files for this version.
    fn boot_count_pid_prefix(&self) -> String {
        format!(".boot_count_pid_{}_", self.current_version)
    }

    /// Path for a specific PID's boot-count marker (current version).
    fn boot_count_pid_path(&self, pid: u32) -> PathBuf {
        self.install_dir
            .join(format!("{}{}", self.boot_count_pid_prefix(), pid))
    }

    /// Count the boot attempts recorded for the current version by summing
    /// `.boot_count_pid_{VERSION}_*` marker files. Returns 0 if the install
    /// directory cannot be read (first boot, missing dir, etc.).
    pub(crate) fn boot_count(&self) -> std::io::Result<u32> {
        let prefix = self.boot_count_pid_prefix();
        let entries = match std::fs::read_dir(&self.install_dir) {
            Ok(e) => e,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(err) => return Err(err),
        };
        let mut count: u32 = 0;
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(&prefix) {
                    count = count.saturating_add(1);
                }
            }
        }
        Ok(count)
    }

    /// Record a boot attempt for this process by creating an empty per-PID
    /// marker file. `create_new` makes the write atomic against concurrent
    /// boots — if two processes happen to share a PID (PID reuse), the
    /// second `create_new` returns AlreadyExists and we silently accept
    /// that path (conservative undercount by 1 in the extreme case, vs.
    /// the unbounded race the single-file approach permitted).
    fn record_boot_attempt(&self) -> std::io::Result<()> {
        let pid = std::process::id();
        let path = self.boot_count_pid_path(pid);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                // PID reuse within the staleness window (< 24h). Rare but
                // observable on long-lived VMs / fork-heavy systems. Log a
                // diagnostic so the conservative-undercount behavior is
                // field-observable rather than silent.
                tracing::warn!(
                    "health probe: PID {pid} boot-marker already exists — possible PID reuse within staleness window"
                );
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    /// Remove all boot-count marker files for the current version (both
    /// the new per-PID format and any legacy single-file). Used by the
    /// healthy-writer path.
    fn cleanup_boot_count_markers(&self) -> std::io::Result<()> {
        let prefix = self.boot_count_pid_prefix();
        if let Ok(entries) = std::fs::read_dir(&self.install_dir) {
            for entry in entries.flatten() {
                if let Some(name) = entry.file_name().to_str() {
                    if name.starts_with(&prefix) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
        // Legacy single-file cleanup (idempotent — may not exist).
        let _ = std::fs::remove_file(self.legacy_boot_count_path());
        Ok(())
    }

    fn self_healthy_path(&self) -> PathBuf {
        self.install_dir
            .join(format!(".self_healthy_{}", self.current_version))
    }

    /// Inspect the probe state files and return the next action for startup.
    ///
    /// Contract: any filesystem error is treated as `Normal` with a warning
    /// log. Probe I/O failures must never block user startup.
    pub fn check_startup_state(&self) -> StartupAction {
        match self.check_startup_state_inner() {
            Ok(action) => action,
            Err(err) => {
                tracing::warn!("health probe filesystem error — proceeding normally: {err}");
                StartupAction::Normal
            }
        }
    }

    fn check_startup_state_inner(&self) -> Result<StartupAction, ProbeError> {
        let self_healthy = self.self_healthy_path();
        let install_pending = self.install_pending_path();

        // Step 1 (short-circuit): self-healthy already written → nothing to do.
        if self_healthy.exists() {
            return Ok(StartupAction::Normal);
        }

        // Step 2 (short-circuit): no pending-install marker → fresh install
        // (or healthy marker was written and cleanup ran). Caller proceeds
        // normally; the post-boot spawn_healthy_writer will write the marker
        // after the healthy threshold.
        if !install_pending.exists() {
            return Ok(StartupAction::Normal);
        }

        // Step 0 (staleness): if the pending marker is > 24h old and we still
        // have no healthy marker, treat as abandoned (same-version manual
        // reinstall or a device that was powered off for days between boots).
        let pending = read_install_pending(&install_pending)?;
        if is_stale(&pending.installed_at, STALENESS_CUTOFF) {
            tracing::info!(
                "health probe: stale install_pending ({}h+ old) — cleaning abandoned state",
                STALENESS_CUTOFF.as_secs() / 3600
            );
            let _ = std::fs::remove_file(&install_pending);
            let _ = self.cleanup_boot_count_markers();
            return Ok(StartupAction::Normal);
        }

        // One-time legacy migration: if a pre-per-PID single-file
        // `.boot_count_{VERSION}` exists from an earlier client build, delete
        // it. The new per-PID format is authoritative; the count is rebuilt
        // from whatever per-PID markers already exist (or starts at 0).
        let _ = std::fs::remove_file(self.legacy_boot_count_path());

        // Steps 3-5: count boot attempts, check threshold, record this boot.
        let current_count = self.boot_count().unwrap_or(0);

        if current_count >= u32::from(self.failed_boot_threshold) {
            tracing::warn!(
                "health probe: boot_count={current_count} >= threshold={}; triggering rollback",
                self.failed_boot_threshold
            );
            return Ok(StartupAction::RollbackRequired {
                from_version: self.current_version.clone(),
                to_version: pending.previous_version.clone(),
                backup_path: pending.backup_path.clone(),
                reason: RollbackReason::RepeatedStartupFailure,
            });
        }

        // Record this boot AFTER the threshold check so a single bad boot
        // is represented as count=1 next time, not count=2. `create_new`
        // is atomic and idempotent against concurrent boots.
        self.record_boot_attempt()?;
        Ok(StartupAction::Normal)
    }

    /// Spawn a tokio background task that waits `healthy_threshold` then
    /// writes the self-healthy marker and cleans related state files.
    ///
    /// Takes `&self` (spec Amendment 1) — the probe instance stays owned by
    /// the launch path; the spawned task captures the data it needs by
    /// value at spawn time.
    ///
    /// Takes an explicit `&tokio::runtime::Handle` rather than calling
    /// `tokio::spawn` directly. On macOS the Tauri `setup` callback runs
    /// synchronously inside `applicationDidFinishLaunching` BEFORE Tauri
    /// enters the tokio runtime context; calling `tokio::spawn` from there
    /// panics with "no reactor running". The handle is already plumbed
    /// through `BootstrapRuntimeBundle::runtime_handle` for exactly this
    /// kind of cross-runtime spawning.
    pub fn spawn_healthy_writer(
        &self,
        handle: &tokio::runtime::Handle,
    ) -> tokio::task::JoinHandle<()> {
        let install_dir = self.install_dir.clone();
        let version = self.current_version.clone();
        let threshold = self.healthy_threshold;

        handle.spawn(async move {
            tokio::time::sleep(threshold).await;
            if let Err(err) = write_self_healthy_and_cleanup(&install_dir, &version) {
                tracing::warn!("spawn_healthy_writer: cleanup error — {err}");
            }
        })
    }
}

// ── Internal helpers (file-level for testability) ─────────────────────

fn read_install_pending(path: &Path) -> Result<InstallPending, ProbeError> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice::<InstallPending>(&bytes)
        .map_err(|e| ProbeError::InstallPendingParse(e.to_string()))
}

/// Returns true when `iso_ts_utc` parses successfully AND is older than `cutoff`.
///
/// If the timestamp cannot be parsed, returns `false` (conservative — do NOT
/// treat a malformed timestamp as stale, since that could cause lost state on
/// a device with a corrupted clock).
fn is_stale(iso_ts_utc: &str, cutoff: Duration) -> bool {
    match DateTime::parse_from_rfc3339(iso_ts_utc) {
        Ok(dt) => {
            let age = Utc::now().signed_duration_since(dt.with_timezone(&Utc));
            age.to_std().map(|d| d > cutoff).unwrap_or(false)
        }
        Err(_) => false,
    }
}

/// Write `.self_healthy_{VERSION}` and clean up the state files that are no
/// longer needed. Also removes old `{binary_name}.rollback.{ts}` files EXCEPT
/// the one currently recorded in the pending marker (which is the canonical
/// rollback target and must remain available).
fn write_self_healthy_and_cleanup(install_dir: &Path, version: &str) -> Result<(), ProbeError> {
    // Read the pending marker FIRST to capture backup_path before deleting it.
    let install_pending_path = install_dir.join(format!(".install_pending_{version}"));
    let keep_backup: Option<PathBuf> = match std::fs::read(&install_pending_path) {
        Ok(bytes) => serde_json::from_slice::<InstallPending>(&bytes)
            .ok()
            .map(|p| p.backup_path),
        Err(_) => None,
    };

    // Write the self-healthy marker.
    let marker_path = install_dir.join(format!(".self_healthy_{version}"));
    std::fs::write(&marker_path, Utc::now().to_rfc3339())?;

    // Remove now-stale pending file (ignore failures — cleanup is best-effort).
    let _ = std::fs::remove_file(&install_pending_path);

    // Remove all per-PID boot-count markers for the CURRENT version + the
    // legacy single-file if still present. Foreign-version files are
    // handled by the sweep below.
    let current_prefix = format!(".boot_count_pid_{version}_");
    if let Ok(entries) = std::fs::read_dir(install_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(&current_prefix) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    let _ = std::fs::remove_file(install_dir.join(format!(".boot_count_{version}")));

    // Sweep sibling rollback backups + foreign-version state files.
    //
    // Loop 3 iter 1 fix (I-2): previously this sweep only removed
    // `*.rollback.*` files, leaving stale `.install_pending_{OLDER}` /
    // `.boot_count_{OLDER}` / `.self_healthy_{OLDER}` files to accrete
    // across upgrades. Now also reclaim state files whose version suffix
    // does NOT match the current version — the current probe has just
    // written its own self_healthy marker, so anything else is stale.
    if let Ok(entries) = std::fs::read_dir(install_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };

            // (a) Rollback backup sweep (existing behavior).
            if name.contains(".rollback.") {
                if let Some(keep) = &keep_backup {
                    if path == *keep {
                        continue;
                    }
                }
                let _ = std::fs::remove_file(&path);
                continue;
            }

            // (b) Foreign-version per-PID boot-count sweep. Format is
            //     `.boot_count_pid_<VER>_<PID>`. Extract VER (the segment
            //     before the final `_` separating version from PID).
            if let Some(suffix) = name.strip_prefix(".boot_count_pid_") {
                if let Some((ver, _pid)) = suffix.rsplit_once('_') {
                    if !ver.is_empty() && ver != version {
                        let _ = std::fs::remove_file(&path);
                    }
                }
                continue;
            }

            // (c) Foreign-version legacy single-file boot-count sweep.
            //     Always deleted when encountered — the per-PID format is
            //     authoritative now, so any `.boot_count_<VER>` residual
            //     (regardless of VER) is stale.
            if let Some(ver_suffix) = name.strip_prefix(".boot_count_") {
                if !ver_suffix.is_empty() {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }

            // (d) Foreign-version install-pending / self-healthy sweep.
            for prefix in [".install_pending_", ".self_healthy_"] {
                if let Some(ver_suffix) = name.strip_prefix(prefix) {
                    if !ver_suffix.is_empty() && ver_suffix != version {
                        let _ = std::fs::remove_file(&path);
                    }
                    break;
                }
            }
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
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
        // Legacy single-file format — used only by tests that exercise
        // migration cleanup. New increment path uses per-PID markers.
        std::fs::write(
            dir.join(format!(".boot_count_{version}")),
            count.to_string(),
        )
        .unwrap();
    }

    fn write_boot_count_pid_marker(dir: &Path, version: &str, pid: u32) {
        // Create a single per-PID boot-count marker (simulates a boot).
        std::fs::write(dir.join(format!(".boot_count_pid_{version}_{pid}")), b"").unwrap();
    }

    fn write_boot_count_pids(dir: &Path, version: &str, count: u32) {
        // Convenience helper for tests that need N simulated boots with
        // distinct PIDs. Uses predictable PIDs starting at 10000 to avoid
        // collision with any actual test-runner PID.
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
        // Even with a pending install, a present healthy marker short-circuits to Normal.
        let backup = dir.path().join("oneshim.rollback.1");
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
        let backup = dir.path().join("oneshim.rollback.1");
        std::fs::write(&backup, b"backup-bytes").unwrap();
        write_pending(
            dir.path(),
            "0.5.0",
            &Utc::now().to_rfc3339(),
            "0.4.39",
            &backup,
        );
        // boot_count=0 on disk → probe reads 0, sees 0 < 2, increments to 1, returns Normal.

        let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
        assert_eq!(probe.check_startup_state(), StartupAction::Normal);

        // Confirm counter was bumped — exactly one per-PID marker exists
        // for the current version.
        assert_eq!(probe.boot_count().unwrap(), 1);
    }

    #[test]
    fn check_startup_at_failed_boot_threshold_triggers_rollback() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("oneshim.rollback.1");
        std::fs::write(&backup, b"backup-bytes").unwrap();
        write_pending(
            dir.path(),
            "0.5.0",
            &Utc::now().to_rfc3339(),
            "0.4.39",
            &backup,
        );
        write_boot_count_pids(dir.path(), "0.5.0", 2); // at threshold via per-PID markers

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

        // At-threshold does NOT record a new boot; the next probe still
        // sees count=2 if rollback somehow fails to execute.
        assert_eq!(probe.boot_count().unwrap(), 2);
    }

    #[test]
    fn stale_install_pending_older_than_24h_returns_normal_without_rollback() {
        let dir = tempdir().unwrap();
        let backup = dir.path().join("oneshim.rollback.1");
        std::fs::write(&backup, b"backup-bytes").unwrap();
        // installed_at 25 hours ago
        let old_ts = (Utc::now() - ChronoDuration::hours(25)).to_rfc3339();
        write_pending(dir.path(), "0.5.0", &old_ts, "0.4.39", &backup);
        write_boot_count(dir.path(), "0.5.0", 5); // would trigger if not stale

        let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
        assert_eq!(probe.check_startup_state(), StartupAction::Normal);

        // Staleness rule deletes the pending + all boot_count files (legacy
        // single-file + per-PID markers). `cleanup_boot_count_markers`
        // handles both.
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
        let backup = dir.path().join("oneshim.rollback.1");
        std::fs::write(&backup, b"backup-bytes").unwrap();
        write_pending(
            dir.path(),
            "0.5.0",
            &Utc::now().to_rfc3339(),
            "0.4.39",
            &backup,
        );
        // Seed 2 per-PID boot markers to confirm cleanup removes all of them.
        write_boot_count_pids(dir.path(), "0.5.0", 2);

        let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into())
            .with_threshold(Duration::from_millis(50));

        let runtime_handle = tokio::runtime::Handle::current();
        let join = probe.spawn_healthy_writer(&runtime_handle);
        join.await.unwrap();

        let marker = dir.path().join(".self_healthy_0.5.0");
        assert!(marker.exists(), "healthy marker should have been written");
        // Cleanup should have removed install_pending + all per-PID markers.
        assert!(!probe.install_pending_path().exists());
        assert_eq!(probe.boot_count().unwrap(), 0);
        // Backup_path recorded in pending should survive the sweep.
        assert!(backup.exists(), "canonical backup should remain");
    }

    /// Regression test for the v0.4.40-rc.1 macOS launch panic
    /// ("there is no reactor running") when the Tauri `setup` callback
    /// invokes `spawn_healthy_writer` from within
    /// `applicationDidFinishLaunching`. The callsite is synchronous and
    /// runs BEFORE Tauri enters the tokio runtime context — so the
    /// previous `tokio::spawn` call panicked. With the explicit
    /// `&Handle` parameter, the writer can be dispatched onto the
    /// pre-built runtime even when no current runtime is entered on the
    /// thread.
    ///
    /// This test deliberately uses `#[test]` (NOT `#[tokio::test]`) so
    /// `Handle::try_current()` would return `Err` if it were called —
    /// matching the macOS launch path. The fix must keep working under
    /// these conditions.
    #[test]
    fn spawn_healthy_writer_does_not_panic_outside_async_context() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let handle = runtime.handle().clone();

        // Pre-condition: no tokio runtime is "current" on this thread.
        assert!(
            tokio::runtime::Handle::try_current().is_err(),
            "test must run outside any tokio runtime context to reproduce the panic scenario",
        );

        let dir = tempdir().unwrap();
        write_pending(
            dir.path(),
            "0.5.0",
            &Utc::now().to_rfc3339(),
            "0.4.39",
            &dir.path().join("oneshim.rollback.1"),
        );

        let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into())
            .with_threshold(Duration::from_millis(50));

        // The previous tokio::spawn-based implementation panicked here.
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

        // Seed a fresh install_pending for the CURRENT version (no backup;
        // the sweep doesn't touch the non-existent path, but the code path
        // only uses backup_path for the `.rollback.` exclusion).
        write_pending(
            dir.path(),
            current_version,
            &Utc::now().to_rfc3339(),
            "0.4.40",
            &dir.path().join("nonexistent-backup"),
        );

        // Seed stale state from a previous version (legacy single-file boot_count
        // + foreign-version per-PID markers).
        std::fs::write(dir.path().join(".install_pending_0.4.40"), "stale-content").unwrap();
        std::fs::write(dir.path().join(".boot_count_0.4.40"), "2").unwrap();
        write_boot_count_pid_marker(dir.path(), "0.4.40", 100);
        write_boot_count_pid_marker(dir.path(), "0.4.40", 200);
        std::fs::write(
            dir.path().join(".self_healthy_0.4.40"),
            Utc::now().to_rfc3339(),
        )
        .unwrap();

        // Also seed a LOOKALIKE file that should NOT be swept (different prefix).
        std::fs::write(dir.path().join("unrelated.txt"), "keep me").unwrap();

        // Invoke the cleanup helper directly.
        write_self_healthy_and_cleanup(dir.path(), current_version).unwrap();

        // Self-healthy for current version: written.
        assert!(dir.path().join(".self_healthy_0.5.0").exists());
        // Current version's pending + boot_count: removed.
        assert!(!dir.path().join(".install_pending_0.5.0").exists());
        assert!(!dir.path().join(".boot_count_0.5.0").exists());

        // Foreign-version state files: swept (including per-PID markers and
        // legacy single-file).
        assert!(!dir.path().join(".install_pending_0.4.40").exists());
        assert!(!dir.path().join(".boot_count_0.4.40").exists());
        assert!(!dir.path().join(".boot_count_pid_0.4.40_100").exists());
        assert!(!dir.path().join(".boot_count_pid_0.4.40_200").exists());
        assert!(!dir.path().join(".self_healthy_0.4.40").exists());

        // Unrelated file: untouched.
        assert!(dir.path().join("unrelated.txt").exists());
    }

    #[test]
    fn probe_io_error_is_non_fatal() {
        // Point install_dir at a non-existent sub-path. The inner probe will
        // encounter read failures (no install_pending → Normal short-circuit);
        // to actually exercise the error path, create a malformed pending file.
        let dir = tempdir().unwrap();
        let pending_path = dir.path().join(".install_pending_0.5.0");
        std::fs::write(&pending_path, b"NOT VALID JSON {{{").unwrap();

        let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
        // Public wrapper catches InstallPendingParse → returns Normal.
        assert_eq!(probe.check_startup_state(), StartupAction::Normal);
    }

    #[test]
    fn concurrent_boot_count_no_undercount() {
        // Two instances of the same version boot in rapid succession. With
        // the single-file read-modify-write pattern, each read sees the old
        // count and each writes count+1 — losing one increment. With
        // per-PID marker files, each instance records independently and the
        // aggregate count reflects both boots.
        let dir = tempdir().unwrap();
        let version = "0.5.0";

        // Simulate PID 100 and PID 200 each writing their per-PID marker.
        write_boot_count_pid_marker(dir.path(), version, 100);
        write_boot_count_pid_marker(dir.path(), version, 200);

        let probe = HealthProbe::new(dir.path().to_path_buf(), version.into());
        assert_eq!(probe.boot_count().unwrap(), 2);
    }

    #[test]
    fn cleanup_boot_count_markers_removes_per_pid_and_legacy_files() {
        let dir = tempdir().unwrap();
        let version = "0.5.0";

        // Seed: 3 per-PID markers + 1 legacy single-file.
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
        // A pre-per-PID build left `.boot_count_0.5.0` behind with the
        // single-file format. The current probe sees the pending-install
        // marker, migrates by deleting the legacy file (DROPPING the
        // legacy count — not reading it), and records a fresh per-PID
        // boot attempt.
        //
        // Seed count=99 (well above threshold=2) so this test would FAIL
        // with RollbackRequired if migration mistakenly read the legacy
        // count. Startup returning Normal proves the legacy value was
        // discarded, not parsed.
        let dir = tempdir().unwrap();
        let backup = dir.path().join("oneshim.rollback.1");
        std::fs::write(&backup, b"backup-bytes").unwrap();
        write_pending(
            dir.path(),
            "0.5.0",
            &Utc::now().to_rfc3339(),
            "0.4.39",
            &backup,
        );
        // Legacy single-file with count=99 (would trigger rollback if read).
        write_boot_count(dir.path(), "0.5.0", 99);

        let probe = HealthProbe::new(dir.path().to_path_buf(), "0.5.0".into());
        assert_eq!(
            probe.check_startup_state(),
            StartupAction::Normal,
            "migration must discard the legacy count, not parse it"
        );

        // Legacy file removed; migration dropped the legacy count.
        assert!(
            !probe.legacy_boot_count_path().exists(),
            "legacy single-file must be removed during migration"
        );
        // New per-PID marker for THIS process is in place (fresh count=1).
        assert_eq!(
            probe.boot_count().unwrap(),
            1,
            "this boot is recorded via the new per-PID format"
        );
    }
}
