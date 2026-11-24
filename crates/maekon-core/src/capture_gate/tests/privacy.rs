use super::*;

// ── 16-row truth table (Test 8) ────────────────────────────────────────

/// Test 8: All 2⁴ combinations of the legacy schedule/consent gates.
///
/// Each combination is constructed by building an `AppConfig` that puts
/// each gate into the desired state, then asserting that
/// `capture_permitted_now` returns `consent AND active_hours AND ts_allowed
/// AND !capture_paused`.
///
/// Gate construction:
///   - `consent`: `ConsentPermissions { screen_capture: true/false, .. }`
///   - `active_hours`: schedule.active_hours_enabled = true, window covers
///     the test `now` (Mon 12:30 / Wed 23:00 accordingly)
///   - `ts_allowed`: TS enabled with a window that covers `now`; disabled
///     and empty schedules are covered by dedicated compatibility tests
///   - `capture_paused`: passed directly as argument
#[test]
fn capture_permitted_combines_all_four_gates() {
    // Fixed test instant: Monday 12:30.
    // For each combo we build cfg to achieve the desired gate state.
    let now = monday_at(12, 30);

    // Iterate all 16 (bool×bool×bool×bool) combinations.
    for consent_val in [true, false] {
        for active_hours_val in [true, false] {
            for ts_allowed_val in [true, false] {
                for paused_val in [true, false] {
                    let cfg = build_scenario_cfg(active_hours_val, ts_allowed_val, now);
                    // Screen gate reads screen_capture; mic mirrors it here (irrelevant to this gate).
                    let c = consent(consent_val, consent_val);
                    let expected = consent_val && active_hours_val && ts_allowed_val && !paused_val;

                    let got = capture_permitted_now(&cfg, &c, paused_val, now);

                    assert_eq!(
                        got, expected,
                        "combo (consent={consent_val}, active_hours={active_hours_val}, \
                         ts_allowed={ts_allowed_val}, paused={paused_val}) expected \
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

fn capture_decisions(
    cfg: &AppConfig,
    permissions: &ConsentPermissions,
    paused: bool,
    battery_saver: bool,
) -> [bool; 4] {
    let now = monday_at(12, 30);
    [
        capture_permitted_now(cfg, permissions, paused, now),
        audio_capture_permitted_now(cfg, permissions, paused, now),
        capture_permitted_now_with_power(cfg, permissions, paused, battery_saver, now),
        audio_capture_permitted_now_with_power(cfg, permissions, paused, battery_saver, now),
    ]
}

#[test]
fn privacy_mode_vetoes_screen_and_audio_with_valid_consent() {
    let mut cfg = build_scenario_cfg(true, true, monday_at(12, 30));
    cfg.audio.enabled = true;
    let permissions = consent(true, true);
    assert_eq!(
        capture_decisions(&cfg, &permissions, false, false),
        [true; 4]
    );

    for _ in 0..3 {
        cfg.vision.privacy_mode = true;
        for battery_saver in [false, true] {
            assert_eq!(
                capture_decisions(&cfg, &permissions, false, battery_saver),
                [false; 4],
                "Privacy Mode must veto every screen/audio gate"
            );
        }
        cfg.vision.privacy_mode = false;
        assert_eq!(
            capture_decisions(&cfg, &permissions, false, false),
            [true; 4]
        );
    }
}

#[test]
fn privacy_mode_release_preserves_each_independent_veto() {
    let mut cfg = build_scenario_cfg(true, true, monday_at(12, 30));
    cfg.audio.enabled = true;
    cfg.vision.privacy_mode = true;
    assert_eq!(
        capture_decisions(&cfg, &consent(true, true), false, false),
        [false; 4]
    );
    cfg.vision.privacy_mode = false;

    assert_eq!(
        capture_decisions(&cfg, &consent(true, false), false, false),
        [true, false, true, false]
    );
    assert_eq!(
        capture_decisions(&cfg, &consent(false, true), false, false),
        [false, true, false, true]
    );
    assert_eq!(
        capture_decisions(&cfg, &consent(true, true), true, false),
        [false; 4]
    );
    cfg.schedule.pause_on_battery_saver = true;
    assert_eq!(
        capture_decisions(&cfg, &consent(true, true), false, true),
        [true, true, false, false]
    );
    cfg.vision.capture_enabled = false;
    assert_eq!(
        capture_decisions(&cfg, &consent(true, true), false, false),
        [false, true, false, true]
    );
    cfg.vision.capture_enabled = true;
    cfg.audio.enabled = false;
    assert_eq!(
        capture_decisions(&cfg, &consent(true, true), false, false),
        [true, false, true, false]
    );

    for (hours, tracking) in [(false, true), (true, false)] {
        let mut restricted = build_scenario_cfg(hours, tracking, monday_at(12, 30));
        restricted.audio.enabled = true;
        assert_eq!(
            capture_decisions(&restricted, &consent(true, true), false, false),
            [false; 4]
        );
    }
}

#[test]
fn privacy_mode_persisted_settings_gate_capture_after_reload_and_restart() {
    use crate::config_manager::ConfigManager;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.json");
    let manager = ConfigManager::with_paths(path.clone(), None).unwrap();
    let mut cfg = build_scenario_cfg(true, true, monday_at(12, 30));
    cfg.audio.enabled = true;
    manager.update(cfg).unwrap();
    let permissions = consent(true, true);
    assert_eq!(
        capture_decisions(&manager.snapshot(), &permissions, false, false),
        [true; 4]
    );

    for enabled in [true, true, false] {
        manager
            .update_with(|cfg| {
                cfg.vision.privacy_mode = enabled;
                Ok(())
            })
            .unwrap();
        let expected = if enabled { [false; 4] } else { [true; 4] };
        assert_eq!(
            capture_decisions(&manager.snapshot(), &permissions, false, false),
            expected
        );
        manager.reload().unwrap();
        assert_eq!(
            capture_decisions(&manager.snapshot(), &permissions, false, false),
            expected
        );
        let restarted = ConfigManager::with_paths(path.clone(), None).unwrap();
        assert_eq!(
            capture_decisions(&restarted.snapshot(), &permissions, false, false),
            expected
        );
    }
}

#[test]
fn privacy_mode_duplicate_and_concurrent_settings_cannot_reopen_capture() {
    use crate::config_manager::ConfigManager;
    use std::sync::{Arc, Barrier};
    let dir = tempfile::tempdir().unwrap();
    let manager = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
    let mut cfg = build_scenario_cfg(true, true, monday_at(12, 30));
    cfg.audio.enabled = true;
    cfg.vision.privacy_mode = true;
    manager.update(cfg).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let decisions = std::thread::scope(|scope| {
        for privacy_writer in [true, false] {
            let manager = manager.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                barrier.wait();
                for i in 0..16 {
                    manager
                        .update_with(|cfg| {
                            if privacy_writer {
                                cfg.vision.privacy_mode = true;
                            } else {
                                cfg.notification.enabled = i % 2 == 0;
                            }
                            Ok(())
                        })
                        .unwrap();
                }
            });
        }
        barrier.wait();
        (0..64)
            .map(|_| capture_decisions(&manager.snapshot(), &consent(true, true), false, false))
            .collect::<Vec<_>>()
    });
    for decision in decisions {
        assert_eq!(decision, [false; 4]);
    }
    assert_eq!(
        capture_decisions(&manager.snapshot(), &consent(true, true), false, false),
        [false; 4]
    );
    manager
        .update_with(|cfg| {
            cfg.vision.privacy_mode = false;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        capture_decisions(&manager.snapshot(), &consent(true, true), false, false),
        [true; 4]
    );
}
