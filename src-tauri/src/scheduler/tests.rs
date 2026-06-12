//! Unit tests for scheduler config and egress policy.
//! Extracted from mod.rs (ADR-013 split).

use super::*;
use crate::scheduler::config::{PlatformEgressPolicy, REDACTED_WINDOW_TITLE};
use maekon_core::config::{ExternalDataPolicy, PiiFilterLevel, PrivacyConfig};
use maekon_core::models::event::{ContextEvent, Event};
use std::time::Duration;

#[test]
fn scheduler_config_default() {
    let config = SchedulerConfig::default();
    assert_eq!(config.poll_interval, Duration::from_secs(1));
    assert_eq!(config.metrics_interval, Duration::from_secs(5));
    assert_eq!(config.idle_threshold_secs, 300);
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
    let policy = PlatformEgressPolicy::new(&config);
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
    let policy = PlatformEgressPolicy::new(&config);
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

// ── #4803: Egress 감사 원장 write-hook 테스트 ────────────────────────────
//
// 루프 caller-site 의 egress 결정 흐름(소비 전 유형/크기 계산 → uploaded/blocked
// disposition 기록)을 검증한다. 전체 Scheduler 는 16개 포트 + tokio 런타임을
// 요구하므로 구성하지 않고, 실제 production write 경로(SqliteStorage::record_egress
// 를 `Arc<dyn SchedulerStorage>` 트레이트 디스패치로 호출)와 실제 PlatformEgressPolicy
// 결정을 함께 행사한다. 이는 30개 메서드 stub 보다 정직하다 — 트레이트 배선 +
// 실제 SQLite 기록 + UNIQUE 제약까지 end-to-end 로 증명한다.
mod egress_hook_tests {
    use crate::scheduler::config::{
        egress_byte_count, egress_event_type, record_event_egress, PlatformEgressPolicy,
        SchedulerConfig, SchedulerStorage,
    };
    use maekon_core::consent::{ConsentManager, ConsentPermissions};
    use maekon_core::models::event::{ClipboardContentType, ClipboardEvent, ContextEvent, Event};
    use maekon_core::models::storage_records::EgressLedgerRecord;
    use maekon_storage::sqlite::SqliteStorage;
    use std::sync::Arc;

    /// `Arc<SqliteStorage>`(read 용 구체 핸들)과 동일 커넥션을 공유하는
    /// `Arc<dyn SchedulerStorage>`(write hook 용 트레이트 핸들)를 함께 만든다.
    /// 두 Arc 는 같은 내부 값을 가리키므로 trait 경유 write 가 구체 read 로 보인다.
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

    /// 루프 caller-site 의 egress 결정 흐름을 그대로 재현한다.
    fn run_egress_hook(
        storage: &Arc<dyn SchedulerStorage>,
        policy: &PlatformEgressPolicy,
        event: Event,
    ) -> bool {
        let etype = egress_event_type(&event);
        let bytes = egress_byte_count(&event);
        let consent_state = policy.consent_state_snapshot();
        if policy.prepare_event_for_upload(event).is_some() {
            record_event_egress(storage, etype, bytes, "uploaded", &consent_state);
            true
        } else {
            record_event_egress(storage, etype, bytes, "blocked", &consent_state);
            false
        }
    }

    fn enabled_policy() -> PlatformEgressPolicy {
        let config = SchedulerConfig {
            upload_enabled: true,
            ..Default::default()
        };
        PlatformEgressPolicy::new(&config)
    }

    fn rows(concrete: &Arc<SqliteStorage>) -> Vec<EgressLedgerRecord> {
        concrete.recent_egress(100)
    }

    /// 업로드 허용 이벤트 → disposition='uploaded' 정확히 1건.
    #[test]
    fn allowed_event_writes_one_uploaded_record() {
        let (concrete, dyn_handle) = storage_pair();
        let policy = enabled_policy();
        assert!(policy.is_enabled());

        let uploaded = run_egress_hook(&dyn_handle, &policy, allowed_context_event());
        assert!(uploaded, "허용 이벤트는 업로드되어야 한다");

        let recorded = rows(&concrete);
        assert_eq!(recorded.len(), 1, "정확히 1건이 기록되어야 한다");
        assert_eq!(recorded[0].disposition, "uploaded");
        assert_eq!(recorded[0].event_type, "Context");
        assert!(recorded[0].byte_count > 0);
    }

    /// 차단 이벤트(클립보드) → disposition='blocked' 1건 + prepare 는 None.
    #[test]
    fn blocked_event_writes_one_blocked_record() {
        let (concrete, dyn_handle) = storage_pair();
        let policy = enabled_policy();

        // prepare_event_for_upload 는 클립보드를 None 으로 반환해야 한다(fail-closed).
        assert!(policy.prepare_event_for_upload(clipboard_event()).is_none());

        let uploaded = run_egress_hook(&dyn_handle, &policy, clipboard_event());
        assert!(!uploaded, "클립보드 이벤트는 차단되어야 한다");

        let recorded = rows(&concrete);
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].disposition, "blocked");
        assert_eq!(recorded[0].event_type, "Clipboard");
    }

    /// 텔레메트리 동의가 없으면(is_enabled==false) 'uploaded' 레코드는 없어야 한다.
    /// (모든 이벤트가 blocked 로 기록된다.)
    #[test]
    fn no_uploaded_record_when_consent_revoked() {
        let (concrete, dyn_handle) = storage_pair();

        // upload_enabled=true 라도 telemetry 동의가 없으면 egress 비활성.
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
        assert!(!policy.is_enabled(), "동의 부재 시 egress 비활성");

        let uploaded = run_egress_hook(&dyn_handle, &policy, allowed_context_event());
        assert!(!uploaded, "동의 부재 시 업로드되지 않아야 한다");

        let recorded = rows(&concrete);
        assert!(
            recorded.iter().all(|r| r.disposition != "uploaded"),
            "동의 부재 시 'uploaded' 레코드가 없어야 한다"
        );
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].disposition, "blocked");
        // consent_state 스냅샷에 telemetry=false 가 남아 사후 감사가 가능해야 한다.
        assert!(recorded[0].consent_state.contains("telemetry=false"));
    }
}
