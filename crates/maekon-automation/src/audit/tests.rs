use super::*;
use chrono::Utc;

#[test]
fn log_and_drain() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start("cmd-001", "sess-001", "MouseClick");
    logger.log_complete("cmd-001", "sess-001", "Success");

    assert_eq!(logger.pending_count(), 2);
    assert!(!logger.has_pending_batch()); // 2 < 10

    let entries = logger.drain_all();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].status, AuditStatus::Started);
    assert_eq!(entries[1].status, AuditStatus::Completed);
}

#[test]
fn buffer_overflow_evicts_oldest() {
    let mut logger = AuditLogger::new(3, 2);
    logger.log_start("cmd-1", "s", "a");
    logger.log_start("cmd-2", "s", "b");
    logger.log_start("cmd-3", "s", "c");
    logger.log_start("cmd-4", "s", "d");

    assert_eq!(logger.pending_count(), 3);
    let entries = logger.drain_all();
    assert_eq!(entries[0].command_id, "cmd-2");
}

#[test]
fn drain_batch_partial() {
    let mut logger = AuditLogger::new(100, 2);
    logger.log_start("cmd-1", "s", "a");
    logger.log_start("cmd-2", "s", "b");
    logger.log_start("cmd-3", "s", "c");

    assert!(logger.has_pending_batch());
    let batch = logger.drain_batch();
    assert_eq!(batch.len(), 2);
    assert_eq!(logger.pending_count(), 1);
}

#[test]
fn audit_entry_serde() {
    let entry = AuditEntry {
        entry_id: "e-001".to_string(),
        timestamp: Utc::now(),
        session_id: "sess-001".to_string(),
        command_id: "cmd-001".to_string(),
        action_type: "MouseClick".to_string(),
        status: AuditStatus::Completed,
        details: Some("Success".to_string()),
        execution_time_ms: Some(150),
    };

    let json = serde_json::to_string(&entry).unwrap();
    let deser: AuditEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.entry_id, "e-001");
    assert_eq!(deser.status, AuditStatus::Completed);
}

#[test]
fn log_start_if_skips_on_none() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start_if(AuditLevel::None, "cmd-1", "sess-1", "KeyPress");
    assert_eq!(logger.pending_count(), 0);
}

#[test]
fn log_start_if_records_on_basic() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start_if(AuditLevel::Basic, "cmd-1", "sess-1", "KeyPress");
    assert_eq!(logger.pending_count(), 1);
    let entries = logger.drain_all();
    assert_eq!(entries[0].status, AuditStatus::Started);
}

#[test]
fn log_complete_with_time_records_execution_ms() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_complete_with_time(AuditLevel::Detailed, "cmd-1", "sess-1", "OK", 150);
    let entries = logger.drain_all();
    assert_eq!(entries[0].execution_time_ms, Some(150));
    assert_eq!(entries[0].status, AuditStatus::Completed);
}

#[test]
fn log_timeout_records_timeout_entry() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_timeout("cmd-1", "sess-1", 5000);
    let entries = logger.drain_all();
    assert_eq!(entries[0].status, AuditStatus::Timeout);
    assert_eq!(entries[0].execution_time_ms, Some(5000));
    assert!(entries[0].details.as_ref().unwrap().contains("5000ms"));
}

#[test]
fn recent_entries_nondestructive() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start("cmd-1", "s", "a");
    logger.log_complete("cmd-2", "s", "ok");
    logger.log_failed("cmd-3", "s", "err");

    let recent = logger.recent_entries(2);
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].command_id, "cmd-3");
    assert_eq!(recent[1].command_id, "cmd-2");
    assert_eq!(logger.pending_count(), 3);
}

#[test]
fn entries_by_status_filter() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start("cmd-1", "s", "a");
    logger.log_complete("cmd-2", "s", "ok");
    logger.log_denied("cmd-3", "s", "x");
    logger.log_complete("cmd-4", "s", "ok2");

    let completed = logger.entries_by_status(&AuditStatus::Completed, 10);
    assert_eq!(completed.len(), 2);
    let denied = logger.entries_by_status(&AuditStatus::Denied, 10);
    assert_eq!(denied.len(), 1);
}

#[test]
fn stats_aggregation() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start("cmd-1", "s", "a");
    logger.log_complete("cmd-2", "s", "ok");
    logger.log_failed("cmd-3", "s", "err");
    logger.log_denied("cmd-4", "s", "x");
    logger.log_timeout("cmd-5", "s", 5000);
    logger.log_complete("cmd-6", "s", "ok2");

    let stats = logger.stats();
    assert_eq!(stats.total, 5); // Started is excluded
    assert_eq!(stats.completed, 2);
    assert_eq!(stats.failed, 1);
    assert_eq!(stats.denied, 1);
    assert_eq!(stats.timeout, 1);
}

#[test]
fn log_complete_with_time_skips_on_none_level() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_complete_with_time(AuditLevel::None, "cmd-1", "sess-1", "OK", 100);
    assert_eq!(logger.pending_count(), 0);
}

#[test]
fn log_denied_has_correct_status() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_denied("cmd-1", "sess-1", "MouseClick");
    let entries = logger.drain_all();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, AuditStatus::Denied);
    assert_eq!(entries[0].action_type, "MouseClick");
}

#[test]
fn log_failed_includes_error_details() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_failed("cmd-1", "sess-1", "connection failure: timeout");
    let entries = logger.drain_all();
    assert_eq!(entries[0].status, AuditStatus::Failed);
    assert_eq!(
        entries[0].details.as_ref().unwrap(),
        "connection failure: timeout"
    );
}

#[test]
fn log_event_records_policy_event() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_event(
        "policy.scene_action_override.applied",
        "settings",
        "override=true reason=calibration",
    );

    let entries = logger.drain_all();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, AuditStatus::Completed);
    assert_eq!(
        entries[0].action_type,
        "policy.scene_action_override.applied"
    );
    assert_eq!(entries[0].session_id, "settings");
}

#[test]
fn log_event_with_status_records_denial_details() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_event_with_status(
        "privacy.external_llm.denied",
        "runtime-ai-egress",
        AuditStatus::Denied,
        "provider=test reason=consent_missing",
    );

    let entries = logger.drain_all();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].status, AuditStatus::Denied);
    assert_eq!(entries[0].action_type, "privacy.external_llm.denied");
    assert!(entries[0]
        .details
        .as_deref()
        .is_some_and(|details| details.contains("reason=consent_missing")));
}

#[test]
fn missing_pii_sanitizer_redacts_secret_detail_fields() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_complete(
        "cmd-secret",
        "sess-secret",
        "stdout=sk-live-stdout stderr=token-abc api_key=secret-123 token=refresh-456",
    );

    let details = logger.drain_all()[0].details.clone().unwrap();
    assert!(!details.contains("sk-live-stdout"));
    assert!(!details.contains("token-abc"));
    assert!(!details.contains("secret-123"));
    assert!(!details.contains("refresh-456"));
    assert!(details.contains("[REDACTED_STDOUT]"));
    assert!(details.contains("[REDACTED_STDERR]"));
    assert!(details.contains("[REDACTED_SECRET]"));
}

#[test]
fn missing_pii_sanitizer_redacts_active_window_metadata() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_event(
        "privacy.external_ocr.allowed",
        "runtime-ocr",
        "provider=openai app=Mail title=Inbox user@example.com redacted_regions=2 metadata_stripped=true",
    );

    let details = logger.drain_all()[0].details.clone().unwrap();
    assert!(!details.contains("Mail"));
    assert!(!details.contains("Inbox user@example.com"));
    assert!(details.contains("app=[REDACTED_APP]"));
    assert!(details.contains("title=[REDACTED_WINDOW_TITLE]"));
    assert!(details.contains("redacted_regions=2"));
    assert!(details.contains("metadata_stripped=true"));
}

#[test]
fn default_constructor_values() {
    let logger = AuditLogger::default();
    assert_eq!(logger.pending_count(), 0);
    assert!(!logger.has_pending_batch());
}

#[test]
fn recent_entries_with_zero_limit() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start("cmd-1", "s", "a");
    logger.log_start("cmd-2", "s", "b");
    let recent = logger.recent_entries(0);
    assert!(recent.is_empty());
}

#[test]
fn entries_by_status_empty_buffer() {
    let logger = AuditLogger::new(100, 10);
    let results = logger.entries_by_status(&AuditStatus::Completed, 10);
    assert!(results.is_empty());
}

#[test]
fn stats_on_empty_logger() {
    let logger = AuditLogger::new(100, 10);
    let stats = logger.stats();
    assert_eq!(stats.total, 0);
    assert_eq!(stats.completed, 0);
    assert_eq!(stats.failed, 0);
    assert_eq!(stats.denied, 0);
    assert_eq!(stats.timeout, 0);
}

#[test]
fn drain_batch_on_empty_logger() {
    let mut logger = AuditLogger::new(100, 10);
    let batch = logger.drain_batch();
    assert!(batch.is_empty());
}

#[test]
fn persistence_callback_invoked_on_push() {
    let persisted = std::sync::Arc::new(std::sync::Mutex::new(Vec::<AuditEntry>::new()));
    let persisted_clone = persisted.clone();
    let cb: std::sync::Arc<dyn AuditPersistence> =
        std::sync::Arc::new(move |entry: &AuditEntry| {
            persisted_clone.lock().unwrap().push(entry.clone());
        });

    let mut logger = AuditLogger::new(100, 10).with_persistence(cb);
    logger.log_start("cmd-1", "sess-1", "MouseClick");
    logger.log_complete("cmd-2", "sess-1", "ok");
    logger.log_complete_with_time(AuditLevel::Detailed, "cmd-3", "sess-1", "timed", 42);

    let entries = persisted.lock().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].action_type, "MouseClick");
    assert_eq!(entries[1].action_type, "complete");
    assert_eq!(entries[2].execution_time_ms, Some(42));
}

#[test]
fn persistence_not_called_without_callback() {
    // No persistence set — should work exactly as before.
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start("cmd-1", "sess-1", "a");
    assert_eq!(logger.pending_count(), 1);
}

#[test]
fn persistence_called_for_all_log_methods() {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_clone = count.clone();
    let cb: std::sync::Arc<dyn AuditPersistence> = std::sync::Arc::new(move |_: &AuditEntry| {
        count_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });

    let mut logger = AuditLogger::new(100, 10).with_persistence(cb);
    logger.log_start("c1", "s", "a");
    logger.log_complete("c2", "s", "ok");
    logger.log_denied("c3", "s", "denied");
    logger.log_failed("c4", "s", "err");
    logger.log_event("evt", "s", "details");
    logger.log_start_if(AuditLevel::Basic, "c5", "s", "a");
    logger.log_complete_with_time(AuditLevel::Full, "c6", "s", "ok", 10);
    logger.log_timeout("c7", "s", 5000);

    assert_eq!(
        count.load(std::sync::atomic::Ordering::Relaxed),
        8,
        "persistence should be called for all 8 log methods"
    );
}

#[test]
fn persistence_skipped_when_level_is_none() {
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_clone = count.clone();
    let cb: std::sync::Arc<dyn AuditPersistence> = std::sync::Arc::new(move |_: &AuditEntry| {
        count_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });

    let mut logger = AuditLogger::new(100, 10).with_persistence(cb);
    logger.log_start_if(AuditLevel::None, "c1", "s", "a");
    logger.log_complete_with_time(AuditLevel::None, "c2", "s", "ok", 10);

    assert_eq!(
        count.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "persistence should NOT be called when level is None"
    );
}

#[tokio::test]
async fn audit_logger_entries_by_command_id_walks_buffer() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start_if(AuditLevel::Basic, "cmd-X", "s1", "act1");
    logger.log_start_if(AuditLevel::Basic, "cmd-Y", "s2", "act1");
    logger.log_start_if(AuditLevel::Basic, "cmd-X", "s3", "act2");

    let results = logger.entries_by_command_id("cmd-X", 10);
    assert_eq!(results.len(), 2);
    for r in &results {
        assert_eq!(r.command_id, "cmd-X");
    }
}

#[tokio::test]
async fn audit_log_adapter_entries_by_command_id_delegates_to_logger() {
    use maekon_core::ports::audit_log::AuditLogPort;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    let logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
    logger
        .write()
        .await
        .log_start_if(AuditLevel::Basic, "cmd-A", "s1", "act");
    let adapter = AuditLogAdapter::new(logger);
    let results = adapter.entries_by_command_id("cmd-A", 10).await;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].command_id, "cmd-A");
}

#[test]
fn gui_state_transitions_emit_audit_entries() {
    let mut logger = AuditLogger::new(100, 50);
    let session_id = "gui-sess-001";

    // State transitions (forwarded from GuiSessionEvent broadcast)
    logger.log_event("gui.session.proposed", session_id, "Session created");
    logger.log_event(
        "gui.session.highlighted",
        session_id,
        "3 candidates highlighted",
    );
    logger.log_event(
        "gui.session.confirmed",
        session_id,
        "Element elem-001 confirmed",
    );
    logger.log_event(
        "gui.session.executing",
        session_id,
        "Executing click on elem-001",
    );
    logger.log_event(
        "gui.session.executed",
        session_id,
        "Action completed successfully",
    );

    // Denied paths
    logger.log_denied("gui-deny-001", session_id, "gui.accessibility_denied");

    // Ticket operations
    logger.log_event("gui.ticket.signed", session_id, "Ticket ticket-001 issued");
    logger.log_event(
        "gui.ticket.verified",
        session_id,
        "Ticket ticket-001 verified",
    );
    logger.log_denied("gui-deny-002", session_id, "gui.ticket.replay_rejected");

    assert_eq!(logger.pending_count(), 9);

    let completed = logger.entries_by_status(&AuditStatus::Completed, 20);
    assert_eq!(completed.len(), 7); // 5 state transitions + 2 ticket ops

    let denied = logger.entries_by_status(&AuditStatus::Denied, 20);
    assert_eq!(denied.len(), 2); // accessibility + replay

    let stats = logger.stats();
    assert_eq!(stats.completed, 7);
    assert_eq!(stats.denied, 2);
    assert_eq!(stats.total, 9);
}
