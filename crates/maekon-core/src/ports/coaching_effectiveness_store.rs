//! Persistence port for coaching effectiveness learning state (#7913 T2.1b).
//!
//! This is a NARROW secondary port — the restart-surviving backing for
//! `FeedbackTracker`'s per-`(profile, trigger)` effectiveness scores. It is NOT
//! the broad `StorageService` god object: `CoachingEngine` deliberately never
//! takes `StorageService`, but it MAY hold an optional handle to this focused
//! port so learned coaching effectiveness survives restart (before #7913 every
//! learned byte evaporated on exit while the sibling queue/regime state already
//! persisted). A `None` store keeps the engine purely in-memory (all unit tests
//! run that way).

use crate::error::CoreError;
use crate::models::coaching::CoachingEffectivenessRecord;

/// Persist + load per-`(profile, trigger)` coaching effectiveness.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite failures. Learned
/// coaching effectiveness is ADVISORY (it only quiets low-value nudges), never
/// security-bearing, so callers treat a load failure as "start empty" and a
/// write failure as a logged best-effort miss — neither must panic the
/// scheduler.
pub trait CoachingEffectivenessStore: Send + Sync {
    /// Upsert the given effectiveness rows, keyed by `(profile_name,
    /// trigger_type)`. Implementations MUST be idempotent (last write wins per
    /// key) so a full-snapshot write-through converges regardless of ordering.
    fn upsert_coaching_effectiveness(
        &self,
        records: &[CoachingEffectivenessRecord],
    ) -> Result<(), CoreError>;

    /// Load all persisted effectiveness rows on startup. Empty Vec on first
    /// launch (or after erasure).
    fn load_coaching_effectiveness(&self) -> Result<Vec<CoachingEffectivenessRecord>, CoreError>;
}
