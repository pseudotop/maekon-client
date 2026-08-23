//! Debug-only screen-capture revocation evidence watch.
//!
//! This module keeps the OS-permission transition probe out of the general CLI
//! runner and limits its consent-axis exception to an isolated release-evidence
//! command. It never persists captured frames.

use maekon_core::capture_gate::OsPermissionWatch;

use super::output::emit_debug_permissions_cli_json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureProbeOutcome {
    Suppressed,
    Attempted { success: bool },
}

fn capture_if_granted(granted: bool, capture: impl FnOnce() -> bool) -> CaptureProbeOutcome {
    if !granted {
        return CaptureProbeOutcome::Suppressed;
    }
    CaptureProbeOutcome::Attempted { success: capture() }
}

// This timing- and OS-coupled orchestration is verified by the disposable-VM
// revocation probe. Keep `capture_if_granted` mutation-tested as the pure
// fail-closed boundary instead of treating synthetic counter mutations as unit
// coverage for TCC, Tauri events, and real screen capture side effects.
#[mutants::skip]
pub(super) fn run_debug_screen_capture_watch<R: tauri::Runtime>(
    app_handle: &tauri::AppHandle<R>,
    samples: u32,
    interval_ms: u64,
    warmup_ms: u64,
) -> i32 {
    let started = serde_json::json!({
        "debug_permissions": true,
        "command": "screen-capture-watch",
        "phase": "started",
        "samples": samples,
        "interval_ms": interval_ms,
        "warmup_ms": warmup_ms,
        "privacy_status": "safe",
    });
    let started = serde_json::to_string(&started).unwrap_or_else(|_| {
        "{\"debug_permissions\":true,\"command\":\"screen-capture-watch\",\"phase\":\"started\"}".to_string()
    });
    if emit_debug_permissions_cli_json(&started) != 0 {
        return 1;
    }

    std::thread::sleep(std::time::Duration::from_millis(warmup_ms));

    let mut watch = OsPermissionWatch::default();
    let mut initial_granted = None;
    let mut previous_granted = None;
    let mut granted_observations = 0_u32;
    let mut revoked_observations = 0_u32;
    let mut revoked_edges = 0_u32;
    let mut restored_edges = 0_u32;
    let mut capture_attempts_before_revocation = 0_u32;
    let mut capture_successes_before_revocation = 0_u32;
    let mut capture_attempts_after_restore = 0_u32;
    let mut capture_successes_after_restore = 0_u32;
    let mut capture_attempts_total = 0_u32;
    let mut capture_attempts_while_revoked = 0_u32;
    let mut capture_suppressions_while_revoked = 0_u32;

    for sample in 0..samples {
        let granted = tauri::async_runtime::block_on(
            crate::scheduler::loops::os_permission_helper::observe_os_capture_permission(
                &mut watch,
                Some(app_handle),
                None,
            ),
        );
        initial_granted.get_or_insert(granted);

        if previous_granted == Some(true) && !granted {
            revoked_edges += 1;
        } else if previous_granted == Some(false) && granted {
            restored_edges += 1;
        }
        previous_granted = Some(granted);

        if granted {
            granted_observations += 1;
        } else {
            revoked_observations += 1;
        }

        match capture_if_granted(granted, || {
            maekon_vision::capture::ScreenCapture::new()
                .capture_primary()
                .is_ok()
        }) {
            CaptureProbeOutcome::Suppressed => {
                capture_suppressions_while_revoked += 1;
            }
            CaptureProbeOutcome::Attempted { success } => {
                capture_attempts_total += 1;
                if !granted {
                    capture_attempts_while_revoked += 1;
                } else {
                    let after_restore = revoked_edges > 0;
                    if after_restore {
                        capture_attempts_after_restore += 1;
                        capture_successes_after_restore += u32::from(success);
                    } else {
                        capture_attempts_before_revocation += 1;
                        capture_successes_before_revocation += u32::from(success);
                    }
                }
            }
        }

        if sample + 1 < samples {
            std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        }
    }

    let ok = initial_granted == Some(true)
        && capture_successes_before_revocation > 0
        && revoked_edges > 0
        && revoked_observations > 0
        && capture_attempts_while_revoked == 0
        && capture_suppressions_while_revoked == revoked_observations;
    let payload = serde_json::json!({
        "debug_permissions": true,
        "command": "screen-capture-watch",
        "phase": "completed",
        "ok": ok,
        "samples": samples,
        "interval_ms": interval_ms,
        "warmup_ms": warmup_ms,
        "initial_granted": initial_granted,
        "granted_observations": granted_observations,
        "revoked_observations": revoked_observations,
        "revoked_edges": revoked_edges,
        "restored_edges": restored_edges,
        "capture_attempts_before_revocation": capture_attempts_before_revocation,
        "capture_successes_before_revocation": capture_successes_before_revocation,
        "capture_attempts_total": capture_attempts_total,
        "capture_attempts_while_revoked": capture_attempts_while_revoked,
        "capture_suppressions_while_revoked": capture_suppressions_while_revoked,
        "capture_attempts_after_restore": capture_attempts_after_restore,
        "capture_successes_after_restore": capture_successes_after_restore,
        "event_surface": "capture-os-permission",
        "raw_frame_persisted": false,
        "privacy_status": "safe",
    });
    let payload = serde_json::to_string(&payload).unwrap_or_else(|_| {
        "{\"debug_permissions\":true,\"command\":\"screen-capture-watch\",\"phase\":\"completed\",\"ok\":false,\"error\":\"json serialization failed\"}".to_string()
    });
    let emit_exit = emit_debug_permissions_cli_json(&payload);
    if emit_exit != 0 || !ok {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{capture_if_granted, CaptureProbeOutcome};

    #[test]
    fn revoked_permission_suppresses_the_capture_call() {
        let called = Cell::new(false);

        let outcome = capture_if_granted(false, || {
            called.set(true);
            true
        });

        assert_eq!(outcome, CaptureProbeOutcome::Suppressed);
        assert!(!called.get());
    }

    #[test]
    fn granted_permission_runs_the_positive_control() {
        let called = Cell::new(false);

        let outcome = capture_if_granted(true, || {
            called.set(true);
            true
        });

        assert_eq!(outcome, CaptureProbeOutcome::Attempted { success: true });
        assert!(called.get());
    }
}
