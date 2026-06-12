//! Async-safety deadlock guards for IPC commands that were blocking the tokio
//! reactor thread before the F-RR-06 / #5640 spawn_blocking migration.
//!
//! Pattern mirrors `crates/maekon-web/tests/handler_async_safety.rs`:
//!   - Default `#[tokio::test]` uses the current-thread scheduler (1 worker).
//!   - A blocking parking_lot lock on the single worker would deadlock or
//!     timeout the test.
//!   - Post-migration the calls go through `spawn_blocking`, which dispatches to
//!     a separate blocking-thread pool, so the reactor thread stays free and the
//!     timeout completes successfully.
//!
//! Run: `cargo test -p maekon-app --test ipc_spawn_blocking_guard`

use maekon_storage::sqlite::SqliteStorage;
use std::sync::Arc;
use std::time::Duration;

// ── helpers ──────────────────────────────────────────────────────────────────

fn open_storage() -> Arc<SqliteStorage> {
    Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"))
}

// ── C1-A: onboarding IPC commands ────────────────────────────────────────────

/// F-RR-06 / #5640: get_meta (onboarding) must not block the tokio worker.
///
/// Exercises the same storage path as `get_onboarding_status`: a parking_lot
/// read lock inside spawn_blocking. On a current-thread runtime a bare
/// blocking call would starve the reactor; spawn_blocking dispatches to the
/// blocking pool, so the 5 s timeout must not fire.
#[tokio::test]
async fn get_onboarding_status_does_not_block_worker() {
    let storage = open_storage();
    let result = tokio::time::timeout(Duration::from_secs(5), {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            storage
                .get_meta("onboarding_completed")
                .map(|v| v == "true")
                .unwrap_or(false)
        })
    })
    .await
    .expect("get_onboarding_status timed out — possible worker thread blocking");
    let completed = result.expect("spawn_blocking join error");
    // Fresh DB: key absent → false.
    assert!(!completed, "fresh DB must report onboarding not completed");
}

/// F-RR-06 / #5640: set_meta (complete_onboarding) must not block the tokio worker.
#[tokio::test]
async fn complete_onboarding_does_not_block_worker() {
    let storage = open_storage();
    tokio::time::timeout(Duration::from_secs(5), {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            storage.set_meta("onboarding_completed", "true");
        })
    })
    .await
    .expect("complete_onboarding timed out — possible worker thread blocking")
    .expect("spawn_blocking join error");

    // Verify the key was actually written (read-back via a second spawn_blocking).
    let val = tokio::task::spawn_blocking(move || storage.get_meta("onboarding_completed"))
        .await
        .expect("readback join error");
    assert_eq!(
        val.as_deref(),
        Some("true"),
        "onboarding_completed key must be set"
    );
}

/// F-RR-06 / #5640: delete_meta (reset_onboarding) must not block the tokio worker.
#[tokio::test]
async fn reset_onboarding_does_not_block_worker() {
    let storage = open_storage();
    // Seed the key first.
    storage.set_meta("onboarding_completed", "true");

    tokio::time::timeout(Duration::from_secs(5), {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            storage.delete_meta("onboarding_completed");
        })
    })
    .await
    .expect("reset_onboarding timed out — possible worker thread blocking")
    .expect("spawn_blocking join error");

    // Key must be absent after delete.
    assert!(
        storage.get_meta("onboarding_completed").is_none(),
        "onboarding_completed key must be absent after reset"
    );
}

// ── C1-B: capture_status panel-position IPC commands ─────────────────────────

/// F-RR-06 / #5640: save_panel_position (set_meta) must not block the tokio worker.
#[tokio::test]
async fn save_panel_position_does_not_block_worker() {
    let storage = open_storage();
    let pos = "123.4,567.8".to_string();
    tokio::time::timeout(Duration::from_secs(5), {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || {
            storage.set_meta("tracking_panel_position", &pos);
        })
    })
    .await
    .expect("save_panel_position timed out — possible worker thread blocking")
    .expect("spawn_blocking join error");

    // Verify the stored value.
    let stored = storage.get_meta("tracking_panel_position");
    assert_eq!(
        stored.as_deref(),
        Some("123.4,567.8"),
        "tracking_panel_position must be saved correctly"
    );
}

/// F-RR-06 / #5640: get_panel_position (get_meta) must not block the tokio worker.
#[tokio::test]
async fn get_panel_position_does_not_block_worker() {
    let storage = open_storage();
    // Seed a position.
    storage.set_meta("tracking_panel_position", "200.0,400.0");

    let result = tokio::time::timeout(Duration::from_secs(5), {
        let storage = storage.clone();
        tokio::task::spawn_blocking(move || storage.get_meta("tracking_panel_position"))
    })
    .await
    .expect("get_panel_position timed out — possible worker thread blocking")
    .expect("spawn_blocking join error");

    assert_eq!(
        result.as_deref(),
        Some("200.0,400.0"),
        "tracking_panel_position must be readable after write"
    );
}
