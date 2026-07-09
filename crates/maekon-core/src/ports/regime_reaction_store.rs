//! Persistence port for `RegimeClassifier`'s per-regime user-reaction stats
//! (#7913 T2.1c).
//!
//! Narrow secondary port: the restart-surviving backing for the in-RAM
//! per-regime accept/reject/defer counts (#7600) that `acceptance_rate` reads to
//! quiet regimes the user rejects suggestions in. Includes the global aggregate
//! under the `RegimeReactionRecord::AGGREGATE_KEY` sentinel.

use crate::error::CoreError;
use crate::models::tiered_memory::RegimeReactionRecord;

/// Persist + load per-regime (and aggregate) user-reaction stats.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite failures. This state
/// is ADVISORY (it only quiets historically-rejected regimes), never
/// security-bearing, so callers treat a load failure as "start empty" and a
/// write failure as a logged best-effort miss — neither must panic the scheduler.
pub trait RegimeReactionStore: Send + Sync {
    /// Upsert one record, keyed by `regime_id` (the `AGGREGATE_KEY` sentinel for
    /// the global aggregate). Idempotent (last write wins per key).
    fn upsert_regime_reaction(&self, record: &RegimeReactionRecord) -> Result<(), CoreError>;

    /// Load all persisted reaction records on startup. Empty Vec on first launch.
    fn load_regime_reactions(&self) -> Result<Vec<RegimeReactionRecord>, CoreError>;
}
