//! Capture policy epochs and the synchronous publication contract they require.

use super::*;

#[test]
fn each_image_policy_setting_advances_the_epoch_even_after_restoration() {
    use crate::config::{PiiFilterLevel, Weekday};

    let changes: &[fn(&mut AppConfig)] = &[
        |c| c.vision.capture_enabled = !c.vision.capture_enabled,
        |c| c.vision.capture_throttle_ms += 1,
        |c| c.vision.thumbnail_width += 1,
        |c| c.vision.thumbnail_height += 1,
        |c| c.vision.ocr_enabled = !c.vision.ocr_enabled,
        |c| c.vision.ocr_languages = vec!["en-US".into()],
        |c| c.vision.privacy_mode = !c.vision.privacy_mode,
        |c| c.schedule.active_hours_enabled = !c.schedule.active_hours_enabled,
        |c| c.schedule.active_start_hour = (c.schedule.active_start_hour + 1) % 24,
        |c| c.schedule.active_end_hour = (c.schedule.active_end_hour + 1) % 24,
        |c| c.schedule.pause_on_screen_lock = !c.schedule.pause_on_screen_lock,
        |c| c.schedule.pause_on_battery_saver = !c.schedule.pause_on_battery_saver,
        |c| c.schedule.active_days = vec![Weekday::Mon],
        |c| c.privacy.excluded_apps.push("Fixture".into()),
        |c| c.privacy.excluded_app_patterns.push("Fixture*".into()),
        |c| {
            c.privacy
                .excluded_title_patterns
                .push("Private fixture*".into())
        },
        |c| c.privacy.auto_exclude_sensitive = !c.privacy.auto_exclude_sensitive,
        |c| c.privacy.pii_filter_level = PiiFilterLevel::Strict,
        |c| c.tracking_schedule.enabled = !c.tracking_schedule.enabled,
        |c| c.tracking_schedule.timezone = "UTC".into(),
    ];
    let dir = TempDir::new().unwrap();
    let manager = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
    let original = manager.get();
    for (index, change) in changes.iter().enumerate() {
        let before = manager.capture_policy_snapshot().unwrap().1;
        manager
            .update_with(|c| {
                change(c);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            manager.capture_policy_snapshot().unwrap().1,
            before + 1,
            "policy setting {index}"
        );
        manager.update(original.clone()).unwrap();
        assert_eq!(
            manager.capture_policy_snapshot().unwrap().1,
            before + 2,
            "restored policy setting {index}"
        );
    }
}

#[test]
fn tracking_window_policy_changes_revoke_but_labels_do_not() {
    use crate::config::{TrackingWindow, Weekday};

    let dir = TempDir::new().unwrap();
    let manager = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
    manager
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
    let before_label = manager.capture_policy_snapshot().unwrap().1;
    manager
        .update_with(|c| {
            c.tracking_schedule.windows[0].label = "Renamed fixture".into();
            Ok(())
        })
        .unwrap();
    assert_eq!(manager.capture_policy_snapshot().unwrap().1, before_label);
    let original = manager.get();
    let changes: &[fn(&mut AppConfig)] = &[
        |c| c.tracking_schedule.windows[0].start = "11:00".into(),
        |c| c.tracking_schedule.windows[0].end = "14:00".into(),
        |c| {
            c.tracking_schedule.windows[0]
                .days_of_week
                .push(Weekday::Tue)
        },
        |c| c.tracking_schedule.windows.clear(),
        |c| {
            c.tracking_schedule
                .windows
                .push(c.tracking_schedule.windows[0].clone())
        },
    ];
    for (index, change) in changes.iter().enumerate() {
        let before = manager.capture_policy_snapshot().unwrap().1;
        manager
            .update_with(|c| {
                change(c);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            manager.capture_policy_snapshot().unwrap().1,
            before + 1,
            "tracking window {index}"
        );
        manager.update(original.clone()).unwrap();
        assert_eq!(
            manager.capture_policy_snapshot().unwrap().1,
            before + 2,
            "restored tracking window {index}"
        );
    }
}

#[test]
fn publications_keep_snapshots_and_watch_receivers_consistent() {
    let dir = TempDir::new().unwrap();
    let manager = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
    let mut receiver = manager.subscribe();
    let (initial, epoch) = manager.capture_policy_snapshot().unwrap();
    assert_eq!(epoch, 0);
    manager
        .update_with(|config| {
            config.notification.enabled = !config.notification.enabled;
            Ok(())
        })
        .unwrap();
    assert!(receiver.has_changed().unwrap());
    assert_ne!(
        receiver.borrow_and_update().notification.enabled,
        initial.notification.enabled
    );
    assert_eq!(manager.capture_policy_snapshot().unwrap().1, epoch);
    manager.update((*manager.snapshot()).clone()).unwrap();
    manager.reload().unwrap();
    assert_eq!(manager.capture_policy_snapshot().unwrap().1, epoch);

    let mut next = (*manager.snapshot()).clone();
    next.vision.capture_enabled = !initial.vision.capture_enabled;
    manager.update(next).unwrap();
    let (changed, next_epoch) = manager.capture_policy_snapshot().unwrap();
    assert_eq!(next_epoch, epoch + 1);
    assert_ne!(
        changed.vision.capture_enabled,
        initial.vision.capture_enabled
    );
    manager
        .clone()
        .update_with(|config| {
            config.vision.capture_enabled = initial.vision.capture_enabled;
            Ok(())
        })
        .unwrap();
    let (restored, restored_epoch) = manager.capture_policy_snapshot().unwrap();
    assert_eq!(
        restored.vision.capture_enabled,
        initial.vision.capture_enabled
    );
    assert_eq!(
        restored_epoch,
        epoch + 2,
        "a restored value does not restore its old epoch"
    );

    let mut on_disk = (*restored).clone();
    on_disk.privacy.excluded_apps.push("Fixture".into());
    persistence::save_to_file(manager.config_path(), &on_disk).unwrap();
    manager.reload().unwrap();
    let (reloaded, reload_epoch) = manager.capture_policy_snapshot().unwrap();
    assert_eq!(reload_epoch, restored_epoch + 1);
    assert_eq!(reloaded.privacy.excluded_apps, ["Fixture"]);
}

#[test]
fn rejected_or_unpersisted_updates_do_not_publish_a_new_epoch() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.json");
    let manager = ConfigManager::with_paths(path.clone(), None).unwrap();
    let (initial, epoch) = manager.capture_policy_snapshot().unwrap();
    let mut invalid = (*initial).clone();
    invalid.vision.capture_enabled = !initial.vision.capture_enabled;
    invalid.vision.capture_throttle_ms = 0;
    match manager.update(invalid).unwrap_err() {
        CoreError::Config { code, message } => {
            assert_eq!(code, crate::error_codes::ConfigCode::Invalid);
            assert_eq!(message, "vision.capture_throttle_ms must be >= 1000");
        }
        other => panic!("expected invalid capture throttle, got {other:?}"),
    }
    assert_eq!(manager.capture_policy_snapshot().unwrap().1, epoch);
    assert!(Arc::ptr_eq(&initial, &manager.snapshot()));

    // An owned fixture directory at the destination makes atomic persistence
    // fail without relying on platform-specific permission bits or root UID.
    fs::rename(&path, dir.path().join("preserved-config.json")).unwrap();
    fs::create_dir(&path).unwrap();
    let error = manager
        .update_with(|config| {
            config.vision.capture_enabled = !config.vision.capture_enabled;
            Ok(())
        })
        .unwrap_err();
    match error {
        CoreError::Config { code, message } => {
            assert_eq!(code, crate::error_codes::ConfigCode::Invalid);
            assert!(message.starts_with("Failed to commit config file:"));
        }
        other => panic!("expected atomic config commit failure, got {other:?}"),
    }
    assert_eq!(manager.capture_policy_snapshot().unwrap().1, epoch);
    assert!(Arc::ptr_eq(&initial, &manager.snapshot()));
}

#[test]
fn capture_epoch_exhaustion_stays_denied_without_wrapping() {
    let dir = TempDir::new().unwrap();
    let manager = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
    manager
        .inner
        .capture_policy_epoch
        .store(u64::MAX - 1, Ordering::Relaxed);
    assert_eq!(manager.capture_policy_snapshot().unwrap().1, u64::MAX - 1);
    for _ in 0..2 {
        manager
            .update_with(|config| {
                config.vision.capture_enabled = !config.vision.capture_enabled;
                Ok(())
            })
            .unwrap();
        assert!(matches!(
            manager.capture_policy_snapshot(),
            Err(CoreError::PermissionDenied { .. })
        ));
        assert_eq!(
            manager.inner.capture_policy_epoch.load(Ordering::Relaxed),
            u64::MAX
        );
    }
}

#[test]
fn concurrent_capture_snapshots_never_mix_policy_and_epoch() {
    let dir = TempDir::new().unwrap();
    let manager = ConfigManager::with_paths(dir.path().join("config.json"), None).unwrap();
    let (initial, initial_epoch) = manager.capture_policy_snapshot().unwrap();
    let barrier = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            barrier.wait();
            for _ in 0..32 {
                manager
                    .update_with(|config| {
                        config.vision.capture_enabled = !config.vision.capture_enabled;
                        Ok(())
                    })
                    .unwrap();
            }
        });
        barrier.wait();
        for _ in 0..4096 {
            let (config, epoch) = manager.capture_policy_snapshot().unwrap();
            assert_eq!(
                config.vision.capture_enabled,
                initial.vision.capture_enabled ^ ((epoch - initial_epoch) % 2 == 1)
            );
        }
        writer.join().unwrap();
    });
    let (final_config, epoch) = manager.capture_policy_snapshot().unwrap();
    assert_eq!(epoch, initial_epoch + 32);
    assert_eq!(
        final_config.vision.capture_enabled,
        initial.vision.capture_enabled
    );
}

/// T-X1-2
#[tokio::test]
async fn update_notifies_subscribers() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let mgr = ConfigManager::with_path(cfg_path).unwrap();

    let mut rx = mgr.subscribe();
    let before = rx.borrow_and_update().web.port;

    let mut new_cfg = mgr.get();
    let updated_port = shifted_test_web_port(before, 7);
    new_cfg.web.port = updated_port;
    mgr.update(new_cfg).unwrap();

    assert!(
        rx.has_changed().unwrap(),
        "update() publishes before returning"
    );
    rx.changed()
        .await
        .expect("changed() must resolve after update()");
    assert_eq!(rx.borrow().web.port, updated_port);
}

/// T-X1-3
#[tokio::test]
async fn update_with_notifies_subscribers() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let mgr = ConfigManager::with_path(cfg_path).unwrap();

    let mut rx = mgr.subscribe();
    let before = rx.borrow_and_update().web.port;

    let updated_port = shifted_test_web_port(before, 1);
    mgr.update_with(|c| {
        c.web.port = updated_port;
        Ok(())
    })
    .unwrap();

    assert!(
        rx.has_changed().unwrap(),
        "update_with() publishes before returning"
    );
    rx.changed()
        .await
        .expect("changed() must resolve after update_with()");
    assert_eq!(rx.borrow().web.port, updated_port);
}

/// T-X1-4
#[tokio::test]
async fn reload_notifies_subscribers() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let mgr = ConfigManager::with_path(cfg_path.clone()).unwrap();

    let mut rx = mgr.subscribe();
    let before = rx.borrow_and_update().web.port;

    // Rewrite the file out-of-band so reload() observes a different value.
    let mut forced = crate::config::AppConfig::default_config();
    let updated_port = shifted_test_web_port(before, 2);
    forced.web.port = updated_port;
    let json = serde_json::to_string_pretty(&forced).unwrap();
    std::fs::write(&cfg_path, json).unwrap();

    mgr.reload().unwrap();
    assert!(
        rx.has_changed().unwrap(),
        "reload() publishes before returning"
    );
    rx.changed()
        .await
        .expect("changed() must resolve after reload()");
    assert_eq!(rx.borrow().web.port, updated_port);
}

/// T-X1-7 — pins latest-wins: identical-content updates still fire.
///
/// Documents the audit-coalescing hazard described in the subscribe() doc
/// comment. Consumers that need transition-per-update semantics must diff
/// or use their own channel.
#[tokio::test]
async fn each_update_fires_even_for_identical_content() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let mgr = ConfigManager::with_path(cfg_path).unwrap();

    let mut rx = mgr.subscribe();
    rx.borrow_and_update(); // consume the initial value

    let cfg = mgr.get();
    mgr.update(cfg.clone()).unwrap();
    assert!(
        rx.has_changed().unwrap(),
        "first identical update publishes before returning"
    );
    rx.changed()
        .await
        .expect("first identical update still fires");
    rx.borrow_and_update();

    mgr.update(cfg).unwrap();
    assert!(
        rx.has_changed().unwrap(),
        "second identical update publishes before returning"
    );
    rx.changed()
        .await
        .expect("second identical update still fires");
}
