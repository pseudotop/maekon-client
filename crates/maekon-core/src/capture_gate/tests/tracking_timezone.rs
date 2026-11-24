use super::*;

fn scheduled(zone: &str, day: Weekday, start: &str, end: &str) -> AppConfig {
    let mut cfg = cfg_with_ts(TrackingScheduleConfig {
        enabled: true,
        timezone: zone.to_owned(),
        windows: vec![window(start, end, vec![day])],
    });
    cfg.vision.capture_enabled = true;
    cfg.audio.enabled = true;
    cfg.schedule.active_hours_enabled = false;
    cfg
}

fn instant(value: &str) -> DateTime<Local> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Local)
}

fn decisions(cfg: &AppConfig, now: DateTime<Local>) -> [bool; 6] {
    let permissions = consent(true, true);
    [
        tracking_schedule_active(cfg, now),
        tracking_schedule_allows_capture(cfg, now),
        capture_permitted_now(cfg, &permissions, false, now),
        audio_capture_permitted_now(cfg, &permissions, false, now),
        capture_permitted_now_with_power(cfg, &permissions, false, false, now),
        audio_capture_permitted_now_with_power(cfg, &permissions, false, false, now),
    ]
}

fn assert_windows(cfg: &AppConfig, cases: &[(&str, bool)]) {
    for &(at, allowed) in cases {
        assert_eq!(
            decisions(cfg, instant(at)),
            [allowed; 6],
            "zone={}, instant={at}",
            cfg.tracking_schedule.timezone
        );
    }
}

#[test]
fn selected_timezone_closes_screen_and_microphone_outside_allowed_hours() {
    let cfg = scheduled("Asia/Seoul", Weekday::Mon, "09:00", "17:00");
    assert_windows(&cfg, &[("2026-11-16T10:00:00Z", false)]);
}

#[test]
fn selected_timezone_allows_screen_and_microphone_inside_allowed_hours() {
    let cfg = scheduled("Asia/Seoul", Weekday::Mon, "09:00", "17:00");
    assert_windows(&cfg, &[("2026-11-16T01:00:00Z", true)]);
}

#[test]
fn selected_timezone_keeps_start_inclusive_and_end_exclusive() {
    let cfg = scheduled("Asia/Seoul", Weekday::Mon, "09:00", "17:00");
    assert_windows(
        &cfg,
        &[
            ("2026-11-15T23:59:59.999999999Z", false),
            ("2026-11-16T00:00:00Z", true),
            ("2026-11-16T07:59:59.999999999Z", true),
            ("2026-11-16T08:00:00Z", false),
        ],
    );
}

#[test]
fn selected_timezone_uses_its_own_weekday() {
    let cfg = scheduled("Asia/Seoul", Weekday::Mon, "00:00", "01:00");
    assert_windows(
        &cfg,
        &[
            ("2026-11-15T15:30:00Z", true),
            ("2026-11-16T15:30:00Z", false),
        ],
    );
}

#[test]
fn selected_timezone_overnight_window_keeps_the_starting_weekday() {
    let cfg = scheduled("Asia/Seoul", Weekday::Mon, "22:00", "02:00");
    assert_windows(
        &cfg,
        &[
            ("2026-11-15T16:30:00Z", false),
            ("2026-11-16T13:00:00Z", true),
            ("2026-11-16T16:30:00Z", true),
            ("2026-11-16T17:00:00Z", false),
            ("2026-11-17T13:30:00Z", false),
        ],
    );
}

#[test]
fn dst_fold_allows_both_real_instants_in_the_repeated_window() {
    let cfg = scheduled("America/New_York", Weekday::Sun, "01:00", "02:00");
    assert_windows(
        &cfg,
        &[
            ("2026-11-01T05:30:00Z", true),
            ("2026-11-01T06:30:00Z", true),
            ("2026-11-01T07:00:00Z", false),
        ],
    );
}

#[test]
fn dst_spring_jump_keeps_wall_clock_window_boundaries() {
    let cfg = scheduled("America/New_York", Weekday::Sun, "01:30", "03:00");
    assert_windows(
        &cfg,
        &[
            ("2026-03-08T06:29:59Z", false),
            ("2026-03-08T06:30:00Z", true),
            ("2026-03-08T06:59:59Z", true),
            ("2026-03-08T07:00:00Z", false),
        ],
    );
    let after_jump = scheduled("America/New_York", Weekday::Sun, "03:00", "04:00");
    assert_windows(&after_jump, &[("2026-03-08T07:00:00Z", true)]);
}

#[test]
fn dst_missing_window_does_not_authorize_neighboring_real_instants() {
    let cfg = scheduled("America/New_York", Weekday::Sun, "02:00", "03:00");
    assert_windows(
        &cfg,
        &[
            ("2026-03-08T06:00:00Z", false),
            ("2026-03-08T06:30:00Z", false),
            ("2026-03-08T06:59:59Z", false),
            ("2026-03-08T07:00:00Z", false),
            ("2026-03-08T07:30:00Z", false),
        ],
    );
}

#[test]
fn quarter_hour_offset_is_not_rounded_to_an_hour() {
    let cfg = scheduled("Asia/Kathmandu", Weekday::Mon, "09:00", "17:00");
    assert_windows(
        &cfg,
        &[
            ("2026-11-16T03:14:59Z", false),
            ("2026-11-16T03:15:00Z", true),
            ("2026-11-16T11:15:00Z", false),
        ],
    );
}

#[test]
fn half_hour_dst_fold_uses_the_offset_of_each_instant() {
    let cfg = scheduled("Australia/Lord_Howe", Weekday::Sun, "01:30", "02:00");
    assert_windows(
        &cfg,
        &[
            ("2026-04-04T14:45:00Z", true),
            ("2026-04-04T15:15:00Z", true),
            ("2026-04-04T15:30:00Z", false),
        ],
    );
}

#[test]
fn local_schedule_retains_the_system_wall_clock_contract() {
    let cfg = scheduled("Local", Weekday::Mon, "09:00", "17:00");
    assert_eq!(decisions(&cfg, monday_at(10, 0)), [true; 6]);
    assert_eq!(decisions(&cfg, monday_at(19, 0)), [false; 6]);
}

#[test]
fn disabled_and_empty_schedules_remain_unrestricted() {
    for zone in ["Local", "Asia/Seoul", "Invalid/Zone"] {
        let mut cfg = scheduled(zone, Weekday::Mon, "09:00", "17:00");
        cfg.tracking_schedule.enabled = false;
        assert_eq!(
            decisions(&cfg, monday_at(19, 0)),
            [false, true, true, true, true, true]
        );
        cfg.tracking_schedule.enabled = true;
        cfg.tracking_schedule.windows.clear();
        assert_eq!(
            decisions(&cfg, monday_at(10, 0)),
            [false, true, true, true, true, true]
        );
    }
}

#[test]
fn invalid_unchecked_timezone_fails_closed_for_a_configured_schedule() {
    let cfg = scheduled("Invalid/Zone", Weekday::Mon, "09:00", "17:00");
    assert_eq!(decisions(&cfg, monday_at(10, 0)), [false; 6]);
}

#[test]
fn persisted_timezone_changes_apply_after_reload_and_restart() {
    use crate::config_manager::ConfigManager;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let manager = ConfigManager::with_paths(path.clone(), None).unwrap();
    manager
        .update(scheduled("UTC", Weekday::Mon, "09:00", "17:00"))
        .unwrap();
    let now = instant("2026-11-16T10:00:00Z");
    assert_eq!(decisions(&manager.snapshot(), now), [true; 6]);

    for (zone, allowed) in [("Asia/Seoul", false), ("Asia/Seoul", false), ("UTC", true)] {
        manager
            .update_with(|cfg| {
                cfg.tracking_schedule.timezone = zone.to_owned();
                Ok(())
            })
            .unwrap();
        assert_eq!(decisions(&manager.snapshot(), now), [allowed; 6]);
        manager.reload().unwrap();
        assert_eq!(decisions(&manager.snapshot(), now), [allowed; 6]);
        let restarted = ConfigManager::with_paths(path.clone(), None).unwrap();
        assert_eq!(restarted.snapshot().tracking_schedule.timezone, zone);
        assert_eq!(decisions(&restarted.snapshot(), now), [allowed; 6]);
    }
}

#[test]
fn vancouver_2025b_schedule_includes_both_2026_fall_back_occurrences() {
    let cfg = scheduled("America/Vancouver", Weekday::Sun, "01:00", "01:30");
    assert_windows(
        &cfg,
        &[
            ("2026-11-01T08:15:00Z", true),
            ("2026-11-01T08:45:00Z", false),
            ("2026-11-01T09:15:00Z", true),
            ("2026-11-01T09:30:00Z", false),
        ],
    );
}

#[test]
fn chisinau_2025b_schedule_keeps_the_2022_spring_transition() {
    let cfg = scheduled("Europe/Chisinau", Weekday::Sun, "03:00", "03:30");
    assert_windows(
        &cfg,
        &[
            ("2022-03-26T23:59:00Z", false),
            ("2022-03-27T00:00:00Z", true),
            ("2022-03-27T00:30:00Z", false),
        ],
    );
}

#[test]
fn london_schedule_keeps_the_final_2099_offset_in_2100() {
    let cfg = scheduled("Europe/London", Weekday::Sun, "09:00", "09:30");
    assert_windows(
        &cfg,
        &[
            ("2100-07-04T08:15:00Z", false),
            ("2100-07-04T09:15:00Z", true),
            ("2100-07-04T09:30:00Z", false),
        ],
    );
}
