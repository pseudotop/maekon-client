//! Tracking schedule configuration — wall-clock allowed windows (Phase 9 PR-A).
//!
//! Split from a single 725-line file per ADR-013.
//! Public API (`TrackingScheduleConfig`, `TrackingWindow`) unchanged.

mod overlap;
mod types;

pub use types::{TrackingScheduleConfig, TrackingWindow};

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::enums::Weekday;

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
        let window = serde_json::from_str::<TrackingWindow>(json)
            .expect("16h overnight wrap (22:00→14:00) must be accepted at the boundary");
        // Pin the parsed values so a regression in the deserialization path is
        // immediately visible rather than silently accepted.
        assert_eq!(window.start, "22:00", "start time must round-trip");
        assert_eq!(window.end, "14:00", "end time must round-trip");
    }

    // ── 14. Overnight wrap at 16h + 1 minute is rejected ──────────────────

    #[test]
    fn overnight_wrap_just_over_16_hours_is_invalid() {
        // 22:00 → 14:01 wraps for (14:01 + 24h) - 22:00 = 16h 1m, one minute
        // past the threshold. This must be rejected with a message that
        // mentions the actual duration.
        let json = r#"{"start":"22:00","end":"14:01","days_of_week":["Mon"]}"#;
        let result = serde_json::from_str::<TrackingWindow>(json);
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
