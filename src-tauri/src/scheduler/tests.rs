//! Unit tests for scheduler config and egress policy.
//! Extracted from mod.rs (ADR-013 split).

use super::*;
use crate::scheduler::egress_policy::{PlatformEgressPolicy, REDACTED_WINDOW_TITLE};
use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::consent::{ConsentManager, ConsentPermissions};
use maekon_core::models::event::{ContextEvent, Event};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use std::sync::Arc;
use std::time::Duration;

/// A `ConsentManager` with `telemetry` granted (`Valid` status). #7728 made
/// `PlatformEgressPolicy::is_enabled()` fail-closed on telemetry when no
/// ConsentManager is injected — every test below that exercises upload-path
/// behavior (title redaction, PII filtering, the egress ledger write-hook)
/// needs a granted manager so it isolates ITS OWN dimension rather than
/// tripping the (now fail-closed) telemetry gate.
fn granted_telemetry_consent_manager() -> Arc<dyn ConsentManagerPort> {
    let dir = tempfile::tempdir().expect("tempdir");
    let cm = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
    cm.grant_consent(
        ConsentPermissions {
            telemetry: true,
            ..Default::default()
        },
        30,
    )
    .expect("grant_consent");
    cm
}

#[test]
fn scheduler_config_default() {
    let config = SchedulerConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(1));
    assert_eq!(config.metrics_interval, Duration::from_secs(5));
    assert_eq!(config.idle_threshold_secs, 300);
}

#[test]
fn health_check_interval_contract_matches_spawn_cadence() {
    assert_eq!(super::config::HEALTH_CHECK_INTERVAL_SECS, 5);
}

#[test]
fn platform_sync_is_disabled_in_current_ai_runtime() {
    let config = SchedulerConfig {
        ..SchedulerConfig::default()
    };

    let policy = PlatformEgressPolicy::new(&config);
    assert!(!policy.is_enabled());
}

#[test]
fn strict_policy_redacts_window_title() {
    let config = SchedulerConfig {
        upload_enabled: true,
        external_data_policy: ExternalDataPolicy::PiiFilterStrict,
        ..SchedulerConfig::default()
    };
    let policy = PlatformEgressPolicy::new(&config)
        .with_consent_manager(Some(granted_telemetry_consent_manager()));
    let event = Event::Context(ContextEvent {
        app_name: "Chrome".to_string(),
        window_title: "Inbox user@example.com".to_string(),
        prev_app_name: None,
        timestamp: chrono::Utc::now(),
        ..Default::default()
    });

    let uploaded = policy.prepare_event_for_upload(event);
    let Some(Event::Context(ctx)) = uploaded else {
        panic!("context event should be uploadable");
    };
    assert_eq!(ctx.window_title, REDACTED_WINDOW_TITLE);
}

#[test]
fn allow_filtered_policy_uses_pii_filter() {
    let privacy = PrivacyConfig {
        pii_filter_level: PiiFilterLevel::Basic,
        ..PrivacyConfig::default()
    };
    let config = SchedulerConfig {
        upload_enabled: true,
        external_data_policy: ExternalDataPolicy::AllowFiltered,
        privacy_config: privacy,
        ..SchedulerConfig::default()
    };
    let policy = PlatformEgressPolicy::new(&config)
        .with_consent_manager(Some(granted_telemetry_consent_manager()));
    let event = Event::Context(ContextEvent {
        app_name: "Chrome".to_string(),
        window_title: "Inbox user@example.com".to_string(),
        prev_app_name: None,
        timestamp: chrono::Utc::now(),
        ..Default::default()
    });

    let uploaded = policy.prepare_event_for_upload(event);
    let Some(Event::Context(ctx)) = uploaded else {
        panic!("context event should be uploadable");
    };
    assert!(ctx.window_title.contains("[EMAIL]"));
    assert!(!ctx.window_title.contains('@'));
}

#[test]
fn sensitive_apps_are_skipped_from_upload() {
    let config = SchedulerConfig {
        ..SchedulerConfig::default()
    };
    let policy = PlatformEgressPolicy::new(&config);
    let event = Event::Context(ContextEvent {
        app_name: "Bitwarden".to_string(),
        window_title: "Vault".to_string(),
        prev_app_name: None,
        timestamp: chrono::Utc::now(),
        ..Default::default()
    });

    assert!(policy.prepare_event_for_upload(event).is_none());
}

// ── #4803: Egress audit-ledger write-hook tests ─────────────────────────
//
// Verifies the egress decision flow at the loop caller-site (compute type/size
// before consuming the event → record the uploaded/blocked disposition). The
// full Scheduler requires 16 ports + a tokio runtime, so we do not construct it;
// instead we exercise the real production write path (calling
// SqliteStorage::record_egress via `Arc<dyn SchedulerStorage>` trait dispatch)
// together with a real PlatformEgressPolicy decision. This is more honest than
// stubbing out 30 methods — it proves the trait wiring + the real SQLite write +
// the UNIQUE constraint end-to-end.
mod egress_hook_tests {
    use crate::scheduler::config::SchedulerConfig;
    use crate::scheduler::egress_policy::{
        egress_byte_count, egress_event_type, record_event_egress, PlatformEgressPolicy,
    };
    use crate::scheduler::SchedulerStorage;
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::models::event::{ClipboardContentType, ClipboardEvent, ContextEvent, Event};
    use maekon_core::models::storage_records::EgressLedgerRecord;
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::Arc;

    /// Builds an `Arc<SqliteStorage>` (concrete handle for reads) together with an
    /// `Arc<dyn SchedulerStorage>` (trait handle for the write hook) that share the
    /// same connection. Both Arcs point at the same inner value, so a write made
    /// through the trait is visible to the concrete read.
    fn storage_pair() -> (Arc<SqliteStorage>, Arc<dyn SchedulerStorage>) {
        let concrete = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"));
        let dyn_handle: Arc<dyn SchedulerStorage> = concrete.clone();
        (concrete, dyn_handle)
    }

    fn allowed_context_event() -> Event {
        Event::Context(ContextEvent {
            app_name: "Firefox".to_string(),
            window_title: "MAEKON".to_string(),
            prev_app_name: None,
            timestamp: chrono::Utc::now(),
            ..Default::default()
        })
    }

    fn clipboard_event() -> Event {
        Event::Clipboard(ClipboardEvent {
            timestamp: chrono::Utc::now(),
            content_type: ClipboardContentType::Text,
            char_count: 7,
            preview: Some("secret".to_string()),
        })
    }

    /// Reproduces the loop caller-site's egress decision flow exactly.
    /// #6134: `record_event_egress` became async (spawn_blocking offload), so
    /// this helper became async as well.
    async fn run_egress_hook(
        storage: &Arc<dyn SchedulerStorage>,
        policy: &PlatformEgressPolicy,
        event: Event,
    ) -> bool {
        let etype = egress_event_type(&event);
        let bytes = egress_byte_count(&event);
        let consent_state = policy.consent_state_snapshot();
        if policy.prepare_event_for_upload(event).is_some() {
            record_event_egress(storage, etype, bytes, "uploaded", &consent_state).await;
            true
        } else {
            record_event_egress(storage, etype, bytes, "blocked", &consent_state).await;
            false
        }
    }

    fn enabled_policy() -> PlatformEgressPolicy {
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        // #7728: is_enabled() now fail-closes telemetry when no ConsentManager
        // is injected, so this "enabled" fixture must grant it explicitly.
        PlatformEgressPolicy::new(&config)
            .with_consent_manager(Some(super::granted_telemetry_consent_manager()))
    }

    fn rows(concrete: &Arc<SqliteStorage>) -> Vec<EgressLedgerRecord> {
        concrete.recent_egress(100)
    }

    /// Upload-allowed event → exactly one disposition='uploaded' record.
    #[tokio::test]
    async fn allowed_event_writes_one_uploaded_record() {
        let (concrete, dyn_handle) = storage_pair();
        let policy = enabled_policy();
        assert!(policy.is_enabled());

        let uploaded = run_egress_hook(&dyn_handle, &policy, allowed_context_event()).await;
        assert!(uploaded, "allowed event must be uploaded");

        let recorded = rows(&concrete);
        assert_eq!(recorded.len(), 1, "exactly one record must be written");
        assert_eq!(recorded[0].disposition, "uploaded");
        assert_eq!(recorded[0].event_type, "Context");
        assert!(recorded[0].byte_count > 0);
    }

    /// Blocked event (clipboard) → one disposition='blocked' record + prepare is None.
    #[tokio::test]
    async fn blocked_event_writes_one_blocked_record() {
        let (concrete, dyn_handle) = storage_pair();
        let policy = enabled_policy();

        // prepare_event_for_upload must return None for clipboard (fail-closed).
        assert!(policy.prepare_event_for_upload(clipboard_event()).is_none());

        let uploaded = run_egress_hook(&dyn_handle, &policy, clipboard_event()).await;
        assert!(!uploaded, "clipboard event must be blocked");

        let recorded = rows(&concrete);
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].disposition, "blocked");
        assert_eq!(recorded[0].event_type, "Clipboard");
    }

    /// Without telemetry consent (is_enabled==false) there must be no 'uploaded'
    /// record. (Every event is recorded as blocked.)
    #[tokio::test]
    async fn no_uploaded_record_when_consent_revoked() {
        let (concrete, dyn_handle) = storage_pair();

        // Even with upload_enabled=true, egress is disabled without telemetry consent.
        let dir = tempfile::tempdir().expect("tempdir");
        let cm = Arc::new(ConsentManager::new(dir.path().join("consent.json")));
        cm.grant_consent(
            ConsentPermissions {
                telemetry: false,
                ..Default::default()
            },
            30,
        )
        .expect("grant_consent");
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        let policy = PlatformEgressPolicy::new(&config).with_consent_manager(Some(cm));
        assert!(
            !policy.is_enabled(),
            "egress disabled when consent is absent"
        );

        let uploaded = run_egress_hook(&dyn_handle, &policy, allowed_context_event()).await;
        assert!(!uploaded, "must not be uploaded when consent is absent");

        let recorded = rows(&concrete);
        assert!(
            recorded.iter().all(|r| r.disposition != "uploaded"),
            "no 'uploaded' record when consent is absent"
        );
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].disposition, "blocked");
        // The consent_state snapshot must retain telemetry=false so a later audit is possible.
        assert!(recorded[0].consent_state.contains("telemetry=false"));
    }
}
