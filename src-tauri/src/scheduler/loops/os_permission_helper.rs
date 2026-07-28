//! OS screen-capture permission axis for the monitor loop (#8686 AC4).
//!
//! macOS TCC lets the user revoke Screen Recording while the app is running;
//! before this helper the capture path just logged xcap errors and silently
//! retried forever. The monitor loop now probes the OS permission each tick
//! (only while the config/consent capture gate is open), stops capture
//! fail-closed on revocation, and surfaces a one-shot recovery path:
//! a `capture-os-permission` Tauri event (frontend banner with a System
//! Settings deep link) plus a desktop notification.
//!
//! Pure edge detection lives in `maekon_core::capture_gate::OsPermissionWatch`;
//! this helper owns only the adapter concerns (probe, event emit, notify).

use maekon_core::capture_gate::{OsPermissionEdge, OsPermissionWatch, TsNotifier};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tracing::{info, warn};

/// Payload of the `capture-os-permission` Tauri event.
#[derive(Debug, Clone, Serialize)]
pub(super) struct OsPermissionEventPayload {
    /// `"revoked"` or `"restored"`.
    pub state: &'static str,
    /// Machine-readable reason mirroring the desktop-permission snapshot
    /// vocabulary (currently always the macOS screen-capture reason — the
    /// probe only blocks on macOS).
    pub status_reason: &'static str,
}

/// Name of the Tauri event carrying OS capture-permission transitions.
pub(super) const OS_PERMISSION_EVENT: &str = "capture-os-permission";

/// Probes the OS screen-capture permission, feeds the edge watch, and
/// surfaces transitions (event + desktop notification). Returns whether the
/// OS axis currently permits capture.
///
/// Callers must invoke this only while the config/consent capture gate is
/// open — a closed gate already stops capture, and probing then would emit
/// notification noise for a state the user cannot observe.
pub(super) async fn observe_os_capture_permission<R: Runtime>(
    watch: &mut OsPermissionWatch,
    app_handle: Option<&AppHandle<R>>,
    notifier: Option<&dyn TsNotifier>,
) -> bool {
    let ok = crate::desktop_permissions::screen_capture_os_permission_ok();
    match watch.observe(ok) {
        Some(OsPermissionEdge::Revoked) => {
            warn!(
                "monitor loop: OS screen-capture permission revoked mid-session - capture stopped fail-closed (#8686 AC4)"
            );
            emit_transition(app_handle, "revoked", "macos_screen_capture_missing");
            if let Some(n) = notifier {
                n.notify_ts(
                    "Screen capture stopped",
                    "Screen Recording permission was revoked. Maekon stopped capturing. Re-enable it in System Settings \u{2192} Privacy & Security \u{2192} Screen Recording to resume.",
                )
                .await;
            }
        }
        Some(OsPermissionEdge::Restored) => {
            info!("monitor loop: OS screen-capture permission restored - capture resuming");
            emit_transition(app_handle, "restored", "macos_screen_capture_granted");
            if let Some(n) = notifier {
                n.notify_ts(
                    "Screen capture resumed",
                    "Screen Recording permission is back. Maekon resumed capturing under your existing consent settings.",
                )
                .await;
            }
        }
        None => {}
    }
    ok
}

fn emit_transition<R: Runtime>(
    app_handle: Option<&AppHandle<R>>,
    state: &'static str,
    status_reason: &'static str,
) {
    if let Some(handle) = app_handle {
        let payload = OsPermissionEventPayload {
            state,
            status_reason,
        };
        if let Err(e) = handle.emit(OS_PERMISSION_EVENT, &payload) {
            warn!("capture-os-permission event emit failed: {e}");
        }
    }
}
