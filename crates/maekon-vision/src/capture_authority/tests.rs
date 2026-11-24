use super::*;
use chrono::TimeZone;
use maekon_core::consent::ConsentManager;
use maekon_core::ports::vision::CaptureObservationError;
use std::sync::atomic::{AtomicBool, AtomicUsize};

#[track_caller]
pub(super) fn assert_denied<T>(result: Result<T, CoreError>, reason: &str) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("capture unexpectedly allowed: {reason}"),
    };
    match error {
        CoreError::PermissionDenied { code, message } => {
            assert_eq!(
                code,
                maekon_core::error_codes::PermissionCode::PrivacyDenied
            );
            assert_eq!(message, format!("capture denied: {reason}"));
        }
        other => panic!("unexpected capture error: {other:?}"),
    }
}

pub(super) struct Observer {
    pub(super) scene: Mutex<Result<CaptureScene, CaptureObservationError>>,
    pub(super) permitted: AtomicBool,
    pub(super) observations: AtomicUsize,
    pub(super) on_observe: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl CaptureObserver for Observer {
    fn check_permission(&self) -> Result<(), CaptureObservationError> {
        if self.permitted.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(CaptureObservationError::PermissionDenied)
        }
    }

    fn observe(&self, display_id: Option<u32>) -> Result<CaptureScene, CaptureObservationError> {
        self.observations.fetch_add(1, Ordering::SeqCst);
        let scene = self.scene.lock().unwrap().clone();
        let callback = self.on_observe.lock().unwrap().take();
        if let Some(callback) = callback {
            callback();
        }
        if let Ok(scene) = &scene {
            assert!(display_id.is_none_or(|id| id == scene.display_id));
        }
        scene
    }
}

pub(super) fn scene() -> CaptureScene {
    let foreground = CaptureWindow {
        native_id: 100,
        pid: 42,
        app_name: "CaptureFixture".into(),
        title: "Allowed fixture".into(),
        bounds: WindowBounds {
            x: -100,
            y: 0,
            width: 100,
            height: 80,
        },
    };
    CaptureScene {
        foreground: foreground.clone(),
        display_id: 7,
        display_bounds: WindowBounds {
            x: -200,
            y: 0,
            width: 200,
            height: 100,
        },
        windows: vec![foreground],
    }
}

pub(super) fn permissions() -> ConsentPermissions {
    ConsentPermissions {
        screen_capture: true,
        ocr_processing: true,
        full_text_extraction: true,
        ..Default::default()
    }
}

pub(super) struct Harness {
    _dir: tempfile::TempDir,
    pub(super) authority: LiveCaptureAuthority,
    pub(super) config: ConfigManager,
    pub(super) consent: Arc<ConsentManager>,
    pub(super) observer: Arc<Observer>,
    pub(super) environment: Arc<Mutex<CaptureEnvironment>>,
}

impl Harness {
    pub(super) fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
        config
            .update_with(|config| {
                config.vision.capture_enabled = true;
                config.schedule.active_hours_enabled = false;
                config.tracking_schedule.enabled = false;
                Ok(())
            })
            .unwrap();
        let consent = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        consent.grant_consent(permissions(), 30).unwrap();
        let observer = Arc::new(Observer {
            scene: Mutex::new(Ok(scene())),
            permitted: AtomicBool::new(true),
            observations: AtomicUsize::new(0),
            on_observe: Mutex::new(None),
        });
        let environment = Arc::new(Mutex::new(CaptureEnvironment {
            now: Local
                .with_ymd_and_hms(2026, 11, 16, 12, 0, 0)
                .single()
                .unwrap(),
            battery_saver_active: false,
        }));
        let clock = environment.clone();
        let authority = LiveCaptureAuthority::new(
            config.clone(),
            consent.clone(),
            Arc::new(CapturePauseState::new(Arc::new(AtomicBool::new(false)))),
            observer.clone(),
            Arc::new(move || *clock.lock().unwrap()),
        );
        Self {
            _dir: dir,
            authority,
            config,
            consent,
            observer,
            environment,
        }
    }

    pub(super) fn permit(&self) -> CapturePermit {
        self.authority.authorize(None).unwrap()
    }
}

#[test]
fn authorized_capture_binds_display_policy_and_separate_text_permissions() {
    let h = Harness::new();
    for (ocr, full_text) in [(false, false), (false, true), (true, false), (true, true)] {
        h.consent
            .grant_consent(
                ConsentPermissions {
                    ocr_processing: ocr,
                    full_text_extraction: full_text,
                    ..permissions()
                },
                30,
            )
            .unwrap();
        let permit = h.permit();
        assert_eq!(permit.display_id(), Some(7));
        assert!(Arc::ptr_eq(
            &permit.config_snapshot().unwrap(),
            &h.config.snapshot()
        ));
        permit
            .check(CaptureAccess::Acquire { display_id: 7 })
            .unwrap();
        assert_denied(
            permit.check(CaptureAccess::Acquire { display_id: 8 }),
            "capture display changed",
        );
        for (access, allowed) in [
            (CaptureAccess::Ocr, ocr),
            (CaptureAccess::FullText, full_text),
        ] {
            let result = permit.check(access);
            if allowed {
                result.unwrap();
            } else {
                assert_denied(result, "capture text consent unavailable");
            }
        }
        permit.check(CaptureAccess::Retain).unwrap();
    }
}

#[test]
fn consent_pause_and_config_aba_never_revive_an_old_permit() {
    let h = Harness::new();
    let old = h.permit();
    h.consent.revoke_consent().unwrap();
    assert_denied(h.authority.authorize(None), "capture consent unavailable");
    h.consent.grant_consent(permissions(), 30).unwrap();
    assert_denied(old.check(CaptureAccess::Retain), "capture consent changed");
    let before_pause = h.permit();
    assert!(h.authority.toggle_pause().unwrap());
    assert_denied(h.authority.authorize(None), "capture paused");
    assert!(!h.authority.toggle_pause().unwrap());
    assert_denied(
        before_pause.check(CaptureAccess::Retain),
        "capture pause epoch changed",
    );
    let before_config = h.permit();
    h.config
        .update_with(|c| {
            c.vision.capture_enabled = false;
            Ok(())
        })
        .unwrap();
    assert_denied(
        h.authority.authorize(None),
        "capture policy disallows access",
    );
    h.config
        .update_with(|c| {
            c.vision.capture_enabled = true;
            Ok(())
        })
        .unwrap();
    assert_denied(
        before_config.check(CaptureAccess::Retain),
        "capture policy changed",
    );
    h.permit().check(CaptureAccess::Retain).unwrap();
}

#[test]
fn unrelated_settings_and_noop_publications_preserve_existing_capture() {
    let h = Harness::new();
    let permit = h.permit();
    let original = h.config.snapshot();
    h.config
        .update_with(|config| {
            config.notification.enabled = !config.notification.enabled;
            Ok(())
        })
        .unwrap();
    assert_ne!(
        original.notification.enabled,
        h.config.snapshot().notification.enabled
    );
    assert!(!Arc::ptr_eq(&original, &h.config.snapshot()));
    permit.check(CaptureAccess::Retain).unwrap();
    permit.check(CaptureAccess::CurrentScene).unwrap();
    h.config.update((*h.config.snapshot()).clone()).unwrap();
    h.config.reload().unwrap();
    permit.check(CaptureAccess::Ocr).unwrap();
    permit.check(CaptureAccess::FullText).unwrap();
    h.permit().check(CaptureAccess::Retain).unwrap();
}

#[test]
fn relevant_policy_changes_revoke_prior_capture_and_allow_new_work() {
    let changes: &[fn(&mut AppConfig)] = &[
        |c| c.privacy.excluded_apps.push("UnrelatedFixture".into()),
        |c| c.privacy.excluded_app_patterns.push("Other*".into()),
        |c| c.privacy.excluded_title_patterns.push("Other*".into()),
        |c| c.privacy.auto_exclude_sensitive = !c.privacy.auto_exclude_sensitive,
        |c| c.privacy.pii_filter_level = maekon_core::config::PiiFilterLevel::Strict,
        |c| c.vision.thumbnail_width += 1,
        |c| c.vision.thumbnail_height += 1,
        |c| c.vision.capture_throttle_ms += 1,
        |c| c.vision.ocr_enabled = !c.vision.ocr_enabled,
        |c| c.vision.ocr_languages = vec!["en-US".into()],
        |c| c.vision.privacy_mode = !c.vision.privacy_mode,
        |c| c.schedule.pause_on_screen_lock = !c.schedule.pause_on_screen_lock,
        |c| c.schedule.pause_on_battery_saver = !c.schedule.pause_on_battery_saver,
        |c| c.schedule.active_start_hour = (c.schedule.active_start_hour + 1) % 24,
        |c| c.schedule.active_end_hour = (c.schedule.active_end_hour + 1) % 24,
        |c| c.schedule.active_days = vec![maekon_core::config::Weekday::Mon],
        |c| c.tracking_schedule.timezone = "UTC".into(),
    ];
    for change in changes {
        let h = Harness::new();
        let old = h.permit();
        let original = (*h.config.snapshot()).clone();
        h.config
            .update_with(|config| {
                change(config);
                Ok(())
            })
            .unwrap();
        assert_denied(old.check(CaptureAccess::Retain), "capture policy changed");
        h.config.update(original).unwrap();
        assert_denied(old.check(CaptureAccess::Retain), "capture policy changed");
        h.permit().check(CaptureAccess::Retain).unwrap();
    }
}

#[test]
fn privacy_mode_denies_new_observations_and_never_revives_old_permits() {
    let h = Harness::new();
    let old = h.permit();
    let observations = h.observer.observations.load(Ordering::SeqCst);
    h.config
        .update_with(|config| {
            config.vision.privacy_mode = true;
            Ok(())
        })
        .unwrap();
    assert_denied(
        h.authority.authorize(None),
        "capture policy disallows access",
    );
    assert_eq!(h.observer.observations.load(Ordering::SeqCst), observations);
    let accesses = [
        CaptureAccess::Acquire { display_id: 7 },
        CaptureAccess::CurrentScene,
        CaptureAccess::Retain,
        CaptureAccess::Ocr,
        CaptureAccess::FullText,
    ];
    for access in accesses {
        assert_denied(old.check(access), "capture policy changed");
    }
    h.config
        .update_with(|config| {
            config.vision.privacy_mode = false;
            Ok(())
        })
        .unwrap();
    let fresh = h.permit();
    for access in accesses {
        assert_denied(old.check(access), "capture policy changed");
        fresh.check(access).unwrap();
    }
}

#[test]
fn config_aba_during_observation_cannot_issue_a_stale_permit() {
    let h = Harness::new();
    let config = h.config.clone();
    *h.observer.on_observe.lock().unwrap() = Some(Box::new(move || {
        for enabled in [false, true] {
            config
                .update_with(|c| {
                    c.vision.capture_enabled = enabled;
                    Ok(())
                })
                .unwrap();
        }
    }));
    assert_denied(h.authority.authorize(None), "capture policy changed");
    h.permit().check(CaptureAccess::Retain).unwrap();
}

#[test]
fn tracking_window_labels_preserve_capture_but_time_changes_revoke_it() {
    use maekon_core::config::{TrackingWindow, Weekday};

    let changes: &[fn(&mut AppConfig)] = &[
        |c| c.tracking_schedule.enabled = false,
        |c| c.tracking_schedule.windows[0].start = "11:00".into(),
        |c| c.tracking_schedule.windows[0].end = "14:00".into(),
        |c| {
            c.tracking_schedule.windows[0]
                .days_of_week
                .push(Weekday::Tue)
        },
        |c| c.tracking_schedule.windows.clear(),
    ];
    for change in changes {
        let h = Harness::new();
        h.config
            .update_with(|c| {
                c.tracking_schedule.enabled = true;
                c.tracking_schedule.windows = vec![TrackingWindow {
                    start: "12:00".into(),
                    end: "13:00".into(),
                    days_of_week: vec![Weekday::Mon],
                    label: "Fixture".into(),
                }];
                Ok(())
            })
            .unwrap();
        let old = h.permit();
        h.config
            .update_with(|c| {
                c.tracking_schedule.windows[0].label = "Renamed fixture".into();
                Ok(())
            })
            .unwrap();
        old.check(CaptureAccess::CurrentScene).unwrap();
        let original = (*h.config.snapshot()).clone();
        h.config
            .update_with(|c| {
                change(c);
                Ok(())
            })
            .unwrap();
        assert_denied(old.check(CaptureAccess::Retain), "capture policy changed");
        h.config.update(original).unwrap();
        assert_denied(old.check(CaptureAccess::Retain), "capture policy changed");
        h.permit().check(CaptureAccess::CurrentScene).unwrap();
    }
}

#[test]
fn permission_loss_seen_by_one_permit_revokes_all_earlier_permits() {
    let h = Harness::new();
    let first = h.permit();
    let unobserved = h.permit();
    h.observer.permitted.store(false, Ordering::SeqCst);
    assert_denied(
        first.check(CaptureAccess::Retain),
        "native capture permission unavailable",
    );
    assert_denied(
        h.authority.authorize(None),
        "native capture permission unavailable",
    );
    h.observer.permitted.store(true, Ordering::SeqCst);
    for old in [first, unobserved] {
        assert_denied(
            old.check(CaptureAccess::Retain),
            "native capture permission changed",
        );
    }
    h.permit().check(CaptureAccess::CurrentScene).unwrap();
}

#[test]
fn permission_loss_reported_during_scene_observation_revokes_older_images() {
    let h = Harness::new();
    let old = h.permit();
    *h.observer.scene.lock().unwrap() = Err(CaptureObservationError::EnumerationFailed);
    assert_denied(
        h.authority.authorize(None),
        "native capture scene unavailable",
    );
    old.check(CaptureAccess::Retain).unwrap();
    *h.observer.scene.lock().unwrap() = Err(CaptureObservationError::PermissionDenied);
    assert_denied(
        h.authority.authorize(None),
        "native capture permission unavailable",
    );
    *h.observer.scene.lock().unwrap() = Ok(scene());
    assert_denied(
        old.check(CaptureAccess::Retain),
        "native capture permission changed",
    );
    h.permit().check(CaptureAccess::Retain).unwrap();
}

#[test]
fn retained_frames_do_not_require_their_window_to_remain_foreground() {
    let h = Harness::new();
    let old = h.permit();
    let before = h.observer.observations.load(Ordering::SeqCst);
    let mut changed = scene();
    changed.foreground.native_id += 1;
    changed.foreground.pid += 1;
    changed.windows = vec![changed.foreground.clone()];
    *h.observer.scene.lock().unwrap() = Ok(changed);
    old.check(CaptureAccess::Retain).unwrap();
    old.check(CaptureAccess::Ocr).unwrap();
    assert_eq!(h.observer.observations.load(Ordering::SeqCst), before);
    assert_denied(
        old.check(CaptureAccess::CurrentScene),
        "capture scene changed",
    );
    h.permit().check(CaptureAccess::CurrentScene).unwrap();
}

#[test]
fn native_observation_errors_and_incomplete_scenes_fail_closed() {
    let h = Harness::new();
    for error in [
        CaptureObservationError::Unsupported,
        CaptureObservationError::ForegroundUnavailable,
        CaptureObservationError::EnumerationFailed,
        CaptureObservationError::OwnerUnavailable,
        CaptureObservationError::TitleUnavailable,
        CaptureObservationError::GeometryUnavailable,
    ] {
        *h.observer.scene.lock().unwrap() = Err(error);
        assert_denied(
            h.authority.authorize(None),
            "native capture scene unavailable",
        );
    }
    let mut cases = Vec::new();
    let mut s = scene();
    s.windows.clear();
    cases.push(s);
    let mut s = scene();
    s.windows[0].pid = 0;
    cases.push(s);
    let mut s = scene();
    s.windows[0].app_name = " ".into();
    cases.push(s);
    let mut s = scene();
    s.display_bounds.width = 0;
    cases.push(s);
    let mut s = scene();
    s.windows.push(s.windows[0].clone());
    cases.push(s);
    for invalid in cases {
        *h.observer.scene.lock().unwrap() = Ok(invalid);
        assert_denied(h.authority.authorize(None), "capture scene policy denied");
    }
    *h.observer.scene.lock().unwrap() = Ok(scene());
    h.authority.authorize(None).unwrap();
}

#[test]
fn visible_background_exclusion_is_checked_again_at_acquisition() {
    let h = Harness::new();
    let permit = h.permit();
    let mut changed = scene();
    let mut background = changed.foreground.clone();
    background.native_id = 101;
    background.app_name = "1Password".into();
    background.title.clear();
    changed.windows.push(background);
    *h.observer.scene.lock().unwrap() = Ok(changed);
    assert_denied(h.authority.authorize(None), "capture scene policy denied");
    assert_denied(
        permit.check(CaptureAccess::Acquire { display_id: 7 }),
        "capture scene policy denied",
    );
    // This earlier allowed image does not suddenly contain the new background.
    permit.check(CaptureAccess::Retain).unwrap();
    h.config
        .update_with(|c| {
            c.privacy.auto_exclude_sensitive = false;
            Ok(())
        })
        .unwrap();
    h.authority.authorize(None).unwrap();
}

#[test]
fn configured_app_and_title_exclusions_use_the_existing_policy() {
    let h = Harness::new();
    h.config
        .update_with(|c| {
            c.privacy.excluded_apps = vec!["CaptureFixture".into()];
            Ok(())
        })
        .unwrap();
    assert_denied(h.authority.authorize(None), "capture scene policy denied");
    h.config
        .update_with(|c| {
            c.privacy.excluded_apps.clear();
            c.privacy.excluded_title_patterns = vec!["*fixture*".into()];
            Ok(())
        })
        .unwrap();
    assert_denied(h.authority.authorize(None), "capture scene policy denied");
    h.config
        .update_with(|c| {
            c.privacy.excluded_title_patterns.clear();
            Ok(())
        })
        .unwrap();
    h.authority.authorize(None).unwrap();
}

#[test]
fn expected_foreground_and_display_geometry_cannot_be_retargeted() {
    let h = Harness::new();
    let s = scene();
    let mut expected = WindowInfo {
        pid: s.foreground.pid,
        app_name: s.foreground.app_name,
        title: s.foreground.title,
        app_bundle_id: None,
        bounds: Some(s.foreground.bounds),
    };
    h.authority.authorize(Some(&expected)).unwrap();
    expected.pid += 1;
    assert_denied(
        h.authority.authorize(Some(&expected)),
        "foreground observation changed",
    );
    expected.pid -= 1;
    expected.title.push_str(" changed");
    assert_denied(
        h.authority.authorize(Some(&expected)),
        "foreground observation changed",
    );
    let permit = h.permit();
    let mut changed = scene();
    changed.display_bounds.width *= 2;
    *h.observer.scene.lock().unwrap() = Ok(changed);
    assert_denied(
        permit.check(CaptureAccess::CurrentScene),
        "capture scene changed",
    );
    h.permit().check(CaptureAccess::CurrentScene).unwrap();
}

#[test]
fn native_observation_cannot_race_consent_or_pause_changes() {
    let h = Harness::new();
    let consent = h.consent.clone();
    *h.observer.on_observe.lock().unwrap() = Some(Box::new(move || {
        consent.revoke_consent().unwrap();
        consent.grant_consent(permissions(), 30).unwrap();
    }));
    assert_denied(h.authority.authorize(None), "capture consent changed");
    let permit = h.permit();
    let authority = h.authority.clone();
    *h.observer.on_observe.lock().unwrap() = Some(Box::new(move || {
        authority.toggle_pause().unwrap();
        authority.toggle_pause().unwrap();
    }));
    assert_denied(
        permit.check(CaptureAccess::CurrentScene),
        "capture pause epoch changed",
    );
    h.authority.authorize(None).unwrap();
}

#[test]
fn active_hours_battery_saver_and_erasure_are_live_policy_inputs() {
    let h = Harness::new();
    h.config
        .update_with(|c| {
            c.schedule.active_hours_enabled = true;
            c.schedule.active_start_hour = 9;
            c.schedule.active_end_hour = 17;
            c.schedule.active_days = vec![maekon_core::config::Weekday::Mon];
            c.schedule.pause_on_battery_saver = true;
            Ok(())
        })
        .unwrap();
    let permit = h.permit();
    h.environment.lock().unwrap().battery_saver_active = true;
    assert_denied(
        permit.check(CaptureAccess::Retain),
        "capture policy disallows access",
    );
    assert_denied(
        h.authority.authorize(None),
        "capture policy disallows access",
    );
    h.environment.lock().unwrap().battery_saver_active = false;
    h.authority.authorize(None).unwrap();
    h.environment.lock().unwrap().now = Local
        .with_ymd_and_hms(2026, 11, 16, 17, 0, 0)
        .single()
        .unwrap();
    assert_denied(
        permit.check(CaptureAccess::Retain),
        "capture policy disallows access",
    );
    assert_denied(
        h.authority.authorize(None),
        "capture policy disallows access",
    );
    h.environment.lock().unwrap().now = Local
        .with_ymd_and_hms(2026, 11, 16, 12, 0, 0)
        .single()
        .unwrap();
    h.consent.erasing().store(true, Ordering::SeqCst);
    assert_denied(
        h.authority.authorize(None),
        "capture policy disallows access",
    );
    h.consent.erasing().store(false, Ordering::SeqCst);
    h.authority.authorize(None).unwrap();
}

#[test]
fn unavailable_native_permission_adapter_does_not_gain_authority() {
    struct LegacyObserver;
    impl CaptureObserver for LegacyObserver {
        fn observe(&self, _: Option<u32>) -> Result<CaptureScene, CaptureObservationError> {
            panic!("permission denial must precede scene enumeration")
        }
    }
    let h = Harness::new();
    let authority = LiveCaptureAuthority::new(
        h.config,
        h.consent,
        Arc::new(CapturePauseState::new(Arc::new(AtomicBool::new(false)))),
        Arc::new(LegacyObserver),
        Arc::new(move || *h.environment.lock().unwrap()),
    );
    assert_denied(
        authority.authorize(None),
        "native capture permission unavailable",
    );
}

#[test]
fn missing_screen_consent_prevents_even_native_observation() {
    let h = Harness::new();
    h.consent
        .grant_consent(
            ConsentPermissions {
                screen_capture: false,
                ..permissions()
            },
            30,
        )
        .unwrap();
    assert_denied(
        h.authority.authorize(None),
        "capture policy disallows access",
    );
    assert_eq!(h.observer.observations.load(Ordering::SeqCst), 0);
    h.consent.grant_consent(permissions(), 30).unwrap();
    h.authority.authorize(None).unwrap();
}

#[test]
fn tracking_schedule_and_overnight_active_hours_keep_their_boundary_rules() {
    use maekon_core::config::{TrackingWindow, Weekday};

    let h = Harness::new();
    h.config
        .update_with(|c| {
            c.tracking_schedule.enabled = true;
            c.tracking_schedule.windows = vec![TrackingWindow {
                start: "12:00".into(),
                end: "13:00".into(),
                days_of_week: vec![Weekday::Mon],
                label: String::new(),
            }];
            Ok(())
        })
        .unwrap();
    let before = h.permit();
    h.environment.lock().unwrap().now = Local
        .with_ymd_and_hms(2026, 11, 16, 13, 0, 0)
        .single()
        .unwrap();
    assert_denied(
        before.check(CaptureAccess::Retain),
        "capture policy disallows access",
    );
    assert_denied(
        h.authority.authorize(None),
        "capture policy disallows access",
    );
    h.config
        .update_with(|c| {
            c.tracking_schedule.enabled = false;
            c.schedule.active_hours_enabled = true;
            c.schedule.active_start_hour = 22;
            c.schedule.active_end_hour = 6;
            c.schedule.active_days = vec![Weekday::Mon];
            Ok(())
        })
        .unwrap();
    for (day, hour, allowed) in [
        (16, 22, true),
        (17, 5, true),
        (17, 6, false),
        (17, 22, false),
    ] {
        h.environment.lock().unwrap().now = Local
            .with_ymd_and_hms(2026, 11, day, hour, 0, 0)
            .single()
            .unwrap();
        let result = h.authority.authorize(None);
        if allowed {
            result.unwrap().check(CaptureAccess::CurrentScene).unwrap();
        } else {
            assert_denied(result, "capture policy disallows access");
        }
    }
}

#[test]
fn concurrent_regrant_during_native_observation_cannot_mint_an_old_permit() {
    let h = Harness::new();
    let (observed_tx, observed_rx) = std::sync::mpsc::sync_channel(0);
    let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(0);
    *h.observer.on_observe.lock().unwrap() = Some(Box::new(move || {
        observed_tx.send(()).unwrap();
        resume_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    }));
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| h.authority.authorize(None));
        observed_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        h.consent.revoke_consent().unwrap();
        h.consent.grant_consent(permissions(), 30).unwrap();
        resume_tx.send(()).unwrap();
        assert_denied(handle.join().unwrap(), "capture consent changed");
    });
    h.authority.authorize(None).unwrap();
}

#[test]
fn permission_epoch_exhaustion_and_poisoning_deny_access() {
    let h = Harness::new();
    let old = h.permit();
    *h.authority.state.permission_epoch.lock().unwrap() = u64::MAX - 1;
    h.observer.permitted.store(false, Ordering::SeqCst);
    assert_denied(
        old.check(CaptureAccess::Retain),
        "native capture permission unavailable",
    );
    h.observer.permitted.store(true, Ordering::SeqCst);
    assert_denied(
        h.authority.authorize(None),
        "capture permission epoch exhausted",
    );

    let other = Harness::new();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _lock = other.authority.state.permission_epoch.lock().unwrap();
        panic!("injected permission state failure");
    }));
    assert_denied(
        other.authority.authorize(None),
        "capture permission state poisoned",
    );
}
