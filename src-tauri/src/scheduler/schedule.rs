//! Active-hours gate, tracking schedule gate, and capture-permitted composite
//! gate — all time-injectable for deterministic testing.
//!
//! Extracted from scheduler/mod.rs (ADR-013 split). #7735 E-3: the pure
//! policy functions (`should_run_now_with_time`, `tracking_schedule_active`'s
//! 2-arg core, the 4-arg `capture_permitted_now`/`audio_capture_permitted_now`
//! and their `_with_power` variants) moved to `maekon_core::capture_gate`.
//! This file keeps only the process-global `BATTERY_SAVER_ACTIVE` static and
//! the thin wrappers that read it + inject `Local::now()` — a composition-root
//! concern that cannot live in the tauri-free core crate.

// Only consumed within this file (`should_run_now` + its own tests) — the
// former crate-wide re-export at `scheduler::should_run_now_with_time` was
// removed in the same change since its only external caller moved into
// `maekon_core::capture_gate` too and no longer needs it.
use maekon_core::capture_gate::should_run_now_with_time;
use maekon_core::config::AppConfig;

pub(super) static BATTERY_SAVER_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Sets the process-global battery-saver flag consulted by
/// [`capture_permitted_now`] / [`audio_capture_permitted_now`].
///
/// #7734: widened from `pub(crate)` to `pub` (re-exported at
/// `scheduler::set_battery_saver_active_for_scheduler`) so the
/// `tracking_schedule_gating_integration` test can reset this shared static
/// between test runs; not a stability surface — internal app-library helper.
pub fn set_battery_saver_active_for_scheduler(active: bool) {
    BATTERY_SAVER_ACTIVE.store(active, std::sync::atomic::Ordering::Relaxed);
}

/// Returns `true` when the current wall-clock time falls within the configured
/// active-hours window (or active_hours is disabled).
// A.7 removed the last non-test call-site (monitor.rs now uses capture_permitted_now).
// Retained for tests and potential future callers (e.g. A.9 loop gating helpers).
#[allow(dead_code)]
pub fn should_run_now(config: &AppConfig) -> bool {
    should_run_now_with_time(config, chrono::Local::now())
}

/// Whether the configured tracking schedule permits capture at the current
/// local time. Enabled non-empty schedules require the current instant to fall
/// inside an allowed window; disabled and empty schedules remain unrestricted.
/// Delegates to the time-injectable helper using `chrono::Local::now()`.
pub fn tracking_schedule_allows_capture(config: &AppConfig) -> bool {
    maekon_core::capture_gate::tracking_schedule_allows_capture(config, chrono::Local::now())
}

/// Full capture privacy gate composite — use this at all gate sites rather than
/// piecemeal checks.
///
/// ```text
/// capture_permitted_now =
///     config.vision.capture_enabled       // user-visible capture toggle
///     AND consent.screen_capture          // consent top-authority (CONS-PC02)
///     AND should_run_now(cfg)             // active_hours gate
///     AND tracking_schedule_allows_capture(cfg) // tracking-schedule allow gate
///     AND !capture_paused                 // user tray-toggle veto
/// ```
pub fn capture_permitted_now(
    config: &AppConfig,
    consent: &maekon_core::consent::ConsentPermissions,
    capture_paused: bool,
) -> bool {
    maekon_core::capture_gate::capture_permitted_now_with_power(
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
    maekon_core::capture_gate::audio_capture_permitted_now_with_power(
        config,
        consent,
        capture_paused,
        BATTERY_SAVER_ACTIVE.load(std::sync::atomic::Ordering::Relaxed),
        chrono::Local::now(),
    )
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
            maekon_core::config::Weekday::Mon,
            maekon_core::config::Weekday::Tue,
            maekon_core::config::Weekday::Wed,
            maekon_core::config::Weekday::Thu,
            maekon_core::config::Weekday::Fri,
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
            !maekon_core::capture_gate::capture_permitted_now(&cfg, &consent, false, now),
            "Mon 20:00 must be blocked when active_hours is 09-17 (Mon only)"
        );
    }

    #[test]
    fn scheduler_allows_capture_when_schedule_disabled() {
        let cfg = AppConfig::default_config();
        let consent = capture_consent(true);
        let now = fixed_at_local(2024, 1, 7, 0, 0);

        assert!(
            maekon_core::capture_gate::capture_permitted_now(&cfg, &consent, false, now),
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
        let permit =
            |now| maekon_core::capture_gate::capture_permitted_now(&cfg, &consent, false, now);

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

    /// #8094 first-run pin: a FRESH profile (a `ConsentManager` over a path with
    /// no `consent.json`) must deny screen capture, microphone capture, AND
    /// telemetry through the PRODUCTION gate wrappers the monitor loop
    /// (`crate::scheduler::capture_permitted_now`, monitor.rs), audio path
    /// (`audio_capture_permitted_now`), and egress path (`may_upload_telemetry`)
    /// actually call. This pins the whole default-deny chain end-to-end:
    /// fresh `ConsentManager` → `ConsentGate` fail-closed snapshot → composite
    /// gate == closed, on default config (capture_enabled=true, active_hours off).
    #[test]
    fn fresh_profile_default_deny_capture_audio_and_telemetry_first_run() {
        use maekon_core::consent::{ConsentManager, ConsentPermissions};
        use maekon_core::ports::consent_manager::{ConsentGate, ConsentManagerPort};

        // Fresh profile: no consent.json exists yet (first run).
        let dir = tempfile::tempdir().unwrap();
        let manager: std::sync::Arc<dyn ConsentManagerPort> =
            std::sync::Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        let owned = Some(manager);
        let gate = ConsentGate::from_ref(owned.as_ref());
        let snapshot = gate.permissions_snapshot();
        let cfg = AppConfig::default_config();

        assert!(
            !capture_permitted_now(&cfg, &snapshot, false),
            "fresh no-consent profile must deny screen capture (first-run fail-closed, #8094)"
        );
        assert!(
            !audio_capture_permitted_now(&cfg, &snapshot, false),
            "fresh no-consent profile must deny microphone capture (first-run fail-closed)"
        );
        assert!(
            !gate.may_upload_telemetry(),
            "fresh no-consent profile must deny telemetry upload (first-run fail-closed)"
        );

        // Positive control: granting ONLY screen_capture opens the screen gate but
        // NOT the mic gate — proving the denials above were the consent term, not an
        // unrelated always-closed gate, and that mic keeps its own consent (#4568).
        owned
            .as_ref()
            .unwrap()
            .grant_consent(
                ConsentPermissions {
                    screen_capture: true,
                    ..Default::default()
                },
                30,
            )
            .unwrap();
        let granted = ConsentGate::from_ref(owned.as_ref()).permissions_snapshot();
        assert!(
            capture_permitted_now(&cfg, &granted, false),
            "granting screen_capture consent must open the capture gate on default config"
        );
        assert!(
            !audio_capture_permitted_now(&cfg, &granted, false),
            "screen consent must NOT open the mic gate (#4568: mic has its own consent)"
        );
    }
}
