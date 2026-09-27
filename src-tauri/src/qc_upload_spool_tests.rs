#![cfg(test)]

use super::*;

#[test]
fn synthetic_upload_events_retain_their_stable_timestamps() {
    let events = synthetic_events().expect("valid fixed upload-spool timestamps");
    let timestamps: Vec<_> = events
        .iter()
        .map(|event| match event {
            Event::Context(context) => context.timestamp.timestamp(),
            _ => panic!("upload-spool fixtures must remain context events"),
        })
        .collect();
    assert_eq!(timestamps, [1_768_780_800, 1_768_780_801]);
}

#[test]
fn commands_are_exact_and_flavor_is_bounded() {
    assert!(prepare_command_requested([PREPARE_COMMAND].into_iter()));
    assert!(verify_command_requested([VERIFY_COMMAND].into_iter()));
    assert!(!prepare_command_requested(
        ["debug-prepare-qc-upload-spool-extra"].into_iter()
    ));
    assert!(is_isolated_flavor("qc-8568-upload-spool"));
    assert!(is_isolated_flavor("tc-upload_spool"));
    assert!(!is_isolated_flavor("dev"));
    assert!(!is_isolated_flavor("qc-../../real-profile"));
}

#[test]
fn ui_surface_requires_every_exact_fixture_gate() {
    let enabled = [
        Some("1"),
        Some("1"),
        Some("1"),
        Some(CONFIRM_VALUE),
        Some("qc-8568-upload-spool-ui"),
    ];
    assert!(fixture_enabled_from(
        enabled[0], enabled[1], enabled[2], enabled[3], enabled[4]
    ));

    for missing in 0..enabled.len() {
        let mut values = enabled;
        values[missing] = None;
        assert!(
            !fixture_enabled_from(values[0], values[1], values[2], values[3], values[4]),
            "fixture unexpectedly enabled with gate {missing} missing"
        );
    }
    assert!(!fixture_enabled_from(
        Some("true"),
        Some("1"),
        Some("1"),
        Some(CONFIRM_VALUE),
        Some("qc-8568-upload-spool-ui")
    ));
    assert!(!fixture_enabled_from(
        Some("1"),
        Some("1"),
        Some("1"),
        Some("yes"),
        Some("qc-8568-upload-spool-ui")
    ));
    assert!(!fixture_enabled_from(
        Some("1"),
        Some("1"),
        Some("1"),
        Some(CONFIRM_VALUE),
        Some("production")
    ));
}

#[test]
fn isolated_profile_disables_sensitive_capabilities() {
    let mut config = AppConfig::default_config();
    config.vision.capture_enabled = true;
    config.audio.enabled = true;
    config.audio.cloud_api_key = "synthetic-secret".to_string();
    config.sync.enabled = true;
    config.integration.enabled = true;
    config.telemetry.enabled = true;
    config.telemetry.crash_reports = true;
    config.telemetry.usage_analytics = true;
    config.telemetry.performance_metrics = true;
    config.web.allow_external = true;
    config.external_grpc.enabled = true;
    config.automation.enabled = true;
    config.update.enabled = true;
    config.update.auto_install = true;

    configure_isolated_profile(&mut config);

    ensure_isolated_profile(&config).expect("profile must be fail-closed");
    assert!(!config.update.enabled);
    assert!(!config.update.auto_install);
}

#[tokio::test]
async fn interruption_then_reprime_marks_only_confirmed_ids() {
    let temp = tempfile::tempdir().expect("temp dir");
    let key = EncryptionKey::from_bytes([0x89; 32]);

    let interrupted = prepare_fixture(temp.path(), key.clone(), 30)
        .await
        .expect("prepare interrupted spool");
    assert_eq!(interrupted.phase, "interrupted");
    assert_eq!(interrupted.seeded, 2);
    assert_eq!(interrupted.confirmed, 0);
    assert_eq!(interrupted.pending, 2);
    assert_eq!(interrupted.upload_attempts, 1);
    assert_eq!(interrupted.egress_ledger_entries, 0);

    let interrupted_state = read_state(temp.path()).expect("read interrupted state");
    assert!(interrupted_state.confirmed_storage_ids.is_empty());
    assert!(!interrupted_state.sent_markers_written_after_success);
    assert_eq!(interrupted_state.pending_storage_ids.len(), 2);

    let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
        .expect("reopen interrupted spool for unrelated metric");
    storage
        .save_event(&Event::Context(ContextEvent {
            app_name: "maekon-qc-os-metric".to_string(),
            window_title: "Synthetic unrelated OS metric".to_string(),
            timestamp: Utc
                .timestamp_opt(1_768_780_900, 0)
                .single()
                .expect("fixed QC timestamp must be valid"),
            ..Default::default()
        }))
        .await
        .expect("persist unrelated metric");
    drop(storage);

    let verified = verify_fixture(temp.path(), key.clone(), 30)
        .await
        .expect("verify re-primed spool");
    assert_eq!(verified.phase, "verified");
    assert_eq!(verified.seeded, 2);
    assert_eq!(verified.confirmed, 2);
    assert_eq!(verified.pending, 0);
    assert_eq!(verified.upload_attempts, 2);
    assert_eq!(verified.egress_ledger_entries, 0);

    let verified_state = read_state(temp.path()).expect("read verified state");
    assert!(verified_state.sent_markers_written_after_success);
    assert_eq!(verified_state.confirmed_storage_ids.len(), 2);
    assert!(verified_state.pending_storage_ids.is_empty());
    assert!(!verified_state.external_egress_enabled);
    assert!(!verified_state.host_mutation);

    let storage = SqliteStorage::open(&temp.path().join("maekon.db"), 30, Some(&key))
        .expect("reopen verified spool");
    let remaining = storage
        .get_pending_events(10)
        .await
        .expect("read unrelated pending metric");
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        event_ids(&remaining),
        event_ids(&[Event::Context(ContextEvent {
            app_name: "maekon-qc-os-metric".to_string(),
            window_title: "Synthetic unrelated OS metric".to_string(),
            timestamp: Utc
                .timestamp_opt(1_768_780_900, 0)
                .single()
                .expect("fixed QC timestamp must be valid"),
            ..Default::default()
        })])
    );
}
