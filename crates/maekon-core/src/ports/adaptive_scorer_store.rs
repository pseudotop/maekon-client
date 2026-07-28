//! Persistence port for the coaching `AdaptiveScorer` model state (#8058 P2-1).
//!
//! This is a NARROW secondary port — the restart-surviving backing for the
//! coaching engine's online-logistic-regression weights. Like its siblings
//! ([`super::coaching_effectiveness_store`], [`super::feedback_scorer_store`],
//! [`super::regime_reaction_store`]) it is NOT the broad `StorageService` god
//! object: `CoachingEngine` deliberately never takes `StorageService`, but it MAY
//! hold an optional handle to this focused port so learned scorer weights survive
//! restart. A `None` store keeps the scorer purely in-memory (all unit tests run
//! that way, starting from the neutral default model).

use crate::error::CoreError;
use crate::models::coaching::AdaptiveScorerState;

/// Persist + load the singleton coaching `AdaptiveScorer` state.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite failures. Learned
/// scorer weights are ADVISORY (they only refine an already-gated "should show"
/// decision once warmed up), never security-bearing, so callers treat a load
/// failure as "start from the neutral default model" and a write failure as a
/// logged best-effort miss — neither must panic the scheduler.
pub trait AdaptiveScorerStore: Send + Sync {
    /// Upsert the single scorer-state row (idempotent; last write wins). The
    /// scorer is a per-install singleton, so implementations write to a fixed
    /// row rather than accumulating history.
    fn save_adaptive_scorer_state(&self, state: &AdaptiveScorerState) -> Result<(), CoreError>;

    /// Load the persisted scorer state on startup. `None` on first launch (or
    /// after erasure) — the caller then starts from the neutral default model.
    fn load_adaptive_scorer_state(&self) -> Result<Option<AdaptiveScorerState>, CoreError>;
}
