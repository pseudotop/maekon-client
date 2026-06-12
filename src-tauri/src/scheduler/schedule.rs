//! Active-hours gate, tracking schedule gate, and capture-permitted composite
//! gate — all time-injectable for deterministic testing.
//!
//! Extracted from scheduler/mod.rs (ADR-013 split).

use chrono::{Datelike, Timelike};
use maekon_core::config::{AppConfig, Weekday};

pub(super) static BATTERY_SAVER_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_battery_saver_active_for_scheduler(active: bool) {
    BATTERY_SAVER_ACTIVE.store(active, std::sync::atomic::Ordering::Relaxed);
}

/// Time-injectable core of the active-hours gate.
///
/// Accepts an explicit `now: DateTime<Local>` so callers in tests can drive
/// deterministic scenarios (including the overnight wrap covered by CONS-C05).
/// Production call-sites should use [`should_run_now`] which calls
/// `chrono::Local::now()` internally.
///
/// # Overnight wrap (CONS-C05)
///
/// When `active_end_hour < active_start_hour` the window wraps midnight, e.g.
/// `22:00 – 06:00`.  For the hour-in-range check the rule is:
/// - Non-wrapping (`end > start`): `hour ∈ [start, end)` on `now.weekday()`.
/// - Wrapping (`end < start`): `hour ≥ start` OR `hour < end`.
///   - If `hour ≥ start`: check `now.weekday()` is in `active_days`.
///   - If `hour < end`:  check the *previous* weekday is in `active_days`
///     (because the window was opened last night).
/// - Equal (`end == start`): treated as empty window → returns `false`.
pub(crate) fn should_run_now_with_time(
    config: &AppConfig,
    now: chrono::DateTime<chrono::Local>,
) -> bool {
    let schedule = &config.schedule;
    if !schedule.active_hours_enabled {
        return true;
    }

    let hour = now.hour() as u8;
    let weekday = match now.weekday() {
        chrono::Weekday::Mon => Weekday::Mon,
        chrono::Weekday::Tue => Weekday::Tue,
        chrono::Weekday::Wed => Weekday::Wed,
        chrono::Weekday::Thu => Weekday::Thu,
        chrono::Weekday::Fri => Weekday::Fri,
        chrono::Weekday::Sat => Weekday::Sat,
        chrono::Weekday::Sun => Weekday::Sun,
    };

    let start = schedule.active_start_hour;
    let end = schedule.active_end_hour;

    if end > start {
        // Non-wrapping window: e.g. 09:00–17:00.
        if !schedule.active_days.contains(&weekday) {
            return false;
        }
        hour >= start && hour < end
    } else if end < start {
        // Overnight (wrapping) window: e.g. 22:00–06:00.
        if hour >= start {
            schedule.active_days.contains(&weekday)
        } else if hour < end {
            let yesterday = weekday_pred(weekday);
            schedule.active_days.contains(&yesterday)
        } else {
            false
        }
    } else {
        // start == end: empty / degenerate window → inactive.
        false
    }
}

/// Returns `true` when the current wall-clock time falls within the configured
/// active-hours window (or active_hours is disabled).
// A.7 removed the last non-test call-site (monitor.rs now uses capture_permitted_now).
// Retained for tests and potential future callers (e.g. A.9 loop gating helpers).
#[allow(dead_code)]
pub fn should_run_now(config: &AppConfig) -> bool {
    should_run_now_with_time(config, chrono::Local::now())
}

/// Returns `true` when the current instant falls inside any configured
/// tracking-schedule mute window.
///
/// Delegates to the time-injectable helper; uses `chrono::Local::now()`.
#[allow(dead_code)]
pub fn tracking_schedule_active(config: &AppConfig) -> bool {
    super::loops::tracking_schedule_helper::tracking_schedule_active(config, chrono::Local::now())
}

/// Full capture privacy gate composite — use this at all gate sites rather than
/// piecemeal checks.
///
/// ```text
/// capture_permitted_now =
///     config.vision.capture_enabled       // user-visible capture toggle
///     AND consent.screen_capture          // consent top-authority (CONS-PC02)
///     AND should_run_now(cfg)             // active_hours gate
///     AND !tracking_schedule_active(cfg)  // tracking-schedule mute gate
///     AND !capture_paused                 // user tray-toggle veto
/// ```
pub fn capture_permitted_now(
    config: &AppConfig,
    consent: &maekon_core::consent::ConsentPermissions,
    capture_paused: bool,
) -> bool {
    super::loops::tracking_schedule_helper::capture_permitted_now_with_power(
        config,
        consent,
        capture_paused,
        BATTERY_SAVER_ACTIVE.load(std::sync::atomic::Ordering::Relaxed),
        chrono::Local::now(),
    )
}

/// Microphone-capture privacy gate composite — the audio analogue of
/// [`capture_permitted_now`]. Differs only in the enable flag (`audio.enabled`
/// instead of `vision.capture_enabled`); all other terms (consent, active hours,
/// tracking schedule, pause, battery saver) are identical. Injects the live
/// `BATTERY_SAVER_ACTIVE` flag and `Local::now()`.
pub fn audio_capture_permitted_now(
    config: &AppConfig,
    consent: &maekon_core::consent::ConsentPermissions,
    capture_paused: bool,
) -> bool {
    super::loops::tracking_schedule_helper::audio_capture_permitted_now_with_power(
        config,
        consent,
        capture_paused,
        BATTERY_SAVER_ACTIVE.load(std::sync::atomic::Ordering::Relaxed),
        chrono::Local::now(),
    )
}

/// Returns the predecessor (previous) weekday.
///
/// Used by [`should_run_now_with_time`] for overnight window carry-over checks.
pub(crate) fn weekday_pred(day: Weekday) -> Weekday {
    match day {
        Weekday::Mon => Weekday::Sun,
        Weekday::Tue => Weekday::Mon,
        Weekday::Wed => Weekday::Tue,
        Weekday::Thu => Weekday::Wed,
        Weekday::Fri => Weekday::Thu,
        Weekday::Sat => Weekday::Fri,
        Weekday::Sun => Weekday::Sat,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::AppConfig;

    /// Build a `DateTime<Local>` for a known weekday at HH:MM.
    fn fixed_at_local(
        year: i32,
        month: u32,
        day: u32,
        hour: u32,
        minute: u32,
    ) -> chrono::DateTime<chrono::Local> {
        use chrono::{NaiveDate, TimeZone as _};
        let naive = NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap();
        chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .unwrap()
    }

    /// Build an `AppConfig` with an overnight active_hours window 22:00–06:00
    /// on Mon–Fri.
    fn overnight_cfg() -> AppConfig {
        let mut cfg = AppConfig::default_config();
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 22;
        cfg.schedule.active_end_hour = 6;
        cfg.schedule.active_days = vec![
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
        ];
        cfg
    }

    #[test]
    fn should_run_when_disabled() {
        let config = AppConfig::default_config();
        assert!(should_run_now(&config));
    }

    #[test]
    fn should_run_now_handles_overnight_range() {
        let cfg = overnight_cfg();

        let wed_23 = fixed_at_local(2024, 1, 10, 23, 0);
        assert!(
            should_run_now_with_time(&cfg, wed_23),
            "Wed 23:00 must be inside overnight window 22-06 (CONS-C05)"
        );

        let thu_01 = fixed_at_local(2024, 1, 11, 1, 0);
        assert!(
            should_run_now_with_time(&cfg, thu_01),
            "Thu 01:00 must be inside carry-over from Wed night (CONS-C05)"
        );

        let thu_0559 = fixed_at_local(2024, 1, 11, 5, 59);
        assert!(
            should_run_now_with_time(&cfg, thu_0559),
            "Thu 05:59 must be inside carry-over (end 06:00 is exclusive) (CONS-C05)"
        );

        let thu_0601 = fixed_at_local(2024, 1, 11, 6, 1);
        assert!(
            !should_run_now_with_time(&cfg, thu_0601),
            "Thu 06:01 must be outside the window (past end hour 06) (CONS-C05)"
        );
    }

    #[test]
    fn should_run_now_wraps_midnight_thu_01() {
        let cfg = overnight_cfg();

        let sat_0001 = fixed_at_local(2024, 1, 13, 0, 1);
        assert!(
            should_run_now_with_time(&cfg, sat_0001),
            "Sat 00:01 must be inside carry-over from Fri night \
             (Fri is in active_days, hour 0 < end 6) (CONS-C05)"
        );

        let sat_0601 = fixed_at_local(2024, 1, 13, 6, 1);
        assert!(
            !should_run_now_with_time(&cfg, sat_0601),
            "Sat 06:01 must be outside (past end 06, Sat not in active_days) (CONS-C05)"
        );

        let wed_2159 = fixed_at_local(2024, 1, 10, 21, 59);
        assert!(
            !should_run_now_with_time(&cfg, wed_2159),
            "Wed 21:59 must be outside (before start hour 22 on Wed) (CONS-C05)"
        );
    }

    fn capture_consent(granted: bool) -> maekon_core::consent::ConsentPermissions {
        maekon_core::consent::ConsentPermissions {
            screen_capture: granted,
            ..Default::default()
        }
    }

    #[test]
    fn scheduler_blocks_capture_outside_active_hours() {
        let mut cfg = AppConfig::default_config();
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 9;
        cfg.schedule.active_end_hour = 17;
        cfg.schedule.active_days = vec![maekon_core::config::Weekday::Mon];

        let consent = capture_consent(true);
        let now = fixed_at_local(2024, 1, 8, 20, 0);

        assert!(
            !super::super::loops::tracking_schedule_helper::capture_permitted_now(
                &cfg, &consent, false, now
            ),
            "Mon 20:00 must be blocked when active_hours is 09-17 (Mon only)"
        );
    }

    #[test]
    fn scheduler_allows_capture_when_schedule_disabled() {
        let cfg = AppConfig::default_config();
        let consent = capture_consent(true);
        let now = fixed_at_local(2024, 1, 7, 0, 0);

        assert!(
            super::super::loops::tracking_schedule_helper::capture_permitted_now(
                &cfg, &consent, false, now
            ),
            "capture must be permitted when active_hours_enabled=false (any time, any day)"
        );
    }

    #[test]
    fn scheduler_handles_overnight_active_hours() {
        let mut cfg = AppConfig::default_config();
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 22;
        cfg.schedule.active_end_hour = 6;
        cfg.schedule.active_days = vec![
            maekon_core::config::Weekday::Mon,
            maekon_core::config::Weekday::Tue,
            maekon_core::config::Weekday::Wed,
            maekon_core::config::Weekday::Thu,
            maekon_core::config::Weekday::Fri,
        ];

        let consent = capture_consent(true);
        let permit = |now| {
            super::super::loops::tracking_schedule_helper::capture_permitted_now(
                &cfg, &consent, false, now,
            )
        };

        let wed_23 = fixed_at_local(2024, 1, 10, 23, 0);
        assert!(permit(wed_23), "Wed 23:00 must be inside window (CONS-C05)");

        let thu_01 = fixed_at_local(2024, 1, 11, 1, 0);
        assert!(
            permit(thu_01),
            "Thu 01:00 must be inside (carry-over from Wed night) (CONS-C05)"
        );

        let thu_0559 = fixed_at_local(2024, 1, 11, 5, 59);
        assert!(
            permit(thu_0559),
            "Thu 05:59 must be inside (end 06:00 is exclusive) (CONS-C05)"
        );

        let thu_0601 = fixed_at_local(2024, 1, 11, 6, 1);
        assert!(
            !permit(thu_0601),
            "Thu 06:01 must be outside (past end hour 06) (CONS-C05)"
        );

        let sat_0001 = fixed_at_local(2024, 1, 13, 0, 1);
        assert!(
            permit(sat_0001),
            "Sat 00:01 must be inside (Fri carry-over, interpretation B — \
             pred-weekday check against active_days) (CONS-C05)"
        );
    }
}
