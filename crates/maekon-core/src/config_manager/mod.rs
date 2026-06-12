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

pub use path_resolution::{config_dir, data_dir, managed_config_dir};

use crate::config::{load_managed_config, AppConfig, ManagedConfig};
use crate::error::CoreError;
use parking_lot::Mutex;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

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

        let mut initial = if config_path.exists() {
            match migration::load_and_migrate_from_file(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        path = %config_path.display(),
                        error = %e,
                        "config file corrupted, falling back to defaults"
                    );
                    let default_config = AppConfig::default_config();
                    // Overwrite the corrupt file so the next launch is clean.
                    if let Err(e) = persistence::save_to_file(&config_path, &default_config) {
                        debug!("save_to_file failed: {e}");
                    }
                    default_config
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

        let (sender, _rx) = watch::channel(Arc::new(initial));
        // Dropping `_rx` is fine — `watch::Sender` does not require any receivers
        // to exist. `subscribe()` lazily creates them.

        Ok(Self {
            inner: Arc::new(Inner {
                sender,
                writer_lock: Mutex::new(()),
                config_path,
                managed,
            }),
        })
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
        persistence::save_to_file(&self.inner.config_path, &new_config)?;
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
        persistence::save_to_file(&self.inner.config_path, &new_cfg)?;
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
