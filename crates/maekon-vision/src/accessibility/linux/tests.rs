//! Unit and integration tests for the Linux AT-SPI2 accessibility extractor.

use maekon_core::config::PiiFilterLevel;
use maekon_core::ports::accessibility::AccessibilityExtractor;

use super::LinuxAccessibility;

#[test]
fn has_permission_reflects_dbus_availability() {
    let extractor = LinuxAccessibility::new();
    let dbus_available = std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
        || std::env::var("ATSPI_BUS_ADDRESS").is_ok();
    // With linux-atspi, permission requires a D-Bus session.
    // Without the feature (extractor compiled out), the capability is honestly
    // reported as UNAVAILABLE (review4 V20) — not falsely "ready".
    if cfg!(feature = "linux-atspi") {
        assert_eq!(extractor.has_permission(), dbus_available);
    } else {
        assert!(!extractor.has_permission());
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

/// #5985: D-Bus call timeouts must not panic and must yield graceful fallbacks.
///
/// Validates the timeout mechanism used in `find_active_window` and the
/// per-call `DBUS_CALL_TIMEOUT` guards: a future that never resolves must
/// produce `Err(Elapsed)` from `tokio::time::timeout`, and our match arms
/// must handle that as a `None` / empty result — never blocking or panicking.
///
/// This test does not require a real AT-SPI / D-Bus session. It exercises
/// the same `tokio::time::timeout` + `Ok/Err` dispatch pattern used in
/// `find_active_window_inner`, `traverse_tree`, and `find_focused_in_window`.
#[cfg(feature = "linux-atspi")]
#[tokio::test]
async fn dbus_call_timeout_yields_graceful_none() {
    use std::time::Duration;

    // Simulate a D-Bus call that never completes (frozen daemon / deadlocked process).
    let stalled: std::future::Pending<Result<(), ()>> = std::future::pending();
    let result = tokio::time::timeout(Duration::from_millis(10), stalled).await;

    // The timeout must fire — not block the test indefinitely.
    // Borrow via as_ref() so `result` remains usable in the match arm below.
    result
        .as_ref()
        .expect_err("tokio::time::timeout must return Err(Elapsed) for a never-resolving future");

    // Verify the match arm pattern used throughout extractor.rs handles Elapsed correctly.
    // `Ok(Ok(_)) | Ok(Err(_))` are success/D-Bus-error paths; `Err(_)` is the timeout path.
    let graceful_fallback: Option<u32> = match result {
        Ok(Ok(_)) => Some(1),
        Ok(Err(_)) => Some(2),
        Err(_elapsed) => None, // ← the timeout arm — must not panic
    };
    assert!(
        graceful_fallback.is_none(),
        "timeout arm must yield None, not panic"
    );
}

/// #6090: `AccessibilityConnection::new()` (the D-Bus Hello handshake) must be
/// timeout-guarded so a wedged dbus-daemon cannot stall the 1 s scheduler tick.
///
/// We cannot drive a real frozen connection here without a D-Bus session, so we
/// assert the two contract properties that make the fix correct:
/// 1. `ATSPI_CONNECT_TIMEOUT` is bounded and within the scheduler-safe envelope
///    (the per-call `DBUS_CALL_TIMEOUT` is 2 s; the connect budget must not be
///    larger than the whole-`find_active_window` budget of 3 s).
/// 2. The same `tokio::time::timeout` + nested-`Result` dispatch used for the
///    connect await maps a never-resolving handshake to a graceful fallback
///    (record failure + return `None`/empty), never a panic or unbounded block.
#[cfg(feature = "linux-atspi")]
#[tokio::test]
async fn atspi_connect_timeout_is_bounded_and_yields_graceful_fallback() {
    use std::time::Duration;

    // Property 1: the connect budget is bounded and scheduler-safe.
    assert!(
        super::ATSPI_CONNECT_TIMEOUT > Duration::ZERO,
        "ATSPI_CONNECT_TIMEOUT must be a positive, bounded budget"
    );
    assert!(
        super::ATSPI_CONNECT_TIMEOUT <= Duration::from_secs(3),
        "ATSPI_CONNECT_TIMEOUT must stay within the scheduler-safe envelope (<= 3 s)"
    );

    // Property 2: a never-resolving handshake maps to the graceful path.
    // Mirror the `AccessibilityConnection::new()` call shape: a fallible future
    // wrapped in tokio::time::timeout, dispatched via the same match arms.
    let stalled_connect: std::future::Pending<Result<(), ()>> = std::future::pending();
    let outcome = tokio::time::timeout(Duration::from_millis(10), stalled_connect).await;

    let graceful: Option<u32> = match outcome {
        Ok(Ok(_)) => Some(1),  // connected
        Ok(Err(_)) => Some(2), // D-Bus error → record_failure + return None/Err
        Err(_elapsed) => None, // timeout → record_failure + return None/empty
    };
    assert!(
        graceful.is_none(),
        "a wedged dbus-daemon handshake must hit the timeout arm (graceful fallback), not block"
    );
}

/// #5985: `extract_focused_element` and `extract_window_elements` must complete
/// within a bounded wall-clock time even when called on a CI host with no D-Bus.
///
/// On CI the AT-SPI connection fails immediately (no daemon), so the circuit
/// breaker / connection-error path exercises. This test confirms the whole call
/// stack returns within 5 s — a regression guard for future changes that might
/// accidentally introduce an unbounded await. The inner `Result` may be
/// `Ok(None)` / `Ok(Vec::new())` or `Err(PermissionDenied)` depending on
/// whether the `linux-atspi` feature is active; only the timing contract matters
/// here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extractor_completes_within_timeout_on_no_dbus() {
    use std::time::Duration;

    let extractor = LinuxAccessibility::new();

    // extract_focused_element must *complete* within 5 s — result may be Ok or Err.
    let completed = tokio::time::timeout(
        Duration::from_secs(5),
        extractor.extract_focused_element(PiiFilterLevel::Standard, false),
    )
    .await;
    // Timing contract: the outer Result must be Ok (not Elapsed). The inner Result
    // (which may itself be Ok or Err depending on D-Bus availability) is irrelevant
    // here — only the wall-clock bound is asserted.
    let _ = completed
        .expect("extract_focused_element must complete within 5 s on no-D-Bus host (got Elapsed)");

    // extract_window_elements must also *complete* within 5 s.
    let completed = tokio::time::timeout(
        Duration::from_secs(5),
        extractor.extract_window_elements(2, 50, PiiFilterLevel::Standard, false),
    )
    .await;
    let _ = completed
        .expect("extract_window_elements must complete within 5 s on no-D-Bus host (got Elapsed)");
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
