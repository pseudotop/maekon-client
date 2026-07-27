//! Storage fall-through tests for recent and command-scoped audit queries.
//!
//! Covers Task 0.3.1 (PR `feat/audit-storage-fall-through`):
//! - buffer-only path (no query attached)
//! - recent storage-only fall-through and buffer/storage dedup
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
    fn stats(&self) -> AuditStats {
        let mut stats = AuditStats::default();
        for entry in &self.entries {
            match entry.status {
                AuditStatus::Completed => stats.completed += 1,
                AuditStatus::Failed => stats.failed += 1,
                AuditStatus::Denied => stats.denied += 1,
                AuditStatus::Timeout => stats.timeout += 1,
                AuditStatus::Started => {}
            }
        }
        stats.total = stats.completed + stats.failed + stats.denied + stats.timeout;
        stats
    }

    fn recent_entries(&self, limit: usize) -> Vec<AuditEntry> {
        let mut entries = self.entries.clone();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.timestamp));
        entries.truncate(limit);
        entries
    }

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

#[test]
fn recent_entries_fall_through_when_buffer_is_empty() {
    let logger = AuditLogger::new(100, 10).with_query(Arc::new(MockQuery {
        entries: vec![
            make_entry("privacy-old", "system.privacy", 200),
            make_entry("privacy-new", "system.privacy", 100),
        ],
    }));

    let results = logger.recent_entries(10);

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].entry_id, "privacy-new");
    assert_eq!(results[1].entry_id, "privacy-old");
}

#[test]
fn recent_entries_merge_buffer_and_storage_without_duplicates() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start_if(AuditLevel::Basic, "cmd-buffer", "s", "buffer-act");
    let buffered = logger.recent_entries(1)[0].clone();
    let logger = logger.with_query(Arc::new(MockQuery {
        entries: vec![
            buffered.clone(),
            make_entry("privacy-only", "system.privacy", 100),
        ],
    }));

    let results = logger.recent_entries(10);

    assert_eq!(results.len(), 2);
    assert_eq!(
        results
            .iter()
            .filter(|entry| entry.entry_id == buffered.entry_id)
            .count(),
        1
    );
    assert!(results.iter().any(|entry| entry.entry_id == "privacy-only"));
}

#[test]
fn recent_entries_consider_storage_when_buffer_already_fills_limit() {
    let mut logger = AuditLogger::new(100, 10);
    logger.log_start_if(AuditLevel::Basic, "cmd-buffer", "s", "buffer-act");
    let logger = logger.with_query(Arc::new(MockQuery {
        entries: vec![make_entry("privacy-newer", "system.privacy", -100)],
    }));

    let results = logger.recent_entries(1);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].entry_id, "privacy-newer");
}

#[test]
fn stats_use_durable_query_when_attached() {
    let completed = make_entry("completed", "system.privacy", 0);
    let mut denied = make_entry("denied", "system.privacy", 0);
    denied.status = AuditStatus::Denied;
    let mut started = make_entry("started", "system.privacy", 0);
    started.status = AuditStatus::Started;
    let logger = AuditLogger::new(100, 10).with_query(Arc::new(MockQuery {
        entries: vec![completed, denied, started],
    }));

    let stats = logger.stats();

    assert_eq!(stats.total, 2);
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.denied, 1);
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

// ── #8045 C2: audit fail-closed + best-effort drop accounting ─────

/// A persistence sink whose `persist_checked` always reports a full channel —
/// exercises the Full-level persist-rejection fail-closed path.
struct RejectingPersistence;
impl AuditPersistence for RejectingPersistence {
    fn persist(&self, _entry: &AuditEntry) {}
    fn persist_checked(&self, _entry: &AuditEntry) -> Result<(), AuditPersistError> {
        Err(AuditPersistError::ChannelFull)
    }
}

/// #8045 C2: at `AuditLevel::Full`, a forced buffer overflow makes the checked
/// start-log return `Err` (so the action is denied) instead of silently
/// dropping the oldest entry.
#[test]
fn full_level_buffer_overflow_fails_closed() {
    let mut logger = AuditLogger::new(1, 50); // capacity 1
    logger.log_start("cmd-0", "s", "act"); // fill the single slot
    assert_eq!(logger.pending_count(), 1);

    let err = logger
        .try_log_start_if(AuditLevel::Full, "cmd-1", "s", "act")
        .expect_err("Full-level overflow must fail closed");
    assert!(
        matches!(err, AuditError::BufferOverflow { capacity: 1 }),
        "expected BufferOverflow, got {err:?}"
    );
    assert!(
        logger.dropped_count() >= 1,
        "the refused record must be counted as a drop"
    );
}

/// #8045 C2: below `Full`, a buffer overflow drops the oldest best-effort and
/// increments the dropped-entries counter (observability, not fail-closed).
#[test]
fn lower_level_overflow_increments_counter() {
    let mut logger = AuditLogger::new(1, 50);
    logger.log_start_if(AuditLevel::Basic, "cmd-0", "s", "act");
    assert_eq!(logger.dropped_count(), 0);
    // Second Basic record overflows the single slot → drop-oldest + count.
    logger.log_start_if(AuditLevel::Basic, "cmd-1", "s", "act");
    assert_eq!(
        logger.dropped_count(),
        1,
        "one best-effort drop must be counted"
    );
    assert_eq!(logger.pending_count(), 1, "buffer stays at capacity");
}

/// #8045 C2: at `Full`, a persistence-sink rejection fails the record closed
/// even when the buffer has room; below `Full` the same rejection is
/// best-effort (`Ok`, still buffered locally).
#[test]
fn full_level_persist_rejection_fails_closed() {
    let mut logger =
        AuditLogger::new(1000, 50).with_persistence(std::sync::Arc::new(RejectingPersistence));

    let err = logger
        .try_log_with_status_and_time(
            AuditLevel::Full,
            "cmd-1",
            "s",
            "complete",
            AuditStatus::Completed,
            "ok",
            5,
        )
        .expect_err("Full-level persist rejection must fail closed");
    assert!(
        matches!(
            err,
            AuditError::PersistRejected(AuditPersistError::ChannelFull)
        ),
        "expected PersistRejected(ChannelFull), got {err:?}"
    );

    logger
        .try_log_with_status_and_time(
            AuditLevel::Basic,
            "cmd-2",
            "s",
            "complete",
            AuditStatus::Completed,
            "ok",
            5,
        )
        .expect("below Full a persist rejection is best-effort Ok");
    assert_eq!(
        logger.pending_count(),
        1,
        "the best-effort Basic entry is still buffered locally"
    );
}
