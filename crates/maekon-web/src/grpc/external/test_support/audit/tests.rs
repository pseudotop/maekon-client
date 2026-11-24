use std::sync::Arc;
use std::time::Duration;

use maekon_core::models::audit::{AuditLevel, AuditStatus};
use maekon_core::ports::audit_log::AuditLogPort;

use crate::grpc::external::conn_info::AuthContext;
use crate::grpc::external::test_support::{fixture_bridge, CapturingAudit};

#[tokio::test]
async fn typed_capture_keeps_status_correlation_time_and_raw_details() {
    let audit = CapturingAudit::new();
    let details = " {\"result\":\"ok\",\"grpc_status_code\":7,\"note\":\"한글\"} \n";
    for status in [
        AuditStatus::Started,
        AuditStatus::Completed,
        AuditStatus::Denied,
        AuditStatus::Timeout,
        AuditStatus::Failed,
    ] {
        audit
            .log_with_status_and_time(
                AuditLevel::Full,
                "command",
                "session",
                "specific-action",
                status.clone(),
                details,
                u64::MAX,
            )
            .await;
        let entry = audit.snapshot().pop().unwrap();
        assert_eq!(
            entry.status, status,
            "typed status must override JSON result"
        );
        assert_eq!(entry.command_id, "command");
        assert_eq!(entry.action_type, "specific-action");
        assert_eq!(entry.grpc_status_code, Some(7));
        assert_eq!(entry.execution_time_ms, u64::MAX);
        assert_eq!(entry.details.as_deref(), Some(details));
    }
    assert_eq!(audit.snapshot().len(), 5);
}

#[tokio::test]
async fn legacy_completion_preserves_status_and_unparsed_payload() {
    let audit = CapturingAudit::new();
    let cases = [
        ("ok", AuditStatus::Completed),
        ("denied", AuditStatus::Denied),
        ("timeout", AuditStatus::Timeout),
        ("error", AuditStatus::Failed),
        ("failed", AuditStatus::Failed),
        ("auth_failed", AuditStatus::Failed),
        ("legacy-extension", AuditStatus::Completed),
    ];
    for (result, expected) in cases {
        let details = format!(" {{\"result\":\"{result}\",\"grpc_status_code\":16}} ");
        audit
            .log_complete_with_time(AuditLevel::Basic, result, "session", &details, 42)
            .await;
        let entry = audit.snapshot().pop().unwrap();
        assert_eq!(entry.status, expected);
        assert_eq!(entry.command_id, result);
        assert_eq!(entry.action_type, "");
        assert_eq!(entry.grpc_status_code, Some(16));
        assert_eq!(entry.execution_time_ms, 42);
        assert_eq!(entry.details.as_deref(), Some(details.as_str()));
    }
    for details in ["", "invalid json", "{}", "{\"result\":false}"] {
        audit
            .log_complete_with_time(AuditLevel::Basic, "raw", "session", details, 0)
            .await;
        let entry = audit.snapshot().pop().unwrap();
        assert_eq!(entry.status, AuditStatus::Completed, "legacy default");
        assert_eq!(entry.grpc_status_code, None);
        assert_eq!(entry.details.as_deref(), Some(details));
    }
}

#[tokio::test]
async fn raw_grpc_code_rejects_overflow_and_wrong_json_types() {
    let audit = CapturingAudit::new();
    for (value, expected) in [
        ("0", Some(0)),
        ("7", Some(7)),
        ("4294967295", Some(u32::MAX)),
        ("4294967296", None),
        ("18446744073709551615", None),
        ("-1", None),
        ("1.5", None),
        ("\"7\"", None),
        ("null", None),
    ] {
        let details = format!("{{\"grpc_status_code\":{value}}}");
        audit
            .log_complete_with_time(AuditLevel::Full, "legacy", "session", &details, 0)
            .await;
        audit
            .log_with_status_and_time(
                AuditLevel::Full,
                "typed",
                "session",
                "action",
                AuditStatus::Failed,
                &details,
                1,
            )
            .await;
        let entries = audit.snapshot();
        for entry in &entries[entries.len() - 2..] {
            assert_eq!(entry.grpc_status_code, expected, "raw code {value}");
            assert_eq!(entry.details.as_deref(), Some(details.as_str()));
        }
    }
}

#[tokio::test]
async fn legacy_events_preserve_denied_and_timeout_statuses() {
    let audit = CapturingAudit::new();
    for (action, status) in [
        ("external_grpc_started", AuditStatus::Started),
        ("external_grpc_completed", AuditStatus::Completed),
        ("external_grpc_denied", AuditStatus::Denied),
        ("external_grpc_timeout", AuditStatus::Timeout),
        ("external_grpc_failed", AuditStatus::Failed),
        ("unclassified-event", AuditStatus::Completed),
    ] {
        audit.log_event(action, "session", "raw event").await;
        let entry = audit.snapshot().pop().unwrap();
        assert_eq!(entry.status, status);
        assert_eq!(entry.action_type, action);
        assert_eq!(entry.command_id, "");
        assert_eq!(entry.execution_time_ms, 0);
        assert_eq!(entry.details.as_deref(), Some("raw event"));
    }
}

#[tokio::test]
async fn capturing_audit_remains_usable_after_a_producer_panics() {
    let audit = CapturingAudit::new();
    audit
        .log_start_if(AuditLevel::Basic, "before", "session", "start")
        .await;
    let producer = Arc::clone(&audit);
    let failure = std::thread::spawn(move || {
        let _guard = producer.entries.lock();
        panic!("injected producer failure while holding the actual fixture lock");
    })
    .join()
    .expect_err("the capture producer must emit the injected panic");
    assert_eq!(
        failure.downcast_ref::<&str>(),
        Some(&"injected producer failure while holding the actual fixture lock")
    );
    audit
        .log_start_if(AuditLevel::Basic, "after", "session", "resume")
        .await;
    let entries = audit.snapshot();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].command_id, "before");
    assert_eq!(entries[1].command_id, "after");
    assert_eq!(entries[1].action_type, "resume");
    assert!(entries
        .iter()
        .all(|e| e.status == AuditStatus::Started && e.details.is_none()));
}

#[tokio::test]
async fn durable_recorder_retains_entries_after_a_producer_panics() {
    let (_, recorder) = fixture_bridge();
    recorder
        .log_complete_with_time(AuditLevel::Full, "before", "s", "{}", 1)
        .await;
    let producer = Arc::clone(&recorder);
    let failure = std::thread::spawn(move || {
        let _guard = producer.entries.lock();
        panic!("injected durable-recorder producer failure");
    })
    .join()
    .expect_err("the durable producer must emit the injected panic");
    assert_eq!(
        failure.downcast_ref::<&str>(),
        Some(&"injected durable-recorder producer failure")
    );
    recorder
        .log_complete_with_time(AuditLevel::Full, "after", "s", "{}", 2)
        .await;
    let entries = recorder.snapshot();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].command_id, "before");
    assert_eq!(entries[1].command_id, "after");
    assert_eq!(entries[1].execution_time_ms, Some(2));
    assert_ne!(entries[0].entry_id, entries[1].entry_id);
}

#[tokio::test]
async fn real_bridge_keeps_explicit_status_and_action_in_durable_fixture() {
    let (bridge, recorder) = fixture_bridge();
    let ctx = AuthContext::fixture();
    for (status, action) in [
        (AuditStatus::Started, "external_grpc_started"),
        (AuditStatus::Completed, "external_grpc_completed"),
        (AuditStatus::Denied, "external_grpc_denied"),
        (AuditStatus::Timeout, "external_grpc_timeout"),
        (AuditStatus::Failed, "external_grpc_failed"),
    ] {
        let returned = bridge
            .record(
                &ctx,
                "127.0.0.1:50001".into(),
                "ContractProbe",
                "ok",
                status.clone(),
                Duration::from_millis(17),
                Some(3),
                Some(5),
                None,
                Some("request-override".into()),
            )
            .await;
        assert_eq!(returned, "request-override");
        let entry = recorder.snapshot().pop().unwrap();
        assert_eq!(entry.status, status);
        assert_eq!(entry.action_type, action);
        assert_eq!(entry.command_id, "request-override");
        assert_eq!(entry.session_id, ctx.client_id);
        assert_eq!(entry.execution_time_ms, Some(17));
        let details: serde_json::Value =
            serde_json::from_str(entry.details.as_deref().unwrap()).unwrap();
        assert_eq!(details["operation"], "ContractProbe");
        assert_eq!(details["request_size_bytes"], 3);
        assert_eq!(details["response_size_bytes"], 5);
    }
    assert_eq!(recorder.snapshot().len(), 5);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_producers_retain_start_terminal_pairs_and_independent_snapshots() {
    let audit = CapturingAudit::new();
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..64_u64 {
        let audit = Arc::clone(&audit);
        tasks.spawn(async move {
            let command = format!("command-{index}");
            audit
                .log_start_if(AuditLevel::Full, &command, "session", "start")
                .await;
            tokio::task::yield_now().await;
            let status = [
                AuditStatus::Completed,
                AuditStatus::Denied,
                AuditStatus::Timeout,
                AuditStatus::Failed,
            ][(index % 4) as usize]
                .clone();
            audit
                .log_with_status_and_time(
                    AuditLevel::Full,
                    &command,
                    "session",
                    "terminal",
                    status,
                    "{\"grpc_status_code\":7}",
                    index,
                )
                .await;
        });
    }
    while let Some(outcome) = tasks.join_next().await {
        outcome.unwrap();
    }
    let frozen = audit.snapshot();
    assert_eq!(frozen.len(), 128);
    for index in 0..64_u64 {
        let command = format!("command-{index}");
        let pair: Vec<_> = frozen
            .iter()
            .filter(|entry| entry.command_id == command)
            .collect();
        assert_eq!(
            pair.len(),
            2,
            "exactly one start and one terminal per command"
        );
        assert_eq!(pair[0].status, AuditStatus::Started);
        assert_eq!(pair[0].action_type, "start");
        assert_eq!(pair[0].details, None);
        assert_eq!(pair[1].action_type, "terminal");
        assert_eq!(
            pair[1].status,
            [
                AuditStatus::Completed,
                AuditStatus::Denied,
                AuditStatus::Timeout,
                AuditStatus::Failed
            ][(index % 4) as usize]
        );
        assert_eq!(pair[1].grpc_status_code, Some(7));
        assert_eq!(pair[1].execution_time_ms, index);
        assert_eq!(pair[1].details.as_deref(), Some("{\"grpc_status_code\":7}"));
    }
    audit
        .log_start_if(AuditLevel::Basic, "later", "session", "start")
        .await;
    assert_eq!(
        frozen.len(),
        128,
        "snapshots must not alias the live collection"
    );
    assert_eq!(audit.snapshot().len(), 129);
}

#[tokio::test]
async fn capturing_query_stubs_do_not_consume_recorded_entries() {
    let audit = CapturingAudit::new();
    audit
        .log_start_if(AuditLevel::Full, "command", "session", "start")
        .await;
    assert_eq!(audit.pending_count().await, 0);
    assert!(!audit.has_pending_batch().await);
    assert!(audit.recent_entries(10).await.is_empty());
    assert!(audit
        .entries_by_status(&AuditStatus::Started, 10)
        .await
        .is_empty());
    assert!(audit.entries_by_action_prefix("start", 10).await.is_empty());
    assert!(audit.entries_by_command_id("command", 10).await.is_empty());
    assert!(audit.drain_batch().await.is_empty());
    assert!(audit.drain_all().await.is_empty());
    assert_eq!(
        audit.snapshot().len(),
        1,
        "only snapshot exposes this capture-only fixture"
    );
}

#[tokio::test]
async fn durable_fixture_preserves_legacy_results_without_query_side_effects() {
    let (_, recorder) = fixture_bridge();
    let cases = [
        ("{\"result\":\"ok\"}", AuditStatus::Completed),
        ("{\"result\":\"denied\"}", AuditStatus::Denied),
        ("{\"result\":\"timeout\"}", AuditStatus::Timeout),
        ("{\"result\":\"error\"}", AuditStatus::Failed),
        ("{\"result\":\"failed\"}", AuditStatus::Failed),
        ("{\"result\":\"auth_failed\"}", AuditStatus::Failed),
        ("{\"result\":\"extension\"}", AuditStatus::Failed),
        ("{\"result\":false}", AuditStatus::Completed),
        ("raw details: 한글", AuditStatus::Completed),
    ];
    for (index, (details, expected)) in cases.iter().enumerate() {
        recorder
            .log_complete_with_time(
                AuditLevel::Full,
                &format!("legacy-{index}"),
                "legacy-session",
                details,
                u64::MAX,
            )
            .await;
        let row = recorder.snapshot().pop().unwrap();
        assert_eq!(&row.status, expected);
        assert_eq!(row.command_id, format!("legacy-{index}"));
        assert_eq!(row.session_id, "legacy-session");
        assert_eq!(row.action_type, "external_grpc");
        assert_eq!(row.details.as_deref(), Some(*details));
        assert_eq!(row.execution_time_ms, Some(u64::MAX));
    }
    assert_eq!(recorder.pending_count().await, 0);
    assert!(!recorder.has_pending_batch().await);
    assert!(recorder.recent_entries(1).await.is_empty());
    assert!(recorder
        .entries_by_status(&AuditStatus::Completed, 1)
        .await
        .is_empty());
    assert!(recorder
        .entries_by_action_prefix("external", 1)
        .await
        .is_empty());
    assert!(recorder
        .entries_by_command_id("legacy-0", 1)
        .await
        .is_empty());
    assert!(recorder.drain_batch().await.is_empty());
    assert!(recorder.drain_all().await.is_empty());
    let stats = recorder.stats().await;
    assert_eq!(
        (
            stats.total,
            stats.completed,
            stats.failed,
            stats.denied,
            stats.timeout
        ),
        (0, 0, 0, 0, 0)
    );
    assert_eq!(recorder.snapshot().len(), cases.len());
}

#[tokio::test]
async fn noop_fixture_accepts_real_bridge_calls_without_retaining_rows() {
    use crate::grpc::external::audit_bridge::AuditBridge;
    use crate::grpc::external::test_support::NoopAudit;

    let recorder: Arc<dyn AuditLogPort> = Arc::new(NoopAudit);
    let bridge = AuditBridge::new(Arc::clone(&recorder));
    let context = AuthContext::fixture();
    for index in 0..64 {
        let command = format!("noop-{index}");
        assert_eq!(
            bridge
                .record(
                    &context,
                    "127.0.0.1:50001".into(),
                    "NoopProbe",
                    "denied",
                    AuditStatus::Denied,
                    Duration::ZERO,
                    None,
                    None,
                    None,
                    Some(command.clone())
                )
                .await,
            command
        );
    }
    assert_eq!(recorder.pending_count().await, 0);
    assert!(!recorder.has_pending_batch().await);
    assert!(recorder.recent_entries(1).await.is_empty());
    assert!(recorder
        .entries_by_status(&AuditStatus::Denied, 1)
        .await
        .is_empty());
    assert!(recorder
        .entries_by_action_prefix("external", 1)
        .await
        .is_empty());
    assert!(recorder.entries_by_command_id("noop-0", 1).await.is_empty());
    assert!(recorder.drain_batch().await.is_empty());
    assert!(recorder.drain_all().await.is_empty());
    let stats = recorder.stats().await;
    assert_eq!(
        (
            stats.total,
            stats.completed,
            stats.failed,
            stats.denied,
            stats.timeout
        ),
        (0, 0, 0, 0, 0)
    );
}

#[tokio::test]
async fn public_factories_create_independent_audit_and_metrics_state() {
    use crate::grpc::external::test_support::fixture_metrics;

    let (_, first) = fixture_bridge();
    let (_, second) = fixture_bridge();
    first
        .log_complete_with_time(AuditLevel::Basic, "first", "session", "{}", 1)
        .await;
    assert_eq!(first.snapshot().len(), 1);
    assert!(second.snapshot().is_empty());
    let first_metrics = fixture_metrics();
    let second_metrics = fixture_metrics();
    first_metrics.request_bump("external", "jwt", "ok");
    first_metrics.auth_failure_bump("invalid_jwt");
    assert_eq!(first_metrics.get_request_count("external|jwt|ok"), 1);
    assert_eq!(first_metrics.get_auth_failure_count("invalid_jwt"), 1);
    assert_eq!(second_metrics.get_request_count("external|jwt|ok"), 0);
    assert_eq!(second_metrics.get_auth_failure_count("invalid_jwt"), 0);
}
