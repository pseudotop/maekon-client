//! `TrackingScheduleConfig` and `TrackingWindow` — configuration types for
//! wall-clock allowed windows (Phase 9 PR-A).
//!
//! Tracking schedule configuration — privacy-hardening feature (Phase 9 PR-A).
//!
//! Allows users to configure wall-clock windows during which telemetry/capture
//! is allowed. A window is specified as a start/end HH:MM pair on selected days
//! of the week. Overnight wrap (end < start) is supported when the resulting
//! window spans <= 16 hours (windows spanning > 16 hours are rejected as likely
//! config errors — see validation comments below).
use chrono::{NaiveTime, Timelike};
use serde::{Deserialize, Serialize};

use crate::config::enums::Weekday;

// ── TrackingScheduleConfig ──────────────────────────────────────────

/// Top-level config section controlling tracking schedule allowed windows.
///
/// When `enabled` is true and `windows` is non-empty, telemetry/capture is
/// suppressed outside the configured windows. `timezone` is an IANA timezone
/// name or the special value `"Local"`
/// meaning the system local timezone.
///
/// Default: disabled, no windows, timezone `"Local"`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(try_from = "TrackingScheduleConfigRaw")]
pub struct TrackingScheduleConfig {
    /// Master switch; false = schedule is ignored and tracking always runs.
    #[serde(default)]
    pub enabled: bool,
    /// Wall-clock windows during which tracking is allowed. Empty vec means no
    /// schedule restriction is configured.
    #[serde(default)]
    pub windows: Vec<TrackingWindow>,
    /// IANA timezone name used for window matching, or `"Local"` for the
    /// system timezone. Default: `"Local"`.
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

/// Raw serde helper for `TrackingScheduleConfig` — accepts all strings without
/// validation; `TryFrom` performs validation after deserialization.
#[derive(Deserialize)]
pub(super) struct TrackingScheduleConfigRaw {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub windows: Vec<TrackingWindow>,
    #[serde(default = "default_timezone")]
    pub timezone: String,
}

impl TryFrom<TrackingScheduleConfigRaw> for TrackingScheduleConfig {
    type Error = String;

    fn try_from(raw: TrackingScheduleConfigRaw) -> Result<Self, Self::Error> {
        // Validate timezone: must be "Local" or a valid IANA timezone recognized
        // by chrono_tz. An invalid value produces a "config.invalid" error.
        if raw.timezone != "Local" {
            raw.timezone
                .parse::<chrono_tz::Tz>()
                .map_err(|_| format!("config.invalid: unknown timezone '{}'", raw.timezone))?;
        }
        Ok(TrackingScheduleConfig {
            enabled: raw.enabled,
            windows: raw.windows,
            timezone: raw.timezone,
        })
    }
}

// Custom Deserialize routes through the raw helper + TryFrom validation.
impl<'de> Deserialize<'de> for TrackingScheduleConfig {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = TrackingScheduleConfigRaw::deserialize(d)?;
        TrackingScheduleConfig::try_from(raw).map_err(serde::de::Error::custom)
    }
}

impl Default for TrackingScheduleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            windows: vec![],
            timezone: default_timezone(),
        }
    }
}

// ── TrackingWindow ──────────────────────────────────────────────────

/// A single wall-clock window within which tracking behaviour is altered.
///
/// `start` and `end` are `"HH:MM"` strings (24-hour). If `end < start` the
/// window wraps overnight (e.g. `22:00`–`06:00`). `days_of_week` lists the
/// days the window is active; an empty vec means the window never fires.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(try_from = "TrackingWindowRaw")]
pub struct TrackingWindow {
    /// Window open time, `"HH:MM"` (24-hour). Must be a valid HH:MM string.
    pub start: String,
    /// Window close time, `"HH:MM"` (24-hour). Must be a valid HH:MM string.
    pub end: String,
    /// Days of week on which this window is active. Empty = never active.
    #[serde(default)]
    pub days_of_week: Vec<Weekday>,
    /// Optional human-readable label for display purposes.
    #[serde(default)]
    pub label: String,
}

/// Raw serde helper for `TrackingWindow` — accepts all strings without
/// validation; `TryFrom` performs validation after deserialization.
#[derive(Deserialize)]
pub(super) struct TrackingWindowRaw {
    pub start: String,
    pub end: String,
    #[serde(default)]
    pub days_of_week: Vec<Weekday>,
    #[serde(default)]
    pub label: String,
}

/// Parse a strict `HH:MM` string (hours 00-23, minutes 00-59) into a
/// `NaiveTime`. Returns an error message containing
/// `"validation.invalid_field"` on failure.
pub(super) fn parse_hhmm(s: &str, field: &str) -> Result<NaiveTime, String> {
    if s.is_empty() {
        return Err(format!(
            "validation.invalid_field: '{field}' must not be empty"
        ));
    }
    // Must be exactly HH:MM (5 characters).
    let parts: Vec<&str> = s.splitn(2, ':').collect();
    if parts.len() != 2 || parts[0].len() != 2 || parts[1].len() != 2 {
        return Err(format!(
            "validation.invalid_field: '{field}' is not a valid HH:MM value (got '{s}')"
        ));
    }
    let h: u32 = parts[0].parse().map_err(|_| {
        format!("validation.invalid_field: '{field}' hour is not a number (got '{s}')")
    })?;
    let m: u32 = parts[1].parse().map_err(|_| {
        format!("validation.invalid_field: '{field}' minute is not a number (got '{s}')")
    })?;
    NaiveTime::from_hms_opt(h, m, 0)
        .ok_or_else(|| format!("validation.invalid_field: '{field}' is out of range (got '{s}')"))
}

impl TryFrom<TrackingWindowRaw> for TrackingWindow {
    type Error = String;

    fn try_from(raw: TrackingWindowRaw) -> Result<Self, Self::Error> {
        let start_time = parse_hhmm(&raw.start, "start")?;
        let end_time = parse_hhmm(&raw.end, "end")?;

        // Reject zero-length windows (start == end).
        if start_time == end_time {
            return Err(format!(
                "validation.invalid_field: 'start' and 'end' must not be equal (got '{}')",
                raw.start,
            ));
        }

        // Overnight-wrap policy:
        //
        // When end < start the window wraps across midnight. Classic overnight
        // windows (e.g. 22:00–06:00) are valid and common (8-hour wrap).
        // However, a window like 13:00–12:00 spans 23 hours — almost the entire
        // day — and is almost certainly a config error rather than intentional
        // scheduling.
        //
        // Rule: overnight wraps that exceed 16 hours are rejected.
        //   - 22:00 → 06:00: (06:00 + 24h) - 22:00 = 8h  → VALID
        //   - 13:00 → 12:00: (12:00 + 24h) - 13:00 = 23h → INVALID (> 16h)
        //
        // 16h was chosen as the threshold because legitimate overnight windows
        // (e.g. evenings + mornings) rarely exceed 12-14 hours, while a 23h
        // wrap is clearly unintentional. 16h provides a comfortable safety margin
        // between the two classes.
        if end_time < start_time {
            // Compute wrap duration in minutes.
            let start_mins = start_time.num_seconds_from_midnight() / 60;
            let end_mins = end_time.num_seconds_from_midnight() / 60;
            let wrap_duration_mins = (end_mins + 24 * 60) - start_mins;
            if wrap_duration_mins > 16 * 60 {
                return Err(format!(
                    "validation.invalid_field: overnight window '{}–{}' spans {}h {}m which exceeds the 16-hour safety threshold; \
                     did you swap start/end? Use a shorter window or split into two windows.",
                    raw.start,
                    raw.end,
                    wrap_duration_mins / 60,
                    wrap_duration_mins % 60,
                ));
            }
        }

        Ok(TrackingWindow {
            start: raw.start,
            end: raw.end,
            days_of_week: raw.days_of_week,
            label: raw.label,
        })
    }
}

// Custom Deserialize routes through the raw helper + TryFrom validation.
impl<'de> Deserialize<'de> for TrackingWindow {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = TrackingWindowRaw::deserialize(d)?;
        TrackingWindow::try_from(raw).map_err(serde::de::Error::custom)
    }
}

pub(super) fn default_timezone() -> String {
    "Local".to_string()
}
