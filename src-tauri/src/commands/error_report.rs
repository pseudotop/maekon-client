//! Tauri IPC command for receiving frontend route error reports.
//!
//! Classifies severity, logs via tracing (with per-route cooldown to prevent
//! log floods), shows native desktop notifications via tauri_plugin_notification
//! (with longer cooldown), and signals recovery strategies back to the main
//! webview via `frontend-recovery` events.
//!
//! ## Defense-in-depth
//!
//! - **Length limits**: error_message and stack are truncated before logging
//!   or being shown in notifications. Prevents disk fill / memory DoS.
//! - **Route allowlist**: route param must match the known routeTree paths,
//!   otherwise the call is rejected. Prevents log injection.
//! - **Logging cooldown**: per-route 10s cooldown on tracing::error! prevents
//!   crash loops from filling the rolling log file.
//! - **Notification cooldown**: per-route 30s cooldown prevents notification spam.
//! - **Recovery cooldown**: per-route 5s cooldown prevents infinite recovery loops.
//! - **Stale entry pruning**: cooldown maps drop entries older than 2× the
//!   cooldown window on each access. Bounded memory.
//! - **Scoped emit**: recovery events are emitted only to the main webview,
//!   not broadcast to overlay/tracking-panel windows.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{Emitter, EventTarget};
use tracing::{error, warn};

use crate::commands::system::{sanitize_frontend_surface, truncate_log_field};
use crate::ipc_error::IpcError;

// ── Length limits (prevent DoS via huge error payloads) ──

const MAX_ERROR_MESSAGE_LEN: usize = 4_000;
const MAX_STACK_LEN: usize = 12_000;
const MAX_ROUTE_LEN: usize = 256;
const MAX_SEVERITY_LEN: usize = 16;

// ── Bounded map size (prevent memory DoS via distinct routes) ──
//
// 256 comfortably covers the legitimate routeTree (~30 routes × ~3 severities
// or strategies = ~90 entries) with headroom for future growth. When the map
// is full, new routes are rejected — existing entries continue to honor
// cooldowns until they expire and are pruned by `prune_stale`.
const MAX_COOLDOWN_ENTRIES: usize = 256;

// ── Cooldown windows ──

const NOTIFICATION_COOLDOWN_SECS: u64 = 30;
const RECOVERY_COOLDOWN_SECS: u64 = 5;
const LOG_COOLDOWN_SECS: u64 = 10;

// ── Allowed route prefixes (matches routeTree top-level paths + sub-paths) ──
//
// Routes are validated against a permissive shape rather than an exhaustive
// allowlist so that future routes don't require Rust changes. The shape rule:
// - Must start with "/"
// - May contain "/", "-", "_", lowercase letters, digits
// - No control characters, no whitespace, no shell metacharacters

fn is_valid_route(route: &str) -> bool {
    if route.is_empty() || route.len() > MAX_ROUTE_LEN {
        return false;
    }
    if !route.starts_with('/') {
        return false;
    }
    route
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '/' | '-' | '_'))
}

// ── Module-level cooldown maps ──
//
// Mutex<Option<HashMap<...>>> pattern is used (not LazyLock<Mutex<HashMap>>)
// because the workspace MSRV is 1.77.1 and LazyLock requires Rust 1.80+.

static NOTIFICATION_COOLDOWN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
static RECOVERY_COOLDOWN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
static LOG_COOLDOWN: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);

// ── Payload types ──

/// Payload emitted to the frontend via `frontend-recovery` event.
#[derive(Debug, Serialize, Clone)]
struct RecoveryPayload {
    strategy: String,
    route: String,
    reason: String,
}

// ── Helpers ──

/// Build the cooldown key for the recovery map.
///
/// Composite (route, strategy) so reset-route and full-reload have
/// independent cooldowns — fixes NC-1 where the critical full-reload
/// emission was always suppressed by the in-flight reset-route cooldown.
fn recovery_cooldown_key(route: &str, strategy: &str) -> String {
    format!("{route}|{strategy}")
}

/// Build the cooldown key for the notification map.
///
/// Composite (route, severity) so a benign warning notification does
/// not silently suppress a subsequent critical notification on the same
/// route — fixes NC-NEW-4.
fn notification_cooldown_key(route: &str, severity: &str) -> String {
    format!("{route}|{severity}")
}

/// Drop entries older than `keep_for` from the cooldown map.
/// Called on each access to bound memory usage.
fn prune_stale(map: &mut HashMap<String, Instant>, keep_for: Duration) {
    let now = Instant::now();
    map.retain(|_, last| now.duration_since(*last) < keep_for);
}

/// Check the cooldown map: returns true if the route should be allowed
/// (not in cooldown), false if it should be suppressed. On allow, the
/// route's last-seen timestamp is updated.
///
/// Two layers of memory protection:
///  1. `prune_stale` drops entries older than 2× the cooldown window
///  2. If the map is at MAX_COOLDOWN_ENTRIES capacity AND the key is new,
///     the call is rejected (suppressed). Existing keys can still update.
///     This prevents an attacker flooding distinct routes from ballooning
///     map memory before pruning catches up.
fn check_and_update_cooldown(
    map_mutex: &Mutex<Option<HashMap<String, Instant>>>,
    route: &str,
    cooldown: Duration,
) -> bool {
    let mut guard = match map_mutex.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let map = guard.get_or_insert_with(HashMap::new);

    // Drop entries older than 2× the cooldown window — bounded memory
    prune_stale(map, cooldown.saturating_mul(2));

    let now = Instant::now();
    if let Some(last) = map.get(route) {
        if now.duration_since(*last) < cooldown {
            return false;
        }
    } else if map.len() >= MAX_COOLDOWN_ENTRIES {
        // Map is at capacity and this is a new route — suppress to bound memory
        return false;
    }
    map.insert(route.to_owned(), now);
    true
}

/// Show a native desktop notification via tauri_plugin_notification.
/// Respects a 30-second per-(route, severity) cooldown.
///
/// Severity is part of the cooldown key so a benign warning notification
/// does not silently suppress a subsequent critical notification on the
/// same route within the cooldown window.
fn maybe_notify(app: &tauri::AppHandle, route: &str, severity: &str, message: &str) {
    let cooldown = Duration::from_secs(NOTIFICATION_COOLDOWN_SECS);
    let cooldown_key = notification_cooldown_key(route, severity);
    if !check_and_update_cooldown(&NOTIFICATION_COOLDOWN, &cooldown_key, cooldown) {
        return;
    }

    let title = format!("Maekon - Route Error: {route}");
    // Notification body is naturally short — also clamp to 200 chars to keep
    // the notification visually compact.
    let body: String = message.chars().take(200).collect();

    if let Err(e) = tauri_plugin_notification::NotificationExt::notification(app)
        .builder()
        .title(&title)
        .body(&body)
        .show()
    {
        warn!("native route-error notification failed, suppressing: {e}");
    }
}

/// Emit a recovery signal to the main webview only.
/// Respects a 5-second per-(route, strategy) cooldown.
///
/// IMPORTANT: The cooldown key includes both route AND strategy so that an
/// escalation from `reset-route` to `full-reload` is not suppressed by the
/// in-flight reset cooldown. This is the fix for the CS-2 escalation gap
/// where the critical full-reload would otherwise share the same cooldown
/// bucket as the rapid reset-route signals that triggered the escalation.
fn maybe_emit_recovery(app: &tauri::AppHandle, route: &str, strategy: &str, reason: &str) {
    let cooldown = Duration::from_secs(RECOVERY_COOLDOWN_SECS);
    let cooldown_key = recovery_cooldown_key(route, strategy);
    if !check_and_update_cooldown(&RECOVERY_COOLDOWN, &cooldown_key, cooldown) {
        return;
    }

    let payload = RecoveryPayload {
        strategy: strategy.to_owned(),
        route: route.to_owned(),
        reason: reason.to_owned(),
    };

    // Scope to main webview — don't broadcast to overlay/tracking-panel
    if let Err(e) = app.emit_to(EventTarget::webview("main"), "frontend-recovery", &payload) {
        warn!("failed to emit frontend-recovery to main webview: {e}");
    }
}

/// Check if logging should proceed for this route, respecting a per-route cooldown.
fn should_log(route: &str) -> bool {
    let cooldown = Duration::from_secs(LOG_COOLDOWN_SECS);
    check_and_update_cooldown(&LOG_COOLDOWN, route, cooldown)
}

// ── Main command ──

/// Receive a frontend route error, log it, optionally notify, and signal recovery.
///
/// All inputs are sanitized:
/// - `route` is validated against `is_valid_route` (rejected if invalid)
/// - `error_message` is truncated to MAX_ERROR_MESSAGE_LEN
/// - `stack` and `component_stack` are truncated to MAX_STACK_LEN
/// - All fields are trimmed before truncation
///
/// Severity mapping:
/// - `info`     — log only (no notification, no recovery)
/// - `warning`  — log + desktop notification
/// - `error`    — log + desktop notification + `reset-route` recovery
/// - `critical` — log + desktop notification + `full-reload` recovery
///
/// Each severity level applies its respective cooldown to prevent flooding.
#[tauri::command]
pub async fn report_frontend_error(
    app: tauri::AppHandle,
    error_message: String,
    route: String,
    severity: String,
    stack: Option<String>,
    component_stack: Option<String>,
) -> Result<(), IpcError> {
    // Validate route shape — reject log injection and DoS via huge route strings
    if !is_valid_route(&route) {
        // Truncate the route to a small bounded preview BEFORE sanitization,
        // so an attacker passing a 1MB route doesn't cause a 1MB allocation
        // in the error path. Take chars (not bytes) to stay UTF-8 safe.
        let preview: String = route.chars().take(64).collect();
        return Err(IpcError::new(
            "validation.invalid_arguments",
            format!("invalid route: {}", sanitize_frontend_surface(&preview)),
        ));
    }

    // Validate severity length — prevent log DoS via attacker-controlled
    // severity string (the only string field that previously had no cap).
    if severity.len() > MAX_SEVERITY_LEN {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "invalid severity: too long",
        ));
    }

    // Truncate inputs to bounded sizes
    let error_message = truncate_log_field(error_message.trim().to_string(), MAX_ERROR_MESSAGE_LEN);
    let stack = stack.map(|s| truncate_log_field(s.trim().to_string(), MAX_STACK_LEN));
    let component_stack =
        component_stack.map(|s| truncate_log_field(s.trim().to_string(), MAX_STACK_LEN));

    // D5 iter-13: sanitize frontend-provided strings before logging. The
    // frontend may pass user input verbatim in error_message (e.g., a JS
    // error from a form validator that echoes the user's email). src-tauri
    // binary crate can use oneshim-vision directly.
    let error_message = oneshim_vision::privacy::sanitize_title_with_level(
        &error_message,
        oneshim_core::config::PiiFilterLevel::Standard,
    );
    let stack = stack.map(|s| {
        oneshim_vision::privacy::sanitize_title_with_level(
            &s,
            oneshim_core::config::PiiFilterLevel::Standard,
        )
    });
    let component_stack = component_stack.map(|s| {
        oneshim_vision::privacy::sanitize_title_with_level(
            &s,
            oneshim_core::config::PiiFilterLevel::Standard,
        )
    });

    // Apply per-route logging cooldown only for non-critical severities.
    // Critical bypasses the cooldown AND should not reset the bucket so a
    // subsequent warning/error within 10s is not silently suppressed.
    //
    // Severity contract matches the TS `Severity` type in reportToNative.ts:
    // `'warning' | 'error' | 'critical'`. The frontend type system forbids
    // any other value — any unknown severity is rejected as a contract
    // violation (likely a compromised/malicious caller).
    match severity.as_str() {
        "warning" => {
            if should_log(&route) {
                warn!(route, error_message, "frontend route warning");
            }
            maybe_notify(&app, &route, severity.as_str(), &error_message);
        }
        "error" => {
            if should_log(&route) {
                error!(
                    route,
                    error_message,
                    ?stack,
                    ?component_stack,
                    "frontend route error"
                );
            }
            maybe_notify(&app, &route, severity.as_str(), &error_message);
            maybe_emit_recovery(&app, &route, "reset-route", &error_message);
        }
        "critical" => {
            // Critical errors always log (bypass log cooldown — these are rare)
            error!(
                route,
                error_message,
                ?stack,
                ?component_stack,
                "CRITICAL frontend route error"
            );
            maybe_notify(&app, &route, severity.as_str(), &error_message);
            maybe_emit_recovery(&app, &route, "full-reload", &error_message);
        }
        other => {
            // Contract violation: the TS type forbids anything other than
            // warning/error/critical. An unknown severity indicates a
            // compromised caller or a contract drift — reject loudly but
            // without logging the raw `other` string (which is attacker-
            // controlled and already length-bounded by MAX_SEVERITY_LEN,
            // but we still truncate + sanitize defensively to match the
            // invalid-route rejection path's hardening.
            let preview: String = other.chars().take(16).collect();
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("invalid severity: {}", sanitize_frontend_surface(&preview)),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    // Tests that mutate the three global cooldown maps
    // (NOTIFICATION_COOLDOWN, RECOVERY_COOLDOWN, LOG_COOLDOWN) must run
    // serially — reset_all_cooldowns() wipes all three maps and would
    // race with other cooldown tests running in parallel, causing
    // intermittent assertion failures (observed ~1 in 5 parallel runs
    // before this serialization was added).
    //
    // Tests using `#[serial]` share a single global lock, so they
    // effectively run in sequence while tests without the attribute
    // continue to run in parallel with each other.

    fn reset_all_cooldowns() {
        for mutex in [&NOTIFICATION_COOLDOWN, &RECOVERY_COOLDOWN, &LOG_COOLDOWN] {
            let mut guard = mutex.lock().unwrap();
            let map = guard.get_or_insert_with(HashMap::new);
            map.clear();
        }
    }

    #[test]
    fn route_validation_accepts_known_shapes() {
        assert!(is_valid_route("/"));
        assert!(is_valid_route("/focus"));
        assert!(is_valid_route("/settings/general"));
        assert!(is_valid_route("/settings/ai-automation"));
        assert!(is_valid_route("/dashboard/day"));
        assert!(is_valid_route("/recalibration"));
        assert!(is_valid_route("/audit/entries"));
    }

    #[test]
    fn route_validation_rejects_dangerous_inputs() {
        assert!(!is_valid_route(""));
        assert!(!is_valid_route("focus"));
        assert!(!is_valid_route("/focus\nINJECTED"));
        assert!(!is_valid_route("/focus\r\n[ATTACKER]"));
        assert!(!is_valid_route("/Focus")); // uppercase rejected
        assert!(!is_valid_route("/focus?tab=1"));
        assert!(!is_valid_route("/focus;rm -rf /"));
        assert!(!is_valid_route("../etc/passwd"));
        assert!(!is_valid_route(&format!("/{}", "a".repeat(MAX_ROUTE_LEN))));
    }

    #[test]
    #[serial]
    fn check_and_update_cooldown_first_call_allows() {
        reset_all_cooldowns();
        let allowed =
            check_and_update_cooldown(&LOG_COOLDOWN, "/test-allow", Duration::from_secs(10));
        assert!(allowed);
    }

    #[test]
    #[serial]
    fn check_and_update_cooldown_blocks_within_window() {
        reset_all_cooldowns();
        let route = "/test-cooldown-block";
        let cooldown = Duration::from_secs(10);
        assert!(check_and_update_cooldown(&LOG_COOLDOWN, route, cooldown));
        // Immediate second call should be blocked
        assert!(!check_and_update_cooldown(&LOG_COOLDOWN, route, cooldown));
    }

    #[test]
    #[serial]
    fn check_and_update_cooldown_separates_by_route() {
        reset_all_cooldowns();
        let cooldown = Duration::from_secs(10);
        assert!(check_and_update_cooldown(
            &LOG_COOLDOWN,
            "/route-a",
            cooldown
        ));
        // Different route — should also be allowed
        assert!(check_and_update_cooldown(
            &LOG_COOLDOWN,
            "/route-b",
            cooldown
        ));
    }

    #[test]
    fn recovery_payload_serializes_correctly() {
        let payload = RecoveryPayload {
            strategy: "reset-route".to_owned(),
            route: "/focus".to_owned(),
            reason: "component crashed".to_owned(),
        };

        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["strategy"], "reset-route");
        assert_eq!(json["route"], "/focus");
        assert_eq!(json["reason"], "component crashed");
    }

    #[test]
    fn truncation_clamps_oversize_inputs() {
        let oversize = "x".repeat(MAX_ERROR_MESSAGE_LEN + 5_000);
        let truncated = truncate_log_field(oversize, MAX_ERROR_MESSAGE_LEN);
        assert!(truncated.len() <= MAX_ERROR_MESSAGE_LEN + 50); // 50 = " …(truncated)" suffix margin
        assert!(truncated.ends_with("(truncated)"));
    }

    #[test]
    fn check_and_update_cooldown_caps_distinct_route_count() {
        // IMPORTANT-2 regression: an attacker flooding distinct routes must
        // not balloon the cooldown map beyond MAX_COOLDOWN_ENTRIES.
        //
        // Uses a local Mutex (not LOG_COOLDOWN) to isolate from other tests
        // that may run in parallel and share the static map.
        let local_map: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
        let cooldown = Duration::from_secs(60);
        // Fill up to capacity
        for i in 0..MAX_COOLDOWN_ENTRIES {
            let route = format!("/cap-test-{i}");
            assert!(
                check_and_update_cooldown(&local_map, &route, cooldown),
                "expected slot {i} to be free"
            );
        }
        // The next NEW route must be rejected (suppressed)
        assert!(!check_and_update_cooldown(
            &local_map,
            "/cap-test-overflow",
            cooldown
        ));
        // Existing keys are also blocked by their own cooldown
        assert!(!check_and_update_cooldown(
            &local_map,
            "/cap-test-0",
            cooldown
        ));
    }

    #[test]
    fn prune_stale_keeps_fresh_entries() {
        // SUGGESTION-1 (rename): documents that prune_stale does NOT drop
        // entries within the keep window. The original test name was
        // misleading. A future deterministic-clock test could exercise the
        // drop path; for now we verify the no-op behavior.
        let mut map = HashMap::new();
        let now = Instant::now();
        map.insert("/fresh".to_string(), now);
        prune_stale(&mut map, Duration::from_secs(60));
        assert!(map.contains_key("/fresh"));
    }

    #[test]
    fn truncation_does_not_panic_on_multibyte_utf8_boundary() {
        // Korean and emoji are 3-4 bytes per char. With a 4000-byte limit
        // and 3-byte chars, the boundary lands inside a code point. The
        // earlier String::truncate(limit) implementation would panic here.
        let korean = "한".repeat(2000); // 6000 bytes total
        let truncated = truncate_log_field(korean, 4000);
        assert!(truncated.len() <= 4050);
        assert!(truncated.ends_with("(truncated)"));

        let emoji = "💥".repeat(1500); // 6000 bytes total (4 bytes each)
        let truncated_emoji = truncate_log_field(emoji, 4000);
        assert!(truncated_emoji.len() <= 4050);
        assert!(truncated_emoji.ends_with("(truncated)"));
    }

    #[test]
    fn recovery_cooldown_key_format() {
        assert_eq!(
            recovery_cooldown_key("/focus", "reset-route"),
            "/focus|reset-route"
        );
        assert_eq!(recovery_cooldown_key("/", "full-reload"), "/|full-reload");
    }

    #[test]
    fn notification_cooldown_key_format() {
        assert_eq!(
            notification_cooldown_key("/focus", "warning"),
            "/focus|warning"
        );
        assert_eq!(
            notification_cooldown_key("/audit", "critical"),
            "/audit|critical"
        );
    }

    #[test]
    #[serial]
    fn notification_cooldown_separates_severities() {
        // NC-NEW-4 regression: warning/error/critical on the same route
        // must not share a cooldown bucket. A benign warning notification
        // should not silently suppress a subsequent critical notification.
        reset_all_cooldowns();
        let cooldown = Duration::from_secs(30);
        let warn_key = notification_cooldown_key("/focus", "warning");
        let critical_key = notification_cooldown_key("/focus", "critical");
        assert!(check_and_update_cooldown(
            &NOTIFICATION_COOLDOWN,
            &warn_key,
            cooldown
        ));
        // Immediate critical call must NOT be blocked by the warning cooldown
        assert!(check_and_update_cooldown(
            &NOTIFICATION_COOLDOWN,
            &critical_key,
            cooldown
        ));
    }

    #[test]
    #[serial]
    fn recovery_cooldown_separates_strategies() {
        // NC-1 regression: reset-route and full-reload share a route, so
        // the cooldown key must include strategy. Use the helper (NC-NEW-5)
        // so the test exercises the same code path as production.
        reset_all_cooldowns();
        let cooldown = Duration::from_secs(5);
        let reset_key = recovery_cooldown_key("/focus", "reset-route");
        let reload_key = recovery_cooldown_key("/focus", "full-reload");
        assert!(check_and_update_cooldown(
            &RECOVERY_COOLDOWN,
            &reset_key,
            cooldown
        ));
        // Immediate first call for the full-reload key must NOT be blocked
        // by the reset-route cooldown — different keys.
        assert!(check_and_update_cooldown(
            &RECOVERY_COOLDOWN,
            &reload_key,
            cooldown
        ));
    }
}
