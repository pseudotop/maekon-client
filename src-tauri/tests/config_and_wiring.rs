#![cfg(feature = "server")]
#![allow(deprecated)]

use maekon_core::config::AppConfig;
use maekon_monitor::system::SysInfoMonitor;
use maekon_network::auth::TokenManager;
use maekon_network::compression::AdaptiveCompressor;
use maekon_storage::sqlite::SqliteStorage;
use maekon_suggestion::queue::SuggestionQueue;
use maekon_vision::trigger::SmartCaptureTrigger;
use std::sync::Arc;

#[test]
fn config_defaults_are_valid() {
    let config = AppConfig::default_config();

    assert!(!config.server.base_url.is_empty());
    assert!(config.server.request_timeout_ms > 0);
    assert!(config.server.sse_max_retry_secs > 0);

    assert!(config.monitor.poll_interval_ms > 0);
    assert!(config.monitor.sync_interval_ms > config.monitor.poll_interval_ms);
    assert!(config.monitor.heartbeat_interval_ms > config.monitor.sync_interval_ms);

    assert!(config.storage.retention_days > 0);
    assert!(config.storage.max_storage_mb > 0);

    assert!(config.vision.capture_throttle_ms > 0);
    assert!(config.vision.thumbnail_width > 0);
    assert!(config.vision.thumbnail_height > 0);
}

#[test]
fn config_duration_conversions() {
    let config = AppConfig::default_config();

    let timeout = config.request_timeout();
    assert!(timeout.as_millis() > 0);

    let poll = config.poll_interval();
    assert_eq!(poll.as_millis(), config.monitor.poll_interval_ms as u128);

    let sync = config.sync_interval();
    assert_eq!(sync.as_millis(), config.monitor.sync_interval_ms as u128);
}

#[test]
fn all_adapters_instantiate_from_config() {
    let config = AppConfig::default_config();

    let _token_manager = Arc::new(TokenManager::new(&config.server.base_url));

    let _sys_monitor = SysInfoMonitor::new();

    let _trigger = SmartCaptureTrigger::new(config.vision.capture_throttle_ms);

    let _storage = SqliteStorage::open_in_memory(config.storage.retention_days).unwrap();

    let _compressor = AdaptiveCompressor::new();

    let _queue = SuggestionQueue::new(50);
}

#[test]
fn config_serde_roundtrip() {
    let config = AppConfig::default_config();

    let json = serde_json::to_string(&config).unwrap();
    let deserialized: AppConfig = serde_json::from_str(&json).unwrap();

    assert_eq!(config.server.base_url, deserialized.server.base_url);
    assert_eq!(
        config.monitor.poll_interval_ms,
        deserialized.monitor.poll_interval_ms
    );
    assert_eq!(
        config.storage.retention_days,
        deserialized.storage.retention_days
    );
    assert_eq!(
        config.vision.thumbnail_width,
        deserialized.vision.thumbnail_width
    );
}

#[tokio::test]
async fn storage_adapter_implements_port() {
    use maekon_core::ports::storage::StorageService;

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let storage: Arc<dyn StorageService> = Arc::new(storage);

    let result = storage.enforce_retention().await;
    // enforce_retention returns Ok(usize) — the count of rows deleted.
    // On a freshly created in-memory DB with no events, zero rows are deleted.
    let deleted = result.expect("enforce_retention on an empty DB must succeed");
    assert_eq!(
        deleted, 0,
        "no events to purge in a freshly opened in-memory DB"
    );
}
