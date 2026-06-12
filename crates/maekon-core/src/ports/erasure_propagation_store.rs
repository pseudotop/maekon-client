//! Persisted marker of the last GDPR erasure this device has propagated.

use crate::error::CoreError;

/// Durable, single-value store for the id of the last Art. 17 erasure whose
/// cross-device `DeletionEvent` this device has successfully propagated (#5156).
///
/// The SyncEngine fires each DISTINCT erasure exactly ONCE (keyed on
/// `ConsentManagerPort::pending_erasure_id`) and persists the id here on success.
/// Persisting it (rather than an in-memory flag that resets on restart) is what
/// prevents a re-announce after restart — a re-announce would make peers re-run
/// the UNBOUNDED `handle_deletion_event` and delete legitimately re-collected
/// post-re-grant data (the regression this fix exists to prevent). The backing
/// row is **retained across erasure** (we must remember a propagated erasure even
/// after local data is wiped).
///
/// Object-safe (sync, no `.await`) for `Arc<dyn ErasurePropagationStore>` DI.
pub trait ErasurePropagationStore: Send + Sync {
    /// The id of the last erasure whose DeletionEvent was confirmed-delivered, or
    /// `None` if this device has never propagated one. Infallible — returns `None`
    /// on a read error (the caller will re-attempt, which is safe: re-attempting a
    /// not-yet-propagated erasure is correct; only re-announcing an ALREADY-pushed
    /// one is harmful, and that path is gated by a successful prior write here).
    fn last_pushed_erasure_id(&self) -> Option<String>;

    /// Record `id` as the last propagated erasure, AFTER its DeletionEvent reached
    /// at least one destination. Surfaces write errors so the caller can log them —
    /// a lost write risks a re-announce on the next restart, so it must be visible.
    ///
    /// # Errors
    /// Returns `CoreError` if the durable write fails.
    fn record_pushed_erasure_id(&self, id: &str) -> Result<(), CoreError>;
}
