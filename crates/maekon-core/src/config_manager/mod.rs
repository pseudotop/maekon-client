//! Configuration manager — ADR-013 folder layout.
//!
//! # Submodule responsibilities
//! - [`migration`]  — version migration applied on load
//! - [`persistence`] — file I/O (load / save) with path-injection barriers
//! - [`path_resolution`] — platform config/data directory resolution
//!
//! # Public API
//! All public items that existed on the flat `config_manager.rs` are
//! re-exported from this module so every existing import path
//! (`maekon_core::config_manager::ConfigManager`, etc.) continues to work
//! without any caller-side changes.

mod migration;
mod path_resolution;
pub(crate) mod persistence;

pub use path_resolution::{config_dir, data_dir, keychain_service_name, managed_config_dir};

use crate::config::{load_managed_config, AppConfig, ManagedConfig};
use crate::error::CoreError;
use parking_lot::Mutex;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Executes `f` (a synchronous, potentially blocking filesystem operation)
/// without stalling the tokio multi-thread worker thread pool.
///
/// In production the Tauri app runs on a `multi_thread` tokio runtime.
/// `tokio::task::block_in_place` notifies the scheduler that this worker thread
/// is about to block, allowing it to move other ready tasks to idle workers so
/// no async task is starved during the disk write.
///
/// `block_in_place` is **only valid on the multi-thread scheduler** — on a
/// `current_thread` runtime it panics with "can call blocking only when running
/// on the multi-threaded runtime". This is detected at runtime (not via a
/// `#[cfg(test)]` gate, which would only cover this crate's own test builds and
/// not downstream crates that compile `maekon-core` in release mode but call
/// `update`/`update_with` from a single-threaded `#[tokio::test]`). When the
/// current runtime is `current_thread`, or there is no runtime at all, `f` is
/// called directly: the IO is a single short config-file write and the
/// `writer_lock` already serialises writers, so a direct call is safe.
///
/// # Holds the parking_lot writer_lock
/// The caller holds `Inner::writer_lock` across this call — that is intentional
/// and safe: we never cross an `.await` point while the guard is held, so
/// the guard's non-`Send`-ness is never an issue.
#[inline]
fn blocking_io<T>(f: impl FnOnce() -> T) -> T {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        // current_thread runtime, or called outside any tokio runtime.
        _ => f(),
    }
}

/// Configuration store with a `watch`-backed broadcast bus.
///
/// The source of truth is `inner.sender.borrow()`. Writers go through
/// `update`, `update_with`, or `reload`, each of which serialises on
/// `inner.writer_lock` and then calls `send_replace`. `subscribe()` /
/// `snapshot()` are zero-cost reads.
///
/// `Clone` is cheap: clones share `Arc<Inner>`. The `writer_lock` is therefore
/// process-wide (all clones contend on the same mutex), which matches the
/// previous `Arc<RwLock<AppConfig>>` semantics.
#[derive(Debug, Clone)]
pub struct ConfigManager {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    /// Broadcast + source of truth. `borrow()` is cheap.
    sender: watch::Sender<Arc<AppConfig>>,
    /// Linearises concurrent writers across the (non-atomic) compute-new →
    /// persist → send_replace sequence. Held briefly, never across `.await`.
    writer_lock: Mutex<()>,
    config_path: PathBuf,
    /// Admin-deployed managed (MDM) policy, loaded once at construction.
    /// `None` = no policy file = consumer/normal operation (#4832).
    managed: Option<ManagedConfig>,
    /// `true` when the on-disk config was found corrupt at construction and we
    /// fell back to the privacy-preserving recovery default (telemetry/capture
    /// off). Surfaced to the UI via
    /// [`ConfigManager::config_was_reset_from_corruption`] so the user can be
    /// told their settings were reset rather than silently re-enabling
    /// collection. Set once at construction; never mutated after.
    config_reset_from_corruption: bool,
}

/// Map an `AppConfig::validate_bounds` failure message to a typed config error
/// (#6102-4). Shared by the `update` / `update_with` / `reload` write paths so
/// an out-of-range value is rejected consistently.
fn map_bounds_error(message: String) -> CoreError {
    CoreError::Config {
        code: crate::error_codes::ConfigCode::Invalid,
        message,
    }
}

impl ConfigManager {
    pub fn new() -> Result<Self, CoreError> {
        let config_path = path_resolution::default_config_path()?;
        Self::with_path(config_path)
    }

    /// Construct using the platform-default managed-policy path (`managed.json`).
    ///
    /// Resolves the managed path via [`path_resolution::managed_config_path`]
    /// (honoring `MAEKON_MANAGED_CONFIG_PATH`). A present-but-broken managed
    /// file makes this fail-closed; an absent one is normal operation.
    pub fn with_path(config_path: PathBuf) -> Result<Self, CoreError> {
        let managed_path = path_resolution::managed_config_path()?;
        Self::with_paths(config_path, Some(managed_path))
    }

    /// Construct with explicit config + managed paths.
    ///
    /// `managed_path == None` means "no managed policy at all" (used by tests to
    /// exercise the consumer path without the platform default leaking in).
    /// `Some(path)` is loaded fail-closed: malformed/future-schema ⇒ `Err`.
    pub fn with_paths(
        config_path: PathBuf,
        managed_path: Option<PathBuf>,
    ) -> Result<Self, CoreError> {
        persistence::validate_config_file_path(&config_path)?;

        // Load managed policy first: a present-but-broken file must abort
        // startup BEFORE we touch the user config (fail-closed).
        let managed = match managed_path {
            Some(path) => load_managed_config(&path)?,
            None => None,
        };

        if let Some(parent) = config_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| CoreError::Config {
                    code: crate::error_codes::ConfigCode::Invalid,
                    message: format!(
                        "Failed to create config directory: {}: {}",
                        parent.display(),
                        e
                    ),
                })?;
                info!("settings create: {}", parent.display());
            }
        }

        // Set when an existing config file fails to parse and we fall back to
        // the privacy-preserving recovery default (see below).
        let mut config_reset_from_corruption = false;
        let mut initial = if config_path.exists() {
            match migration::load_and_migrate_from_file(&config_path) {
                Ok(c) => c,
                // #10985: a config written by a NEWER client is not corrupt. It
                // is the user's settings, still well-formed, and the previous
                // code overwrote it with recovery defaults — so rolling back to
                // an older build silently destroyed them, and rolling forward
                // again found a downgraded file. Read-only fallback here: run on
                // defaults for this session, leave the file exactly as it is.
                Err(e)
                    if matches!(
                        &e,
                        CoreError::Config { code, .. }
                            if *code == crate::error_codes::ConfigCode::SchemaTooNew
                    ) =>
                {
                    error!(
                        path = %config_path.display(),
                        error = %e,
                        "config was written by a newer client; running on defaults for this \
                         session WITHOUT modifying the file — install the newer build again to \
                         recover these settings"
                    );
                    config_reset_from_corruption = true;
                    AppConfig::fail_closed_recovery_default()
                }
                Err(e) => {
                    // Distinct error-level signal: a corrupt config means the
                    // user's prior privacy choices were lost. This is NOT the
                    // routine "file missing -> seed defaults" path, so it must
                    // not be hidden behind the warn-level noise floor.
                    error!(
                        path = %config_path.display(),
                        error = %e,
                        "config file corrupted; settings reset to privacy-preserving \
                         defaults (telemetry/capture disabled until re-enabled)"
                    );
                    config_reset_from_corruption = true;
                    // Fail-closed: do not seed any telemetry/capture intent
                    // over an unrecoverable opt-out.
                    let recovery_config = AppConfig::fail_closed_recovery_default();
                    // Overwrite the corrupt file so the next launch is clean.
                    if let Err(e) = persistence::save_to_file(&config_path, &recovery_config) {
                        debug!("save_to_file failed: {e}");
                    }
                    recovery_config
                }
            }
        } else {
            let default_config = AppConfig::default_config();
            persistence::save_to_file(&config_path, &default_config)?;
            info!("default settings file create: {}", config_path.display());
            default_config
        };

        // Clamp the loaded config to managed policy BEFORE seeding the broadcast
        // bus, so the very first `get()`/`subscribe()` already observes the
        // locked values (no stale-read gap), and rewrite the user config on disk
        // so the lock survives a future launch even if policy later disappears.
        if let Some(managed) = &managed {
            let clamped = managed.apply(&mut initial);
            if !clamped.is_empty() {
                warn!(
                    target: "managed_policy",
                    fields = ?clamped,
                    "managed policy clamped user config at startup (override blocked)"
                );
                if let Err(e) = persistence::save_to_file(&config_path, &initial) {
                    debug!("save_to_file (managed clamp) failed: {e}");
                }
            }
        }

        // #6169/#6177: bounds-clamp the loaded config BEFORE seeding the bus.
        // Unlike the write chokepoints (`update`/`update_with`/`reload`), which
        // REJECT out-of-range values, the INITIAL-LOAD path must stay fail-open:
        // a hand-edited / downgrade config.json with sub-floor intervals (e.g.
        // `monitor.poll_interval_ms: 0` or `analysis.interval_secs: 0`) would
        // otherwise reach the scheduler unvalidated and panic or hot-spin a
        // `tokio::time::interval`. Raise such values to their floor and persist
        // the correction (mirrors the managed-clamp rewrite above) so the fix
        // survives a relaunch. Applied after the managed overlay so a managed
        // value is clamped to policy first, then to its floor.
        //
        // #6883: the same clamp also fail-closes `web.allow_external` when the persisted
        // `integration_auth_token` is sub-strength and snaps an out-of-range `web.port`,
        // so a downgrade / hand-edited config that never passed the #6772 write-path
        // strength gate cannot bind the integration API to 0.0.0.0 with a weak bearer.
        let bounds_clamped = initial.clamp_bounds();
        if !bounds_clamped.is_empty() {
            warn!(
                target: "config_bounds",
                fields = ?bounds_clamped,
                "config bounds clamped at startup (interval floors: fail-open; web.allow_external/port: fail-closed) — see `fields`"
            );
            if let Err(e) = persistence::save_to_file(&config_path, &initial) {
                debug!("save_to_file (bounds clamp) failed: {e}");
            }
        }

        let (sender, _rx) = watch::channel(Arc::new(initial));
        // Dropping `_rx` is fine — `watch::Sender` does not require any receivers
        // to exist. `subscribe()` lazily creates them.

        Ok(Self {
            inner: Arc::new(Inner {
                sender,
                writer_lock: Mutex::new(()),
                config_path,
                managed,
                config_reset_from_corruption,
            }),
        })
    }

    /// Whether the config was reset to privacy-preserving defaults because the
    /// on-disk file was found corrupt at construction.
    ///
    /// `true` means the user's prior settings (possibly including an explicit
    /// telemetry/capture opt-out) could not be recovered, so collection knobs
    /// were forced off and the user should be prompted to review them. The UI
    /// reads this once after startup to surface a "settings were reset" notice.
    pub fn config_was_reset_from_corruption(&self) -> bool {
        self.inner.config_reset_from_corruption
    }

    pub fn get(&self) -> AppConfig {
        AppConfig::clone(&self.inner.sender.borrow())
    }

    /// Subscribe to whole-config change notifications.
    ///
    /// The receiver starts at the current config. `changed().await` resolves
    /// after the next `update` / `update_with` / `reload`. Dropping a receiver
    /// does not affect any other subscriber.
    ///
    /// `watch` has latest-wins semantics: rapid mutations may be coalesced and
    /// a subscriber that wakes late will see only the final value, not every
    /// intermediate transition. Consumers whose correctness depends on
    /// observing every transition (audit-log callers, counters) must either
    /// keep a tick-based poll structure OR run every `update` through their
    /// own side-effect channel. See ADR-016 for the audit-coalescing hazard.
    pub fn subscribe(&self) -> watch::Receiver<Arc<AppConfig>> {
        self.inner.sender.subscribe()
    }

    /// Cheap read-only snapshot of the current config.
    ///
    /// Equivalent to `subscribe().borrow().clone()` without registering a
    /// subscriber. Prefer this over `get()` when the caller is happy with an
    /// `Arc<AppConfig>` (no deep clone).
    pub fn snapshot(&self) -> Arc<AppConfig> {
        self.inner.sender.borrow().clone()
    }

    pub fn update(&self, mut new_config: AppConfig) -> Result<(), CoreError> {
        let _guard = self.inner.writer_lock.lock();
        // Managed-policy clamp: this is THE write chokepoint, so every caller
        // (settings API, Tauri IPC, backup restore, scheduler) is enforced by
        // construction — a per-callsite guard would leak via siblings (#4832).
        self.enforce_managed_overlay(&mut new_config);
        // #6102-4: reject out-of-range values at THE write chokepoint (after the
        // managed clamp, so a managed value clamped into range is honored, but a
        // caller override that escapes the bounds is rejected rather than spinning
        // the scheduler loops on a sub-second interval).
        new_config.validate_bounds().map_err(map_bounds_error)?;
        // #6256: reject an auto-updater signature-verification downgrade at THE
        // write chokepoint, so every interactive write surface (settings HTTP
        // API, Tauri IPC, backup restore) is covered by construction — not just
        // the WebView allowlist. `validate_integrity_policy` is a no-op when
        // updates are disabled and passes the default (verification on), so this
        // only blocks `update.enabled=true` paired with
        // `require_signature_verification=false`.
        new_config.update.validate_integrity_policy()?;
        // `blocking_io` wraps `block_in_place` so that the multi-thread tokio
        // scheduler can move other tasks off this worker thread while the
        // synchronous fs::write executes, preventing worker-thread starvation.
        // The parking_lot guard is held across the call (no `.await`), which is
        // safe — parking_lot guards are non-`Send` only in the async-Send sense,
        // and we are in a synchronous stack frame throughout.
        blocking_io(|| persistence::save_to_file(&self.inner.config_path, &new_config))?;
        self.inner.sender.send_replace(Arc::new(new_config));
        debug!(
            "settings save complete: {}",
            self.inner.config_path.display()
        );
        Ok(())
    }

    /// Atomically read-modify-write the config while holding the writer lock
    /// throughout, preventing TOCTOU races between concurrent callers.
    pub fn update_with<F>(&self, updater: F) -> Result<AppConfig, CoreError>
    where
        F: FnOnce(&mut AppConfig) -> Result<(), String>,
    {
        let _guard = self.inner.writer_lock.lock();
        let mut new_cfg = (**self.inner.sender.borrow()).clone();
        updater(&mut new_cfg).map_err(|message| CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message,
        })?;
        // Managed-policy clamp at the chokepoint (see `update`).
        self.enforce_managed_overlay(&mut new_cfg);
        // #6102-4: bounds check after the managed clamp (see `update`).
        new_cfg.validate_bounds().map_err(map_bounds_error)?;
        // #6256: same updater signature-verification downgrade guard as `update`.
        new_cfg.update.validate_integrity_policy()?;
        // Same `block_in_place` guard as `update` — see comment there.
        blocking_io(|| persistence::save_to_file(&self.inner.config_path, &new_cfg))?;
        let snapshot = new_cfg.clone();
        self.inner.sender.send_replace(Arc::new(new_cfg));
        debug!(
            "settings save complete: {}",
            self.inner.config_path.display()
        );
        Ok(snapshot)
    }

    pub fn config_path(&self) -> &PathBuf {
        &self.inner.config_path
    }

    pub fn reload(&self) -> Result<(), CoreError> {
        let _guard = self.inner.writer_lock.lock();
        let mut reloaded = migration::load_and_migrate_from_file(&self.inner.config_path)?;
        // Re-clamp on reload so an externally edited config.json cannot escape
        // managed policy (cheap defense-in-depth at an existing writer).
        self.enforce_managed_overlay(&mut reloaded);
        // #6102-4: an externally hand-edited config.json must not load out-of-range
        // values that escape the scheduler's interval floors.
        reloaded.validate_bounds().map_err(map_bounds_error)?;
        self.inner.sender.send_replace(Arc::new(reloaded));
        info!("settings load complete");
        Ok(())
    }

    /// Clamp `cfg` to managed policy in place. No-op when no policy is loaded.
    ///
    /// Emits a `managed_policy` warn record for each clamp so every write path
    /// produces an override-attempt audit trail by construction. (A structured
    /// audit-event sink is the ADMX follow-up hook, #4837.)
    fn enforce_managed_overlay(&self, cfg: &mut AppConfig) {
        let Some(managed) = &self.inner.managed else {
            return;
        };
        let clamped = managed.apply(cfg);
        if !clamped.is_empty() {
            warn!(
                target: "managed_policy",
                fields = ?clamped,
                "managed policy re-clamped a write (user override blocked)"
            );
        }
    }

    /// Dotted-path identities where `candidate` violates a locked managed value.
    ///
    /// Interactive write paths (settings API, Tauri IPC) call this BEFORE
    /// `update`/`update_with` to reject a user override with a clear message,
    /// instead of relying on the silent chokepoint clamp. Empty when there is no
    /// managed policy or the candidate complies.
    pub fn detect_managed_violations(&self, candidate: &AppConfig) -> Vec<String> {
        match &self.inner.managed {
            Some(managed) => managed
                .violations(candidate)
                .into_iter()
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        }
    }

    /// Dotted-path identities of all fields locked by managed policy (for ADMX
    /// docs / UI greying, #4837). Empty when there is no managed policy.
    pub fn managed_locked_fields(&self) -> Vec<String> {
        match &self.inner.managed {
            Some(managed) => managed
                .locked_fields()
                .into_iter()
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        }
    }

    /// Returns the platform-appropriate config directory for this application.
    ///
    /// Delegates to [`path_resolution::config_dir`].
    pub fn config_dir() -> Result<PathBuf, CoreError> {
        path_resolution::config_dir()
    }

    /// Returns the platform-appropriate data directory for this application.
    ///
    /// Delegates to [`path_resolution::data_dir`].
    pub fn data_dir() -> Result<PathBuf, CoreError> {
        path_resolution::data_dir()
    }
}

#[cfg(test)]
mod tests;
