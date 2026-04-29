use crate::config::AppConfig;
use crate::error::CoreError;
use parking_lot::Mutex;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{debug, info, warn};

const CONFIG_FILE_NAME: &str = "config.json";

const APP_DIR_NAME: &str = "oneshim";
const APP_FLAVOR_ENV: &str = "ONESHIM_APP_FLAVOR";

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
}

impl ConfigManager {
    pub fn new() -> Result<Self, CoreError> {
        let config_path = Self::default_config_path()?;
        Self::with_path(config_path)
    }

    pub fn with_path(config_path: PathBuf) -> Result<Self, CoreError> {
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

        let initial = if config_path.exists() {
            match Self::load_and_migrate_from_file(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        path = %config_path.display(),
                        error = %e,
                        "config file corrupted, falling back to defaults"
                    );
                    let default_config = AppConfig::default_config();
                    // Overwrite the corrupt file so the next launch is clean.
                    if let Err(e) = Self::save_to_file(&config_path, &default_config) {
                        debug!("save_to_file failed: {e}");
                    }
                    default_config
                }
            }
        } else {
            let default_config = AppConfig::default_config();
            Self::save_to_file(&config_path, &default_config)?;
            info!("default settings file create: {}", config_path.display());
            default_config
        };

        let (sender, _rx) = watch::channel(Arc::new(initial));
        // Dropping `_rx` is fine — `watch::Sender` does not require any receivers
        // to exist. `subscribe()` lazily creates them.

        Ok(Self {
            inner: Arc::new(Inner {
                sender,
                writer_lock: Mutex::new(()),
                config_path,
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

    pub fn update(&self, new_config: AppConfig) -> Result<(), CoreError> {
        let _guard = self.inner.writer_lock.lock();
        Self::save_to_file(&self.inner.config_path, &new_config)?;
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
        Self::save_to_file(&self.inner.config_path, &new_cfg)?;
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
        let reloaded = Self::load_and_migrate_from_file(&self.inner.config_path)?;
        self.inner.sender.send_replace(Arc::new(reloaded));
        info!("settings load complete");
        Ok(())
    }

    fn default_config_path() -> Result<PathBuf, CoreError> {
        let config_dir = Self::config_dir()?;
        Ok(config_dir.join(CONFIG_FILE_NAME))
    }

    fn app_dir_name() -> String {
        let flavor = std::env::var(APP_FLAVOR_ENV).ok();
        Self::app_dir_name_for_flavor(flavor.as_deref())
    }

    fn app_dir_name_for_flavor(flavor: Option<&str>) -> String {
        let Some(flavor) = flavor.map(str::trim).filter(|s| !s.is_empty()) else {
            return APP_DIR_NAME.to_string();
        };

        let suffix: String = flavor
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(*c, '-' | '_'))
            .collect();

        if suffix.is_empty() {
            APP_DIR_NAME.to_string()
        } else {
            format!("{APP_DIR_NAME}-{suffix}")
        }
    }

    pub fn config_dir() -> Result<PathBuf, CoreError> {
        let app_dir_name = Self::app_dir_name();

        #[cfg(target_os = "macos")]
        {
            // macOS: ~/Library/Application Support/{app_dir_name}/
            let home = std::env::var("HOME").map_err(|_| CoreError::Config {
                code: crate::error_codes::ConfigCode::Missing,
                message: "HOME environment variable not found".to_string(),
            })?;
            Ok(PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join(app_dir_name))
        }

        #[cfg(target_os = "windows")]
        {
            // Windows: %APPDATA%\{app_dir_name}\
            let appdata = std::env::var("APPDATA").map_err(|_| CoreError::Config {
                code: crate::error_codes::ConfigCode::Missing,
                message: "APPDATA environment variable not found".to_string(),
            })?;
            Ok(PathBuf::from(appdata).join(app_dir_name))
        }

        #[cfg(target_os = "linux")]
        {
            // Linux: ~/.config/{app_dir_name}/
            let home = std::env::var("HOME").map_err(|_| CoreError::Config {
                code: crate::error_codes::ConfigCode::Missing,
                message: "HOME environment variable not found".to_string(),
            })?;
            Ok(PathBuf::from(home).join(".config").join(app_dir_name))
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            tracing::warn!("Unsupported platform; using current directory as config base");
            Ok(PathBuf::from(".").join(app_dir_name))
        }
    }

    pub fn data_dir() -> Result<PathBuf, CoreError> {
        #[cfg(target_os = "macos")]
        {
            // macOS: ~/Library/Application Support/{app_dir_name}/data/
            Self::config_dir().map(|p| p.join("data"))
        }

        #[cfg(target_os = "windows")]
        {
            let app_dir_name = Self::app_dir_name();
            // Windows: %LOCALAPPDATA%\{app_dir_name}\data\
            let local_appdata = std::env::var("LOCALAPPDATA").map_err(|_| CoreError::Config {
                code: crate::error_codes::ConfigCode::Missing,
                message: "LOCALAPPDATA environment variable not found".to_string(),
            })?;
            Ok(PathBuf::from(local_appdata).join(app_dir_name).join("data"))
        }

        #[cfg(target_os = "linux")]
        {
            let app_dir_name = Self::app_dir_name();
            // Linux: ~/.local/share/{app_dir_name}/
            let home = std::env::var("HOME").map_err(|_| CoreError::Config {
                code: crate::error_codes::ConfigCode::Missing,
                message: "HOME environment variable not found".to_string(),
            })?;
            Ok(PathBuf::from(home)
                .join(".local")
                .join("share")
                .join(app_dir_name))
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let app_dir_name = Self::app_dir_name();
            Ok(PathBuf::from(".").join(app_dir_name).join("data"))
        }
    }

    fn load_and_migrate_from_file(path: &PathBuf) -> Result<AppConfig, CoreError> {
        let mut config = Self::load_from_file(path)?;
        if Self::migrate_loaded_config(&mut config) {
            if let Err(e) = Self::save_to_file(path, &config) {
                warn!(path = %path.display(), error = %e, "settings migration persist failed");
            } else {
                info!("settings migration applied: {}", path.display());
            }
        }
        Ok(config)
    }

    fn migrate_loaded_config(config: &mut AppConfig) -> bool {
        if config.web.grpc_port == crate::config::LEGACY_GRPC_DASHBOARD_PORT {
            config.web.grpc_port = crate::config::DEFAULT_GRPC_DASHBOARD_PORT;
            return true;
        }
        false
    }

    fn load_from_file(path: &PathBuf) -> Result<AppConfig, CoreError> {
        let content = fs::read_to_string(path).map_err(|e| CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("Failed to read config file: {}: {}", path.display(), e),
        })?;

        let config: AppConfig = serde_json::from_str(&content).map_err(|e| CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("Failed to parse config file: {}: {}", path.display(), e),
        })?;

        debug!("settings file load complete: {}", path.display());
        Ok(config)
    }

    fn save_to_file(path: &PathBuf, config: &AppConfig) -> Result<(), CoreError> {
        let content = serde_json::to_string_pretty(config).map_err(|e| CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("Failed to serialize config: {}", e),
        })?;

        fs::write(path, content).map_err(|e| CoreError::Config {
            code: crate::error_codes::ConfigCode::Invalid,
            message: format!("Failed to write config file: {}: {}", path.display(), e),
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::{Mutex as StdMutex, OnceLock};
    use tempfile::TempDir;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(())).lock().unwrap()
    }

    fn restore_env_var(key: &str, original: Option<OsString>) {
        match original {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn create_and_load_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let manager = ConfigManager::with_path(config_path.clone()).unwrap();
        assert!(config_path.exists());

        let config = manager.get();
        assert_eq!(config.web.port, crate::config::DEFAULT_WEB_PORT);
    }

    #[test]
    fn update_and_persist_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let manager = ConfigManager::with_path(config_path.clone()).unwrap();

        manager
            .update_with(|c| {
                c.web.port = 8080;
                c.storage.retention_days = 60;
                Ok(())
            })
            .unwrap();

        let manager2 = ConfigManager::with_path(config_path).unwrap();
        let config = manager2.get();

        assert_eq!(config.web.port, 8080);
        assert_eq!(config.storage.retention_days, 60);
    }

    #[test]
    fn load_migrates_legacy_grpc_dashboard_default_port() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let mut config = AppConfig::default_config();
        config.web.grpc_port = crate::config::LEGACY_GRPC_DASHBOARD_PORT;
        let content = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&config_path, content).unwrap();

        let manager = ConfigManager::with_path(config_path.clone()).unwrap();
        assert_eq!(
            manager.get().web.grpc_port,
            crate::config::DEFAULT_GRPC_DASHBOARD_PORT
        );

        let persisted = ConfigManager::load_from_file(&config_path).unwrap();
        assert_eq!(
            persisted.web.grpc_port,
            crate::config::DEFAULT_GRPC_DASHBOARD_PORT
        );
    }

    #[test]
    fn load_preserves_custom_grpc_dashboard_port() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let mut config = AppConfig::default_config();
        config.web.grpc_port = 55_555;
        let content = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&config_path, content).unwrap();

        let manager = ConfigManager::with_path(config_path).unwrap();
        assert_eq!(manager.get().web.grpc_port, 55_555);
    }

    #[test]
    fn reload_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let manager = ConfigManager::with_path(config_path.clone()).unwrap();

        let mut config = manager.get();
        config.web.port = 7777;
        let content = serde_json::to_string_pretty(&config).unwrap();
        fs::write(&config_path, content).unwrap();

        manager.reload().unwrap();
        assert_eq!(manager.get().web.port, 7777);
    }

    #[test]
    fn config_dir_exists() {
        let config_dir = ConfigManager::config_dir();
        assert!(config_dir.is_ok());

        let data_dir = ConfigManager::data_dir();
        assert!(data_dir.is_ok());
    }

    #[test]
    fn app_flavor_suffixes_default_directories() {
        let _guard = env_lock();
        let temp_dir = TempDir::new().unwrap();
        let original_home = std::env::var_os("HOME");
        let original_appdata = std::env::var_os("APPDATA");
        let original_local_appdata = std::env::var_os("LOCALAPPDATA");
        let original_flavor = std::env::var_os("ONESHIM_APP_FLAVOR");

        std::env::set_var("HOME", temp_dir.path());
        std::env::set_var("APPDATA", temp_dir.path());
        std::env::set_var("LOCALAPPDATA", temp_dir.path());
        std::env::set_var("ONESHIM_APP_FLAVOR", "dev");

        let config_dir = ConfigManager::config_dir().unwrap();
        let data_dir = ConfigManager::data_dir().unwrap();

        restore_env_var("ONESHIM_APP_FLAVOR", original_flavor);
        restore_env_var("LOCALAPPDATA", original_local_appdata);
        restore_env_var("APPDATA", original_appdata);
        restore_env_var("HOME", original_home);

        assert_eq!(
            config_dir.file_name().and_then(|name| name.to_str()),
            Some("oneshim-dev"),
            "config dir should be separated by app flavor: {}",
            config_dir.display()
        );

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert!(
            data_dir
                .parent()
                .and_then(|path| path.file_name())
                .and_then(|name| name.to_str())
                == Some("oneshim-dev"),
            "data dir should be separated by app flavor: {}",
            data_dir.display()
        );

        #[cfg(target_os = "linux")]
        assert_eq!(
            data_dir.file_name().and_then(|name| name.to_str()),
            Some("oneshim-dev"),
            "data dir should be separated by app flavor: {}",
            data_dir.display()
        );

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        assert!(
            data_dir.ends_with(std::path::Path::new("oneshim-dev").join("data")),
            "data dir should be separated by app flavor: {}",
            data_dir.display()
        );
    }

    #[test]
    fn reload_with_corrupted_json_returns_error() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        // Create a valid config so the manager initialises successfully.
        let manager = ConfigManager::with_path(config_path.clone()).unwrap();
        assert!(config_path.exists());

        // Overwrite the file with invalid JSON.
        fs::write(&config_path, r#"{"invalid": }"#).unwrap();

        // reload() must propagate the parse error as a CoreError::Config variant.
        let result = manager.reload();
        assert!(
            result.is_err(),
            "reload() should return Err when the config file contains invalid JSON"
        );
        match result.unwrap_err() {
            CoreError::Config {
                code: crate::error_codes::ConfigCode::Invalid,
                message: msg,
            } => {
                assert!(
                    msg.contains("Failed to parse config file"),
                    "error message should describe the parse failure, got: {msg}"
                );
            }
            other => panic!("expected CoreError::Config, got {other:?}"),
        }

        // The in-memory config must remain unchanged (still the original defaults).
        let config = manager.get();
        assert_eq!(
            config.web.port,
            crate::config::DEFAULT_WEB_PORT,
            "in-memory config should not be mutated after a failed reload"
        );
    }

    // ── Task 27b: ConfigManager file I/O tests ─────────────────────

    #[test]
    fn load_creates_default_when_file_missing() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("nonexistent_dir").join("config.json");

        // The file (and parent dir) do not exist yet.
        assert!(!config_path.exists());

        let manager = ConfigManager::with_path(config_path.clone()).unwrap();

        // File should have been created with defaults.
        assert!(
            config_path.exists(),
            "config file should be created on init"
        );

        let config = manager.get();
        assert_eq!(
            config.web.port,
            crate::config::DEFAULT_WEB_PORT,
            "missing file should produce default config"
        );
        assert_eq!(
            config.storage.retention_days, 30,
            "default retention_days should be 30"
        );
    }

    #[test]
    fn save_then_load_round_trip() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        let manager = ConfigManager::with_path(config_path.clone()).unwrap();

        // Mutate several fields across different sections.
        manager
            .update_with(|c| {
                c.web.port = 9999;
                c.storage.retention_days = 90;
                c.server.base_url = "https://prod.example.com".to_string();
                c.monitor.idle_threshold_secs = 600;
                c.vision.ocr_enabled = true;
                Ok(())
            })
            .unwrap();

        // Create a fresh manager pointing at the same file.
        let manager2 = ConfigManager::with_path(config_path).unwrap();
        let loaded = manager2.get();

        assert_eq!(loaded.web.port, 9999);
        assert_eq!(loaded.storage.retention_days, 90);
        assert_eq!(loaded.server.base_url, "https://prod.example.com");
        assert_eq!(loaded.monitor.idle_threshold_secs, 600);
        assert!(loaded.vision.ocr_enabled);
    }

    #[test]
    fn load_corrupt_file_falls_back_to_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        // Write garbage content before the manager tries to load.
        fs::write(&config_path, "<<<NOT JSON>>>").unwrap();

        // Should NOT crash — falls back to default config.
        let manager = ConfigManager::with_path(config_path.clone()).unwrap();
        let config = manager.get();
        assert_eq!(
            config.web.port,
            crate::config::DEFAULT_WEB_PORT,
            "corrupt config should fall back to defaults"
        );

        // The corrupt file should have been overwritten with valid defaults.
        let reloaded = ConfigManager::with_path(config_path).unwrap();
        assert_eq!(
            reloaded.get().web.port,
            crate::config::DEFAULT_WEB_PORT,
            "overwritten file should contain valid defaults"
        );
    }

    // ── X1 ConfigChangeBus tests ───────────────────────────────────────
    // See docs/reviews/2026-04-17-phase2-config-telemetry-spec.md §2.8.

    /// T-X1-1
    #[test]
    fn subscribe_sees_initial_value() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let rx = mgr.subscribe();
        let via_rx = rx.borrow();
        let via_get = mgr.get();
        // Value equivalence is what matters. The Arc held by rx and the value
        // returned by get() should agree on every field.
        assert_eq!(via_rx.web.port, via_get.web.port);
    }

    /// T-X1-5
    #[tokio::test]
    async fn dropped_receiver_does_not_block_sender() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let rx_a = mgr.subscribe();
        let mut rx_b = mgr.subscribe();
        drop(rx_a);

        // Flip a scalar field so update_with produces a visibly different snapshot.
        mgr.update_with(|c| {
            c.web.port = c.web.port.wrapping_add(1);
            Ok(())
        })
        .expect("update_with must not fail after a receiver drops");

        // The survivor observes the update without deadlocking.
        let _ = rx_b.borrow_and_update();
    }

    /// T-X1-6
    #[test]
    fn snapshot_matches_latest_update() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let baseline_port = mgr.get().web.port;
        mgr.update_with(|c| {
            c.web.port = baseline_port.wrapping_add(1);
            Ok(())
        })
        .unwrap();

        let via_snapshot = mgr.snapshot();
        let rx = mgr.subscribe();
        let via_rx = rx.borrow();

        assert_eq!(via_snapshot.web.port, via_rx.web.port);
        assert_eq!(via_snapshot.web.port, baseline_port.wrapping_add(1));
    }

    /// T-X1-2
    #[tokio::test]
    async fn update_notifies_subscribers() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let mut rx = mgr.subscribe();
        let before = rx.borrow_and_update().web.port;

        let mut new_cfg = mgr.get();
        new_cfg.web.port = before.wrapping_add(7);
        mgr.update(new_cfg).unwrap();

        rx.changed()
            .await
            .expect("changed() must resolve after update()");
        assert_eq!(rx.borrow().web.port, before.wrapping_add(7));
    }

    /// T-X1-3
    #[tokio::test]
    async fn update_with_notifies_subscribers() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let mut rx = mgr.subscribe();
        let before = rx.borrow_and_update().web.port;

        mgr.update_with(|c| {
            c.web.port = before.wrapping_add(11);
            Ok(())
        })
        .unwrap();

        rx.changed()
            .await
            .expect("changed() must resolve after update_with()");
        assert_eq!(rx.borrow().web.port, before.wrapping_add(11));
    }

    /// T-X1-4
    #[tokio::test]
    async fn reload_notifies_subscribers() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path.clone()).unwrap();

        let mut rx = mgr.subscribe();
        let before = rx.borrow_and_update().web.port;

        // Rewrite the file out-of-band so reload() observes a different value.
        let mut forced = AppConfig::default_config();
        forced.web.port = before.wrapping_add(13);
        let json = serde_json::to_string_pretty(&forced).unwrap();
        std::fs::write(&cfg_path, json).unwrap();

        mgr.reload().unwrap();
        rx.changed()
            .await
            .expect("changed() must resolve after reload()");
        assert_eq!(rx.borrow().web.port, before.wrapping_add(13));
    }

    /// T-X1-7 — pins latest-wins: identical-content updates still fire.
    ///
    /// Documents the audit-coalescing hazard described in the subscribe() doc
    /// comment. Consumers that need transition-per-update semantics must diff
    /// or use their own channel.
    #[tokio::test]
    async fn each_update_fires_even_for_identical_content() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let mut rx = mgr.subscribe();
        rx.borrow_and_update(); // consume the initial value

        let cfg = mgr.get();
        mgr.update(cfg.clone()).unwrap();
        rx.changed()
            .await
            .expect("first identical update still fires");
        rx.borrow_and_update();

        mgr.update(cfg).unwrap();
        rx.changed()
            .await
            .expect("second identical update still fires");
    }

    /// T-X1-9 — writer_lock is non-reentrant but never crossed with watch reads.
    ///
    /// `get()` and `snapshot()` only touch `self.inner.sender`, not
    /// `self.inner.writer_lock`. Calling them from inside an `update_with`
    /// closure (where the writer_lock is held) must therefore NOT deadlock.
    /// The reads return the *pre-swap* value because `send_replace` has not
    /// been called yet.
    #[test]
    fn update_with_does_not_reenter() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let baseline_port = mgr.get().web.port;
        let new_port = baseline_port.wrapping_add(1);
        let saw_snapshot = AtomicBool::new(false);

        mgr.update_with(|c| {
            // These reads go through `watch::Sender::borrow()`, bypassing
            // writer_lock. Pre-swap they return the OLD value.
            let snap = mgr.snapshot();
            assert_eq!(snap.web.port, baseline_port);
            let get_value = mgr.get();
            assert_eq!(get_value.web.port, baseline_port);
            saw_snapshot.store(true, Ordering::SeqCst);
            c.web.port = new_port;
            Ok(())
        })
        .unwrap();

        assert!(saw_snapshot.load(Ordering::SeqCst));
        assert_eq!(mgr.get().web.port, new_port);
    }

    /// T-X1-10 — subscriber task exits cleanly when the manager is dropped.
    ///
    /// The production bus-driven telemetry task relies on `rx.changed()`
    /// returning `Err` to terminate. This test exercises that path end-to-end.
    #[tokio::test]
    async fn receiver_changed_returns_err_after_manager_dropped() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        let mgr = ConfigManager::with_path(cfg_path).unwrap();

        let mut rx = mgr.subscribe();

        let task = tokio::spawn(async move {
            // Resolves to Err once all senders have dropped.
            rx.changed().await
        });

        drop(mgr);
        let result = tokio::time::timeout(std::time::Duration::from_secs(1), task)
            .await
            .expect("task must not hang when sender drops")
            .expect("task must not panic");

        assert!(
            result.is_err(),
            "changed() must resolve to Err after sender drop"
        );
    }

    /// T-X1-8 — legacy config.json without new telemetry fields deserialises
    /// cleanly. Protects the serde-defaults contract for users upgrading from a
    /// pre-Phase-2 build.
    #[test]
    fn deserialises_legacy_config_json_without_new_telemetry_fields() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");

        // A minimal JSON payload that mimics a pre-Phase-2 config.json: the
        // `telemetry` section lacks `otlp_endpoint`, `sample_rate`, and
        // `service_name`. Other top-level sections are absent entirely so
        // their serde(default) kicks in.
        let legacy = r#"{
          "telemetry": {
            "enabled": false,
            "crash_reports": false,
            "usage_analytics": false,
            "performance_metrics": false
          }
        }"#;
        std::fs::write(&cfg_path, legacy).unwrap();

        let mgr = ConfigManager::with_path(cfg_path).expect("legacy JSON must deserialise");
        let cfg = mgr.get();
        assert_eq!(cfg.telemetry.otlp_endpoint, None);
        assert!((cfg.telemetry.sample_rate - 1.0).abs() < f64::EPSILON);
        assert_eq!(cfg.telemetry.service_name, "oneshim-client");
    }

    /// T-X2-6 — fresh install has telemetry opted OUT.
    #[test]
    fn telemetry_enabled_defaults_to_false() {
        let cfg = AppConfig::default_config();
        assert!(
            !cfg.telemetry.enabled,
            "telemetry must default to opt-out (fresh install)"
        );
    }
}
