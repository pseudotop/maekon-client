use maekon_core::models::ai_session::SessionAuditEntry;
use maekon_core::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::logger::AuditLogger;
use super::traits::SessionAuditPersistence;

/// Adapter that wraps `Arc<RwLock<AuditLogger>>` as an `AuditLogPort`.
///
/// ADR-001 §2: port traits take `&self`; the implementation uses interior mutability.
pub struct AuditLogAdapter {
    inner: Arc<RwLock<AuditLogger>>,
    /// Durable persister for AI conversation **session** audit entries (#6168).
    ///
    /// When `Some`, [`AuditLogPort::record_session_event`] forwards the entry
    /// here so it lands in the `session_audit_log` table. When `None` (no
    /// session-audit storage wired), `record_session_event` warns once-per-call
    /// and drops the entry — it MUST never panic or propagate (the port method
    /// is infallible).
    session_persistence: Option<Arc<dyn SessionAuditPersistence>>,
}

impl AuditLogAdapter {
    pub fn new(logger: Arc<RwLock<AuditLogger>>) -> Self {
        Self {
            inner: logger,
            session_persistence: None,
        }
    }

    /// Attach a durable persister for AI session audit entries (#6168).
    ///
    /// Without this, [`AuditLogPort::record_session_event`] silently drops every
    /// `SessionAuditEntry` (the trait default is a no-op), so the
    /// `session_audit_log` table never receives production writes. The binary
    /// crate wires a `SqliteStorage`-backed persister here.
    pub fn with_session_persistence(mut self, persister: Arc<dyn SessionAuditPersistence>) -> Self {
        self.session_persistence = Some(persister);
        self
    }

    /// Reference to the inner `Arc<RwLock<AuditLogger>>` (for legacy code that needs
    /// direct access).
    pub fn inner(&self) -> &Arc<RwLock<AuditLogger>> {
        &self.inner
    }
}

#[async_trait::async_trait]
impl maekon_core::ports::audit_log::AuditLogPort for AuditLogAdapter {
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

    // #6277: durable-status-preserving completion log (overrides the trait default
    // that would delegate back to log_complete_with_time and lose the status).
    async fn log_with_status_and_time(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: maekon_core::models::audit::AuditStatus,
        details: &str,
        execution_time_ms: u64,
    ) {
        self.inner.write().await.log_with_status_and_time(
            level,
            command_id,
            session_id,
            action_type,
            status,
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

    /// Persist an AI conversation session audit entry (#6168).
    ///
    /// Overrides the trait's no-op default so the production audit adapter
    /// actually writes to `session_audit_log`. Best-effort and infallible by
    /// contract: if no session persister was wired (`with_session_persistence`),
    /// the entry is logged at `warn!` and dropped — never panics, never blocks
    /// the conversation path. The persister itself runs the (blocking) SQLite
    /// write off-reactor (see `session_wiring.rs`).
    async fn record_session_event(&self, entry: SessionAuditEntry) {
        match self.session_persistence.as_ref() {
            Some(persister) => persister.persist(&entry),
            None => {
                tracing::warn!(
                    session_id = %entry.session_id,
                    event_type = %entry.event_type,
                    "session audit entry dropped: no session-audit persister wired"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::ai_session::SessionAuditCategory;
    use maekon_core::ports::audit_log::AuditLogPort;
    use std::sync::Mutex;

    fn make_session_entry(session_id: &str, event_type: &str) -> SessionAuditEntry {
        SessionAuditEntry {
            timestamp: Utc::now(),
            session_id: session_id.to_string(),
            category: SessionAuditCategory::Message,
            event_type: event_type.to_string(),
            provider: "claude".to_string(),
            payload: None,
            duration_ms: None,
        }
    }

    /// Regression (#6168): with a session persister wired, `record_session_event`
    /// MUST forward the entry to the persister (the production path that lands a
    /// row in `session_audit_log`). Before the fix the only production impl left
    /// this as a no-op default, dropping every session audit entry.
    #[tokio::test]
    async fn record_session_event_forwards_to_persister() {
        let captured: Arc<Mutex<Vec<SessionAuditEntry>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_cb = captured.clone();
        let persister: Arc<dyn SessionAuditPersistence> =
            Arc::new(move |e: &SessionAuditEntry| captured_cb.lock().unwrap().push(e.clone()));

        let logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
        let adapter = AuditLogAdapter::new(logger).with_session_persistence(persister);

        adapter
            .record_session_event(make_session_entry("sess-1", "inbound"))
            .await;

        let rows = captured.lock().unwrap();
        assert_eq!(rows.len(), 1, "entry must reach the wired persister");
        assert_eq!(rows[0].session_id, "sess-1");
        assert_eq!(rows[0].event_type, "inbound");
    }

    /// Without a persister, `record_session_event` must drop silently (no panic,
    /// no propagation) — the port method is infallible.
    #[tokio::test]
    async fn record_session_event_no_persister_is_noop() {
        let logger = Arc::new(RwLock::new(AuditLogger::new(100, 10)));
        let adapter = AuditLogAdapter::new(logger);
        // Must not panic even though nothing is wired.
        adapter
            .record_session_event(make_session_entry("sess-2", "inbound"))
            .await;
    }
}
