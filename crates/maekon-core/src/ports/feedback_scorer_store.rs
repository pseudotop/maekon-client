//! Persistence port for `FeedbackScorer`'s per-`(suggestion_type, source)`
//! tallies (#7913 T2.1c).
//!
//! Narrow secondary port: the restart-surviving backing for the in-RAM
//! `FeedbackScorer` that adjusts/suppresses local suggestions off the shared
//! `enqueue_and_surface` seam (#7914). `last_updated` is persisted so the 12h
//! self-decay is wall-clock-anchored across restarts.

use crate::error::CoreError;
use crate::models::suggestion::FeedbackTallyRecord;

/// Persist + load per-`(suggestion_type, source)` feedback tallies.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite failures. The scorer
/// state is ADVISORY (it only nudges local relevance), never security-bearing, so
/// callers treat a load failure as "start empty" and a write failure as a logged
/// best-effort miss — neither must panic the scheduler.
pub trait FeedbackScorerStore: Send + Sync {
    /// Upsert one tally, keyed by `(suggestion_type, source)`. Idempotent
    /// (last write wins per key).
    fn upsert_feedback_tally(&self, record: &FeedbackTallyRecord) -> Result<(), CoreError>;

    /// Load all persisted tallies on startup. Empty Vec on first launch.
    fn load_feedback_tallies(&self) -> Result<Vec<FeedbackTallyRecord>, CoreError>;
}
