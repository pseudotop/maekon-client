use super::persistence;
use super::*;
use std::ffi::OsString;
use std::fs;
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
fn with_path_rejects_parent_dir_components() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir
        .path()
        .join("profile")
        .join("..")
        .join("escaped")
        .join("config.json");

    let result = ConfigManager::with_path(config_path);

    assert!(
        matches!(result.unwrap_err(), crate::error::CoreError::Config { .. }),
        "config paths with parent directory traversal must be rejected with CoreError::Config"
    );
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

    let mut config = crate::config::AppConfig::default_config();
    config.web.grpc_port = crate::config::LEGACY_GRPC_DASHBOARD_PORT;
    let content = serde_json::to_string_pretty(&config).unwrap();
    fs::write(&config_path, content).unwrap();

    let manager = ConfigManager::with_path(config_path.clone()).unwrap();
    assert_eq!(
        manager.get().web.grpc_port,
        crate::config::DEFAULT_GRPC_DASHBOARD_PORT
    );

    let persisted = persistence::load_from_file(&config_path).unwrap();
    assert_eq!(
        persisted.web.grpc_port,
        crate::config::DEFAULT_GRPC_DASHBOARD_PORT
    );
}

#[test]
fn load_preserves_custom_grpc_dashboard_port() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");

    let mut config = crate::config::AppConfig::default_config();
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
    // Contract: config_dir() and data_dir() must resolve to a non-empty path under
    // the platform-standard home/app-support directory. The test name says "exists"
    // but the contract is path resolution success, not filesystem presence (the
    // directory is not required to exist on disk at this point — that is the
    // ConfigManager::with_path caller's responsibility). Pinning the path suffix
    // guards against platform-resolution regressions.
    let config_dir = ConfigManager::config_dir()
        .expect("config_dir() must successfully resolve the platform config path");
    assert!(
        config_dir.components().count() > 0,
        "resolved config_dir must not be an empty path, got: {config_dir:?}"
    );

    let data_dir = ConfigManager::data_dir()
        .expect("data_dir() must successfully resolve the platform data path");
    assert!(
        data_dir.components().count() > 0,
        "resolved data_dir must not be an empty path, got: {data_dir:?}"
    );
}

#[test]
fn app_flavor_suffixes_default_directories() {
    let _guard = env_lock();
    let temp_dir = TempDir::new().unwrap();
    let original_home = std::env::var_os("HOME");
    let original_appdata = std::env::var_os("APPDATA");
    let original_local_appdata = std::env::var_os("LOCALAPPDATA");
    let original_flavor = std::env::var_os("MAEKON_APP_FLAVOR");

    std::env::set_var("HOME", temp_dir.path());
    std::env::set_var("APPDATA", temp_dir.path());
    std::env::set_var("LOCALAPPDATA", temp_dir.path());
    std::env::set_var("MAEKON_APP_FLAVOR", "dev");

    let config_dir = ConfigManager::config_dir().unwrap();
    let data_dir = ConfigManager::data_dir().unwrap();

    restore_env_var("MAEKON_APP_FLAVOR", original_flavor);
    restore_env_var("LOCALAPPDATA", original_local_appdata);
    restore_env_var("APPDATA", original_appdata);
    restore_env_var("HOME", original_home);

    assert_eq!(
        config_dir.file_name().and_then(|name| name.to_str()),
        Some("maekon-dev"),
        "config dir should be separated by app flavor: {}",
        config_dir.display()
    );

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    assert!(
        data_dir
            .parent()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            == Some("maekon-dev"),
        "data dir should be separated by app flavor: {}",
        data_dir.display()
    );

    #[cfg(target_os = "linux")]
    assert_eq!(
        data_dir.file_name().and_then(|name| name.to_str()),
        Some("maekon-dev"),
        "data dir should be separated by app flavor: {}",
        data_dir.display()
    );

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    assert!(
        data_dir.ends_with(std::path::Path::new("maekon-dev").join("data")),
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
    match manager.reload().unwrap_err() {
        crate::error::CoreError::Config {
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

/// Finding #5 — a corrupt config must fall back FAIL-CLOSED for privacy: it must
/// NOT silently re-enable the fresh-install default-on telemetry/capture intent
/// over an unrecoverable user opt-out, and the reset must be surfaced to the UI.
#[test]
fn load_corrupt_file_falls_back_fail_closed_and_signals_reset() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");

    // Garbage on disk -> parse failure -> fail-closed recovery default.
    // `None` managed path keeps the platform managed policy from leaking in.
    fs::write(&config_path, "<<<NOT JSON>>>").unwrap();

    let manager = ConfigManager::with_paths(config_path.clone(), None).unwrap();
    let config = manager.get();

    // Telemetry/capture must be OFF, NOT the fresh-install default-on intent.
    assert!(
        !config.telemetry.enabled,
        "corrupt config must NOT yield telemetry-enabled (fail-closed)"
    );
    assert!(
        !config.monitor.upload_enabled,
        "corrupt config must NOT re-enable upload (fail-closed)"
    );
    assert!(
        !config.vision.capture_enabled,
        "corrupt config must NOT re-enable screen capture (fail-closed)"
    );
    assert!(
        !config.vision.ocr_enabled,
        "corrupt config must NOT re-enable OCR (fail-closed)"
    );

    // The reset must be a structured, UI-readable signal (not just a log line).
    assert!(
        manager.config_was_reset_from_corruption(),
        "corruption fallback must surface a reset signal the UI can read"
    );

    // The rewritten file must persist the fail-closed posture, so the next
    // launch (loading a now-valid file) also stays opted-out.
    let on_disk = persistence::load_from_file(&config_path).unwrap();
    assert!(
        !on_disk.telemetry.enabled,
        "fail-closed recovery default must be persisted to disk"
    );
    let relaunched = ConfigManager::with_paths(config_path, None).unwrap();
    assert!(
        !relaunched.get().telemetry.enabled,
        "relaunch from the rewritten file must remain opted-out"
    );
    assert!(
        !relaunched.config_was_reset_from_corruption(),
        "a clean relaunch (valid file) must not report a fresh corruption reset"
    );
}

// ── X1 ConfigChangeBus tests ───────────────────────────────────────
// Public decision record: docs/architecture/ADR-016-config-change-bus.md.

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
    let mut forced = crate::config::AppConfig::default_config();
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

    // tokio::sync::watch::error::RecvError is a unit struct with no payload.
    // lint:allow-is-err-hedge — RecvError has no typed payload beyond the unit struct
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

    // A complete legacy config payload that mimics a pre-Phase-2 config.json:
    // the `telemetry` section lacks `otlp_endpoint`, `sample_rate`, and
    // `service_name` but still has the top-level sections required by AppConfig.
    let mut legacy = serde_json::to_value(crate::config::AppConfig::default_config()).unwrap();
    legacy["schema_version"] = serde_json::json!(1);
    let telemetry = legacy
        .get_mut("telemetry")
        .and_then(|v| v.as_object_mut())
        .expect("default config must contain telemetry");
    telemetry.insert("enabled".to_string(), serde_json::json!(false));
    telemetry.remove("otlp_endpoint");
    telemetry.remove("sample_rate");
    telemetry.remove("service_name");
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let mgr =
        ConfigManager::with_paths(cfg_path.clone(), None).expect("legacy JSON must deserialise");
    let cfg = mgr.get();
    assert_eq!(cfg.telemetry.otlp_endpoint, None);
    assert!((cfg.telemetry.sample_rate - 1.0).abs() < f64::EPSILON);
    assert_eq!(cfg.telemetry.service_name, "maekon-client");
}

/// #5056 — fresh install uses the default-on telemetry intent.
///
/// Export is still fail-closed by the consent gate and compile-time feature
/// gate; this flag only means a valid telemetry consent can activate the
/// exporter unless the user or MDM policy opts out.
#[test]
fn telemetry_enabled_defaults_to_true() {
    let cfg = crate::config::AppConfig::default_config();
    assert!(
        cfg.telemetry.enabled,
        "fresh installs should default to telemetry intent on; consent still gates export"
    );
}

/// #5056 — an older sparse config with no telemetry.enabled key should pick up
/// the new default-on intent when loaded.
#[test]
fn legacy_config_without_telemetry_enabled_uses_default_on() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");

    let mut legacy = serde_json::to_value(crate::config::AppConfig::default_config()).unwrap();
    legacy["schema_version"] = serde_json::json!(1);
    legacy
        .get_mut("telemetry")
        .and_then(|v| v.as_object_mut())
        .expect("default config must contain telemetry")
        .remove("enabled");
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let mgr =
        ConfigManager::with_paths(cfg_path.clone(), None).expect("legacy JSON must deserialise");
    let cfg = mgr.get();
    assert!(
        cfg.telemetry.enabled,
        "missing telemetry.enabled should use the default-on intent"
    );
    assert_eq!(cfg.schema_version, crate::config::AppConfig::SCHEMA_VERSION);

    let on_disk = persistence::load_from_file(&cfg_path).expect("migrated config must persist");
    assert!(
        on_disk.telemetry.enabled,
        "migrated on-disk config should persist the default-on intent"
    );
    assert_eq!(
        on_disk.schema_version,
        crate::config::AppConfig::SCHEMA_VERSION,
        "v1 config should be rewritten at the current schema version"
    );
}

/// #5056 — a persisted explicit false remains an opt-out after the default-on
/// migration. This is intentionally conservative for older saved files because
/// the legacy schema cannot distinguish a user opt-out from the old seed value.
#[test]
fn legacy_config_with_explicit_telemetry_false_preserves_opt_out() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");

    let mut legacy = serde_json::to_value(crate::config::AppConfig::default_config()).unwrap();
    legacy["schema_version"] = serde_json::json!(1);
    legacy["telemetry"]["enabled"] = serde_json::json!(false);
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

    let mgr = ConfigManager::with_path(cfg_path.clone()).expect("legacy JSON must deserialise");
    let cfg = mgr.get();
    assert!(
        !cfg.telemetry.enabled,
        "persisted telemetry.enabled=false must remain an opt-out"
    );
    assert_eq!(cfg.schema_version, crate::config::AppConfig::SCHEMA_VERSION);

    let on_disk = persistence::load_from_file(&cfg_path).expect("migrated config must persist");
    assert!(
        !on_disk.telemetry.enabled,
        "migrated on-disk config must preserve explicit telemetry.enabled=false"
    );
    assert_eq!(
        on_disk.schema_version,
        crate::config::AppConfig::SCHEMA_VERSION,
        "v1 opt-out config should be rewritten at the current schema version"
    );
}

/// E20-4 (#4796) — semantic golden: reading a field via `snapshot()` (cheap
/// Arc<AppConfig> clone) yields the SAME value as reading it via `get()` (full
/// deep clone). This is the behavior-preserving contract the monitor-loop swap
/// (per-tick `get()` deep-clones → one `snapshot()` per tick) relies on: both
/// read the current `sender.borrow()` value, so the observed fields are identical.
///
/// A representative non-default config is written first (via the production
/// `update_with` chokepoint) so the assertion cannot pass trivially on defaults.
///
/// NOTE: this asserts SEMANTIC equality only. It deliberately does NOT assert the
/// allocation/perf win (one Arc bump vs N deep clones) — that cannot be measured
/// reliably in this headless CI env (no flamegraph / allocator counters), so the
/// perf claim is verified by code inspection, not this test.
#[test]
fn snapshot_field_reads_equal_get_field_reads() {
    use crate::config::PiiFilterLevel;

    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");
    let manager = ConfigManager::with_path(config_path).unwrap();

    // Drive a representative non-default config through the real write funnel so
    // the golden comparison is meaningful (not all-defaults). These are exactly
    // the sub-trees the monitor loop reads off the per-tick snapshot.
    manager
        .update_with(|c| {
            c.vision.capture_throttle_ms = 1234;
            c.privacy.pii_filter_level = PiiFilterLevel::Strict;
            c.analysis.text_intelligence.pii_extraction_level = PiiFilterLevel::Basic;
            c.analysis.server_coexistence_lookback_secs = 77;
            Ok(())
        })
        .unwrap();

    // `get()` = full deep clone (today's per-call path); `snapshot()` = cheap
    // Arc clone (the new per-tick path). Field reads must match exactly.
    let deep = manager.get();
    let snap = manager.snapshot();

    assert_eq!(
        snap.vision.capture_throttle_ms, deep.vision.capture_throttle_ms,
        "vision.capture_throttle_ms must read identically via snapshot() and get()"
    );
    assert_eq!(
        snap.privacy.pii_filter_level, deep.privacy.pii_filter_level,
        "privacy.pii_filter_level must read identically via snapshot() and get()"
    );
    assert_eq!(
        snap.analysis.text_intelligence.pii_extraction_level,
        deep.analysis.text_intelligence.pii_extraction_level,
        "analysis.text_intelligence.pii_extraction_level must read identically"
    );
    assert_eq!(
        snap.analysis.server_coexistence_lookback_secs,
        deep.analysis.server_coexistence_lookback_secs,
        "analysis.server_coexistence_lookback_secs must read identically"
    );

    // Sanity: the representative values actually took effect (guards against a
    // false-green where both paths happen to return defaults).
    assert_eq!(snap.vision.capture_throttle_ms, 1234);
    assert_eq!(snap.privacy.pii_filter_level, PiiFilterLevel::Strict);
    assert_eq!(snap.analysis.server_coexistence_lookback_secs, 77);
}

// ── Bounds validation at the write chokepoint (#6102-4) ───────────────────────

#[test]
fn update_with_rejects_out_of_range_poll_interval() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");
    let manager = ConfigManager::with_path(config_path).unwrap();

    let result = manager.update_with(|c| {
        c.monitor.poll_interval_ms = 100; // sub-second — would spin the 1s loops
        Ok(())
    });
    assert!(
        matches!(result, Err(CoreError::Config { .. })),
        "out-of-range poll interval must be rejected on the write path"
    );
    // The rejected write must not have mutated the in-memory snapshot.
    assert_eq!(manager.get().monitor.poll_interval_ms, 1_000);
}

#[test]
fn update_rejects_out_of_range_heartbeat_interval() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");
    let manager = ConfigManager::with_path(config_path.clone()).unwrap();

    let mut bad = manager.get();
    bad.monitor.heartbeat_interval_ms = 10; // far below the 5000ms floor
    let result = manager.update(bad);
    assert!(
        matches!(result, Err(CoreError::Config { .. })),
        "out-of-range heartbeat interval must be rejected by update()"
    );
    // Nothing should have been persisted: a fresh manager still sees the default.
    let manager2 = ConfigManager::with_path(config_path).unwrap();
    assert_eq!(manager2.get().monitor.heartbeat_interval_ms, 30_000);
}

#[test]
fn reload_rejects_out_of_range_external_edit() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");
    let manager = ConfigManager::with_path(config_path.clone()).unwrap();
    // Seed a valid persisted config first.
    manager.update_with(|_c| Ok(())).unwrap();

    // Simulate a hand-edited config.json with a sub-second sync interval.
    let mut raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config_path).unwrap()).unwrap();
    raw["monitor"]["sync_interval_ms"] = serde_json::json!(50);
    fs::write(&config_path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();

    let result = manager.reload();
    assert!(
        matches!(result, Err(CoreError::Config { .. })),
        "reload must reject an externally-edited out-of-range interval"
    );
}

// ── #6169/#6177: INITIAL-LOAD path is fail-open bounds-clamped, not rejecting ──
//
// The write chokepoints reject out-of-range values, but construction must NOT
// abort on a hand-edited / downgrade config.json — it must clamp the sub-floor
// value up to its floor and persist the correction, so the scheduler never
// receives a `poll_interval_ms: 0` (hot-spin) or `analysis.interval_secs: 0`
// (tokio interval panic).

#[test]
fn with_paths_clamps_sub_floor_poll_interval_instead_of_panicking() {
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");

    // Hand-edited config with a zero poll interval (sub-floor). Mirrors a
    // downgrade / manual edit that bypassed the write chokepoint.
    let mut raw = serde_json::to_value(AppConfig::default_config()).unwrap();
    raw["monitor"]["poll_interval_ms"] = serde_json::json!(0);
    raw["analysis"]["interval_secs"] = serde_json::json!(0);
    fs::write(&config_path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();

    // Construction must succeed (fail-open), not error or panic.
    let manager = ConfigManager::with_paths(config_path.clone(), None)
        .expect("construction must stay fail-open on a sub-floor config (clamp, not error)");

    // The in-memory snapshot observed by the scheduler must already be clamped.
    let snapshot = manager.snapshot();
    assert!(
        snapshot.monitor.poll_interval_ms >= 1_000,
        "poll_interval_ms must be clamped to its floor, got {}",
        snapshot.monitor.poll_interval_ms
    );
    assert!(
        snapshot.analysis.interval_secs >= 10,
        "analysis.interval_secs must be clamped to its floor, got {}",
        snapshot.analysis.interval_secs
    );
    // The clamped snapshot must itself be in-bounds.
    snapshot
        .validate_bounds()
        .expect("the clamped startup snapshot must satisfy validate_bounds");

    // The correction must be persisted so a relaunch stays safe even if the
    // file is reloaded directly.
    let on_disk = persistence::load_from_file(&config_path).unwrap();
    assert!(
        on_disk.monitor.poll_interval_ms >= 1_000,
        "bounds clamp must be written to disk, not just in-memory"
    );
    assert!(on_disk.analysis.interval_secs >= 10);

    // A relaunch from the rewritten file must also be in-bounds.
    let relaunched = ConfigManager::with_paths(config_path, None).unwrap();
    assert!(relaunched.snapshot().monitor.poll_interval_ms >= 1_000);
}

#[test]
fn with_paths_preserves_in_bounds_config_unchanged() {
    // Regression guard: the clamp must be a no-op for an in-bounds config — it
    // must not silently rewrite a user's valid custom intervals.
    let temp_dir = TempDir::new().unwrap();
    let config_path = temp_dir.path().join("config.json");

    let mut user = AppConfig::default_config();
    user.monitor.poll_interval_ms = 2_500; // custom but in-bounds
    user.analysis.interval_secs = 45;
    user.analysis.full_interval_secs = 600;
    fs::write(&config_path, serde_json::to_string_pretty(&user).unwrap()).unwrap();

    let manager = ConfigManager::with_paths(config_path, None).unwrap();
    let snapshot = manager.snapshot();
    assert_eq!(snapshot.monitor.poll_interval_ms, 2_500);
    assert_eq!(snapshot.analysis.interval_secs, 45);
    assert_eq!(snapshot.analysis.full_interval_secs, 600);
}

// ── Managed (MDM) policy enforcement at the write chokepoint (#4832) ──────────
//
// These tests drive the REAL `ConfigManager::with_paths` (no mock, no env), so
// they exercise the production funnel (`update` / `update_with` / construction).
// The single-chokepoint design means a per-callsite guard is not required:
// proving `update_with` is clamped proves every one of its ~20 callers is too.

mod managed_policy {
    use super::*;
    use crate::config::{
        ManagedConfig, ManagedPrivacy, ManagedTelemetry, ManagedUpdate, PiiFilterLevel,
    };

    /// Write a `managed.json` locking `privacy.pii_filter_level` and return its path.
    fn write_managed_pii(dir: &std::path::Path, level: PiiFilterLevel) -> PathBuf {
        let managed = ManagedConfig {
            privacy: ManagedPrivacy {
                pii_filter_level: Some(level),
            },
            ..Default::default()
        };
        let path = dir.join("managed.json");
        fs::write(&path, serde_json::to_string_pretty(&managed).unwrap()).unwrap();
        path
    }

    /// Write a `managed.json` locking `update.enabled` and return its path.
    /// Locking `false` is the fleet update kill-switch (#4836, Part A).
    fn write_managed_update_enabled(dir: &std::path::Path, enabled: bool) -> PathBuf {
        let managed = ManagedConfig {
            update: ManagedUpdate {
                enabled: Some(enabled),
                ..Default::default()
            },
            ..Default::default()
        };
        let path = dir.join("managed.json");
        fs::write(&path, serde_json::to_string_pretty(&managed).unwrap()).unwrap();
        path
    }

    /// Write a `managed.json` locking `telemetry.enabled` and return its path.
    fn write_managed_telemetry_enabled(dir: &std::path::Path, enabled: bool) -> PathBuf {
        let managed = ManagedConfig {
            telemetry: ManagedTelemetry {
                enabled: Some(enabled),
                ..Default::default()
            },
            ..Default::default()
        };
        let path = dir.join("managed.json");
        fs::write(&path, serde_json::to_string_pretty(&managed).unwrap()).unwrap();
        path
    }

    #[test]
    fn managed_telemetry_off_beats_fresh_default_on() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed_path = write_managed_telemetry_enabled(dir.path(), false);

        let mgr = ConfigManager::with_paths(cfg_path.clone(), Some(managed_path)).unwrap();

        assert!(
            !mgr.get().telemetry.enabled,
            "managed telemetry.enabled=false must override the fresh default-on intent"
        );
        let on_disk = persistence::load_from_file(&cfg_path).unwrap();
        assert!(
            !on_disk.telemetry.enabled,
            "managed telemetry lock must persist the clamped opt-out"
        );
    }

    #[test]
    fn update_clamps_locked_field_on_real_chokepoint() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed_path = write_managed_pii(dir.path(), PiiFilterLevel::Off);

        let mgr = ConfigManager::with_paths(cfg_path.clone(), Some(managed_path)).unwrap();

        // User tries to raise PII filtering above the locked value via update().
        let mut wanted = mgr.get();
        wanted.privacy.pii_filter_level = PiiFilterLevel::Strict;
        mgr.update(wanted).unwrap();

        // In-memory observes the clamp...
        assert_eq!(mgr.get().privacy.pii_filter_level, PiiFilterLevel::Off);
        // ...and so does the on-disk file (no fake-success).
        let on_disk = persistence::load_from_file(&cfg_path).unwrap();
        assert_eq!(on_disk.privacy.pii_filter_level, PiiFilterLevel::Off);
    }

    #[test]
    fn update_with_clamps_locked_field_sibling_proof() {
        // update_with is the path used by backup-restore, scheduler, Tauri
        // commands — clamping here proves the whole sibling family is covered.
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed_path = write_managed_pii(dir.path(), PiiFilterLevel::Off);

        let mgr = ConfigManager::with_paths(cfg_path.clone(), Some(managed_path)).unwrap();
        let result = mgr
            .update_with(|c| {
                c.privacy.pii_filter_level = PiiFilterLevel::Strict;
                Ok(())
            })
            .unwrap();

        assert_eq!(result.privacy.pii_filter_level, PiiFilterLevel::Off);
        assert_eq!(mgr.get().privacy.pii_filter_level, PiiFilterLevel::Off);
        let on_disk = persistence::load_from_file(&cfg_path).unwrap();
        assert_eq!(on_disk.privacy.pii_filter_level, PiiFilterLevel::Off);
    }

    #[test]
    fn construction_clamps_preexisting_user_config_no_stale_read() {
        // User config.json already sits at Strict; managed locks Off. The very
        // first get() (before any update) must already be Off, and disk rewritten.
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let mut user = AppConfig::default_config();
        user.privacy.pii_filter_level = PiiFilterLevel::Strict;
        fs::write(&cfg_path, serde_json::to_string_pretty(&user).unwrap()).unwrap();
        let managed_path = write_managed_pii(dir.path(), PiiFilterLevel::Off);

        let mgr = ConfigManager::with_paths(cfg_path.clone(), Some(managed_path)).unwrap();

        assert_eq!(mgr.get().privacy.pii_filter_level, PiiFilterLevel::Off);
        let on_disk = persistence::load_from_file(&cfg_path).unwrap();
        assert_eq!(on_disk.privacy.pii_filter_level, PiiFilterLevel::Off);
    }

    #[test]
    fn unlocked_field_passes_through_no_over_clamp() {
        // Managed locks only telemetry.enabled; pii must remain user-settable.
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed = ManagedConfig {
            telemetry: ManagedTelemetry {
                enabled: Some(false),
                ..Default::default()
            },
            ..Default::default()
        };
        let managed_path = dir.path().join("managed.json");
        fs::write(
            &managed_path,
            serde_json::to_string_pretty(&managed).unwrap(),
        )
        .unwrap();

        let mgr = ConfigManager::with_paths(cfg_path, Some(managed_path)).unwrap();
        mgr.update_with(|c| {
            c.privacy.pii_filter_level = PiiFilterLevel::Strict;
            c.telemetry.enabled = true;
            Ok(())
        })
        .unwrap();

        assert!(!mgr.get().telemetry.enabled, "locked field clamped");
        assert_eq!(
            mgr.get().privacy.pii_filter_level,
            PiiFilterLevel::Strict,
            "unlocked field must pass through"
        );
    }

    #[test]
    fn absent_managed_file_is_normal_operation() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let absent = dir.path().join("no-such-managed.json");

        let mgr = ConfigManager::with_paths(cfg_path, Some(absent)).unwrap();
        // No policy ⇒ user is free to set anything.
        mgr.update_with(|c| {
            c.privacy.pii_filter_level = PiiFilterLevel::Strict;
            Ok(())
        })
        .unwrap();
        assert_eq!(mgr.get().privacy.pii_filter_level, PiiFilterLevel::Strict);
    }

    #[test]
    fn malformed_managed_file_is_fail_closed() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed_path = dir.path().join("managed.json");

        // (a) invalid JSON, (b) bad enum token, (c) future schema_version.
        for bad in [
            "{ not json",
            r#"{ "privacy": { "pii_filter_level": "Strikt" } }"#,
            r#"{ "schema_version": 9999 }"#,
        ] {
            fs::write(&managed_path, bad).unwrap();
            let result = ConfigManager::with_paths(cfg_path.clone(), Some(managed_path.clone()));
            assert!(
                matches!(result.unwrap_err(), crate::error::CoreError::Config { .. }),
                "present-but-broken managed.json must fail-closed with CoreError::Config, not start unmanaged: {bad}"
            );
        }
    }

    #[test]
    fn detect_violations_powers_interactive_rejection() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed_path = write_managed_pii(dir.path(), PiiFilterLevel::Off);

        let mgr = ConfigManager::with_paths(cfg_path, Some(managed_path)).unwrap();
        let mut candidate = mgr.get();
        candidate.privacy.pii_filter_level = PiiFilterLevel::Strict;

        let violations = mgr.detect_managed_violations(&candidate);
        assert_eq!(violations, vec!["privacy.pii_filter_level".to_string()]);
        assert_eq!(
            mgr.managed_locked_fields(),
            vec!["privacy.pii_filter_level".to_string()]
        );
    }

    #[test]
    fn no_managed_policy_means_no_violations() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");

        let mgr = ConfigManager::with_paths(cfg_path, None).unwrap();
        let mut candidate = mgr.get();
        candidate.privacy.pii_filter_level = PiiFilterLevel::Off;

        assert!(mgr.detect_managed_violations(&candidate).is_empty());
        assert!(mgr.managed_locked_fields().is_empty());
    }

    // --- Update kill-switch (#4836 Part A) ---------------------------------
    // These exercise the remote update kill-switch end-to-end at the #4832
    // managed-config chokepoint. The runtime side (update_coordinator stays
    // Idle / update_runtime never spawns the loop) keys off the SAME
    // `config.update.enabled` bool these tests prove is forced to false, so
    // clamping it here is what disables updates fleet-wide. The tests stay in
    // maekon-core (no updater spawn) to remain headless-safe.

    #[test]
    fn killswitch_managed_lock_beats_user_config_true() {
        // The user's on-disk config.json explicitly opts INTO updates
        // (enabled = true, the product default), but the admin's managed.json
        // locks update.enabled = false. The effective config must be false:
        // the managed clamp wins. This is the kill-switch.
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");

        let mut user = AppConfig::default_config();
        user.update.enabled = true; // user wants updates on
        fs::write(&cfg_path, serde_json::to_string_pretty(&user).unwrap()).unwrap();

        let managed_path = write_managed_update_enabled(dir.path(), false);

        let mgr = ConfigManager::with_paths(cfg_path.clone(), Some(managed_path)).unwrap();

        // The very first read already observes the kill-switch (no stale-read
        // gap): the update runtime reads this exact bool at startup.
        assert!(
            !mgr.get().update.enabled,
            "managed kill-switch must force update.enabled = false at construction"
        );
        // And the clamp is persisted, so it survives a relaunch even if the
        // managed file later disappears.
        let on_disk = persistence::load_from_file(&cfg_path).unwrap();
        assert!(
            !on_disk.update.enabled,
            "kill-switch clamp must be written to disk, not just in-memory"
        );
    }

    #[test]
    fn killswitch_user_override_to_true_is_rejected() {
        // After the kill-switch is active, a user/web attempt to re-enable
        // updates (update.enabled = true) is reported as a violation by the
        // interactive guard AND silently clamped back at the write chokepoint.
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");
        let managed_path = write_managed_update_enabled(dir.path(), false);

        let mgr = ConfigManager::with_paths(cfg_path.clone(), Some(managed_path)).unwrap();

        // (a) The interactive layer rejects the override before the write.
        let mut candidate = mgr.get();
        candidate.update.enabled = true;
        let violations = mgr.detect_managed_violations(&candidate);
        assert_eq!(
            violations,
            vec!["update.enabled".to_string()],
            "re-enabling updates against the kill-switch must be reported as a violation"
        );
        assert_eq!(
            mgr.managed_locked_fields(),
            vec!["update.enabled".to_string()],
            "update.enabled must be advertised as a locked field"
        );

        // (b) Even if the override reaches the write chokepoint (settings API,
        // Tauri IPC, backup restore, scheduler — all funnel through update_with),
        // it is clamped back. Proving update_with is enough proves every sibling.
        mgr.update_with(|c| {
            c.update.enabled = true;
            Ok(())
        })
        .unwrap();
        assert!(
            !mgr.get().update.enabled,
            "write chokepoint must re-clamp a user attempt to re-enable updates"
        );
        let on_disk = persistence::load_from_file(&cfg_path).unwrap();
        assert!(
            !on_disk.update.enabled,
            "clamped override must not leak to disk (no fake-success)"
        );
    }

    #[test]
    fn killswitch_absent_policy_leaves_updates_user_controlled() {
        // Without a managed policy, update.enabled is purely user-controlled —
        // the kill-switch is opt-in for managed fleets only (consumer default
        // is unaffected).
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("config.json");

        let mgr = ConfigManager::with_paths(cfg_path, None).unwrap();
        assert!(
            mgr.get().update.enabled,
            "consumer default: updates remain enabled when unmanaged"
        );
        mgr.update_with(|c| {
            c.update.enabled = false;
            Ok(())
        })
        .unwrap();
        assert!(
            !mgr.get().update.enabled,
            "user is free to toggle updates when no policy is present"
        );
        assert!(mgr.detect_managed_violations(&mgr.get()).is_empty());
    }

    /// #6175: on Unix the persisted config.json must be owner-only (0o600) after a
    /// save, because it can hold inline plaintext API keys / secrets. A default-umask
    /// write would leave it world/group-readable, leaking secrets to other local
    /// users. Asserts both the initial save (via save_to_file directly) and a save
    /// driven through the public `update_with` path.
    #[cfg(unix)]
    #[test]
    fn config_file_is_owner_only_on_unix() {
        use std::os::unix::fs::PermissionsExt as _;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.json");

        // Direct save_to_file path.
        persistence::save_to_file(&config_path, &crate::config::AppConfig::default_config())
            .unwrap();
        let mode = fs::metadata(&config_path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "config.json must be 0o600 after save_to_file, got {:o}",
            mode & 0o777
        );

        // No orphaned tmp must be left behind (atomic write committed).
        assert!(
            !config_path.with_extension("json.tmp").exists(),
            "the .json.tmp must be renamed into place, not left behind"
        );

        // Public update path (ConfigManager::update_with -> save_to_file) must also
        // land owner-only.
        let manager = ConfigManager::with_path(config_path.clone()).unwrap();
        manager
            .update_with(|c| {
                c.web.port = 8123;
                Ok(())
            })
            .unwrap();
        let mode = fs::metadata(&config_path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "config.json must stay 0o600 after update_with, got {:o}",
            mode & 0o777
        );
    }
}
