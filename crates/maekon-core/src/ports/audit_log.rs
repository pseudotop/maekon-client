//! Audit logging port — defines the contract for recording, querying,
//! and batch-flushing automation audit entries.
//! Implemented by `AuditLogger` in `maekon-automation`.

use async_trait::async_trait;

use crate::models::ai_session::SessionAuditEntry;
use crate::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};

/// Audit log port — the audit log interface used by maekon-web handlers.
///
/// Implementations use interior mutability internally to handle mutation
/// through `&self`. (ADR-001 §2: port traits use `&self`, implementations
/// use `RwLock`.)
///
/// # Errors
/// **No fallible methods.** Every method returns `()`, `usize`, `bool`,
/// `Vec<AuditEntry>`, or `AuditStats` — not `Result<_, _>`. This is by
/// design: the audit log is best-effort instrumentation that must never
/// block the automation path. Buffer overflow is silently dropped and
/// surfaced via `stats().dropped_count`; batch upload failures are
/// retained internally and retried by the `AuditLogger` impl.
/// `record_session_event` has a no-op default for adapters that don't
/// support session audit.
#[async_trait]
pub trait AuditLogPort: Send + Sync {
    // ── Query methods ──

    /// Number of entries pending in the buffer.
    async fn pending_count(&self) -> usize;

    /// Query recent entries (non-destructive, newest first).
    async fn recent_entries(&self, limit: usize) -> Vec<AuditEntry>;

    /// Query entries filtered by status.
    async fn entries_by_status(&self, status: &AuditStatus, limit: usize) -> Vec<AuditEntry>;

    /// Query entries filtered by `action_type` prefix.
    async fn entries_by_action_prefix(&self, prefix: &str, limit: usize) -> Vec<AuditEntry>;

    /// Aggregate statistics.
    async fn stats(&self) -> AuditStats;

    /// Whether a batch is ready to be uploaded.
    async fn has_pending_batch(&self) -> bool;

    // ── Mutation methods ──

    /// Log a general event.
    async fn log_event(&self, action_type: &str, session_id: &str, details: &str);

    /// Conditionally log a start (skipped when `AuditLevel::None`).
    async fn log_start_if(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
    );

    /// Log completion with execution time (skipped when `AuditLevel::None`).
    async fn log_complete_with_time(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        details: &str,
        execution_time_ms: u64,
    );

    /// #6277: Log termination with the actual result `status` + `action_type`
    /// (skipped when `AuditLevel::None`).
    ///
    /// `log_complete_with_time` pins the durable row's status to `Completed` and
    /// `action_type` to `"complete"`, so non-completion results such as
    /// Denied/Failed/Timeout/Started are all mis-recorded as `Completed` in the
    /// durable row. This method records the caller-supplied actual `status` and
    /// `action_type` directly into the durable row.
    ///
    /// The default implementation delegates to `log_complete_with_time` (so the
    /// trait stays non-breaking) — the actual durable implementation (the
    /// `AuditLogger` adapter) overrides this to preserve `status`/`action_type`.
    #[allow(clippy::too_many_arguments)]
    async fn log_with_status_and_time(
        &self,
        level: AuditLevel,
        command_id: &str,
        session_id: &str,
        action_type: &str,
        status: AuditStatus,
        details: &str,
        execution_time_ms: u64,
    ) {
        let _ = (action_type, status);
        self.log_complete_with_time(level, command_id, session_id, details, execution_time_ms)
            .await;
    }

    // ── Drain methods (batch upload) ──

    /// Drain up to one batch worth of entries.
    async fn drain_batch(&self) -> Vec<AuditEntry>;

    /// Drain all entries.
    async fn drain_all(&self) -> Vec<AuditEntry>;

    /// Return audit entries whose `command_id` exactly matches the given value.
    /// Ordered by `timestamp DESC`. Returns empty vec if none match or on
    /// storage error (infallible by contract — error is logged by impl).
    ///
    /// # Errors
    /// Infallible (returns empty vec on storage error).
    async fn entries_by_command_id(&self, command_id: &str, limit: usize) -> Vec<AuditEntry>;

    // ── Session audit (best-effort) ──

    /// Record an AI conversation session audit event (best-effort: warns on
    /// failure only, never propagates an error).
    async fn record_session_event(&self, _entry: SessionAuditEntry) {
        // Default no-op — overridden by implementations that support session audit.
    }
}

#[cfg(test)]
mod port_contract_tests {
    use super::*;

    /// Compile-time assertion — validates the trait method signature.
    /// Uses a trait object to avoid the E0401 nested-generic restriction.
    // Never called: existing (and type-checking) IS the assertion — a
    // signature change to `entries_by_command_id` fails this file to compile.
    #[allow(dead_code)]
    fn assert_port_has_entries_by_command_id(
        p: &dyn AuditLogPort,
    ) -> impl std::future::Future<Output = Vec<AuditEntry>> + '_ {
        p.entries_by_command_id("cmd", 10)
    }
}
