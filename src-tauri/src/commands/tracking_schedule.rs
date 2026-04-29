//! Tauri IPC commands for tracking-schedule configuration and status.
//!
//! Three commands mirror the REST surface
//! (`GET/PUT /config/tracking_schedule`, `GET /config/tracking_schedule/status`)
//! so the WebView and REST callers share the same semantics.

use chrono::{DateTime, Local, NaiveTime, Utc};
use tauri::command;

use oneshim_api_contracts::tracking_schedule::TrackingScheduleStatus;
use oneshim_core::config::{TrackingScheduleConfig, TrackingWindow};

use crate::ipc_error::IpcError;
use crate::runtime_state::ConfigRuntimeState;

// ── IPC commands ────────────────────────────────────────────────────────────

/// Return the current tracking-schedule configuration.
#[command]
pub async fn get_tracking_schedule(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<TrackingScheduleConfig, IpcError> {
    get_tracking_schedule_inner(state.config_manager().get().tracking_schedule.clone())
}

/// Inner logic extracted so tests can call it without constructing `tauri::State`.
pub(crate) fn get_tracking_schedule_inner(
    cfg: TrackingScheduleConfig,
) -> Result<TrackingScheduleConfig, IpcError> {
    Ok(cfg)
}

/// Persist a new tracking-schedule configuration.
///
/// Validates HH:MM format and IANA timezone before persisting. Returns
/// `IpcError` with code `"validation.invalid_arguments"` on invalid input.
#[command]
pub async fn set_tracking_schedule(
    cfg: TrackingScheduleConfig,
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<(), IpcError> {
    set_tracking_schedule_inner_with_manager(cfg, state.config_manager())
}

/// Inner logic for tests — validation only, no config manager needed.
///
/// A.16 (REST handlers) will also call this from non-test code; the
/// dead_code allow is a short-lived bridge until then.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn set_tracking_schedule_inner(cfg: TrackingScheduleConfig) -> Result<(), IpcError> {
    validate_tracking_schedule(&cfg)
}

/// Full implementation used by the real Tauri command: validates then persists.
fn set_tracking_schedule_inner_with_manager(
    cfg: TrackingScheduleConfig,
    manager: &oneshim_core::config_manager::ConfigManager,
) -> Result<(), IpcError> {
    validate_tracking_schedule(&cfg)?;
    manager
        .update_with(|app_cfg| {
            app_cfg.tracking_schedule = cfg;
            Ok(())
        })
        .map_err(IpcError::from)?;
    Ok(())
}

/// Validate all HH:MM fields and the timezone in a `TrackingScheduleConfig`.
///
/// The `TrackingScheduleConfig` type already validates on deserialisation via
/// `TryFrom<TrackingScheduleConfigRaw>`, but this function is the explicit IPC
/// boundary guard called before persisting any state mutation.
fn validate_tracking_schedule(cfg: &TrackingScheduleConfig) -> Result<(), IpcError> {
    for window in &cfg.windows {
        validate_hhmm(&window.start, "start")?;
        validate_hhmm(&window.end, "end")?;
    }
    if cfg.timezone != "Local" {
        cfg.timezone.parse::<chrono_tz::Tz>().map_err(|_| {
            IpcError::new(
                "validation.invalid_arguments",
                format!(
                    "unknown or unsupported timezone '{}'; use an IANA name (e.g. 'America/New_York') or 'Local'",
                    cfg.timezone
                ),
            )
        })?;
    }
    Ok(())
}

/// Return a real-time status snapshot for the tracking schedule.
///
/// Computes active_now, ends_at (if active), and next_starts_at (lookahead
/// 7 days).
#[command]
pub async fn get_tracking_schedule_status(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<TrackingScheduleStatus, IpcError> {
    let cfg = state.config_manager().get().tracking_schedule.clone();
    get_tracking_schedule_status_inner(cfg)
}

/// Inner logic extracted so tests can call it without constructing `tauri::State`.
pub(crate) fn get_tracking_schedule_status_inner(
    cfg: TrackingScheduleConfig,
) -> Result<TrackingScheduleStatus, IpcError> {
    if !cfg.enabled || cfg.windows.is_empty() {
        return Ok(TrackingScheduleStatus {
            active_now: false,
            ends_at: None,
            next_starts_at: compute_next_starts_at(&cfg),
            label: String::new(),
        });
    }

    let now_local: DateTime<Local> = Local::now();
    let now_tz = convert_to_target_tz(&now_local, &cfg.timezone);

    // Find the first active window.
    let active_window = cfg.windows.iter().find(|w| w.window_is_active(now_tz));

    if let Some(window) = active_window {
        let ends_at = compute_ends_at(now_tz, window);
        Ok(TrackingScheduleStatus {
            active_now: true,
            ends_at,
            next_starts_at: compute_next_starts_at(&cfg),
            label: window.label.clone(),
        })
    } else {
        Ok(TrackingScheduleStatus {
            active_now: false,
            ends_at: None,
            next_starts_at: compute_next_starts_at(&cfg),
            label: String::new(),
        })
    }
}

// ── Timezone conversion helper ─────────────────────────────────────────────

/// Convert a `DateTime<Local>` to a `DateTime<FixedOffset>` whose wall-clock
/// fields (`time()`, `weekday()`) match the target IANA timezone.
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

// ── ends_at computation ────────────────────────────────────────────────────

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

// ── next_starts_at computation ─────────────────────────────────────────────

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

// ── validate_hhmm ──────────────────────────────────────────────────────────

/// Validate a single `"HH:MM"` string at the IPC boundary.
///
/// Returns `Err(IpcError)` with code `"validation.invalid_arguments"` if the
/// string does not match the strict HH:MM format (hours 00-23, minutes 00-59).
pub(crate) fn validate_hhmm(s: &str, field: &str) -> Result<(), IpcError> {
    parse_hhmm_local(s, field)
        .map(|_| ())
        .map_err(|msg| IpcError::new("validation.invalid_arguments", msg))
}

/// Parse an HH:MM string into a `NaiveTime`. Returns an error message on
/// failure. Mirrors the logic in `oneshim-core`'s tracking_schedule module.
fn parse_hhmm_local(s: &str, field: &str) -> Result<NaiveTime, String> {
    if s.is_empty() {
        return Err(format!("'{field}' must not be empty"));
    }
    let parts: Vec<&str> = s.splitn(2, ':').collect();
    if parts.len() != 2 || parts[0].len() != 2 || parts[1].len() != 2 {
        return Err(format!("'{field}' is not a valid HH:MM value (got '{s}')"));
    }
    let h: u32 = parts[0]
        .parse()
        .map_err(|_| format!("'{field}' hour is not a number (got '{s}')"))?;
    let m: u32 = parts[1]
        .parse()
        .map_err(|_| format!("'{field}' minute is not a number (got '{s}')"))?;
    NaiveTime::from_hms_opt(h, m, 0).ok_or_else(|| format!("'{field}' is out of range (got '{s}')"))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal valid `TrackingScheduleConfig` for roundtrip tests.
    ///
    /// Uses all 7 days so tests that assert `active_now == true` pass on any
    /// day of the week (including weekends in CI).
    fn schedule_with_window(start: &str, end: &str, label: &str) -> TrackingScheduleConfig {
        let json = format!(
            r#"{{
                "enabled": true,
                "windows": [{{
                    "start": "{start}",
                    "end": "{end}",
                    "days_of_week": ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"],
                    "label": "{label}"
                }}],
                "timezone": "Asia/Seoul"
            }}"#
        );
        serde_json::from_str(&json).expect("valid tracking schedule")
    }

    // ── 1. set_then_get_roundtrip ───────────────────────────────────────────

    #[test]
    fn set_then_get_roundtrip() {
        let cfg = schedule_with_window("09:00", "18:00", "Work hours");

        let set_result = set_tracking_schedule_inner(cfg.clone());
        assert!(
            set_result.is_ok(),
            "set_tracking_schedule should succeed for valid config"
        );

        let get_result = get_tracking_schedule_inner(cfg.clone());
        let returned = get_result.expect("get_tracking_schedule should succeed");
        assert_eq!(
            returned, cfg,
            "get_tracking_schedule must return the config that was set"
        );
    }

    // ── 2. get_status_returns_active_when_now_in_window ────────────────────

    #[test]
    fn get_status_returns_active_when_now_in_window() {
        // `schedule_with_window` defaults to Mon-Fri, which fails on weekends.
        // Inline JSON with all 7 days ensures this test is truly "always-on".
        let json = r#"{
            "enabled": true,
            "windows": [{
                "start": "00:00",
                "end": "23:59",
                "days_of_week": ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"],
                "label": "Always-on window"
            }],
            "timezone": "Asia/Seoul"
        }"#;
        let cfg: TrackingScheduleConfig =
            serde_json::from_str(json).expect("valid always-on schedule");

        let status = get_tracking_schedule_status_inner(cfg)
            .expect("get_tracking_schedule_status should succeed");

        assert!(
            status.active_now,
            "active_now must be true when now is inside the window"
        );
        assert!(
            status.ends_at.is_some(),
            "ends_at must be populated when active_now is true"
        );
        assert!(
            !status.label.is_empty(),
            "label must be non-empty when a named window is active"
        );
    }

    // ── 3. get_status_returns_inactive_outside_window ──────────────────────

    #[test]
    fn get_status_returns_inactive_outside_window() {
        let cfg: TrackingScheduleConfig =
            serde_json::from_str(r#"{"enabled": false, "windows": [], "timezone": "Local"}"#)
                .expect("valid disabled schedule");

        let status = get_tracking_schedule_status_inner(cfg)
            .expect("get_tracking_schedule_status should succeed");

        assert!(
            !status.active_now,
            "active_now must be false when schedule is disabled"
        );
        assert!(
            status.ends_at.is_none(),
            "ends_at must be None when not in any window"
        );
    }

    // ── 4. get_status_returns_next_starts_at_within_7_days ─────────────────

    #[test]
    fn get_status_returns_next_starts_at_within_7_days() {
        let json = r#"{
            "enabled": true,
            "windows": [{
                "start": "23:58",
                "end": "23:59",
                "days_of_week": ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"],
                "label": "Night tick"
            }],
            "timezone": "Local"
        }"#;
        let cfg: TrackingScheduleConfig =
            serde_json::from_str(json).expect("valid tight-window schedule");

        let status = get_tracking_schedule_status_inner(cfg)
            .expect("get_tracking_schedule_status should succeed");

        assert!(
            status.next_starts_at.is_some(),
            "next_starts_at must be Some when a future window exists within 7 days"
        );

        if let Some(ref ts) = status.next_starts_at {
            let next: chrono::DateTime<chrono::Utc> = ts
                .parse()
                .expect("next_starts_at must be a valid RFC 3339 timestamp");
            let horizon = chrono::Utc::now() + chrono::Duration::days(7);
            assert!(
                next <= horizon,
                "next_starts_at must be within the 7-day lookahead horizon"
            );
        }
    }

    // ── 5. ipc_error_on_invalid_hhmm_format ────────────────────────────────

    #[test]
    fn ipc_error_on_invalid_hhmm_format() {
        let err = validate_hhmm("12:XX", "start").expect_err("validate_hhmm must reject '12:XX'");

        assert_eq!(
            err.code, "validation.invalid_arguments",
            "IpcError code must be 'validation.invalid_arguments', got '{}'",
            err.code
        );
    }
}
