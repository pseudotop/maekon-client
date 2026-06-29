// OOS-TBD: ADR-013 file split (cycle 37+) — LOC: 880
//! Tracking-schedule scheduler helpers — Phase 9 PR-A.
//!
//! This module exposes two pure functions consumed by the scheduler:
//!
//! * [`tracking_schedule_active`] — returns `true` when the current instant
//!   falls inside any configured tracking-schedule window.
//!
//! * [`capture_permitted_now`] — composes the capture privacy gates:
//!   `capture_enabled AND consent_granted AND active_hours AND !tracking_schedule_active AND !capture_paused`.
//!
//! * [`audio_capture_permitted_now`] — identical gate for microphone capture;
//!   uses `cfg.audio.enabled` instead of `cfg.vision.capture_enabled` (ADR-075 P-3).
//!
//! * [`evaluate_and_notify_transitions`] — fires a desktop notification when the
//!   tracking-schedule window is entered or exited, with a 60-second debounce guard
//!   to suppress flip-flop storms from DST edges or backward clock jumps (A.18).
//!
//! A.5 implements the real logic for both functions.

use async_trait::async_trait;
use chrono::{DateTime, Local};
use maekon_core::config::AppConfig;
use maekon_core::consent::ConsentPermissions;
use std::time::Instant;

// ── TsNotifier — narrow port for tracking-schedule notifications ─────────────

/// Narrow port that emits tracking-schedule transition notifications.
///
/// [`NotificationManager`] implements this trait, so it is used directly in
/// production. Tests use the [`RecordingNotifier`] mock implementation.
///
/// [`NotificationManager`]: crate::notification_manager::NotificationManager
#[async_trait]
pub(crate) trait TsNotifier: Send + Sync {
    /// Sends a desktop notification with the given title and body.
    /// Notification-library failures are ignored (best-effort).
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

/// Shared composite-gate body, parameterized over the per-mode enable flag AND
/// the per-mode consent term.
///
/// `enabled` is `cfg.vision.capture_enabled` for screen/vision capture or
/// `cfg.audio.enabled` for microphone capture. `consent_granted` is
/// `consent.screen_capture` for the screen gate or `consent.microphone` for the
/// audio gate (#4568: the mic has its own consent — screen consent must never
/// silently authorize it). These two terms are the ONLY ones that differ between
/// the gates (ADR-075 P-3: parameterize, don't duplicate).
fn capture_permitted_now_inner(
    cfg: &AppConfig,
    enabled: bool,
    consent_granted: bool,
    capture_paused: bool,
    now: DateTime<Local>,
) -> bool {
    enabled
        && consent_granted
        && crate::scheduler::should_run_now_with_time(cfg, now)
        && !tracking_schedule_active(cfg, now)
        && !capture_paused
}

/// Screen/vision capture privacy gate (enable term = `cfg.vision.capture_enabled`).
///
/// Composes the capture privacy gates per spec §3.4 (CONS-PC02):
///
/// ```text
/// capture_permitted_now =
///     cfg.vision.capture_enabled                 // user-visible capture toggle
///     AND consent.screen_capture                 // consent top-authority gate
///     AND should_run_now_with_time(cfg, now)     // active_hours gate
///     AND !tracking_schedule_active(cfg, now)    // tracking-schedule negative gate
///     AND !capture_paused                         // user tray-toggle veto
/// ```
///
/// All gates must be true for capture to be permitted. Any single `false`
/// short-circuits and returns `false` regardless of the other gate values.
///
/// # Plan deviation note
/// The original plan spec §3.4 cited `consent.allows_tier(ConsentTier::Capture)`
/// but that method does not exist in `maekon-core`. The actual API exposes the
/// field `ConsentPermissions.screen_capture: bool`, confirmed during A.4. This
/// implementation uses the field directly.
pub(crate) fn capture_permitted_now(
    cfg: &AppConfig,
    consent: &ConsentPermissions,
    capture_paused: bool,
    now: DateTime<Local>,
) -> bool {
    capture_permitted_now_inner(
        cfg,
        cfg.vision.capture_enabled,
        consent.screen_capture,
        capture_paused,
        now,
    )
}

/// Microphone capture privacy gate (enable term = `cfg.audio.enabled`).
///
/// Identical to [`capture_permitted_now`] except for the enable flag: audio is
/// opt-in (`audio.enabled` defaults false) and decoupled from the vision toggle.
pub(crate) fn audio_capture_permitted_now(
    cfg: &AppConfig,
    consent: &ConsentPermissions,
    capture_paused: bool,
    now: DateTime<Local>,
) -> bool {
    capture_permitted_now_inner(
        cfg,
        cfg.audio.enabled,
        consent.microphone,
        capture_paused,
        now,
    )
}

/// Returns `true` when screen/vision capture is permitted after the optional
/// battery-saver gate.
///
/// This preserves the existing four-gate behavior unless
/// `schedule.pause_on_battery_saver` is enabled and the platform reports an
/// active battery-saver / low-battery state.
pub(crate) fn capture_permitted_now_with_power(
    cfg: &AppConfig,
    consent: &ConsentPermissions,
    capture_paused: bool,
    battery_saver_active: bool,
    now: DateTime<Local>,
) -> bool {
    capture_permitted_now(cfg, consent, capture_paused, now)
        && !(cfg.schedule.pause_on_battery_saver && battery_saver_active)
}

/// Returns `true` when microphone capture is permitted after the optional
/// battery-saver gate.
///
/// Mirrors [`capture_permitted_now_with_power`] for microphone capture.
pub(crate) fn audio_capture_permitted_now_with_power(
    cfg: &AppConfig,
    consent: &ConsentPermissions,
    capture_paused: bool,
    battery_saver_active: bool,
    now: DateTime<Local>,
) -> bool {
    audio_capture_permitted_now(cfg, consent, capture_paused, now)
        && !(cfg.schedule.pause_on_battery_saver && battery_saver_active)
}

// ── evaluate_and_notify_transitions ─────────────────────────────────────────

/// Detects entry/exit transitions of the tracking-schedule window and emits a
/// desktop notification.
///
/// # Behavior
///
/// 1. If `cfg.notification.tracking_schedule_enabled` is `false`, return
///    immediately (no-op).
/// 2. If `prev_active == now_active`, there is no transition — return immediately.
/// 3. If less than 60 seconds have elapsed since the last notification, debounce
///    and return immediately. This prevents flip-flop storms caused by DST
///    boundaries or backward clock jumps.
/// 4. Once the above conditions pass, update `last_notified_at` to the current
///    `Instant` and emit the notification through `notifier`.
///
/// If `notifier` is `None`, only the state is updated, with no notification.
///
/// # Arguments
///
/// * `cfg` — current app config (for the notification toggle and schedule check)
/// * `prev_active` — tracking-schedule active state from the previous tick
/// * `now_active` — tracking-schedule active state for this tick
/// * `last_notified_at` — timestamp of the last notification (60s debounce state);
///   shared across both enter and exit
/// * `notifier` — notification-emitting implementation (optional)
pub(crate) async fn evaluate_and_notify_transitions<N: TsNotifier>(
    cfg: &AppConfig,
    prev_active: bool,
    now_active: bool,
    last_notified_at: &mut Option<Instant>,
    notifier: Option<&N>,
) {
    // Gate 1: no-op when the notification setting is disabled
    if !cfg.notification.tracking_schedule_enabled {
        return;
    }
    // Gate 2: no-op when there is no transition
    if prev_active == now_active {
        return;
    }
    // Gate 3: 60s debounce — suppress if less than 60s since the last notification
    let now = Instant::now();
    if let Some(last) = *last_notified_at {
        if now.duration_since(last).as_secs() < 60 {
            return;
        }
    }
    // Update state + emit notification
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

/// Tracking-schedule notification evaluation wrapper, called on every monitor tick.
///
/// Takes a `config_manager` snapshot, evaluates `tracking_schedule_active`, then
/// calls `evaluate_and_notify_transitions`. The inline block is extracted into this
/// function to stay within the monitor-loop closure size limit (500 lines)
/// (monitor-loop-size hook).
pub(super) async fn tick_ts_notifications(
    // #6830/#4796: take the already-computed per-tick `Arc<AppConfig>` snapshot
    // (cheap clone) instead of a fresh `ConfigManager::get()` (a FULL deep clone
    // of AppConfig on every 1s tick, incl. idle) — restoring the "idle tick → 0
    // deep clone" invariant #4796 claimed but this helper never honored.
    config: Option<&maekon_core::config::AppConfig>,
    notifier: Option<&crate::notification_manager::NotificationManager>,
    prev_ts_active: &mut bool,
    last_ts_notified_at: &mut Option<std::time::Instant>,
) {
    let default_cfg;
    let cfg: &maekon_core::config::AppConfig = match config {
        Some(cfg) => cfg,
        None => {
            default_cfg = maekon_core::config::AppConfig::default_config();
            &default_cfg
        }
    };
    let now_active = tracking_schedule_active(cfg, chrono::Local::now());
    evaluate_and_notify_transitions(
        cfg,
        *prev_ts_active,
        now_active,
        last_ts_notified_at,
        notifier,
    )
    .await;
    *prev_ts_active = now_active;
}

// ── Monitor power-cadence (idle backoff + battery throttle) ──────────────────
//
// #4795 (idle adaptive backoff) + #4798 (battery-saver throttle).
//
// The monitor loop runs at a fixed ~1s cadence, performing osascript
// (`collect_context`) + accessibility (AX) extraction on every tick. It runs at
// full cadence even when the user is idle or in battery-saver mode, wasting power.
//
// This decision function is *pure* (stateless) — the caller (monitor loop) holds a
// `tick_counter` that increments once per second and asks this function on every
// tick "should this tick run the expensive work?". We do NOT build an
// adaptive-interval framework applicable to all 16 loops (YAGNI — monitor is the
// only loop that runs osascript at 1s).

/// No-input threshold (seconds) at which we start treating the session as idle.
/// Distinct from IdleTracker's 5-minute idle threshold: power backoff must
/// increase the cadence after much shorter inactivity, so it uses a dedicated
/// threshold.
pub(super) const MONITOR_IDLE_BACKOFF_SECS: u64 = 30;

/// Returns true when the carried full-res RGBA frame + its paired OCR regions can
/// be released. Once no-input idle backoff engages, no clicks will consume them
/// (`run_gui_tick` consumes the frame_rgba AND the OCR-region click-correlation
/// only inside the click branch, `click_count > 0`), so dropping them is
/// observationally invisible to correctness and bounds the worst-case resident
/// footprint (a 4K RGBA frame is ~33 MB held across idle ticks). Keyed off the
/// SAME threshold `decide_monitor_tick` uses for idle, so the release point and the
/// idle-backoff point cannot silently diverge (#6830).
pub(super) fn should_release_idle_frame(idle_secs: u64) -> bool {
    idle_secs >= MONITOR_IDLE_BACKOFF_SECS
}

/// Cadence (seconds) for processing expensive monitor ticks while idle. That is,
/// while idle we only do osascript/AX work once every 5 seconds.
pub(super) const MONITOR_IDLE_CADENCE_SECS: u64 = 5;

/// Cadence (seconds) for processing expensive monitor ticks in battery-saver mode.
/// More conservative than idle backoff — the <3% battery budget is unmeasured, so
/// we don't increase it too aggressively.
pub(super) const MONITOR_BATTERY_CADENCE_SECS: u64 = 3;

/// Power-aware cadence decision for a single monitor tick.
///
/// - `process`: whether this tick should run `collect_context` (osascript) + the
///   full context/capture/analysis pipeline. When `false`, the caller skips the tick.
/// - `skip_expensive`: whether, even when processing, to omit the most expensive
///   optional blocks (AX extraction / GUI-LLM feedback). `true` in battery-saver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MonitorTickDecision {
    pub process: bool,
    pub skip_expensive: bool,
}

/// Decides whether to process this monitor tick, from `tick_counter` (increments
/// by 1 each second), the current `idle_secs`, and the battery-saver flag.
///
/// # Edge-restore (latency)
/// When an input edge (idle → active) occurs and `idle_secs` drops below the
/// backoff threshold, full 1s cadence is restored immediately (since the decision
/// is based solely on `idle_secs` with no separate state, the first tick right
/// after the edge yields `process=true`). The cadence phase is determined by
/// `tick_counter` modulo, so a return to activity is never delayed by more than one
/// tick even during backoff.
///
/// When battery-saver and idle apply simultaneously, the longer cadence (the larger
/// of the two periods) is used, but `skip_expensive` is always `true` under
/// battery-saver.
pub(super) fn decide_monitor_tick(
    tick_counter: u64,
    idle_secs: u64,
    battery_saver: bool,
) -> MonitorTickDecision {
    // Active (below the no-input threshold) + AC power → full cadence, expensive work included.
    let idle = idle_secs >= MONITOR_IDLE_BACKOFF_SECS;

    // Select the cadence period to apply (the longer one when both apply).
    let cadence = match (idle, battery_saver) {
        (false, false) => 1,
        (true, false) => MONITOR_IDLE_CADENCE_SECS,
        (false, true) => MONITOR_BATTERY_CADENCE_SECS,
        (true, true) => MONITOR_IDLE_CADENCE_SECS.max(MONITOR_BATTERY_CADENCE_SECS),
    };

    // cadence == 1 → process every tick (phase irrelevant).
    let process = cadence <= 1 || tick_counter.is_multiple_of(cadence);

    MonitorTickDecision {
        process,
        // Under battery-saver, omit the most expensive optional blocks (AX/GUI-LLM).
        skip_expensive: battery_saver,
    }
}

// ── TsNotifier impl for NotificationManager ──────────────────────────────────

// `NotificationManager` implements `TsNotifier`, so it is passed directly from the
// monitor loop. This impl lives inside the helper rather than the
// notification_manager crate module so that it does not cross the `loops` module's
// private boundary.
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
    use maekon_core::config::Weekday;
    use maekon_core::config::{TrackingScheduleConfig, TrackingWindow};

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

    /// Build consent with `screen_capture` and `microphone` set independently.
    ///
    /// The screen gate (`capture_permitted_now`) reads `screen_capture`; the audio
    /// gate (`audio_capture_permitted_now`) reads `microphone` — the two are
    /// independent consent terms (#4568), so tests must drive them separately.
    fn consent(screen_capture: bool, microphone: bool) -> ConsentPermissions {
        ConsentPermissions {
            screen_capture,
            microphone,
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

    /// Test 8: All 2⁴ combinations of the legacy schedule/consent gates.
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
                        // Screen gate reads screen_capture; mic mirrors it here (irrelevant to this gate).
                        let c = consent(consent_val, consent_val);
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

    #[test]
    fn capture_permitted_respects_capture_enabled_setting() {
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now);
        cfg.vision.capture_enabled = false;

        assert!(
            !capture_permitted_now(&cfg, &consent(true, true), false, now),
            "capture_enabled=false must veto capture even when consent, active hours, tracking schedule, and pause gates allow it"
        );
    }

    // ── audio_capture_permitted_now tests ───────────────────────────────────

    #[test]
    fn audio_capture_permitted_respects_audio_enabled_setting() {
        // The audio gate's veto term is audio.enabled — NOT vision.capture_enabled.
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now); // active hours cover now; TS inactive
        cfg.audio.enabled = false;
        assert!(
            !audio_capture_permitted_now(&cfg, &consent(true, true), false, now),
            "audio.enabled=false must veto audio capture even when all other gates allow"
        );
    }

    #[test]
    fn audio_capture_permitted_allows_when_all_pass() {
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now);
        cfg.audio.enabled = true;
        assert!(
            audio_capture_permitted_now(&cfg, &consent(true, true), false, now),
            "audio capture must be permitted when audio.enabled + microphone consent + hours + ts-inactive + not-paused"
        );
    }

    #[test]
    fn audio_capture_permitted_decoupled_from_vision_capture_enabled() {
        // The core fix: audio must NOT be coupled to the vision/screen toggle.
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now);
        cfg.audio.enabled = true;
        cfg.vision.capture_enabled = false; // vision off must NOT block audio
        assert!(
            audio_capture_permitted_now(&cfg, &consent(true, true), false, now),
            "audio.enabled=true must permit audio even when vision.capture_enabled=false (decoupling)"
        );
        cfg.audio.enabled = false;
        cfg.vision.capture_enabled = true; // vision on must NOT enable audio
        assert!(
            !audio_capture_permitted_now(&cfg, &consent(true, true), false, now),
            "audio.enabled=false must deny audio even when vision.capture_enabled=true"
        );
    }

    #[test]
    fn audio_capture_permitted_denies_without_consent() {
        // The microphone consent term — NOT screen_capture — gates audio (#4568).
        // screen_capture=true here proves screen consent does NOT authorize the mic.
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now);
        cfg.audio.enabled = true;
        assert!(
            !audio_capture_permitted_now(&cfg, &consent(true, false), false, now),
            "audio capture must be denied without microphone consent (even with screen_capture granted)"
        );
    }

    #[test]
    fn audio_capture_permitted_denies_when_paused() {
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now);
        cfg.audio.enabled = true;
        assert!(
            !audio_capture_permitted_now(&cfg, &consent(true, true), true, now),
            "audio capture must be denied when capture_paused=true"
        );
    }

    #[test]
    fn audio_capture_permitted_with_power_denies_on_battery_saver() {
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now);
        cfg.audio.enabled = true;
        cfg.schedule.pause_on_battery_saver = true;
        assert!(
            !audio_capture_permitted_now_with_power(&cfg, &consent(true, true), false, true, now),
            "audio capture must be denied during battery-saver when pause_on_battery_saver=true"
        );
        assert!(
            audio_capture_permitted_now_with_power(&cfg, &consent(true, true), false, false, now),
            "audio capture must be permitted when battery-saver inactive"
        );
    }

    /// Bidirectional independence (F4 proof, #4568): screen and microphone consent
    /// terms are fully decoupled. screen-only grant permits screen but NOT audio;
    /// mic-only grant permits audio but NOT screen.
    #[test]
    fn screen_and_microphone_consent_are_independent() {
        let now = monday_at(12, 30);
        let mut cfg = build_scenario_cfg(true, true, now);
        cfg.vision.capture_enabled = true;
        cfg.audio.enabled = true;

        // screen=true, mic=false → screen permitted, audio denied.
        let screen_only = consent(true, false);
        assert!(
            capture_permitted_now(&cfg, &screen_only, false, now),
            "screen consent (mic=false) must permit screen capture"
        );
        assert!(
            !audio_capture_permitted_now(&cfg, &screen_only, false, now),
            "screen consent must NOT authorize the mic (mic=false → audio denied)"
        );

        // mic=true, screen=false → audio permitted, screen denied (mirror).
        let mic_only = consent(false, true);
        assert!(
            audio_capture_permitted_now(&cfg, &mic_only, false, now),
            "microphone consent (screen=false) must permit audio capture"
        );
        assert!(
            !capture_permitted_now(&cfg, &mic_only, false, now),
            "microphone consent must NOT authorize screen capture (screen=false → screen denied)"
        );
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

        let c = consent(true, true); // consent granted
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
        let c = consent(false, false);
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

        let c = consent(true, true); // consent granted
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

        let c = consent(false, false); // revoked
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

        let c = consent(true, true); // granted
        let capture_paused = false;

        assert!(
            capture_permitted_now(&cfg, &c, capture_paused, now),
            "all four gates true must return true"
        );
    }

    /// POWER-001: battery saver closes the capture gate when the user enabled
    /// the battery-saver schedule option.
    #[test]
    fn battery_saver_blocks_capture_when_schedule_option_enabled() {
        let now = monday_at(12, 30);
        let mut cfg = AppConfig::default_config();
        cfg.schedule.pause_on_battery_saver = true;

        assert!(
            !capture_permitted_now_with_power(&cfg, &consent(true, true), false, true, now),
            "battery saver must block capture when pause_on_battery_saver=true"
        );
    }

    /// POWER-002: when AC / normal power is restored, the same capture gates
    /// return to full-rate behavior.
    #[test]
    fn normal_power_restores_capture_when_other_gates_permit() {
        let now = monday_at(12, 30);
        let mut cfg = AppConfig::default_config();
        cfg.schedule.pause_on_battery_saver = true;

        assert!(
            capture_permitted_now_with_power(&cfg, &consent(true, true), false, false, now),
            "normal power must restore capture when all other gates permit"
        );
    }

    /// The new power gate is opt-in: existing users keep the old behavior until
    /// `pause_on_battery_saver` is enabled.
    #[test]
    fn battery_saver_is_ignored_when_schedule_option_disabled() {
        let now = monday_at(12, 30);
        let mut cfg = AppConfig::default_config();
        cfg.schedule.pause_on_battery_saver = false;

        assert!(
            capture_permitted_now_with_power(&cfg, &consent(true, true), false, true, now),
            "battery saver must not block capture when pause_on_battery_saver=false"
        );
    }

    // ── should_release_idle_frame tests (#6830 idle frame release) ──────────

    #[test]
    fn should_release_idle_frame_false_below_threshold() {
        assert!(!should_release_idle_frame(0));
        assert!(!should_release_idle_frame(MONITOR_IDLE_BACKOFF_SECS - 1));
    }

    #[test]
    fn should_release_idle_frame_true_at_and_above_threshold() {
        assert!(should_release_idle_frame(MONITOR_IDLE_BACKOFF_SECS));
        assert!(should_release_idle_frame(MONITOR_IDLE_BACKOFF_SECS + 600));
    }

    /// Boundary-alignment guard (#6830): the frame-release predicate MUST flip at
    /// exactly the same `idle_secs` at which `decide_monitor_tick` first treats the
    /// session as idle (both keyed off `MONITOR_IDLE_BACKOFF_SECS`). If a future
    /// edit changes one idle predicate without the other, this fails — preventing a
    /// silent divergence where the frame is held through the idle-backoff window.
    #[test]
    fn frame_release_threshold_matches_idle_backoff_threshold() {
        // Just below the threshold: NOT released, and decide_monitor_tick still
        // processes every tick (active cadence).
        let below = MONITOR_IDLE_BACKOFF_SECS - 1;
        assert!(!should_release_idle_frame(below));
        assert!(
            decide_monitor_tick(1, below, false).process,
            "below threshold must still be active (process every tick)"
        );
        // At the threshold: released, and decide_monitor_tick enters idle backoff
        // (tick 1 is skipped at the idle cadence of 5).
        let at = MONITOR_IDLE_BACKOFF_SECS;
        assert!(should_release_idle_frame(at));
        assert!(
            !decide_monitor_tick(1, at, false).process,
            "at threshold must be idle backoff (tick 1 skipped at cadence 5)"
        );
    }

    // ── decide_monitor_tick tests (#4795 idle backoff + #4798 battery) ──────

    /// Active (below the no-input threshold) + AC power → process every tick,
    /// expensive work included.
    #[test]
    fn monitor_tick_active_ac_processes_every_tick() {
        for tick in 0..10 {
            let d = decide_monitor_tick(tick, 0, false);
            assert!(d.process, "active+AC must process every tick (tick={tick})");
            assert!(!d.skip_expensive, "active+AC must not skip expensive work");
        }
    }

    /// idle (at or above the no-input threshold) + AC → process only every
    /// MONITOR_IDLE_CADENCE_SECS.
    #[test]
    fn monitor_tick_idle_backs_off_to_idle_cadence() {
        let idle_secs = MONITOR_IDLE_BACKOFF_SECS; // exactly at threshold → idle
        let mut processed = 0u64;
        for tick in 0..MONITOR_IDLE_CADENCE_SECS * 4 {
            let d = decide_monitor_tick(tick, idle_secs, false);
            if d.process {
                processed += 1;
                assert_eq!(
                    tick % MONITOR_IDLE_CADENCE_SECS,
                    0,
                    "idle processing must align to cadence phase"
                );
            }
            assert!(
                !d.skip_expensive,
                "AC power must not skip expensive work even when idle"
            );
        }
        assert_eq!(
            processed, 4,
            "expected exactly 4 processed ticks over 4 cadences"
        );
    }

    /// On input edge (idle → active) return, full cadence is restored immediately
    /// (0-tick latency). Even when dropping to active at an arbitrary phase during
    /// backoff, process=true on that same tick.
    #[test]
    fn monitor_tick_input_edge_restores_full_cadence_immediately() {
        // On a tick that backoff phase would "skip" (e.g. tick=3, cadence=5),
        // when idle_secs drops below the threshold it must be processed immediately.
        let skip_tick = 3u64;
        assert!(!skip_tick.is_multiple_of(MONITOR_IDLE_CADENCE_SECS));
        let idle_d = decide_monitor_tick(skip_tick, MONITOR_IDLE_BACKOFF_SECS, false);
        assert!(
            !idle_d.process,
            "tick 3 must be skipped while idle (cadence 5)"
        );

        let active_d = decide_monitor_tick(skip_tick, 0, false);
        assert!(
            active_d.process,
            "input edge (idle_secs<threshold) must restore processing on the same tick"
        );
    }

    /// Battery-saver + active → process every MONITOR_BATTERY_CADENCE_SECS + omit
    /// expensive work.
    #[test]
    fn monitor_tick_battery_saver_throttles_and_skips_expensive() {
        let mut processed = 0u64;
        for tick in 0..MONITOR_BATTERY_CADENCE_SECS * 3 {
            let d = decide_monitor_tick(tick, 0, true);
            if d.process {
                processed += 1;
            }
            assert!(
                d.skip_expensive,
                "battery saver must skip expensive AX/GUI-LLM work"
            );
        }
        assert_eq!(
            processed, 3,
            "battery saver must process once per battery cadence"
        );
    }

    /// idle + battery-saver simultaneously → use the longer cadence (the larger of
    /// the two periods) + omit expensive work.
    #[test]
    fn monitor_tick_idle_and_battery_use_longer_cadence() {
        let longer = MONITOR_IDLE_CADENCE_SECS.max(MONITOR_BATTERY_CADENCE_SECS);
        for tick in 0..longer * 2 {
            let d = decide_monitor_tick(tick, MONITOR_IDLE_BACKOFF_SECS, true);
            assert_eq!(
                d.process,
                tick.is_multiple_of(longer),
                "combined idle+battery must use the longer cadence"
            );
            assert!(d.skip_expensive, "battery saver must skip expensive work");
        }
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
