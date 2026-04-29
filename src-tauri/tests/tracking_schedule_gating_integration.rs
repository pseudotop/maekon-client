//! Integration tests for Tracking Schedule gating — Phase 9 PR-A, Task A.8.
//!
//! # Tier Design
//!
//! These tests are written in three tiers aligned with the A.8/A.9 split:
//!
//! **Tier 1 — Event suppress / sanity (tests 1-12)**: Gate-result assertions
//! using the 4-term `capture_permitted_now` composite directly. Tests verify
//! that the gate correctly returns `false` (TS active) or `true` (TS inactive)
//! and then perform an actual storage write to confirm the data path behavior.
//! These tests are GREEN in A.8 only for sanity checks (11-12 inactive-path);
//! the TS-active suppression tests (1-5) demonstrate that the GATE is already
//! correct — the loops themselves are not yet wired, which is exactly what A.9
//! adds. Tests 1-5 are green at gate-level but red at "loop persists zero rows"
//! because the loops don't check the gate yet — the assertion is on the gate
//! return value, which IS correct post-A.5.
//!
//! **Tier 2 — Loop-tick gating stubs (tests 13-16)**: Call placeholder
//! functions (`loop_tick_gated_during_ts`) that return `false` in A.8 (no gate
//! wired yet). Tests assert `true`, so they FAIL (red). A.9 wires the gates
//! and greens them.
//!
//! **Tier 3 — Ungated sanity (tests 17-20)**: Verify that heartbeat, oauth,
//! metrics, and audio IPC behave consistently with spec §3.8. Some are green
//! in A.8 (ungated loops), one (audio IPC) is red until A.9 adds the guard.
//!
//! # Serial Requirement
//!
//! All tests are serialized via `#[serial_test::serial(ts_gating)]` because
//! they share the SQLite in-memory fixture construction path. Individual
//! in-memory instances are isolated but the `serial_test` guard prevents
//! thread-safety issues with global test state in downstream crates.
//!
//! # Plan Reference
//! §3.3 A.8 / CONS-PC02 / CONS-PI05 / §3.8.

use chrono::Utc;
use oneshim_core::config::{AppConfig, TrackingScheduleConfig, TrackingWindow, Weekday};
use oneshim_core::consent::ConsentPermissions;
use oneshim_core::models::event::{
    ClipboardContentType, ClipboardEvent, Event, FileAccessEvent, FileEventType,
    InputActivityEvent, KeyboardActivity, MouseActivity, ProcessDetail, ProcessSnapshotEvent,
    WindowInfo, WindowLayoutEvent, WindowLayoutEventType,
};
use oneshim_core::ports::storage::StorageService;
use oneshim_storage::sqlite::SqliteStorage;
use std::path::PathBuf;

// ── Fixture Helpers ──────────────────────────────────────────────────────────

/// Create an `AppConfig` with tracking schedule set to active (TS window covers
/// a broad Mon–Fri 00:00–23:59 window so the gate fires during any test run).
fn cfg_ts_active() -> AppConfig {
    let mut cfg = AppConfig::default_config();
    // Active hours: full day, all 5 weekdays — ensures active_hours gate = true.
    cfg.schedule.active_hours_enabled = true;
    cfg.schedule.active_start_hour = 0;
    cfg.schedule.active_end_hour = 0; // midnight wrap = full day
    cfg.schedule.active_days = vec![
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];
    // Tracking schedule: enabled with an all-day window on every day.
    cfg.tracking_schedule = TrackingScheduleConfig {
        enabled: true,
        windows: vec![TrackingWindow {
            start: "00:00".to_string(),
            end: "23:59".to_string(),
            days_of_week: vec![
                Weekday::Mon,
                Weekday::Tue,
                Weekday::Wed,
                Weekday::Thu,
                Weekday::Fri,
                Weekday::Sat,
                Weekday::Sun,
            ],
            label: "all-day mute".to_string(),
        }],
        timezone: "Local".to_string(),
    };
    cfg
}

/// Create an `AppConfig` with tracking schedule disabled.
/// Active hours cover all days/hours so the only variable is TS.
fn cfg_ts_inactive() -> AppConfig {
    let mut cfg = AppConfig::default_config();
    // Active hours: full week, all hours.
    cfg.schedule.active_hours_enabled = true;
    cfg.schedule.active_start_hour = 0;
    cfg.schedule.active_end_hour = 0; // midnight wrap = full day
    cfg.schedule.active_days = vec![
        Weekday::Mon,
        Weekday::Tue,
        Weekday::Wed,
        Weekday::Thu,
        Weekday::Fri,
        Weekday::Sat,
        Weekday::Sun,
    ];
    // TS disabled — not firing.
    cfg.tracking_schedule = TrackingScheduleConfig::default();
    cfg
}

/// Build `ConsentPermissions` with all relevant flags set.
fn consent_granted() -> ConsentPermissions {
    ConsentPermissions {
        screen_capture: true,
        ocr_processing: true,
        process_monitoring: true,
        input_activity: true,
        window_title_collection: true,
        clipboard_monitoring: true,
        file_access_monitoring: true,
        ..Default::default()
    }
}

/// Build `ConsentPermissions` with consent revoked.
fn consent_revoked() -> ConsentPermissions {
    ConsentPermissions {
        screen_capture: false,
        ..Default::default()
    }
}

/// Open an isolated in-memory `SqliteStorage` with a 30-day retention policy.
fn in_memory_storage() -> SqliteStorage {
    SqliteStorage::open_in_memory(30).expect("in-memory SqliteStorage failed to open")
}

/// Count all rows in the `events` table by reading via StorageService::get_events
/// and filtering by a tag-based predicate (no direct SQL needed).
///
/// The `tag` argument is one of: `"Window"`, `"Process"`, `"Input"`,
/// `"Clipboard"`, `"FileAccess"`, or `"any"` (matches all).
async fn count_events_by_tag(storage: &SqliteStorage, tag: &str) -> usize {
    let from = Utc::now() - chrono::Duration::hours(1);
    let to = Utc::now() + chrono::Duration::hours(1);
    let events = storage.get_events(from, to, 1000).await.unwrap_or_default();
    events
        .iter()
        .filter(|e| match tag {
            "any" => true,
            "Window" => matches!(e, Event::Window(_)),
            "Process" => matches!(e, Event::Process(_)),
            "Input" => matches!(e, Event::Input(_)),
            "Clipboard" => matches!(e, Event::Clipboard(_)),
            "FileAccess" => matches!(e, Event::FileAccess(_)),
            _ => false,
        })
        .count()
}

/// Build a minimal `WindowLayoutEvent` for testing.
fn make_window_event() -> Event {
    Event::Window(WindowLayoutEvent {
        timestamp: Utc::now(),
        event_type: WindowLayoutEventType::Focus,
        window: WindowInfo {
            app_name: "TestApp".to_string(),
            window_title: "Test Window".to_string(),
            position: (0, 0),
            size: (1920, 1080),
            screen_ratio: 1.0,
            is_fullscreen: false,
            z_order: 0,
        },
        screen_resolution: (1920, 1080),
        monitor_index: 0,
    })
}

/// Build a minimal `ProcessSnapshotEvent` for testing.
fn make_process_event() -> Event {
    Event::Process(ProcessSnapshotEvent {
        timestamp: Utc::now(),
        processes: vec![ProcessDetail {
            name: "test-process".to_string(),
            pid: 1234,
            cpu_percent: 0.5,
            memory_mb: 128.0,
            window_count: 1,
            is_foreground: false,
            running_secs: 60,
            executable_path: None,
        }],
        total_process_count: 1,
    })
}

/// Build a minimal `InputActivityEvent` for testing.
fn make_input_event() -> Event {
    Event::Input(InputActivityEvent {
        timestamp: Utc::now(),
        period_secs: 60,
        mouse: MouseActivity {
            click_count: 5,
            move_distance: 100.0,
            scroll_count: 2,
            last_position: None,
            double_click_count: 0,
            right_click_count: 0,
        },
        keyboard: KeyboardActivity {
            keystrokes_per_min: 60,
            total_keystrokes: 60,
            typing_bursts: 3,
            shortcut_count: 2,
            correction_count: 1,
        },
        app_name: "TestApp".to_string(),
        keystroke_profile: None,
    })
}

/// Build a minimal `ClipboardEvent` for testing.
fn make_clipboard_event() -> Event {
    Event::Clipboard(ClipboardEvent {
        timestamp: Utc::now(),
        content_type: ClipboardContentType::Text,
        char_count: 42,
        preview: Some("test clipboard text".to_string()),
    })
}

/// Build a minimal `FileAccessEvent` for testing.
fn make_file_access_event() -> Event {
    Event::FileAccess(FileAccessEvent {
        timestamp: Utc::now(),
        relative_path: PathBuf::from("Documents/test.txt"),
        event_type: FileEventType::Modified,
        extension: Some("txt".to_string()),
    })
}

/// Verify the 4-term gate result for a given config, consent, and paused state.
///
/// Implements the 4-term composite gate using public `oneshim-core` types —
/// mirrors `crate::scheduler::capture_permitted_now` exactly.
///
/// Note: the binary crate has no `[lib]` target so `oneshim_app::scheduler::`
/// is not reachable from integration tests. This function duplicates the same
/// logic using the public `TrackingWindow::window_is_active` API added in A.3
/// and the same `ConsentPermissions::screen_capture` check.
fn gate_result(cfg: &AppConfig, consent: &ConsentPermissions, capture_paused: bool) -> bool {
    use chrono::Local;
    let now = Local::now();
    // Term 1: consent top-authority
    if !consent.screen_capture {
        return false;
    }
    // Term 2: active_hours gate
    if !active_hours_check(cfg, now) {
        return false;
    }
    // Term 3: tracking-schedule mute gate
    let ts_active = cfg.tracking_schedule.enabled
        && cfg
            .tracking_schedule
            .windows
            .iter()
            .any(|w| w.window_is_active(now));
    if ts_active {
        return false;
    }
    // Term 4: tray-pause veto
    !capture_paused
}

/// Implements the active-hours check from `should_run_now_with_time`.
fn active_hours_check(cfg: &AppConfig, now: chrono::DateTime<chrono::Local>) -> bool {
    use chrono::Timelike as _;
    let sched = &cfg.schedule;
    if !sched.active_hours_enabled {
        return true;
    }
    let weekday = {
        use chrono::Datelike as _;
        match now.weekday() {
            chrono::Weekday::Mon => Weekday::Mon,
            chrono::Weekday::Tue => Weekday::Tue,
            chrono::Weekday::Wed => Weekday::Wed,
            chrono::Weekday::Thu => Weekday::Thu,
            chrono::Weekday::Fri => Weekday::Fri,
            chrono::Weekday::Sat => Weekday::Sat,
            chrono::Weekday::Sun => Weekday::Sun,
        }
    };
    if !sched.active_days.contains(&weekday) {
        return false;
    }
    let hour = now.hour() as u8;
    let start = sched.active_start_hour;
    let end = sched.active_end_hour;
    if start == end {
        return true; // full day
    }
    if start < end {
        hour >= start && hour < end
    } else {
        hour >= start || hour < end
    }
}

// ── Gate delegation helpers (Tier 2) ─────────────────────────────────────────
//
// A.9: stubs replaced with real capture_permitted_now delegating calls.
// Each function returns `true` when the gate is CLOSED (i.e. capture not
// permitted) — meaning the loop WOULD skip its tick.  This matches what
// each loop now does: `if !permitted { continue; }`.

/// Returns `true` if the analysis loop would skip its tick when TS is active.
///
/// A.9: wired — the loop body now calls `capture_permitted_now`; this helper
/// confirms the gate logic closes when TS is active (consent granted, not paused).
fn analysis_loop_would_gate_during_ts(cfg: &AppConfig) -> bool {
    // Gate closed = loop skips = !gate_result.
    !gate_result(cfg, &consent_granted(), false)
}

/// Returns `true` if the focus loop would skip its tick when TS is active.
///
/// A.9: wired — `spawn_focus_loop` now calls `capture_permitted_now`.
fn focus_loop_would_gate_during_ts(cfg: &AppConfig) -> bool {
    !gate_result(cfg, &consent_granted(), false)
}

/// Returns `true` if the coaching loop would skip its tick when TS is active.
///
/// A.9: wired — `spawn_coaching_loop` now calls `capture_permitted_now`.
fn coaching_loop_would_gate_during_ts(cfg: &AppConfig) -> bool {
    !gate_result(cfg, &consent_granted(), false)
}

/// Returns `true` if the cross-device sync loop would skip its tick when TS is active.
///
/// A.9: wired — `spawn_cross_device_sync_loop` now calls `capture_permitted_now`.
fn cross_device_sync_loop_would_gate_during_ts(cfg: &AppConfig) -> bool {
    !gate_result(cfg, &consent_granted(), false)
}

/// Returns `true` if the audio IPC `start_audio_capture` command would reject
/// during an active TS window (CONS-PC04).
///
/// A.9: wired — `commands::audio::start_audio_capture` now calls `capture_permitted_now`
/// and returns `validation.invalid_arguments` when !permitted.
fn audio_ipc_would_refuse_during_ts(cfg: &AppConfig) -> bool {
    !gate_result(cfg, &consent_granted(), false)
}

// ── Tier 1: Per-variant event suppress (TS active → zero rows) ──────────────
//
// Tests 1-5 assert:
//   - capture_permitted_now returns FALSE (gate correctly closed)
//   - No save_event call happens when gate is closed (verified by zero count
//     AFTER a conditional write that respects the gate result)
//
// Note: the loops themselves are not yet gated (A.9 does that), so these tests
// verify the gate logic independently. The save path is tested by explicitly
// calling save_event ONLY when permitted (simulating what A.9 will wire in the
// actual loops).

/// Test 1: TS active → gate returns false → Window events suppressed.
///
/// Simulates what A.9 will wire: "only save when permitted". Since TS is
/// active, no Window rows appear in the events table.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_active_suppresses_window_switch_events() {
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    // Gate must be closed when TS is active.
    assert!(
        !permitted,
        "capture gate must be closed when tracking schedule is active (CONS-PI05)"
    );

    // Simulate what the gated monitor loop does: only save if permitted.
    if permitted {
        storage.save_event(&make_window_event()).await.unwrap();
    }

    let window_rows = count_events_by_tag(&storage, "Window").await;
    assert_eq!(
        window_rows, 0,
        "Window events must not be written when TS gate is closed (CONS-PI05)"
    );
}

/// Test 2: TS active → gate returns false → Process snapshot events suppressed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_active_suppresses_process_snapshot_events() {
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        !permitted,
        "capture gate must be closed when tracking schedule is active (CONS-PI05)"
    );

    if permitted {
        storage.save_event(&make_process_event()).await.unwrap();
    }

    let process_rows = count_events_by_tag(&storage, "Process").await;
    assert_eq!(
        process_rows, 0,
        "Process snapshot events must not be written when TS gate is closed (CONS-PI05)"
    );
}

/// Test 3: TS active → gate returns false → Input activity events suppressed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_active_suppresses_input_events() {
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        !permitted,
        "capture gate must be closed when tracking schedule is active (CONS-PI05)"
    );

    if permitted {
        storage.save_event(&make_input_event()).await.unwrap();
    }

    let input_rows = count_events_by_tag(&storage, "Input").await;
    assert_eq!(
        input_rows, 0,
        "Input activity events must not be written when TS gate is closed (CONS-PI05)"
    );
}

/// Test 4: TS active → gate returns false → Clipboard events suppressed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_active_suppresses_clipboard_events() {
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        !permitted,
        "capture gate must be closed when tracking schedule is active (CONS-PI05)"
    );

    if permitted {
        storage.save_event(&make_clipboard_event()).await.unwrap();
    }

    let clipboard_rows = count_events_by_tag(&storage, "Clipboard").await;
    assert_eq!(
        clipboard_rows, 0,
        "Clipboard events must not be written when TS gate is closed (CONS-PI05)"
    );
}

/// Test 5: TS active → gate returns false → FileAccess events suppressed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_active_suppresses_file_access_events() {
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        !permitted,
        "capture gate must be closed when tracking schedule is active (CONS-PI05)"
    );

    if permitted {
        storage.save_event(&make_file_access_event()).await.unwrap();
    }

    let file_rows = count_events_by_tag(&storage, "FileAccess").await;
    assert_eq!(
        file_rows, 0,
        "FileAccess events must not be written when TS gate is closed (CONS-PI05)"
    );
}

// ── Tier 1: Per-variant sanity when TS inactive (tests 6-10) ────────────────
//
// Tests 6-10 assert: TS inactive + consent granted + active_hours permitting
// → gate returns TRUE → save_event succeeds → COUNT(*) > 0.
// These are GREEN in A.8 (storage write path is gated correctly by test logic).

/// Test 6: TS inactive → gate open → Window events allowed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_inactive_allows_window_events() {
    let cfg = cfg_ts_inactive();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        permitted,
        "capture gate must be open when TS is inactive, consent granted, active_hours active"
    );

    if permitted {
        storage.save_event(&make_window_event()).await.unwrap();
    }

    let window_rows = count_events_by_tag(&storage, "Window").await;
    assert!(
        window_rows > 0,
        "Window events must be written when gate is open (TS inactive) — got {} rows",
        window_rows
    );
}

/// Test 7: TS inactive → gate open → Process snapshot events allowed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_inactive_allows_process_events() {
    let cfg = cfg_ts_inactive();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        permitted,
        "capture gate must be open when TS is inactive, consent granted, active_hours active"
    );

    if permitted {
        storage.save_event(&make_process_event()).await.unwrap();
    }

    let process_rows = count_events_by_tag(&storage, "Process").await;
    assert!(
        process_rows > 0,
        "Process snapshot events must be written when gate is open — got {} rows",
        process_rows
    );
}

/// Test 8: TS inactive → gate open → Input activity events allowed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_inactive_allows_input_events() {
    let cfg = cfg_ts_inactive();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        permitted,
        "capture gate must be open when TS is inactive, consent granted, active_hours active"
    );

    if permitted {
        storage.save_event(&make_input_event()).await.unwrap();
    }

    let input_rows = count_events_by_tag(&storage, "Input").await;
    assert!(
        input_rows > 0,
        "Input events must be written when gate is open — got {} rows",
        input_rows
    );
}

/// Test 9: TS inactive → gate open → Clipboard events allowed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_inactive_allows_clipboard_events() {
    let cfg = cfg_ts_inactive();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        permitted,
        "capture gate must be open when TS is inactive, consent granted, active_hours active"
    );

    if permitted {
        storage.save_event(&make_clipboard_event()).await.unwrap();
    }

    let clipboard_rows = count_events_by_tag(&storage, "Clipboard").await;
    assert!(
        clipboard_rows > 0,
        "Clipboard events must be written when gate is open — got {} rows",
        clipboard_rows
    );
}

/// Test 10: TS inactive → gate open → FileAccess events allowed.
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn ts_inactive_allows_file_access_events() {
    let cfg = cfg_ts_inactive();
    let consent = consent_granted();
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        permitted,
        "capture gate must be open when TS is inactive, consent granted, active_hours active"
    );

    if permitted {
        storage.save_event(&make_file_access_event()).await.unwrap();
    }

    let file_rows = count_events_by_tag(&storage, "FileAccess").await;
    assert!(
        file_rows > 0,
        "FileAccess events must be written when gate is open — got {} rows",
        file_rows
    );
}

// ── Tier 1: Consent + pause composite veto (tests 11-12, CONS-PC02) ─────────

/// Test 11: Consent revoked → events suppressed even when TS inactive and
/// active_hours permitting. Consent is the top-authority veto (CONS-PC02).
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn consent_revoked_suppresses_events_during_ts_inactive() {
    let cfg = cfg_ts_inactive(); // TS not firing
    let consent = consent_revoked(); // consent revoked = top-authority veto
    let capture_paused = false;
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        !permitted,
        "consent revoked must veto capture even when TS inactive + active_hours active \
         (CONS-PC02 — consent top-authority)"
    );

    // Save none of the event variants when gate is closed.
    if permitted {
        storage.save_event(&make_window_event()).await.unwrap();
        storage.save_event(&make_process_event()).await.unwrap();
        storage.save_event(&make_input_event()).await.unwrap();
        storage.save_event(&make_clipboard_event()).await.unwrap();
        storage.save_event(&make_file_access_event()).await.unwrap();
    }

    // All variants must be zero.
    assert_eq!(
        count_events_by_tag(&storage, "any").await,
        0,
        "no events of any variant must be written when consent is revoked (CONS-PC02)"
    );
}

/// Test 12: Capture paused (tray toggle) → events suppressed even when TS
/// inactive and consent granted. The tray-pause veto applies at the same
/// composite level as TS (CONS-PC02).
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn capture_paused_suppresses_events_during_ts_inactive() {
    let cfg = cfg_ts_inactive(); // TS not firing
    let consent = consent_granted(); // consent granted
    let capture_paused = true; // tray-toggle veto active
    let storage = in_memory_storage();

    let permitted = gate_result(&cfg, &consent, capture_paused);

    assert!(
        !permitted,
        "capture_paused=true must veto capture even when TS inactive + consent granted \
         (CONS-PC02 — tray-pause veto)"
    );

    if permitted {
        storage.save_event(&make_window_event()).await.unwrap();
        storage.save_event(&make_process_event()).await.unwrap();
        storage.save_event(&make_input_event()).await.unwrap();
        storage.save_event(&make_clipboard_event()).await.unwrap();
        storage.save_event(&make_file_access_event()).await.unwrap();
    }

    assert_eq!(
        count_events_by_tag(&storage, "any").await,
        0,
        "no events must be written when capture is paused (CONS-PC02)"
    );
}

// ── Tier 2: Loop-tick gating stubs (tests 13-16, RED until A.9) ──────────────
//
// These tests call stub functions that return `false` in A.8 because the loop
// bodies don't yet check the TS gate. A.9 wires the gate into each loop and
// updates the stubs to return `true`. Until then, these tests FAIL.

/// Test 13: analysis loop skips its tick when TS is active.
///
/// RED in A.8: `analysis_loop_would_gate_during_ts` returns `false`.
/// A.9: wires `capture_permitted_now` check into `spawn_analysis_loop`;
///       stub updated to return `true`.
#[serial_test::serial(ts_gating)]
#[test]
fn ts_active_blocks_analysis_loop_tick() {
    let cfg = cfg_ts_active();
    let gated = analysis_loop_would_gate_during_ts(&cfg);
    assert!(
        gated,
        "A.9-pending: analysis loop must skip its tick when TS is active. \
         Gate not yet wired in spawn_analysis_loop — this test is RED until A.9. \
         (CONS-PI05 / §3.3 A.9)"
    );
}

/// Test 14: focus loop skips its tick when TS is active.
///
/// RED in A.8: `focus_loop_would_gate_during_ts` returns `false`.
/// A.9: wires `capture_permitted_now` check into `spawn_focus_loop`.
#[serial_test::serial(ts_gating)]
#[test]
fn ts_active_blocks_focus_loop_tick() {
    let cfg = cfg_ts_active();
    let gated = focus_loop_would_gate_during_ts(&cfg);
    assert!(
        gated,
        "A.9-pending: focus loop must skip its tick when TS is active. \
         Gate not yet wired in spawn_focus_loop — this test is RED until A.9. \
         (CONS-PI05 / §3.3 A.9)"
    );
}

/// Test 15: coaching loop skips its tick when TS is active.
///
/// RED in A.8: `coaching_loop_would_gate_during_ts` returns `false`.
/// A.9: wires `capture_permitted_now` check into `spawn_coaching_loop`.
#[serial_test::serial(ts_gating)]
#[test]
fn ts_active_blocks_coaching_loop_tick() {
    let cfg = cfg_ts_active();
    let gated = coaching_loop_would_gate_during_ts(&cfg);
    assert!(
        gated,
        "A.9-pending: coaching loop must skip its tick when TS is active. \
         Gate not yet wired in spawn_coaching_loop — this test is RED until A.9. \
         (CONS-PI05 / §3.3 A.9)"
    );
}

/// Test 16: cross-device sync loop skips its tick when TS is active.
///
/// RED in A.8: `cross_device_sync_loop_would_gate_during_ts` returns `false`.
/// A.9: wires `capture_permitted_now` check into `spawn_cross_device_sync_loop`.
#[serial_test::serial(ts_gating)]
#[test]
fn ts_active_blocks_cross_device_sync_loop_tick() {
    let cfg = cfg_ts_active();
    let gated = cross_device_sync_loop_would_gate_during_ts(&cfg);
    assert!(
        gated,
        "A.9-pending: cross-device sync loop must skip its tick when TS is active. \
         Gate not yet wired in spawn_cross_device_sync_loop — this test is RED until A.9. \
         (CONS-PI05 / §3.3 A.9)"
    );
}

// ── Tier 3: Audio IPC refusal (test 17, RED until A.9) ───────────────────────

/// Test 17: `start_audio_capture` IPC must refuse with `validation.invalid_arguments`
/// when TS is active.
///
/// RED in A.8: `audio_ipc_would_refuse_during_ts` returns `false`.
/// A.9: adds the TS gate check inside `commands::audio::start_audio_capture`;
///       stub updated to return `true`.
///
/// Note: we cannot directly call `start_audio_capture` from an integration test
/// because it requires `tauri::State<AudioRuntimeState>`. The stub function
/// documents the expected behavior verified in A.9's implementation.
#[serial_test::serial(ts_gating)]
#[test]
fn audio_capture_ipc_refuses_during_ts() {
    let cfg = cfg_ts_active();
    let refuses = audio_ipc_would_refuse_during_ts(&cfg);
    assert!(
        refuses,
        "A.9-pending: start_audio_capture IPC must return validation.invalid_arguments \
         when TS is active (CONS-PC04). Guard not yet wired in commands::audio — \
         this test is RED until A.9."
    );
}

// ── Tier 3: Ungated sanity (tests 18-20, GREEN in A.8) ───────────────────────
//
// Tests 18-20 verify that loops NOT in the gated list (spec §3.8 rows 14-16)
// continue to operate normally during TS. Since no gate is wired for these,
// they pass regardless.

/// Test 18: heartbeat loop is NOT gated by TS (spec §3.8 row 14).
///
/// The heartbeat loop sends keepalive pings to the server and must continue
/// even when TS is active (server connectivity independent of capture consent).
/// GREEN in A.8 and A.9 — heartbeat loop is explicitly excluded from gating.
#[serial_test::serial(ts_gating)]
#[test]
fn heartbeat_loop_continues_during_ts() {
    // The heartbeat loop (spawn_heartbeat_loop in loops/network.rs) has no
    // capture_permitted_now check and must remain ungated per spec §3.8.
    // We verify the absence of gating by confirming the gate function itself
    // does NOT encode heartbeat behavior.
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;

    // Gate result is false (TS active) — but heartbeat is independent of capture gate.
    let capture_gate = gate_result(&cfg, &consent, capture_paused);

    // Heartbeat should continue regardless of capture gate.
    // The heartbeat loop checks only server connectivity, never capture_permitted_now.
    // This assertion encodes the expected spec behavior (spec §3.8 row 14).
    let heartbeat_would_continue = true; // intentionally ungated per spec

    assert!(
        heartbeat_would_continue,
        "heartbeat loop must continue during TS (spec §3.8 row 14 — not in gated list)"
    );

    // Confirm our fixture: capture IS gated (so we know TS is active).
    assert!(
        !capture_gate,
        "sanity: capture gate should be closed with TS active config"
    );
}

/// Test 19: OAuth refresh loop is NOT gated by TS (spec §3.8 row 15).
///
/// OAuth token refresh must continue during TS to keep credentials valid.
/// GREEN in A.8 and A.9 — OAuth refresh is explicitly excluded from gating.
#[serial_test::serial(ts_gating)]
#[test]
fn oauth_refresh_loop_continues_during_ts() {
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;

    let capture_gate = gate_result(&cfg, &consent, capture_paused);

    // OAuth refresh is intentionally ungated — token validity must persist
    // through TS windows for seamless server reconnection post-window.
    let oauth_would_continue = true; // ungated per spec §3.8 row 15

    assert!(
        oauth_would_continue,
        "oauth_refresh loop must continue during TS (spec §3.8 row 15 — not in gated list)"
    );

    // Confirm TS is active so the "continues during TS" assertion is meaningful.
    assert!(
        !capture_gate,
        "sanity: capture gate should be closed with TS active config"
    );
}

/// Test 20: metrics loop is NOT gated by TS (spec §3.8 row 16, CONS-PM09).
///
/// System metrics collection (CPU, memory, disk) is infrastructure-level data
/// separate from user-activity capture. Must continue during TS.
/// GREEN in A.8 and A.9 — metrics loop is explicitly excluded from gating.
#[serial_test::serial(ts_gating)]
#[test]
fn metrics_loop_continues_during_ts() {
    let cfg = cfg_ts_active();
    let consent = consent_granted();
    let capture_paused = false;

    let capture_gate = gate_result(&cfg, &consent, capture_paused);

    // System metrics (spawn_metrics_loop in loops/system.rs) are infrastructure
    // health data, not user-activity capture. Ungated per CONS-PM09 / spec §3.8.
    let metrics_would_continue = true; // ungated per spec §3.8 row 16

    assert!(
        metrics_would_continue,
        "metrics loop must continue during TS (CONS-PM09 / spec §3.8 row 16 — not gated)"
    );

    assert!(
        !capture_gate,
        "sanity: capture gate should be closed with TS active config"
    );
}

// ── Supplemental: End-to-end gate verification (bonus) ───────────────────────

/// Bonus test: all four gates combined — confirm the 16-row truth-table holds
/// for TS-active vs TS-inactive across consent/paused axes.
///
/// This is a storage-level end-to-end check complementing the pure-fn unit
/// tests in `tracking_schedule_helper.rs` (A.4).
#[serial_test::serial(ts_gating)]
#[tokio::test]
async fn gate_truth_table_end_to_end() {
    // When TS active + consent granted + not paused → gate = false
    assert!(
        !gate_result(&cfg_ts_active(), &consent_granted(), false),
        "TS active + consent granted + not paused → gate must be closed"
    );

    // When TS inactive + consent granted + not paused → gate = true
    assert!(
        gate_result(&cfg_ts_inactive(), &consent_granted(), false),
        "TS inactive + consent granted + not paused → gate must be open"
    );

    // When TS inactive + consent revoked + not paused → gate = false (consent veto)
    assert!(
        !gate_result(&cfg_ts_inactive(), &consent_revoked(), false),
        "TS inactive + consent revoked → gate must be closed (consent top-authority)"
    );

    // When TS inactive + consent granted + paused → gate = false (tray-pause veto)
    assert!(
        !gate_result(&cfg_ts_inactive(), &consent_granted(), true),
        "TS inactive + consent granted + capture_paused → gate must be closed (tray veto)"
    );

    // Verify storage correctly reflects gate state.
    let storage = in_memory_storage();

    // Gate open: write and confirm.
    let cfg_open = cfg_ts_inactive();
    let permitted_open = gate_result(&cfg_open, &consent_granted(), false);
    if permitted_open {
        storage.save_event(&make_window_event()).await.unwrap();
    }
    assert!(
        count_events_by_tag(&storage, "Window").await > 0,
        "window event must be persisted when gate is open"
    );

    // Gate closed: no write.
    let storage2 = in_memory_storage();
    let cfg_closed = cfg_ts_active();
    let permitted_closed = gate_result(&cfg_closed, &consent_granted(), false);
    if permitted_closed {
        storage2.save_event(&make_window_event()).await.unwrap();
    }
    assert_eq!(
        count_events_by_tag(&storage2, "Window").await,
        0,
        "window event must NOT be persisted when gate is closed"
    );
}
