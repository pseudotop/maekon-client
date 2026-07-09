//! REST handlers for the tracking-schedule configuration and status endpoints.
//!
//! - `GET  /api/tracking-schedule`         — return current config
//! - `PUT  /api/tracking-schedule`         — validate + persist new config
//! - `GET  /api/tracking-schedule/status`  — real-time status snapshot
//!
//! A.15 (TDD red): stub handlers returning 501.
//! A.16 (TDD green): real logic supplied here.
//!
//! #7600: the sibling Tauri IPC commands (`src-tauri/src/commands/tracking_schedule.rs`)
//! were removed as dead duplicates — this REST surface is now the sole
//! implementation the frontend uses.

use axum::extract::rejection::JsonRejection;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Local, Utc};
use maekon_api_contracts::error::ErrorResponse;
use maekon_api_contracts::tracking_schedule::TrackingScheduleStatus;
use maekon_core::config::{TrackingScheduleConfig, TrackingWindow};

use crate::services::web_contexts::ConfigWebContext;

// ── GET /api/tracking-schedule ───────────────────────────────────────────────

/// Return the current `TrackingScheduleConfig` as JSON 200.
///
/// Falls back to `TrackingScheduleConfig::default()` when no `ConfigManager`
/// is wired (e.g. in tests that only configure storage).
pub async fn get_config(State(context): State<ConfigWebContext>) -> Json<TrackingScheduleConfig> {
    let cfg = context
        .config_manager
        .as_ref()
        .map(|m| m.get().tracking_schedule.clone())
        .unwrap_or_default();
    Json(cfg)
}

// ── PUT /api/tracking-schedule ───────────────────────────────────────────────

/// Persist a new `TrackingScheduleConfig`.
///
/// Validation happens in two layers:
///
/// 1. **Deserialization** — `TrackingScheduleConfig`'s custom `Deserialize`
///    impl already validates HH:MM syntax and the IANA timezone via
///    `TryFrom<TrackingWindowRaw>`. If the body is malformed the Axum
///    `JsonRejection` is caught here and mapped to 400 with the wire code
///    `"validation.invalid_arguments"`.
///
/// 2. **Persist** — `ConfigManager::update_with` is called after successful
///    deserialization. Storage errors are mapped to 500.
///
/// On success returns 200 with the accepted config as the echo body.
pub async fn put_config(
    State(context): State<ConfigWebContext>,
    body: Result<Json<TrackingScheduleConfig>, JsonRejection>,
) -> Response {
    // Layer 1: parse + validate (HH:MM, timezone) via custom Deserialize.
    let Json(cfg) = match body {
        Ok(json) => json,
        Err(rejection) => {
            // The serde error message from TrackingWindow/TrackingScheduleConfig
            // already contains human-readable context. Prefix with the wire code
            // so callers can pattern-match without inspecting the free-text portion.
            let msg = format!("validation.invalid_arguments: {}", rejection.body_text());
            let resp = ErrorResponse {
                code: "validation.invalid_arguments".to_string(),
                error: msg,
                status: 400,
            };
            return (StatusCode::BAD_REQUEST, Json(resp)).into_response();
        }
    };

    // Layer 2: persist.
    let Some(ref manager) = context.config_manager else {
        // No config manager wired — echo the config without persisting.
        return Json(cfg).into_response();
    };

    match manager.update_with(|app_cfg| {
        app_cfg.tracking_schedule = cfg.clone();
        Ok(())
    }) {
        Ok(_) => Json(cfg).into_response(),
        Err(e) => {
            let resp = ErrorResponse {
                code: "internal.generic".to_string(),
                error: format!("internal: {e}"),
                status: 500,
            };
            (StatusCode::INTERNAL_SERVER_ERROR, Json(resp)).into_response()
        }
    }
}

// ── GET /api/tracking-schedule/status ────────────────────────────────────────

/// Return a real-time status snapshot for the tracking schedule.
///
/// Computes `active_now`, `ends_at` (if active), `next_starts_at` (7-day
/// lookahead), and `label`.
///
/// The compute helpers below duplicate the logic from
/// `src-tauri/src/commands/tracking_schedule.rs` because `maekon-web` cannot
/// depend on the `src-tauri` binary crate.
///
/// Follow-up: factor the helpers into `maekon-api-contracts` or a shared
/// library crate so A.14 (IPC) and A.16 (REST) share a single source.
pub async fn get_status(State(context): State<ConfigWebContext>) -> Json<TrackingScheduleStatus> {
    let cfg = context
        .config_manager
        .as_ref()
        .map(|m| m.get().tracking_schedule.clone())
        .unwrap_or_default();

    Json(compute_status(&cfg))
}

// ── Status compute helpers ─────────────────────────────────────────────────
// #7600: this used to be duplicated from src-tauri/src/commands/tracking_schedule.rs
// (a sibling Tauri IPC implementation). That command module was removed as a
// dead duplicate, so this REST-side implementation is now the sole copy.

/// Compute a `TrackingScheduleStatus` snapshot for the given config.
fn compute_status(cfg: &TrackingScheduleConfig) -> TrackingScheduleStatus {
    if !cfg.enabled || cfg.windows.is_empty() {
        return TrackingScheduleStatus {
            active_now: false,
            ends_at: None,
            next_starts_at: compute_next_starts_at(cfg),
            label: String::new(),
        };
    }

    let now_local: DateTime<Local> = Local::now();
    let now_tz = convert_to_target_tz(&now_local, &cfg.timezone);

    let active_window = cfg.windows.iter().find(|w| w.window_is_active(now_tz));

    if let Some(window) = active_window {
        let ends_at = compute_ends_at(now_tz, window);
        TrackingScheduleStatus {
            active_now: true,
            ends_at,
            next_starts_at: compute_next_starts_at(cfg),
            label: window.label.clone(),
        }
    } else {
        TrackingScheduleStatus {
            active_now: false,
            ends_at: None,
            next_starts_at: compute_next_starts_at(cfg),
            label: String::new(),
        }
    }
}

/// Convert a `DateTime<Local>` to a `DateTime<FixedOffset>` whose wall-clock
/// fields match the target IANA timezone.
fn convert_to_target_tz(local: &DateTime<Local>, tz_name: &str) -> DateTime<chrono::FixedOffset> {
    if tz_name == "Local" {
        return local.fixed_offset();
    }
    if let Ok(tz) = tz_name.parse::<chrono_tz::Tz>() {
        let utc_instant: DateTime<Utc> = local.with_timezone(&Utc);
        return utc_instant.with_timezone(&tz).fixed_offset();
    }
    // Fallback: return local (validation should have caught invalid names).
    local.fixed_offset()
}

/// Walk forward minute-by-minute until the active window closes, returning
/// the transition UTC time as RFC 3339. Caps at 24 hours.
fn compute_ends_at(now: DateTime<chrono::FixedOffset>, window: &TrackingWindow) -> Option<String> {
    let mut probe = now;
    let cap = now + chrono::Duration::hours(24);

    while probe < cap {
        probe += chrono::Duration::minutes(1);
        if !window.window_is_active(probe) {
            return Some(probe.with_timezone(&Utc).to_rfc3339());
        }
    }
    // Window covers the full cap horizon.
    Some(cap.with_timezone(&Utc).to_rfc3339())
}

/// Scan forward minute-by-minute over the next 7 days to find the next window
/// start (inactive → active rising edge). Returns an RFC 3339 UTC string or
/// `None` if no window starts within 7 days.
fn compute_next_starts_at(cfg: &TrackingScheduleConfig) -> Option<String> {
    if !cfg.enabled || cfg.windows.is_empty() {
        return None;
    }

    let now_local: DateTime<Local> = Local::now();
    let now_tz = convert_to_target_tz(&now_local, &cfg.timezone);
    let horizon = now_tz + chrono::Duration::days(7);

    let mut prev_active = cfg.windows.iter().any(|w| w.window_is_active(now_tz));
    let mut probe = now_tz + chrono::Duration::minutes(1);

    while probe <= horizon {
        let cur_active = cfg.windows.iter().any(|w| w.window_is_active(probe));
        if !prev_active && cur_active {
            return Some(probe.with_timezone(&Utc).to_rfc3339());
        }
        prev_active = cur_active;
        probe += chrono::Duration::minutes(1);
    }
    None
}
