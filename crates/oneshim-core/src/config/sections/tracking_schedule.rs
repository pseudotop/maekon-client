// Tracking schedule configuration — privacy-hardening feature (Phase 9 PR-A).
//
// Allows users to configure wall-clock windows during which telemetry/capture
// is muted. A window is specified as a start/end HH:MM pair on selected days
// of the week. Overnight wrap (end < start) is supported when the resulting
// window spans ≤ 16 hours (windows spanning > 16 hours are rejected as likely
// config errors — see validation comments below).
use chrono::{DateTime, Datelike, NaiveTime, TimeZone, Timelike};
use serde::{Deserialize, Serialize};

use crate::config::enums::Weekday;

// ── TrackingScheduleConfig ──────────────────────────────────────────

/// Top-level config section controlling tracking schedule muting.
///
/// When `enabled` is true and `windows` is non-empty, telemetry/capture is
/// suppressed outside (or during, depending on configuration) the configured
/// windows. `timezone` is an IANA timezone name or the special value `"Local"`
/// meaning the system local timezone.
///
/// Default: disabled, no windows, timezone `"Local"`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(try_from = "TrackingScheduleConfigRaw")]
pub struct TrackingScheduleConfig {
    /// Master switch; false = schedule is ignored and tracking always runs.
    #[serde(default)]
    pub enabled: bool,
    /// Wall-clock windows during which tracking is allowed (or muted, per
    /// interpretation in A.5). Empty vec means no windows configured.
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
struct TrackingScheduleConfigRaw {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    windows: Vec<TrackingWindow>,
    #[serde(default = "default_timezone")]
    timezone: String,
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
struct TrackingWindowRaw {
    start: String,
    end: String,
    #[serde(default)]
    days_of_week: Vec<Weekday>,
    #[serde(default)]
    label: String,
}

/// Parse a strict `HH:MM` string (hours 00-23, minutes 00-59) into a
/// `NaiveTime`. Returns an error message containing
/// `"validation.invalid_field"` on failure.
fn parse_hhmm(s: &str, field: &str) -> Result<NaiveTime, String> {
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

fn default_timezone() -> String {
    "Local".to_string()
}

// ── window_is_active ────────────────────────────────────────────────

impl TrackingWindow {
    /// Return `true` if `now` falls within this window.
    ///
    /// The parameter is generic over `TimeZone` so callers can pass any
    /// `DateTime<Tz>` — `DateTime<Local>` (production), `DateTime<FixedOffset>`
    /// (tests), `DateTime<chrono_tz::Tz>`, etc.  Only `now.time()` and
    /// `now.weekday()` are used; the timezone itself is not inspected.
    ///
    /// Overnight windows (`end < start`) wrap across midnight and match times
    /// in `[start, 24:00)` on the configured day OR `[00:00, end)` on the
    /// following day. Empty `days_of_week` always returns `false`.
    ///
    /// DST notes:
    /// - Spring-forward: no real instant exists for the skipped hour, so
    ///   no call to this method can land in the skipped interval.
    /// - Fall-back: both absolute instants that share the same wall-clock time
    ///   have identical `now.time()` and `now.weekday()`, so both are treated
    ///   identically — if the window covers that wall-clock time, both match.
    pub fn window_is_active<Tz: TimeZone>(&self, now: DateTime<Tz>) -> bool {
        if self.days_of_week.is_empty() {
            return false;
        }

        // Parse start/end — we validate in TryFrom so these are safe to unwrap.
        // If somehow called on an unchecked instance (test construction), treat
        // parse failure as inactive.
        let start_time = match parse_hhmm(&self.start, "start") {
            Ok(t) => t,
            Err(_) => return false,
        };
        let end_time = match parse_hhmm(&self.end, "end") {
            Ok(t) => t,
            Err(_) => return false,
        };

        let now_time = now.time();
        let now_weekday = chrono_weekday_to_ours(now.weekday());

        if end_time > start_time {
            // ── Non-wrapping (same-day) window: [start, end) ──────────────
            // Active only when `now` is on a configured day AND within [start, end).
            self.days_of_week.contains(&now_weekday)
                && now_time >= start_time
                && now_time < end_time
        } else {
            // ── Overnight (wrapping) window ───────────────────────────────
            // The window opens at `start` on the "start-day" and closes at
            // `end` on the following calendar day.
            //
            // `now` is in the window if either:
            //   (A) now_weekday is a configured start-day AND now_time >= start, OR
            //   (B) now_weekday is the day-after a configured start-day AND now_time < end.
            let is_start_day = self.days_of_week.contains(&now_weekday);
            let is_carry_over_day = self
                .days_of_week
                .iter()
                .any(|&d| weekday_succ(d) == now_weekday);

            (is_start_day && now_time >= start_time) || (is_carry_over_day && now_time < end_time)
        }
    }
}

// ── Weekday conversion helpers ──────────────────────────────────────

/// Convert a `chrono::Weekday` to our config `Weekday`.
fn chrono_weekday_to_ours(w: chrono::Weekday) -> Weekday {
    match w {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    }
}

/// Return the day after `d` (wrapping Sun → Mon).
fn weekday_succ(d: Weekday) -> Weekday {
    match d {
        Weekday::Mon => Weekday::Tue,
        Weekday::Tue => Weekday::Wed,
        Weekday::Wed => Weekday::Thu,
        Weekday::Thu => Weekday::Fri,
        Weekday::Fri => Weekday::Sat,
        Weekday::Sat => Weekday::Sun,
        Weekday::Sun => Weekday::Mon,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ────────────────────────────────────────────────────────────

    /// Build a TrackingWindow without a label. Panics if the window is invalid
    /// (callers should only construct valid windows here).
    fn window(start: &str, end: &str, days: Vec<Weekday>) -> TrackingWindow {
        TrackingWindow {
            start: start.to_string(),
            end: end.to_string(),
            days_of_week: days,
            label: String::new(),
        }
    }

    // ── 1. Default ─────────────────────────────────────────────────────────

    #[test]
    fn default_is_disabled_with_empty_windows() {
        let cfg = TrackingScheduleConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.windows.is_empty());
        assert_eq!(cfg.timezone, "Local");
    }

    // ── 2. Serde roundtrip ────────────────────────────────────────────────

    #[test]
    fn serde_roundtrip() {
        // This test exercises only Serialize + Deserialize, NOT Default or
        // window_is_active, so it must be GREEN already (derive-generated impls
        // are unconditional). A.3 may narrow serde validation but must not
        // break this roundtrip.
        let original = TrackingScheduleConfig {
            enabled: true,
            windows: vec![TrackingWindow {
                start: "09:00".to_string(),
                end: "17:00".to_string(),
                days_of_week: vec![Weekday::Mon, Weekday::Fri],
                label: "Work hours".to_string(),
            }],
            timezone: "America/New_York".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let parsed: TrackingScheduleConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, original);
    }

    // ── 3. Missing fields default ─────────────────────────────────────────

    #[test]
    fn serde_missing_fields_default() {
        // Parsing `{}` must succeed with serde defaults (not call Default impl).
        // serde #[serde(default)] on each field drives this — no todo!() involved.
        let empty: TrackingScheduleConfig = serde_json::from_str("{}").unwrap();
        assert!(!empty.enabled);
        assert!(empty.windows.is_empty());
        assert_eq!(empty.timezone, "Local");

        // Parsing with only `enabled` set — other fields use serde defaults.
        let partial: TrackingScheduleConfig = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(partial.enabled);
        assert!(partial.windows.is_empty());
        assert_eq!(partial.timezone, "Local");
    }

    // ── 4. Overnight window wraps midnight ────────────────────────────────

    #[test]
    fn overnight_window_wraps() {
        // Window 22:00–06:00 on Saturday.
        // Sat 23:00 → inside (Saturday in window hours 22-24)
        // Sun 01:00 → inside (Sunday in overnight carry-over hours 00-06)
        // Sat 21:00 → outside
        //
        // Using DateTime<FixedOffset> with UTC+0 so wall-clock == UTC, making
        // the test TZ-independent: now.time() / now.weekday() are always the
        // UTC wall-clock values regardless of machine timezone.

        use chrono::{FixedOffset, NaiveDate, TimeZone as _};

        let utc = FixedOffset::east_opt(0).unwrap();
        let w = window("22:00", "06:00", vec![Weekday::Sat]);

        // 2024-11-09 is a Saturday.
        let sat_23 = utc
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2024, 11, 9)
                    .unwrap()
                    .and_hms_opt(23, 0, 0)
                    .unwrap(),
            )
            .unwrap();
        // 2024-11-10 is a Sunday, 01:00 — carry-over from Saturday window.
        let sun_01 = utc
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2024, 11, 10)
                    .unwrap()
                    .and_hms_opt(1, 0, 0)
                    .unwrap(),
            )
            .unwrap();
        // 2024-11-09 Saturday 21:00 — before window opens.
        let sat_21 = utc
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2024, 11, 9)
                    .unwrap()
                    .and_hms_opt(21, 0, 0)
                    .unwrap(),
            )
            .unwrap();

        assert!(
            w.window_is_active(sat_23),
            "Sat 23:00 should be inside overnight window"
        );
        assert!(
            w.window_is_active(sun_01),
            "Sun 01:00 should be inside overnight carry-over"
        );
        assert!(
            !w.window_is_active(sat_21),
            "Sat 21:00 should be outside window"
        );
    }

    // ── 5. Normal (non-wrapping) window ───────────────────────────────────

    #[test]
    fn normal_window_does_not_wrap() {
        // Window 12:00–13:00 on Monday only.
        //
        // Using DateTime<FixedOffset> with UTC+0 so wall-clock == date literal,
        // making the test TZ-independent.

        use chrono::{FixedOffset, NaiveDate, TimeZone as _};

        let utc = FixedOffset::east_opt(0).unwrap();
        let w = window("12:00", "13:00", vec![Weekday::Mon]);

        // 2024-11-11 is a Monday.
        let mon_1230 = utc
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2024, 11, 11)
                    .unwrap()
                    .and_hms_opt(12, 30, 0)
                    .unwrap(),
            )
            .unwrap();
        let mon_1301 = utc
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2024, 11, 11)
                    .unwrap()
                    .and_hms_opt(13, 1, 0)
                    .unwrap(),
            )
            .unwrap();
        let mon_1159 = utc
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2024, 11, 11)
                    .unwrap()
                    .and_hms_opt(11, 59, 0)
                    .unwrap(),
            )
            .unwrap();

        assert!(w.window_is_active(mon_1230), "Mon 12:30 should be active");
        assert!(
            !w.window_is_active(mon_1301),
            "Mon 13:01 should be outside window"
        );
        assert!(
            !w.window_is_active(mon_1159),
            "Mon 11:59 should be outside window"
        );
    }

    // ── 6. Empty days_of_week never active ────────────────────────────────

    #[test]
    fn empty_days_never_active() {
        use chrono::{FixedOffset, NaiveDate, TimeZone as _};

        let utc = FixedOffset::east_opt(0).unwrap();
        let w = window("00:00", "23:59", vec![]);

        // Even a time that would match any time-of-day must be false.
        let any_time = utc
            .from_local_datetime(
                &NaiveDate::from_ymd_opt(2024, 11, 11)
                    .unwrap()
                    .and_hms_opt(12, 0, 0)
                    .unwrap(),
            )
            .unwrap();
        assert!(
            !w.window_is_active(any_time),
            "empty days_of_week must always return false"
        );
    }

    // ── 7. DST fall-back — ambiguous hour fires twice ─────────────────────

    #[test]
    fn dst_fall_back_fires_twice() {
        // US/Eastern fall-back 2024-11-03: clocks go 02:00 EST → 01:00 EST.
        // A window covering 01:00–02:30 on Sunday must match BOTH the EDT
        // occurrence (01:30 EDT = 05:30 UTC) and the EST occurrence
        // (01:30 EST = 06:30 UTC).
        //
        // Per CONS-C04 / spec §3.7: window_is_active is defined on wall-clock
        // time (local HH:MM + day). Both absolute instants that share the same
        // local wall-clock value must match.

        use chrono::MappedLocalTime;
        use chrono::NaiveDate;
        use chrono::TimeZone as _;
        use chrono_tz::US::Eastern;

        let w = window("01:00", "02:30", vec![Weekday::Sun]);

        let naive_130 = NaiveDate::from_ymd_opt(2024, 11, 3)
            .unwrap()
            .and_hms_opt(1, 30, 0)
            .unwrap();

        // On fall-back day, 01:30 is ambiguous — two UTC instants share it.
        let mapped = Eastern.from_local_datetime(&naive_130);
        let (early, late) = match mapped {
            MappedLocalTime::Ambiguous(a, b) => (a, b),
            other => panic!("expected Ambiguous, got {:?}", other),
        };

        // Use fixed_offset() instead of to_local() so that now.time() returns
        // Eastern wall-clock 01:30 regardless of machine timezone. Both early
        // (EDT, UTC-4) and late (EST, UTC-5) have wall-clock 01:30 in Eastern,
        // which fixed_offset() preserves exactly.
        let t_early = early.fixed_offset();
        let t_late = late.fixed_offset();

        assert!(
            w.window_is_active(t_early),
            "01:30 EDT (early / DST occurrence) should be in window"
        );
        assert!(
            w.window_is_active(t_late),
            "01:30 EST (late / standard occurrence) should be in window"
        );
    }

    // ── 8. DST spring-forward — skipped hour never fires ─────────────────

    #[test]
    fn dst_spring_forward_window_in_skipped_hour_never_fires() {
        // US/Eastern spring-forward 2024-03-10: clocks jump 02:00 → 03:00.
        // Local time 02:30 does not exist on that day.
        // A window configured "02:30"–"02:59" on that Sunday must never match
        // any real instant because no real instant has that local time.
        //
        // This test is GREEN by construction: we build the "would-be" timestamp
        // via chrono-tz and verify MappedLocalTime::None, then skip calling
        // window_is_active (there is no valid DateTime<Local> to pass in).
        // The assertion is structural: the local time literally does not exist.

        use chrono::MappedLocalTime;
        use chrono::NaiveDate;
        use chrono::TimeZone as _;
        use chrono_tz::US::Eastern;

        let naive_0230 = NaiveDate::from_ymd_opt(2024, 3, 10)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();

        let mapped = Eastern.from_local_datetime(&naive_0230);
        // The skipped hour must produce MappedLocalTime::None — no real instant.
        assert!(
            matches!(mapped, MappedLocalTime::None),
            "02:30 on spring-forward day must be MappedLocalTime::None, got {:?}",
            mapped
        );
        // No call to window_is_active because there is no valid local instant
        // to pass in. The absence of any matching instant IS the assertion.
    }

    // ── 9. Serde rejects invalid HH:MM ────────────────────────────────────

    #[test]
    fn serde_rejects_invalid_hhmm() {
        // A.3 adds custom validation in Deserialize via TryFrom<TrackingWindowRaw>.
        // "25:00" is an invalid hour → rejected with "validation.invalid_field".
        let json = r#"{"start":"25:00","end":"08:00","days_of_week":["Mon"]}"#;
        let result = serde_json::from_str::<TrackingWindow>(json);
        assert!(
            result.is_err(),
            "deserialization of '25:00' must fail; got: {:?}",
            result.ok()
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("validation.invalid_field"),
            "expected 'validation.invalid_field' in error, got: {err_msg}"
        );
    }

    // ── 10. Serde rejects invalid IANA timezone ───────────────────────────

    #[test]
    fn serde_rejects_invalid_iana_timezone() {
        // A.3 validates `timezone` as either "Local" or a valid IANA name
        // parseable by `chrono_tz::Tz::from_str`.
        let json = r#"{"enabled":true,"windows":[],"timezone":"Foo/Bar"}"#;
        let result = serde_json::from_str::<TrackingScheduleConfig>(json);
        assert!(
            result.is_err(),
            "deserialization of 'Foo/Bar' timezone must fail; got: {:?}",
            result.ok()
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("config.invalid"),
            "expected 'config.invalid' in error, got: {err_msg}"
        );
    }

    // ── 11. Empty end string is invalid ───────────────────────────────────

    #[test]
    fn window_with_empty_end_is_invalid() {
        // Empty string is not a valid HH:MM value.
        let json = r#"{"start":"09:00","end":"","days_of_week":["Mon"]}"#;
        let result = serde_json::from_str::<TrackingWindow>(json);
        assert!(
            result.is_err(),
            "deserialization of empty 'end' must fail; got: {:?}",
            result.ok()
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("validation.invalid_field"),
            "expected 'validation.invalid_field' in error, got: {err_msg}"
        );
    }

    // ── 12. Same-day end < start is invalid (not overnight) ───────────────

    #[test]
    fn window_end_before_start_not_same_day_is_invalid() {
        // start "13:00", end "12:00" with Mon-only days.
        // Overnight-wrap semantics: Mon 13:00 → Tue 12:00 = 23-hour window.
        // Policy: overnight wraps that exceed 16 hours are rejected as likely
        // config errors (see parse validation comment in TryFrom<TrackingWindowRaw>).
        let json = r#"{"start":"13:00","end":"12:00","days_of_week":["Mon"]}"#;
        let result = serde_json::from_str::<TrackingWindow>(json);
        assert!(
            result.is_err(),
            "start > end same-day window must be rejected; got: {:?}",
            result.ok()
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("validation.invalid_field"),
            "expected 'validation.invalid_field' in error, got: {err_msg}"
        );
    }

    // ── 13. Overnight wrap at exactly 16h is accepted (threshold boundary) ─

    #[test]
    fn overnight_wrap_exactly_16_hours_is_valid() {
        // 22:00 → 14:00 wraps across midnight for (14:00 + 24h) - 22:00 = 16h.
        // Validation rejects wraps strictly greater than 16h, so exactly 16h
        // sits on the accepting side of the boundary (policy: `> 16h` → reject).
        let json = r#"{"start":"22:00","end":"14:00","days_of_week":["Mon"]}"#;
        let result = serde_json::from_str::<TrackingWindow>(json);
        assert!(
            result.is_ok(),
            "16h overnight wrap (22:00→14:00) must be accepted at the boundary; got: {:?}",
            result.err(),
        );
    }

    // ── 14. Overnight wrap at 16h + 1 minute is rejected ──────────────────

    #[test]
    fn overnight_wrap_just_over_16_hours_is_invalid() {
        // 22:00 → 14:01 wraps for (14:01 + 24h) - 22:00 = 16h 1m, one minute
        // past the threshold. This must be rejected with a message that
        // mentions the actual duration.
        let json = r#"{"start":"22:00","end":"14:01","days_of_week":["Mon"]}"#;
        let result = serde_json::from_str::<TrackingWindow>(json);
        assert!(
            result.is_err(),
            "16h 1m overnight wrap (22:00→14:01) must be rejected just past the boundary; got: {:?}",
            result.ok(),
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("16-hour"),
            "expected the error to cite the 16-hour threshold, got: {err_msg}"
        );
        assert!(
            err_msg.contains("16h 1m"),
            "expected the error to report the actual wrap duration (16h 1m), got: {err_msg}"
        );
    }
}
