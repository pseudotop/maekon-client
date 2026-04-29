//! Tracking-schedule scheduler helpers — Phase 9 PR-A.
//!
//! This module exposes two pure functions consumed by the scheduler:
//!
//! * [`tracking_schedule_active`] — returns `true` when the current instant
//!   falls inside any configured tracking-schedule window.
//!
//! * [`capture_permitted_now`] — composes all four privacy gates per spec §3.4:
//!   `consent_granted AND active_hours AND !tracking_schedule_active AND !capture_paused`.
//!
//! * [`evaluate_and_notify_transitions`] — fires a desktop notification when the
//!   tracking-schedule window is entered or exited, with a 60-second debounce guard
//!   to suppress flip-flop storms from DST edges or backward clock jumps (A.18).
//!
//! A.5 implements the real logic for both functions.

use async_trait::async_trait;
use chrono::{DateTime, Local};
use oneshim_core::config::AppConfig;
use oneshim_core::consent::ConsentPermissions;
use std::time::Instant;

// ── TsNotifier — narrow port for tracking-schedule notifications ─────────────

/// 트래킹 스케줄 전환 알림을 발송하는 좁은 포트.
///
/// [`NotificationManager`] 는 이 트레이트를 구현하므로 프로덕션에서 직접 사용된다.
/// 테스트에서는 [`RecordingNotifier`] 모의 구현을 사용한다.
///
/// [`NotificationManager`]: crate::notification_manager::NotificationManager
#[async_trait]
pub(crate) trait TsNotifier: Send + Sync {
    /// 제목과 본문으로 데스크탑 알림을 발송한다.
    /// 알림 라이브러리 실패는 무시 (best-effort).
    async fn notify_ts(&self, title: &str, body: &str);
}

// ── Implementations ─────────────────────────────────────────────────────────

/// Returns `true` when `now` falls inside any configured tracking-schedule
/// mute window.
///
/// When `cfg.tracking_schedule.enabled` is `false` or the `windows` list is
/// empty, the schedule is considered inactive and this returns `false`.
///
/// The function iterates all configured windows and delegates per-window
/// range checks to [`TrackingWindow::window_is_active`] (implemented in A.3),
/// which handles both same-day and overnight wrap windows correctly.
pub(crate) fn tracking_schedule_active(cfg: &AppConfig, now: DateTime<Local>) -> bool {
    let ts = &cfg.tracking_schedule;
    if !ts.enabled {
        return false;
    }
    ts.windows.iter().any(|w| w.window_is_active(now))
}

/// Returns `true` when capture is permitted right now.
///
/// Composes all 4 privacy gates per spec §3.4 (CONS-PC02):
///
/// ```text
/// capture_permitted_now =
///     consent.screen_capture                     // consent top-authority gate
///     AND should_run_now_with_time(cfg, now)     // active_hours gate
///     AND !tracking_schedule_active(cfg, now)    // tracking-schedule negative gate
///     AND !capture_paused                         // user tray-toggle veto
/// ```
///
/// All four gates must be true for capture to be permitted. Any single `false`
/// short-circuits and returns `false` regardless of the other gate values.
///
/// # Plan deviation note
/// The original plan spec §3.4 cited `consent.allows_tier(ConsentTier::Capture)`
/// but that method does not exist in `oneshim-core`. The actual API exposes the
/// field `ConsentPermissions.screen_capture: bool`, confirmed during A.4. This
/// implementation uses the field directly.
pub(crate) fn capture_permitted_now(
    cfg: &AppConfig,
    consent: &ConsentPermissions,
    capture_paused: bool,
    now: DateTime<Local>,
) -> bool {
    consent.screen_capture
        && crate::scheduler::should_run_now_with_time(cfg, now)
        && !tracking_schedule_active(cfg, now)
        && !capture_paused
}

// ── evaluate_and_notify_transitions ─────────────────────────────────────────

/// 트래킹 스케줄 윈도우의 진입/퇴장 전환을 감지하여 데스크탑 알림을 발송한다.
///
/// # 동작
///
/// 1. `cfg.notification.tracking_schedule_enabled` 가 `false` 면 즉시 반환 (no-op).
/// 2. `prev_active == now_active` 이면 전환 없음 — 즉시 반환.
/// 3. 마지막 알림 발송 이후 60초 미만이면 디바운스 — 즉시 반환.
///    이는 DST 경계나 역방향 클락 점프로 인한 플립-플랍 폭풍을 방지한다.
/// 4. 위 조건을 통과하면 `last_notified_at` 을 현재 `Instant` 로 갱신하고
///    `notifier` 를 통해 알림을 발송한다.
///
/// `notifier` 가 `None` 이면 알림 없이 상태만 갱신한다.
///
/// # 인자
///
/// * `cfg` — 현재 앱 설정 (알림 토글 및 스케줄 확인용)
/// * `prev_active` — 이전 틱의 트래킹 스케줄 활성 상태
/// * `now_active` — 이번 틱의 트래킹 스케줄 활성 상태
/// * `last_notified_at` — 마지막 알림 시각 (60초 디바운스 상태); 인/아웃 모두 공유
/// * `notifier` — 알림 발송 구현체 (옵션)
pub(crate) async fn evaluate_and_notify_transitions<N: TsNotifier>(
    cfg: &AppConfig,
    prev_active: bool,
    now_active: bool,
    last_notified_at: &mut Option<Instant>,
    notifier: Option<&N>,
) {
    // Gate 1: 알림 설정 비활성화 시 no-op
    if !cfg.notification.tracking_schedule_enabled {
        return;
    }
    // Gate 2: 전환 없음 시 no-op
    if prev_active == now_active {
        return;
    }
    // Gate 3: 60초 디바운스 — 마지막 알림으로부터 60초 미만이면 억제
    let now = Instant::now();
    if let Some(last) = *last_notified_at {
        if now.duration_since(last).as_secs() < 60 {
            return;
        }
    }
    // 상태 갱신 + 알림 발송
    *last_notified_at = Some(now);
    if let Some(n) = notifier {
        if now_active {
            n.notify_ts(
                "Tracking Schedule Active",
                "Capture/telemetry paused during configured window",
            )
            .await;
        } else {
            n.notify_ts("Tracking Schedule Ended", "Capture/telemetry resumed")
                .await;
        }
    }
}

// ── Monitor-loop tick helper ─────────────────────────────────────────────────

/// 매 모니터 틱에서 호출되는 트래킹 스케줄 알림 평가 래퍼.
///
/// `config_manager` 스냅샷을 가져오고 `tracking_schedule_active` 를 평가한 뒤
/// `evaluate_and_notify_transitions` 를 호출한다. 모니터 루프 클로저 크기 제한(500줄)
/// 을 지키기 위해 인라인 블록을 이 함수로 추출한다 (monitor-loop-size hook).
pub(super) async fn tick_ts_notifications(
    config_manager: &Option<oneshim_core::config_manager::ConfigManager>,
    notifier: Option<&crate::notification_manager::NotificationManager>,
    prev_ts_active: &mut bool,
    last_ts_notified_at: &mut Option<std::time::Instant>,
) {
    let cfg = config_manager
        .as_ref()
        .map(|cm| cm.get())
        .unwrap_or_else(oneshim_core::config::AppConfig::default_config);
    let now_active = tracking_schedule_active(&cfg, chrono::Local::now());
    evaluate_and_notify_transitions(
        &cfg,
        *prev_ts_active,
        now_active,
        last_ts_notified_at,
        notifier,
    )
    .await;
    *prev_ts_active = now_active;
}

// ── TsNotifier impl for NotificationManager ──────────────────────────────────

// `NotificationManager` 가 `TsNotifier` 를 구현하므로 monitor 루프에서 직접 전달된다.
// 이 impl 은 notification_manager 크레이트 모듈이 아닌 helper 내에 위치하여
// `loops` 모듈의 비공개 경계를 넘지 않도록 한다.
#[async_trait]
impl TsNotifier for crate::notification_manager::NotificationManager {
    async fn notify_ts(&self, title: &str, body: &str) {
        self.notify(title, body).await;
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone as _, Timelike};
    use oneshim_core::config::Weekday;
    use oneshim_core::config::{TrackingScheduleConfig, TrackingWindow};

    // ── Fixture helpers ─────────────────────────────────────────────────────

    /// Build a `DateTime<Local>` for a known-Monday date (2024-01-08) at the
    /// given HH:MM.
    ///
    /// The result is independent of the test machine's timezone: the naive
    /// wall-clock values (weekday, hour, minute) are interpreted as "Local"
    /// time, so `now.time()` and `now.weekday()` return exactly what the
    /// literal says regardless of the machine's UTC offset.
    fn monday_at(hour: u32, minute: u32) -> DateTime<Local> {
        // 2024-01-08 is a Monday (verified: python3 -c "import datetime; print(datetime.date(2024,1,8).strftime('%A'))" → Monday)
        fixed_at(2024, 1, 8, hour, minute) // Monday
    }

    /// Like `monday_at`, but for Wednesday 2024-01-10.
    fn wednesday_at(hour: u32, minute: u32) -> DateTime<Local> {
        fixed_at(2024, 1, 10, hour, minute) // Wednesday
    }

    /// Build a `DateTime<Local>` for any ymd/hms whose `.time()` and
    /// `.weekday()` return the literal values supplied.
    ///
    /// Uses `NaiveDateTime::and_local_timezone(Local)` to interpret the naive
    /// datetime *as* local wall-clock time rather than UTC, so the result is
    /// independent of the test machine's UTC offset.
    ///
    /// # Panics
    /// Panics if the given ymd/hms is invalid or ambiguous in the local timezone
    /// (DST spring-forward gap). All test call-sites use unambiguous, valid dates.
    fn fixed_at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        let naive = NaiveDate::from_ymd_opt(year, month, day)
            .unwrap()
            .and_hms_opt(hour, minute, 0)
            .unwrap();
        // Interpret the naive datetime as local wall-clock time.
        // This ensures now.time() == NaiveTime { hour, minute, 0 } and
        // now.weekday() == the weekday of the given date, on any machine.
        Local.from_local_datetime(&naive).earliest().unwrap()
    }

    /// Build a `TrackingWindow` for use in tests (no label, panics on invalid input
    /// only if A.3 validation rejects it — all test windows here are valid).
    fn window(start: &str, end: &str, days: Vec<Weekday>) -> TrackingWindow {
        TrackingWindow {
            start: start.to_string(),
            end: end.to_string(),
            days_of_week: days,
            label: String::new(),
        }
    }

    /// Build an `AppConfig` with a custom `TrackingScheduleConfig`.
    fn cfg_with_ts(ts: TrackingScheduleConfig) -> AppConfig {
        let mut cfg = AppConfig::default_config();
        cfg.tracking_schedule = ts;
        cfg
    }

    /// Build consent with `screen_capture` set to `granted`.
    fn consent(screen_capture: bool) -> ConsentPermissions {
        ConsentPermissions {
            screen_capture,
            ..Default::default()
        }
    }

    // ── tracking_schedule_active tests ──────────────────────────────────────

    /// Test 1: disabled config → false regardless of `now`.
    #[test]
    fn disabled_config_returns_false() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: false,
            windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
            timezone: "Local".to_string(),
        });
        // The window would match Mon 12:30, but enabled=false must override.
        assert!(!tracking_schedule_active(&cfg, monday_at(12, 30)));
    }

    /// Test 2: enabled=true but empty windows → false.
    #[test]
    fn empty_windows_returns_false() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![],
            timezone: "Local".to_string(),
        });
        assert!(!tracking_schedule_active(&cfg, monday_at(12, 30)));
    }

    /// Test 3: single window in range → true.
    #[test]
    fn normal_window_in_range() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
            timezone: "Local".to_string(),
        });
        // Mon 12:30 is inside [12:00, 13:00) on Monday.
        assert!(tracking_schedule_active(&cfg, monday_at(12, 30)));
    }

    /// Test 4: single window, `now` is outside range → false.
    #[test]
    fn normal_window_out_of_range() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
            timezone: "Local".to_string(),
        });
        // Mon 13:01 is past the window end.
        assert!(!tracking_schedule_active(&cfg, monday_at(13, 1)));
    }

    /// Test 5: multiple windows, one matches → true.
    #[test]
    fn multiple_windows_one_matches() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![
                window("12:00", "13:00", vec![Weekday::Mon]),
                window("18:00", "22:00", vec![Weekday::Mon]),
            ],
            timezone: "Local".to_string(),
        });
        // Mon 19:00 is inside [18:00, 22:00).
        assert!(tracking_schedule_active(&cfg, monday_at(19, 0)));
    }

    /// Test 6: multiple windows, none match → false.
    #[test]
    fn multiple_windows_none_match() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![
                window("12:00", "13:00", vec![Weekday::Mon]),
                window("18:00", "22:00", vec![Weekday::Mon]),
            ],
            timezone: "Local".to_string(),
        });
        // Mon 10:00 is before both windows.
        assert!(!tracking_schedule_active(&cfg, monday_at(10, 0)));
    }

    /// Test 7: timezone = "Local" smoke-test — function must not panic.
    #[test]
    fn timezone_local_uses_chrono_local() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![window(
                "00:00",
                "23:59",
                vec![
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri,
                    Weekday::Sat,
                    Weekday::Sun,
                ],
            )],
            timezone: "Local".to_string(),
        });
        // With a window covering virtually all hours on all days, the result
        // must be `true` when the function runs. We just verify it doesn't
        // panic and returns a bool.
        let result = tracking_schedule_active(&cfg, monday_at(12, 0));
        assert!(
            result,
            "all-day window on all days must be active at Mon 12:00"
        );
    }

    // ── capture_permitted_now tests ─────────────────────────────────────────

    // ── 16-row truth table (Test 8) ────────────────────────────────────────

    /// Test 8: All 2⁴ combinations of the four gates.
    ///
    /// Each combination is constructed by building an `AppConfig` that puts
    /// each gate into the desired state, then asserting that
    /// `capture_permitted_now` returns `consent AND active_hours AND ts_inactive
    /// AND !capture_paused`.
    ///
    /// Gate construction:
    ///   - `consent`: `ConsentPermissions { screen_capture: true/false, .. }`
    ///   - `active_hours`: schedule.active_hours_enabled = true, window covers
    ///     the test `now` (Mon 12:30 / Wed 23:00 accordingly)
    ///   - `ts_inactive` (= !tracking_schedule_active): TS enabled with a window
    ///     that does NOT cover `now` → active = false → ts_inactive = true
    ///     OR TS disabled / empty → ts_inactive = true
    ///   - `capture_paused`: passed directly as argument
    #[test]
    fn capture_permitted_combines_all_four_gates() {
        // Fixed test instant: Monday 12:30.
        // For each combo we build cfg to achieve the desired gate state.
        let now = monday_at(12, 30);

        // Iterate all 16 (bool×bool×bool×bool) combinations.
        for consent_val in [true, false] {
            for active_hours_val in [true, false] {
                for ts_inactive_val in [true, false] {
                    for paused_val in [true, false] {
                        let cfg = build_scenario_cfg(active_hours_val, ts_inactive_val, now);
                        let c = consent(consent_val);
                        let expected =
                            consent_val && active_hours_val && ts_inactive_val && !paused_val;

                        let got = capture_permitted_now(&cfg, &c, paused_val, now);

                        assert_eq!(
                            got, expected,
                            "combo (consent={consent_val}, active_hours={active_hours_val}, \
                             ts_inactive={ts_inactive_val}, paused={paused_val}) expected \
                             {expected} but got {got}"
                        );
                    }
                }
            }
        }
    }

    /// Builds an `AppConfig` that puts active_hours and ts_inactive gates into
    /// the desired states for the given test `now`.
    ///
    /// - `active_hours=true`: schedule.active_hours_enabled + window covers `now`'s hour.
    /// - `active_hours=false`: schedule.active_hours_enabled + window does NOT cover `now`.
    /// - `ts_inactive=true` (TS not firing): TS disabled (enabled=false).
    /// - `ts_inactive=false` (TS firing): TS enabled with a window covering `now`.
    fn build_scenario_cfg(
        active_hours_val: bool,
        ts_inactive_val: bool,
        now: DateTime<Local>,
    ) -> AppConfig {
        // now = Monday 12:30 in all callers from the truth table.
        let hour = now.time().hour() as u8;

        let mut cfg = AppConfig::default_config();

        // ── Active hours gate ─────────────────────────────────────────────
        if active_hours_val {
            // Enable, window covers `now` (12:00–13:00).
            cfg.schedule.active_hours_enabled = true;
            cfg.schedule.active_start_hour = hour; // e.g. 12
            cfg.schedule.active_end_hour = hour + 1; // e.g. 13
            cfg.schedule.active_days = vec![Weekday::Mon];
        } else {
            // Enable, but window does NOT cover `now` (e.g. 14:00–15:00).
            cfg.schedule.active_hours_enabled = true;
            cfg.schedule.active_start_hour = hour + 2; // e.g. 14
            cfg.schedule.active_end_hour = hour + 3; // e.g. 15
            cfg.schedule.active_days = vec![Weekday::Mon];
        }

        // ── Tracking schedule gate ────────────────────────────────────────
        if ts_inactive_val {
            // TS not firing: disabled entirely.
            cfg.tracking_schedule = TrackingScheduleConfig {
                enabled: false,
                windows: vec![],
                timezone: "Local".to_string(),
            };
        } else {
            // TS firing: enabled with a window that covers `now` (12:00–13:00 Mon).
            cfg.tracking_schedule = TrackingScheduleConfig {
                enabled: true,
                windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
                timezone: "Local".to_string(),
            };
        }

        cfg
    }

    /// Test 9: overnight active_hours (22:00–06:00, Mon–Fri) + empty TS at
    /// Wed 23:00 → capture_permitted_now returns `true`.
    ///
    /// This exercises the post-CONS-C05 fix where `should_run_now_with_time`
    /// must correctly wrap an overnight window. Consent is granted,
    /// capture_paused is false, TS is disabled.
    #[test]
    fn capture_permitted_respects_should_run_now_wrap() {
        // Wed 23:00 is inside the overnight window [22:00, 06:00) on Wed.
        let now = wednesday_at(23, 0);

        let mut cfg = AppConfig::default_config();
        // Overnight active_hours window: 22:00–06:00 on Mon–Fri.
        // A.5's should_run_now_with_time must handle end_hour < start_hour wrap.
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 22;
        cfg.schedule.active_end_hour = 6; // overnight wrap: 22 > 6
        cfg.schedule.active_days = vec![
            Weekday::Mon,
            Weekday::Tue,
            Weekday::Wed,
            Weekday::Thu,
            Weekday::Fri,
        ];
        // TS disabled — not firing.
        cfg.tracking_schedule = TrackingScheduleConfig::default();

        let c = consent(true); // consent granted
        let capture_paused = false;

        assert!(
            capture_permitted_now(&cfg, &c, capture_paused, now),
            "Wed 23:00 with overnight 22-06 active_hours + no TS + consent granted \
             must be permitted (CONS-C05 overnight wrap)"
        );
    }

    /// Test 10: consent revoked overrides TS inactive + active_hours active
    /// (CONS-PC02 — consent has top-authority veto).
    #[test]
    fn consent_revoked_overrides_ts_inactive_active_hours() {
        let now = monday_at(12, 30);
        let mut cfg = AppConfig::default_config();
        // Active hours cover now.
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 12;
        cfg.schedule.active_end_hour = 13;
        cfg.schedule.active_days = vec![Weekday::Mon];
        // TS not firing.
        cfg.tracking_schedule = TrackingScheduleConfig::default();

        // Consent revoked: screen_capture = false.
        let c = consent(false);
        let capture_paused = false;

        assert!(
            !capture_permitted_now(&cfg, &c, capture_paused, now),
            "consent revoked (screen_capture=false) must veto capture even when \
             active_hours and TS are both permitting (CONS-PC02)"
        );
    }

    /// Test 11: capture_paused veto — even when TS inactive + active_hours +
    /// consent granted, capture_paused=true → false (CONS-PC02).
    #[test]
    fn capture_paused_overrides_ts_inactive() {
        let now = monday_at(12, 30);
        let mut cfg = AppConfig::default_config();
        // All other gates permitting.
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 12;
        cfg.schedule.active_end_hour = 13;
        cfg.schedule.active_days = vec![Weekday::Mon];
        cfg.tracking_schedule = TrackingScheduleConfig::default();

        let c = consent(true); // consent granted
        let capture_paused = true; // user tray-toggle veto

        assert!(
            !capture_permitted_now(&cfg, &c, capture_paused, now),
            "capture_paused=true must veto capture even when all other gates permit (CONS-PC02)"
        );
    }

    // ── Clock-irregularity coverage (CONS-PI07 / spec §3.7a) ──────────────

    /// Test 12a: suspend/resume — `tracking_schedule_active` returns correct
    /// value regardless of tick timing between suspend/resume. Tested via two
    /// distinct `now` values flanking a suspend interval.
    #[test]
    fn window_active_across_suspend() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
            timezone: "Local".to_string(),
        });

        // Pre-suspend: Mon 12:10 — inside window.
        let pre_suspend = monday_at(12, 10);
        assert!(
            tracking_schedule_active(&cfg, pre_suspend),
            "Mon 12:10 must be inside window before suspend"
        );

        // Post-resume: Mon 13:10 — outside window (machine was suspended 60 min).
        let post_resume = monday_at(13, 10);
        assert!(
            !tracking_schedule_active(&cfg, post_resume),
            "Mon 13:10 must be outside window after suspend/resume (CONS-PI07)"
        );
    }

    /// Test 12b: forward clock jump into future window.
    ///
    /// Clock jumps from Mon 11:50 to Mon 12:30 (window [12:00, 13:00)).
    /// Returns `true` after jump.
    #[test]
    fn forward_clock_jump_into_future_window() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
            timezone: "Local".to_string(),
        });

        // Before jump: Mon 11:50 — outside window.
        let before_jump = monday_at(11, 50);
        assert!(
            !tracking_schedule_active(&cfg, before_jump),
            "Mon 11:50 must be outside [12:00, 13:00) before clock jump"
        );

        // After jump: Mon 12:30 — inside window.
        let after_jump = monday_at(12, 30);
        assert!(
            tracking_schedule_active(&cfg, after_jump),
            "Mon 12:30 must be inside [12:00, 13:00) after forward clock jump (CONS-PI07)"
        );
    }

    /// Test 12c: forward clock jump past window end.
    ///
    /// Clock jumps from Mon 12:50 to Mon 13:10.
    /// Returns `false` after jump (window [12:00, 13:00) has ended).
    #[test]
    fn forward_clock_jump_past_window_end() {
        let cfg = cfg_with_ts(TrackingScheduleConfig {
            enabled: true,
            windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
            timezone: "Local".to_string(),
        });

        // Before jump: Mon 12:50 — inside window.
        let before_jump = monday_at(12, 50);
        assert!(
            tracking_schedule_active(&cfg, before_jump),
            "Mon 12:50 must be inside [12:00, 13:00) before clock jump"
        );

        // After jump: Mon 13:10 — outside window (jumped past end).
        let after_jump = monday_at(13, 10);
        assert!(
            !tracking_schedule_active(&cfg, after_jump),
            "Mon 13:10 must be outside [12:00, 13:00) after forward clock jump past end (CONS-PI07)"
        );
    }

    // ── Additional edge-case tests ──────────────────────────────────────────

    /// Test 13: all four gates false → false.
    #[test]
    fn all_gates_false_returns_false() {
        let now = monday_at(12, 30);
        // active_hours: enabled but window does NOT cover now (14-15).
        let mut cfg = AppConfig::default_config();
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 14;
        cfg.schedule.active_end_hour = 15;
        cfg.schedule.active_days = vec![Weekday::Mon];
        // TS: enabled with window covering now → ts_active = true → ts_inactive = false.
        cfg.tracking_schedule = TrackingScheduleConfig {
            enabled: true,
            windows: vec![window("12:00", "13:00", vec![Weekday::Mon])],
            timezone: "Local".to_string(),
        };

        let c = consent(false); // revoked
        let capture_paused = true;

        assert!(
            !capture_permitted_now(&cfg, &c, capture_paused, now),
            "all four gates false must return false"
        );
    }

    /// Test 14: all four gates true → true.
    #[test]
    fn all_gates_true_returns_true() {
        let now = monday_at(12, 30);
        let mut cfg = AppConfig::default_config();
        // active_hours covers now.
        cfg.schedule.active_hours_enabled = true;
        cfg.schedule.active_start_hour = 12;
        cfg.schedule.active_end_hour = 13;
        cfg.schedule.active_days = vec![Weekday::Mon];
        // TS not firing (disabled).
        cfg.tracking_schedule = TrackingScheduleConfig::default();

        let c = consent(true); // granted
        let capture_paused = false;

        assert!(
            capture_permitted_now(&cfg, &c, capture_paused, now),
            "all four gates true must return true"
        );
    }

    // ── evaluate_and_notify_transitions tests (A.18) ────────────────────────

    /// Mock TsNotifier that records all (title, body) pairs sent to it.
    struct RecordingNotifier {
        calls: std::sync::Mutex<Vec<(String, String)>>,
    }

    impl RecordingNotifier {
        fn new() -> Self {
            Self {
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn recorded(&self) -> Vec<(String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl TsNotifier for RecordingNotifier {
        async fn notify_ts(&self, title: &str, body: &str) {
            self.calls
                .lock()
                .unwrap()
                .push((title.to_string(), body.to_string()));
        }
    }

    /// Build an `AppConfig` with `notification.tracking_schedule_enabled` set.
    fn notif_cfg(enabled: bool) -> AppConfig {
        let mut cfg = AppConfig::default_config();
        cfg.notification.tracking_schedule_enabled = enabled;
        cfg
    }

    /// Test A.18-1: transition false → true fires "Tracking Schedule Active".
    #[tokio::test]
    async fn notifier_fires_on_ts_enter() {
        let cfg = notif_cfg(true);
        let notifier = RecordingNotifier::new();
        let mut last: Option<Instant> = None;

        evaluate_and_notify_transitions(&cfg, false, true, &mut last, Some(&notifier)).await;

        let calls = notifier.recorded();
        assert_eq!(
            calls.len(),
            1,
            "expected exactly one notification on TS enter"
        );
        assert_eq!(calls[0].0, "Tracking Schedule Active");
        assert!(last.is_some(), "last_notified_at must be set after firing");
    }

    /// Test A.18-2: transition true → false fires "Tracking Schedule Ended".
    #[tokio::test]
    async fn notifier_fires_on_ts_exit() {
        let cfg = notif_cfg(true);
        let notifier = RecordingNotifier::new();
        let mut last: Option<Instant> = None;

        evaluate_and_notify_transitions(&cfg, true, false, &mut last, Some(&notifier)).await;

        let calls = notifier.recorded();
        assert_eq!(
            calls.len(),
            1,
            "expected exactly one notification on TS exit"
        );
        assert_eq!(calls[0].0, "Tracking Schedule Ended");
    }

    /// Test A.18-3: second transition within 60s is suppressed by the debounce.
    ///
    /// Simulates a backward clock-jump / DST flip-flop: first notification fires,
    /// then a second immediate transition (within the debounce window) is suppressed.
    #[tokio::test]
    async fn notifier_debounces_within_60s() {
        let cfg = notif_cfg(true);
        let notifier = RecordingNotifier::new();

        // Prime last_notified_at to "just now" — debounce should block next fire.
        let mut last: Option<Instant> = Some(Instant::now());

        // Immediately attempt a transition (0 secs elapsed — well under 60s).
        evaluate_and_notify_transitions(&cfg, false, true, &mut last, Some(&notifier)).await;

        let calls = notifier.recorded();
        assert!(
            calls.is_empty(),
            "second notification within 60s must be suppressed by debounce; got {calls:?}"
        );
    }

    /// Test A.18-4: `notification.tracking_schedule_enabled = false` → no notifications.
    #[tokio::test]
    async fn notifier_does_not_fire_when_config_disabled() {
        let cfg = notif_cfg(false);
        let notifier = RecordingNotifier::new();
        let mut last: Option<Instant> = None;

        // Both enter and exit transitions attempted.
        evaluate_and_notify_transitions(&cfg, false, true, &mut last, Some(&notifier)).await;
        evaluate_and_notify_transitions(&cfg, true, false, &mut last, Some(&notifier)).await;

        let calls = notifier.recorded();
        assert!(
            calls.is_empty(),
            "no notifications must fire when tracking_schedule_enabled=false; got {calls:?}"
        );
        assert!(
            last.is_none(),
            "last_notified_at must remain None when config disabled"
        );
    }
}
