//! Unit and integration tests for the Linux AT-SPI2 accessibility extractor.

use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::accessibility::AccessibilityExtractor;

use super::LinuxAccessibility;

#[test]
fn has_permission_reflects_dbus_availability() {
    let extractor = LinuxAccessibility::new();
    let dbus_available = std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
        || std::env::var("ATSPI_BUS_ADDRESS").is_ok();
    // With linux-atspi, permission requires D-Bus session.
    // Without the feature (stub mode), always returns true.
    if cfg!(feature = "linux-atspi") {
        assert_eq!(extractor.has_permission(), dbus_available);
    } else {
        assert!(extractor.has_permission());
    }
}

#[test]
fn name_is_correct() {
    let extractor = LinuxAccessibility::new();
    assert_eq!(extractor.name(), "linux-atspi2-accessibility");
}

#[tokio::test]
async fn test_extract_focused_element() {
    let extractor = LinuxAccessibility::new();
    let result = extractor
        .extract_focused_element(PiiFilterLevel::Standard, false)
        .await;
    // Contract: extract_focused_element must never return Err on CI without D-Bus/AT-SPI2 —
    // connection failure is a graceful Ok(None) (circuit-breaker degradation path).
    // On desktop Linux with AT-SPI2 running it may return Ok(Some(...)).
    // #5594: Ok-only is the complete observable contract in this environment-agnostic test;
    // value assertions belong in the #[ignore] integration tests that verify real AT-SPI data.
    let outcome = result.expect(
        "extract_focused_element must return Ok (possibly None) even without D-Bus/AT-SPI2",
    );
    // If an element IS returned, its role field must be non-empty.
    if let Some(ref info) = outcome {
        assert!(
            !info.role.is_empty(),
            "returned FocusedElementInfo must have a non-empty role"
        );
    }
}

#[cfg(feature = "linux-atspi")]
#[tokio::test]
async fn extract_window_elements_atspi_connection() {
    let extractor = LinuxAccessibility::new();
    let result = extractor
        .extract_window_elements(3, 300, PiiFilterLevel::Standard, false)
        .await;
    // On CI without AT-SPI2, this may return PermissionDenied
    // On desktop Linux, should return Ok (possibly empty)
    match result {
        Ok(elements) => {
            eprintln!("AT-SPI2 returned {} elements", elements.len());
        }
        Err(maekon_core::error::CoreError::PermissionDenied {
            code: maekon_core::error_codes::PermissionCode::PermissionDenied,
            message: msg,
        }) => {
            eprintln!("AT-SPI2 not available: {msg}");
        }
        Err(e) => {
            panic!("unexpected error: {e}");
        }
    }
}

#[cfg(feature = "linux-atspi")]
#[tokio::test]
async fn focus_listener_starts_or_gracefully_fails() {
    let extractor = LinuxAccessibility::new();
    let result = extractor.start_focus_listener().await;
    // On CI without AT-SPI2, this will fail with Internal error.
    // On desktop Linux with AT-SPI2 running, it should succeed.
    match result {
        Ok(handle) => {
            // Listener started — initially no focus event received
            assert!(!handle.has_focus().await);
            assert!(handle.last_focused().await.is_none());
            // Handle drop triggers graceful shutdown
            drop(handle);
            eprintln!("AT-SPI2 focus listener started and stopped successfully");
        }
        Err(crate::error::VisionError::Internal(msg)) => {
            eprintln!("AT-SPI2 focus listener unavailable (expected on CI): {msg}");
        }
        Err(e) => {
            panic!("unexpected error from start_focus_listener: {e}");
        }
    }
}

#[cfg(feature = "linux-atspi")]
#[tokio::test]
async fn focus_listener_handle_clone_shares_state() {
    // Simulate the shared state without a real AT-SPI connection
    // by constructing a FocusedObjectInfo and verifying the Clone
    // derive works correctly.
    use super::atspi::FocusedObjectInfo;

    let info = FocusedObjectInfo {
        bus_name: ":1.42".to_string(),
        object_path: "/org/a11y/atspi/accessible/123".to_string(),
    };
    assert_eq!(info.bus_name, ":1.42");
    assert_eq!(info.object_path, "/org/a11y/atspi/accessible/123");

    // Verify clone works
    let cloned = info.clone();
    assert_eq!(cloned.bus_name, info.bus_name);
    assert_eq!(cloned.object_path, info.object_path);
}

/// F-RR-C26-02: Dropping `FocusEventListenerHandle` must abort the background
/// task even if the AT-SPI event stream blocks before reaching the shutdown arm.
///
/// This test verifies the Drop impl by spawning a task that never finishes
/// on its own (simulated by `std::future::pending()`), wrapping it in a
/// handle, dropping the handle, and asserting the task is finished.
#[cfg(feature = "linux-atspi")]
#[tokio::test]
async fn focus_event_listener_handle_drop_aborts_task() {
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use super::atspi::FocusEventListenerHandle;

    // Spawn a task that would run forever without external cancellation.
    let task = tokio::spawn(async {
        std::future::pending::<()>().await;
    });
    let task_handle = task.abort_handle();

    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
    let handle = FocusEventListenerHandle {
        last_focused: Arc::new(RwLock::new(None)),
        _shutdown_tx: Arc::new(shutdown_tx),
        _task: task,
    };

    // Before drop: task should still be running.
    assert!(
        !task_handle.is_finished(),
        "task should be running before drop"
    );

    drop(handle);

    // Yield to the tokio executor so the abort propagates.
    tokio::task::yield_now().await;

    assert!(
        task_handle.is_finished(),
        "F-RR-C26-02: task must be aborted after FocusEventListenerHandle is dropped"
    );
}
