// ADR-013: audit module split (was 1418 lines)
// Responsibilities:
//   traits.rs              — AuditPersistence + AuditQuery port traits
//   logger.rs              — AuditLogger struct, all logging methods, PII sanitization helpers
//   adapter.rs             — AuditLogAdapter (AuditLogPort impl, bridges tokio RwLock to port)
//   channel_persistence.rs — ChannelAuditPersistence (off-reactor blocking-SQLite drain, #6123)

mod adapter;
mod channel_persistence;
mod logger;
mod traits;

// Canonical types from maekon-core — re-exported for backward compat
pub use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};

// Public surface — all callers use `maekon_automation::audit::{...}`
pub use adapter::AuditLogAdapter;
pub use channel_persistence::ChannelAuditPersistence;
pub use logger::AuditLogger;
pub use traits::{AuditPersistence, AuditQuery, SessionAuditPersistence};

#[cfg(test)]
mod tests {
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
        let cb: std::sync::Arc<dyn AuditPersistence> =
            std::sync::Arc::new(move |_: &AuditEntry| {
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
        let cb: std::sync::Arc<dyn AuditPersistence> =
            std::sync::Arc::new(move |_: &AuditEntry| {
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
}

#[cfg(test)]
mod query_tests {
    //! Storage fall-through tests for `AuditLogger::entries_by_command_id`.
    //!
    //! Covers Task 0.3.1 (PR `feat/audit-storage-fall-through`):
    //! - buffer-only path (no query attached)
    //! - storage-only fall-through (buffer empty, query has entries)
    //! - buffer + storage merge with dedup (same entry_id in both sources)
    //! - limit==0 short-circuit
    //! - limit > buffer + storage total (returns all available)
    //! - newest-first ordering after merge
    //! - dedup leaves storage-newer / buffer-older surviving variant correct

    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use std::sync::Arc;

    /// Mock implementation of `AuditQuery` returning a pre-built fixture.
    struct MockQuery {
        entries: Vec<AuditEntry>,
    }

    impl AuditQuery for MockQuery {
        fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
            // Mirror SqliteStorage contract: timestamp DESC, exact match, limit.
            let mut matching: Vec<AuditEntry> = self
                .entries
                .iter()
                .filter(|e| e.command_id == command_id)
                .cloned()
                .collect();
            matching.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
            matching.truncate(limit);
            matching
        }
    }

    fn make_entry(entry_id: &str, command_id: &str, ts_offset_ms: i64) -> AuditEntry {
        AuditEntry {
            entry_id: entry_id.to_string(),
            timestamp: Utc::now() - ChronoDuration::milliseconds(ts_offset_ms),
            session_id: "s".to_string(),
            command_id: command_id.to_string(),
            action_type: "act".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: Some(10),
        }
    }

    #[test]
    fn buffer_only_path_when_no_query_attached() {
        // No query — pure buffer walk (legacy behavior).
        let mut logger = AuditLogger::new(100, 10);
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "act");
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "act2");
        logger.log_start_if(AuditLevel::Basic, "cmd-Y", "s", "act");

        let results = logger.entries_by_command_id("cmd-X", 10);
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_eq!(r.command_id, "cmd-X");
        }
    }

    #[test]
    fn storage_only_fall_through_when_buffer_empty() {
        // Buffer has nothing for "cmd-X" but storage does — fall-through serves results.
        let mock = Arc::new(MockQuery {
            entries: vec![
                make_entry("e-1", "cmd-X", 100),
                make_entry("e-2", "cmd-X", 50),
                make_entry("e-3", "cmd-X", 0),
            ],
        });

        let logger = AuditLogger::new(100, 10).with_query(mock);
        let results = logger.entries_by_command_id("cmd-X", 10);

        assert_eq!(results.len(), 3);
        // Newest-first: e-3 (0ms), e-2 (50ms), e-1 (100ms back).
        assert_eq!(results[0].entry_id, "e-3");
        assert_eq!(results[1].entry_id, "e-2");
        assert_eq!(results[2].entry_id, "e-1");
    }

    #[test]
    fn buffer_and_storage_merge_with_dedup() {
        // Persist + query share the entry_id e-buf — dedup must NOT include twice.
        // Buffer is fed first; entry in buffer also exists in storage with same id.
        let mut logger = AuditLogger::new(100, 10);
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "buffer-act");
        // The buffer entry above was given a fresh entry_id by push_entry.
        // Capture it for the mock to return as a duplicate.
        let buf_entry = logger.recent_entries(1)[0].clone();

        let mock = Arc::new(MockQuery {
            entries: vec![
                buf_entry.clone(),                    // duplicate — must be deduped
                make_entry("e-old-1", "cmd-X", 1000), // older, not in buffer
                make_entry("e-old-2", "cmd-X", 2000), // older, not in buffer
            ],
        });

        let logger = logger.with_query(mock);
        let results = logger.entries_by_command_id("cmd-X", 10);

        // 1 from buffer + 2 unique from storage = 3 total (dup removed).
        assert_eq!(results.len(), 3);
        let mut ids: Vec<&str> = results.iter().map(|e| e.entry_id.as_str()).collect();
        ids.sort();
        // Buffer entry id ends up sorted; the two storage ids are present.
        assert!(ids.contains(&"e-old-1"));
        assert!(ids.contains(&"e-old-2"));
        assert!(ids.contains(&buf_entry.entry_id.as_str()));
        // Each id appears once.
        let unique_count = ids.iter().collect::<std::collections::HashSet<_>>().len();
        assert_eq!(unique_count, 3);
    }

    #[test]
    fn limit_zero_short_circuits_to_empty() {
        // limit=0 must early-return without consulting buffer or query.
        let mock = Arc::new(MockQuery {
            entries: vec![make_entry("e-1", "cmd-X", 0)],
        });
        let mut logger = AuditLogger::new(100, 10).with_query(mock);
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "act");

        let results = logger.entries_by_command_id("cmd-X", 0);
        assert!(results.is_empty());
    }

    #[test]
    fn limit_exceeding_total_returns_all_available() {
        // Buffer 1 + storage 2 = 3 unique; limit=10 returns all 3.
        let mut logger = AuditLogger::new(100, 10);
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "buffer-act");

        let mock = Arc::new(MockQuery {
            entries: vec![
                make_entry("e-old-1", "cmd-X", 500),
                make_entry("e-old-2", "cmd-X", 1000),
            ],
        });

        let logger = logger.with_query(mock);
        let results = logger.entries_by_command_id("cmd-X", 10);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn results_ordered_newest_first_after_merge() {
        // Buffer entries are inserted-newest, storage returns timestamp DESC.
        // After merge + re-sort the final order must be timestamp DESC.
        let mut logger = AuditLogger::new(100, 10);
        // Insert 1 buffer entry. Buffer Utc::now() will be very recent (~0ms back).
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "newest-buf");

        let mock = Arc::new(MockQuery {
            entries: vec![
                make_entry("e-mid", "cmd-X", 100),
                make_entry("e-old", "cmd-X", 200),
            ],
        });

        let logger = logger.with_query(mock);
        let results = logger.entries_by_command_id("cmd-X", 10);
        assert_eq!(results.len(), 3);
        // Newest is the buffer entry (Utc::now() ~ 0ms ago).
        assert_eq!(results[0].action_type, "newest-buf");
        // Then the mid then the old, by ascending timestamp offset.
        assert_eq!(results[1].entry_id, "e-mid");
        assert_eq!(results[2].entry_id, "e-old");
        // Verify monotonic timestamp DESC.
        for w in results.windows(2) {
            assert!(
                w[0].timestamp >= w[1].timestamp,
                "expected newest-first ordering after merge"
            );
        }
    }

    #[test]
    fn limit_truncates_after_merge() {
        // Buffer=2, storage=3, limit=3 → exactly 3 returned, newest-first.
        let mut logger = AuditLogger::new(100, 10);
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "buf-1");
        logger.log_start_if(AuditLevel::Basic, "cmd-X", "s", "buf-2");

        let mock = Arc::new(MockQuery {
            entries: vec![
                make_entry("e-1", "cmd-X", 100),
                make_entry("e-2", "cmd-X", 200),
                make_entry("e-3", "cmd-X", 300),
            ],
        });

        let logger = logger.with_query(mock);
        let results = logger.entries_by_command_id("cmd-X", 3);
        assert_eq!(results.len(), 3);
        // The two buffer entries are newest (Utc::now() ~ 0ms ago); third = e-1.
        assert_eq!(results[2].entry_id, "e-1");
    }

    #[test]
    fn other_command_id_not_leaked_through_storage() {
        // Storage contains rows for cmd-X and cmd-Y. Caller asks for cmd-X only.
        // Must not leak cmd-Y entries even when the mock filtered correctly.
        let mock = Arc::new(MockQuery {
            entries: vec![
                make_entry("e-X-1", "cmd-X", 100),
                make_entry("e-Y-1", "cmd-Y", 200),
                make_entry("e-X-2", "cmd-X", 300),
            ],
        });
        let logger = AuditLogger::new(100, 10).with_query(mock);
        let results = logger.entries_by_command_id("cmd-X", 10);
        assert_eq!(results.len(), 2);
        for r in &results {
            assert_eq!(r.command_id, "cmd-X");
        }
    }

    #[test]
    fn large_limit_returns_all_merged_no_truncation() {
        // 5 buffer entries for cmd-target + 5 different entries in storage,
        // limit = 100 → expect 10 returned, newest-first, no truncation,
        // no dedup loss (all entry_ids are distinct).
        let mut logger = AuditLogger::new(100, 10);
        // Seed buffer with 5 entries for the target command_id.
        for i in 0..5_u32 {
            logger.log_start_if(
                AuditLevel::Basic,
                "cmd-target",
                "s",
                &format!("buf-action-{i}"),
            );
        }

        // Build 5 storage entries with distinct entry_ids, older than the
        // buffer entries (offsets 500–900 ms back so they sort after buffer).
        let storage_entries: Vec<AuditEntry> = (0..5_i64)
            .map(|i| make_entry(&format!("storage-id-{i}"), "cmd-target", 500 + i * 100))
            .collect();

        let logger = logger.with_query(Arc::new(MockQuery {
            entries: storage_entries,
        }));

        let results = logger.entries_by_command_id("cmd-target", 100);
        assert_eq!(
            results.len(),
            10,
            "10 unique entries should all return when limit > total"
        );

        // All must belong to the requested command_id.
        for r in &results {
            assert_eq!(r.command_id, "cmd-target");
        }

        // Verify monotonic ordering: newest-first (timestamps DESC).
        for w in results.windows(2) {
            assert!(
                w[0].timestamp >= w[1].timestamp,
                "results must be newest-first; got {:?} before {:?}",
                w[0].timestamp,
                w[1].timestamp,
            );
        }
    }
}
