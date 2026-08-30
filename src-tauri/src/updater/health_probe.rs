//! Post-install self-healthy probe with 2-failed-boot automatic rollback.
//!
//! Public self-update and rollback behavior is summarized in
//! `docs/guides/ci-transparency.md#self-update-mechanism` and
//! `docs/guides/updater-rollback-windows.md`.
//!
//! State-machine summary:
//! - On every startup, `check_startup_state` inspects `.install_pending_{VERSION}`,
//!   `.boot_count_pid_{VERSION}_{PID}` (per-PID markers; aggregate count is the
//!   number of such files), and `.self_healthy_{VERSION}` in the mutable probe
//!   state directory.
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

#[cfg(target_os = "macos")]
use maekon_core::config_manager::ConfigManager;

/// Staleness cutoff for an `.install_pending_{VERSION}` marker (§4.3).
/// Entries older than this age without a self-healthy marker are treated as
/// abandoned — probe deletes them without triggering rollback.
const STALENESS_CUTOFF: Duration = Duration::from_secs(24 * 60 * 60);

const HEALTH_STATE_DIR_NAME: &str = "updater-health";

/// Resolve mutable updater-health state without ever placing it inside a
/// signed macOS `.app` bundle.
///
/// Loose binaries keep the historical adjacent-file layout. A bundled macOS
/// executable (`<App>.app/Contents/MacOS/<binary>`) uses the app-flavored data
/// directory instead, because adding even a dotfile below `Contents/` breaks
/// the bundle's sealed-resource signature and invalidates TCC identity.
fn is_macos_app_executable(current_exe: &Path) -> bool {
    let Some(executable_dir) = current_exe.parent() else {
        return false;
    };
    executable_dir
        .file_name()
        .is_some_and(|name| name == "MacOS")
        && executable_dir
            .parent()
            .is_some_and(|contents| contents.file_name().is_some_and(|name| name == "Contents"))
        && executable_dir
            .parent()
            .and_then(Path::parent)
            .is_some_and(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))
}

fn resolve_health_state_dir(
    current_exe: &Path,
    macos_data_dir: Option<&Path>,
) -> std::io::Result<PathBuf> {
    let executable_dir = current_exe.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "current executable has no parent directory",
        )
    })?;

    if is_macos_app_executable(current_exe) {
        let data_dir = macos_data_dir.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "macOS app health state requires a resolved app data directory",
            )
        })?;
        return Ok(data_dir.join(HEALTH_STATE_DIR_NAME));
    }

    Ok(executable_dir.to_path_buf())
}

pub(crate) fn health_state_dir_for_executable(current_exe: &Path) -> Result<PathBuf, ProbeError> {
    #[cfg(target_os = "macos")]
    let macos_data_dir = Some(ConfigManager::data_dir().map_err(|error| {
        std::io::Error::other(format!(
            "failed to resolve app data directory for updater health state: {error}"
        ))
    })?);

    #[cfg(not(target_os = "macos"))]
    let macos_data_dir: Option<PathBuf> = None;

    Ok(resolve_health_state_dir(
        current_exe,
        macos_data_dir.as_deref(),
    )?)
}

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
    state_dir: PathBuf,
    backup_dir: PathBuf,
    current_version: String,
    healthy_threshold: Duration,
    failed_boot_threshold: u8,
}

impl HealthProbe {
    /// Default thresholds: 30s healthy + 2 failed boots before rollback.
    pub fn new(state_dir: PathBuf, current_version: String) -> Self {
        Self {
            backup_dir: state_dir.clone(),
            state_dir,
            current_version,
            healthy_threshold: Duration::from_secs(30),
            failed_boot_threshold: 2,
        }
    }

    /// Builder: locate rollback binaries separately from mutable probe state.
    /// macOS app bundles keep probe markers in app data, while updater backup
    /// binaries retain their historical location beside the executable.
    pub fn with_backup_dir(mut self, backup_dir: PathBuf) -> Self {
        self.backup_dir = backup_dir;
        self
    }

    /// Builder: override the healthy-threshold. Primarily for tests
    /// (inject a short duration so `spawn_healthy_writer` fires quickly).
    #[allow(dead_code)]
    pub fn with_threshold(mut self, threshold: Duration) -> Self {
        self.healthy_threshold = threshold;
        self
    }

    fn install_pending_path(&self) -> PathBuf {
        self.state_dir
            .join(format!(".install_pending_{}", self.current_version))
    }

    /// Legacy single-file path — retained only for migration cleanup.
    fn legacy_boot_count_path(&self) -> PathBuf {
        self.state_dir
            .join(format!(".boot_count_{}", self.current_version))
    }

    /// Prefix used by per-PID boot-count marker files for this version.
    fn boot_count_pid_prefix(&self) -> String {
        format!(".boot_count_pid_{}_", self.current_version)
    }

    /// Path for a specific PID's boot-count marker (current version).
    fn boot_count_pid_path(&self, pid: u32) -> PathBuf {
        self.state_dir
            .join(format!("{}{}", self.boot_count_pid_prefix(), pid))
    }

    /// Count the boot attempts recorded for the current version by summing
    /// `.boot_count_pid_{VERSION}_*` marker files. Returns 0 if the install
    /// directory cannot be read (first boot, missing dir, etc.).
    pub(crate) fn boot_count(&self) -> std::io::Result<u32> {
        let prefix = self.boot_count_pid_prefix();
        let entries = match std::fs::read_dir(&self.state_dir) {
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
        if let Ok(entries) = std::fs::read_dir(&self.state_dir) {
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
        self.state_dir
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

    /// Mark a clean process shutdown as a successful boot when the app exits
    /// before the healthy-writer threshold fires.
    pub fn mark_clean_shutdown(&self) -> Result<(), ProbeError> {
        if !self.install_pending_path().exists() || self.self_healthy_path().exists() {
            return Ok(());
        }
        write_self_healthy_and_cleanup(&self.state_dir, &self.backup_dir, &self.current_version)
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
        let state_dir = self.state_dir.clone();
        let backup_dir = self.backup_dir.clone();
        let version = self.current_version.clone();
        let threshold = self.healthy_threshold;

        handle.spawn(async move {
            tokio::time::sleep(threshold).await;
            if let Err(err) = write_self_healthy_and_cleanup(&state_dir, &backup_dir, &version) {
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
fn write_self_healthy_and_cleanup(
    state_dir: &Path,
    backup_dir: &Path,
    version: &str,
) -> Result<(), ProbeError> {
    // Read the pending marker FIRST to capture backup_path before deleting it.
    let install_pending_path = state_dir.join(format!(".install_pending_{version}"));
    let keep_backup: Option<PathBuf> = match std::fs::read(&install_pending_path) {
        Ok(bytes) => serde_json::from_slice::<InstallPending>(&bytes)
            .ok()
            .map(|p| p.backup_path),
        Err(_) => None,
    };

    // Write the self-healthy marker.
    let marker_path = state_dir.join(format!(".self_healthy_{version}"));
    std::fs::write(&marker_path, Utc::now().to_rfc3339())?;

    // Remove now-stale pending file (ignore failures — cleanup is best-effort).
    let _ = std::fs::remove_file(&install_pending_path);

    // Remove all per-PID boot-count markers for the CURRENT version + the
    // legacy single-file if still present. Foreign-version files are
    // handled by the sweep below.
    let current_prefix = format!(".boot_count_pid_{version}_");
    if let Ok(entries) = std::fs::read_dir(state_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with(&current_prefix) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
    let _ = std::fs::remove_file(state_dir.join(format!(".boot_count_{version}")));

    // Rollback backups remain adjacent to the executable, while mutable probe
    // state may live in app data on macOS. Sweep the backup's own directory so
    // moving markers outside the signed bundle does not weaken backup cleanup.
    let backup_dir = keep_backup
        .as_deref()
        .and_then(Path::parent)
        .unwrap_or(backup_dir);
    if let Ok(entries) = std::fs::read_dir(backup_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let is_rollback_backup = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".rollback."));
            if is_rollback_backup && keep_backup.as_ref() != Some(&path) {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    // Sweep foreign-version state files.
    //
    // Loop 3 iter 1 fix (I-2): previously this sweep only removed
    // `*.rollback.*` files, leaving stale `.install_pending_{OLDER}` /
    // `.boot_count_{OLDER}` / `.self_healthy_{OLDER}` files to accrete
    // across upgrades. Now also reclaim state files whose version suffix
    // does NOT match the current version — the current probe has just
    // written its own self_healthy marker, so anything else is stale.
    if let Ok(entries) = std::fs::read_dir(state_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };

            // (a) Foreign-version per-PID boot-count sweep. Format is
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

            // (b) Foreign-version legacy single-file boot-count sweep.
            //     Always deleted when encountered — the per-PID format is
            //     authoritative now, so any `.boot_count_<VER>` residual
            //     (regardless of VER) is stale.
            if let Some(ver_suffix) = name.strip_prefix(".boot_count_") {
                if !ver_suffix.is_empty() {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }

            // (c) Foreign-version install-pending / self-healthy sweep.
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
// Extracted to health_probe_tests.rs per ADR-013 (file was 855L total).

#[cfg(test)]
#[path = "health_probe_tests.rs"]
mod tests;
