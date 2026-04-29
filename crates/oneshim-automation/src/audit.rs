use chrono::Utc;
use oneshim_core::config::PiiFilterLevel;
use oneshim_core::ports::pii_sanitizer::PiiSanitizer;
use std::collections::VecDeque;
use std::sync::Arc;

// Canonical types from oneshim-core — re-exported for backward compat
pub use oneshim_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};

/// Callback trait for persisting audit entries to durable storage.
///
/// Implemented by the binary crate to bridge AuditLogger (library) with
/// SQLite (infrastructure), preserving hexagonal architecture boundaries.
pub trait AuditPersistence: Send + Sync {
    fn persist(&self, entry: &AuditEntry);
}

/// Blanket impl: any `Fn(&AuditEntry) + Send + Sync` satisfies `AuditPersistence`.
impl<F: Fn(&AuditEntry) + Send + Sync> AuditPersistence for F {
    fn persist(&self, entry: &AuditEntry) {
        self(entry);
    }
}

/// Query interface for historical audit lookup.
///
/// Implemented by the binary crate to bridge `AuditLogger` (library) with
/// SQLite-backed historical storage, preserving hexagonal architecture
/// boundaries (`oneshim-automation` cannot depend on `oneshim-storage`
/// directly per ADR-001).
///
/// Used by [`AuditLogger::entries_by_command_id`] to fall through from the
/// in-memory `VecDeque` buffer (~1000-row cap) to persistent storage when
/// the buffer doesn't have enough matching entries.
///
/// # Invariant
///
/// Implementations MUST return entries with stable, unique `entry_id`. The
/// dedupe step in [`AuditLogger::entries_by_command_id`] keys on `entry_id`
/// to merge buffer + storage results — the same `entry_id` MUST always carry
/// the same logical entry (same timestamp, same details). The production
/// `SqliteAuditQuery` satisfies this via the V25 schema's
/// `UNIQUE(entry_id)` constraint. Custom implementations that violate
/// this invariant may silently drop legitimate entries during dedup.
pub trait AuditQuery: Send + Sync {
    /// Return audit entries whose `command_id` exactly matches.
    /// Ordered by `timestamp DESC`. Empty vec if none match.
    /// Synchronous — implementations doing I/O should use `block_in_place`.
    fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry>;
}

pub struct AuditLogger {
    buffer: VecDeque<AuditEntry>,
    max_buffer_size: usize,
    batch_size: usize,
    persistence: Option<Arc<dyn AuditPersistence>>,
    /// Storage-backed historical query handle. When set, `entries_by_command_id`
    /// falls through to this after exhausting the in-memory buffer.
    query: Option<Arc<dyn AuditQuery>>,
    /// D5 iter-6: Audit log details may include command stdout/stderr which
    /// can contain API keys, tokens, or other sensitive output. Apply the
    /// strictest PII filtering unconditionally at the record boundary (not
    /// user-configurable — audit log is a security control, not a feature).
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
}

impl AuditLogger {
    pub fn new(max_buffer_size: usize, batch_size: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(max_buffer_size),
            max_buffer_size,
            batch_size,
            persistence: None,
            query: None,
            pii_sanitizer: None,
        }
    }

    /// Attach a persistence callback for durable storage of audit entries.
    ///
    /// When set, every new audit entry is forwarded to this callback
    /// immediately after being added to the in-memory buffer.
    pub fn with_persistence(mut self, cb: Arc<dyn AuditPersistence>) -> Self {
        self.persistence = Some(cb);
        self
    }

    /// Attach a query handle for historical (storage-backed) audit lookup.
    ///
    /// When set, [`Self::entries_by_command_id`] falls through to this query
    /// handle after consulting the in-memory buffer, merging results and
    /// deduplicating by `entry_id`. Use the matching binary-crate wrapper
    /// (e.g., `SqliteAuditQuery` in `src-tauri`) to bridge to storage —
    /// `oneshim-automation` itself MUST NOT depend on `oneshim-storage`.
    pub fn with_query(mut self, q: Arc<dyn AuditQuery>) -> Self {
        self.query = Some(q);
        self
    }

    /// D5 iter-6: attach a PII sanitizer. Audit log applies
    /// `PiiFilterLevel::Strict` unconditionally (not user-configurable per
    /// O3 in the D5 design spec) — audit trails are a security control.
    pub fn with_pii_sanitizer(mut self, sanitizer: Arc<dyn PiiSanitizer>) -> Self {
        self.pii_sanitizer = Some(sanitizer);
        self
    }

    /// D5 iter-6: sanitize a details string for audit storage.
    fn sanitize_details(&self, details: Option<String>) -> Option<String> {
        details.map(|raw| {
            self.pii_sanitizer
                .as_ref()
                .map(|s| s.sanitize_text(&raw, PiiFilterLevel::Strict))
                .unwrap_or(raw)
        })
    }

    pub fn log_start(&mut self, command_id: &str, session_id: &str, action_type: &str) {
        self.push_entry(
            command_id,
            session_id,
            action_type,
            AuditStatus::Started,
            None,
        );
    }

    pub fn log_complete(&mut self, command_id: &str, session_id: &str, details: &str) {
        self.push_entry(
            command_id,
            session_id,
            "complete",
            AuditStatus::Completed,
            Some(details.to_string()),
        );
    }

    pub fn log_denied(&mut self, command_id: &str, session_id: &str, action_type: &str) {
        self.push_entry(
            command_id,
            session_id,
            action_type,
            AuditStatus::Denied,
            None,
        );
    }

    pub fn log_failed(&mut self, command_id: &str, session_id: &str, error: &str) {
        self.push_entry(
            command_id,
            session_id,
            "failed",
            AuditStatus::Failed,
            Some(error.to_string()),
        );
    }

    pub fn log_event(&mut self, action_type: &str, session_id: &str, details: &str) {
        self.push_entry(
            &format!("event-{}", uuid::Uuid::new_v4()),
            session_id,
            action_type,
            AuditStatus::Completed,
            Some(details.to_string()),
        );
    }

    pub fn log_start_if(
        &mut self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
    ) {
        if matches!(level, AuditLevel::None) {
            return;
        }
        self.push_entry(
            command_id,
            session_id,
            action_type,
            AuditStatus::Started,
            None,
        );
    }

    pub fn log_complete_with_time(
        &mut self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        if matches!(level, AuditLevel::None) {
            return;
        }
        self.push_entry_with_time(
            command_id,
            session_id,
            "complete",
            AuditStatus::Completed,
            Some(details.to_string()),
            Some(execution_time_ms),
        );
    }

    pub fn log_timeout(&mut self, command_id: &str, session_id: &str, timeout_ms: u64) {
        self.push_entry_with_time(
            command_id,
            session_id,
            "timeout",
            AuditStatus::Timeout,
            Some(format!("Exceeded {}ms", timeout_ms)),
            Some(timeout_ms),
        );
    }

    pub fn has_pending_batch(&self) -> bool {
        self.buffer.len() >= self.batch_size
    }

    pub fn pending_count(&self) -> usize {
        self.buffer.len()
    }

    pub fn drain_batch(&mut self) -> Vec<AuditEntry> {
        let count = self.buffer.len().min(self.batch_size);
        self.buffer.drain(..count).collect()
    }

    pub fn drain_all(&mut self) -> Vec<AuditEntry> {
        self.buffer.drain(..).collect()
    }

    pub fn recent_entries(&self, limit: usize) -> Vec<AuditEntry> {
        self.buffer.iter().rev().take(limit).cloned().collect()
    }

    pub fn entries_by_status(&self, status: &AuditStatus, limit: usize) -> Vec<AuditEntry> {
        self.buffer
            .iter()
            .rev()
            .filter(|e| &e.status == status)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Filter entries by action_type prefix at the data level (no over-reading).
    pub fn entries_by_action_prefix(&self, prefix: &str, limit: usize) -> Vec<AuditEntry> {
        self.buffer
            .iter()
            .rev()
            .filter(|e| e.action_type.starts_with(prefix))
            .take(limit)
            .cloned()
            .collect()
    }

    /// Lookup audit entries by `command_id` with storage fall-through.
    ///
    /// Walks the in-memory `VecDeque` buffer first (newest-first via
    /// `iter().rev()`). If the buffer doesn't satisfy `limit` and an
    /// [`AuditQuery`] handle was attached via [`Self::with_query`], queries
    /// the historical storage for the remainder, deduplicating by `entry_id`
    /// (entries persisted to storage may still be present in the buffer —
    /// both write paths fire on the same insertion). Final results are
    /// re-sorted by `timestamp DESC` and truncated to `limit`.
    pub fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
        if limit == 0 {
            return Vec::new();
        }

        // Buffer first — newest entries (capacity ~1000 by default).
        let mut results: Vec<AuditEntry> = self
            .buffer
            .iter()
            .rev()
            .filter(|e| e.command_id == command_id)
            .take(limit)
            .cloned()
            .collect();

        // Fall-through: if buffer didn't satisfy `limit`, query storage for
        // the remainder, deduping by entry_id (entries persisted to storage
        // may still be in buffer — both write paths fire on the same insertion).
        if results.len() < limit {
            if let Some(q) = &self.query {
                let buffer_ids: std::collections::HashSet<String> =
                    results.iter().map(|e| e.entry_id.clone()).collect();
                let storage_results = q.entries_by_command_id(command_id, limit);
                for entry in storage_results {
                    if results.len() >= limit {
                        break;
                    }
                    if !buffer_ids.contains(&entry.entry_id) {
                        results.push(entry);
                    }
                }
            }
        }

        // Re-sort by timestamp DESC. Buffer rows are inserted-newest-first
        // (VecDeque + .rev()), and storage rows arrive in timestamp DESC. After
        // merge they may interleave, so re-sort to maintain newest-first.
        results.sort_by_key(|e| std::cmp::Reverse(e.timestamp));
        results.truncate(limit);
        results
    }

    pub fn stats(&self) -> AuditStats {
        let mut completed = 0;
        let mut failed = 0;
        let mut denied = 0;
        let mut timeout = 0;
        for entry in &self.buffer {
            match entry.status {
                AuditStatus::Completed => completed += 1,
                AuditStatus::Failed => failed += 1,
                AuditStatus::Denied => denied += 1,
                AuditStatus::Timeout => timeout += 1,
                AuditStatus::Started => {}
            }
        }
        let total = completed + failed + denied + timeout;
        AuditStats {
            total,
            completed,
            failed,
            denied,
            timeout,
        }
    }

    fn push_entry(
        &mut self,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: AuditStatus,
        details: Option<String>,
    ) {
        if self.buffer.len() >= self.max_buffer_size {
            self.buffer.pop_front();
            tracing::warn!("audit buffer full: dropping oldest entry");
        }

        let entry = AuditEntry {
            entry_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status,
            details: self.sanitize_details(details),
            execution_time_ms: None,
        };

        if let Some(ref cb) = self.persistence {
            cb.persist(&entry);
        }

        self.buffer.push_back(entry);
    }

    fn push_entry_with_time(
        &mut self,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: AuditStatus,
        raw_details: Option<String>,
        execution_time_ms: Option<u64>,
    ) {
        // D5 iter-6: sanitize details at record boundary.
        let details = self.sanitize_details(raw_details);
        if self.buffer.len() >= self.max_buffer_size {
            self.buffer.pop_front();
            tracing::warn!("audit buffer full: dropping oldest entry");
        }

        let entry = AuditEntry {
            entry_id: uuid::Uuid::new_v4().to_string(),
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            command_id: command_id.to_string(),
            action_type: action_type.to_string(),
            status,
            details,
            execution_time_ms,
        };

        if let Some(ref cb) = self.persistence {
            cb.persist(&entry);
        }

        self.buffer.push_back(entry);
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new(1000, 50)
    }
}

// ── AuditLogPort adapter ──

use tokio::sync::RwLock;

/// `Arc<RwLock<AuditLogger>>`를 `AuditLogPort`로 래핑하는 어댑터
///
/// ADR-001 §2: 포트 트레잇은 `&self`, 구현체는 interior mutability 사용
pub struct AuditLogAdapter {
    inner: Arc<RwLock<AuditLogger>>,
}

impl AuditLogAdapter {
    pub fn new(logger: Arc<RwLock<AuditLogger>>) -> Self {
        Self { inner: logger }
    }

    /// 내부 `Arc<RwLock<AuditLogger>>`에 대한 참조 (직접 접근이 필요한 레거시 코드용)
    pub fn inner(&self) -> &Arc<RwLock<AuditLogger>> {
        &self.inner
    }
}

#[async_trait::async_trait]
impl oneshim_core::ports::audit_log::AuditLogPort for AuditLogAdapter {
    async fn pending_count(&self) -> usize {
        self.inner.read().await.pending_count()
    }

    async fn recent_entries(&self, limit: usize) -> Vec<AuditEntry> {
        self.inner.read().await.recent_entries(limit)
    }

    async fn entries_by_status(&self, status: &AuditStatus, limit: usize) -> Vec<AuditEntry> {
        self.inner.read().await.entries_by_status(status, limit)
    }

    async fn entries_by_action_prefix(&self, prefix: &str, limit: usize) -> Vec<AuditEntry> {
        self.inner
            .read()
            .await
            .entries_by_action_prefix(prefix, limit)
    }

    async fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry> {
        self.inner
            .read()
            .await
            .entries_by_command_id(command_id, limit)
    }

    async fn stats(&self) -> AuditStats {
        self.inner.read().await.stats()
    }

    async fn has_pending_batch(&self) -> bool {
        self.inner.read().await.has_pending_batch()
    }

    async fn log_event(&self, action_type: &str, session_id: &str, details: &str) {
        self.inner
            .write()
            .await
            .log_event(action_type, session_id, details);
    }

    async fn log_start_if(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
    ) {
        self.inner
            .write()
            .await
            .log_start_if(level, command_id, session_id, action_type);
    }

    async fn log_complete_with_time(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    ) {
        self.inner.write().await.log_complete_with_time(
            level,
            command_id,
            session_id,
            details,
            execution_time_ms,
        );
    }

    async fn drain_batch(&self) -> Vec<AuditEntry> {
        self.inner.write().await.drain_batch()
    }

    async fn drain_all(&self) -> Vec<AuditEntry> {
        self.inner.write().await.drain_all()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let persisted = Arc::new(std::sync::Mutex::new(Vec::<AuditEntry>::new()));
        let persisted_clone = persisted.clone();
        let cb: Arc<dyn AuditPersistence> = Arc::new(move |entry: &AuditEntry| {
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
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = count.clone();
        let cb: Arc<dyn AuditPersistence> = Arc::new(move |_: &AuditEntry| {
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
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_clone = count.clone();
        let cb: Arc<dyn AuditPersistence> = Arc::new(move |_: &AuditEntry| {
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
        use oneshim_core::ports::audit_log::AuditLogPort;
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
