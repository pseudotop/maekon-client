//! Object-safe sink for the #4803 egress audit ledger.

use crate::error::CoreError;
use crate::models::storage_records::EgressLedgerRecord;

/// Append-only sink for the egress audit ledger — the record of what data left
/// the device (#4803). Object-safe so it can be injected as
/// `Arc<dyn EgressLedgerSink>` (ADR-001 §3 DI).
///
/// **Synchronous**: a single local SQLite `INSERT`, no `.await`. The ledger row
/// is deliberately RETAINED across GDPR Art. 17 erasure (it is compliance
/// evidence *that* egress happened, not user content), so the implementation
/// writes through the erase-retained path rather than the #4928 erase barrier —
/// see `SqliteStorage::record_egress` (`retained_write_lock`).
pub trait EgressLedgerSink: Send + Sync {
    /// Append one egress record. The store de-duplicates on `record_id`
    /// (`INSERT OR IGNORE`), so re-submitting a record with the same id is
    /// idempotent. Callers that want a crash/restart re-push to dedup should
    /// derive `record_id` deterministically from a stable identity — e.g.
    /// SyncEngine keys it on `destination|event_type|<changeset watermark, or
    /// device id for a DeletionEvent>` (#5147), so a replayed egress collapses
    /// to a single audit row instead of double-counting.
    ///
    /// # Errors
    /// Returns `CoreError` if the underlying ledger write fails. Callers on the
    /// hot egress path should treat a failure as **non-fatal** (log and
    /// continue) so a ledger hiccup never blocks or fails the data flow it
    /// audits.
    fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError>;
}
