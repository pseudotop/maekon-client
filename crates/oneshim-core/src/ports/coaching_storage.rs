//! Persistence port for coaching event records emitted by the scheduler.

use crate::error::CoreError;
use crate::models::coaching::CoachingEventRow;

/// Focused persistence contract for scheduler coaching events.
///
/// # Errors
/// `CoreError::Storage` (wire: `storage.failed`) for SQLite operations
/// (iter-47 mass fix pattern). The current `SqliteStorage` adapter
/// treats rowcount=0 during `update_coaching_event_personalized` as a
/// silent no-op (`Ok(())`) — unknown `event_id` does NOT surface a
/// NotFound. Callers that need to detect missing events must query
/// before updating.
pub trait CoachingStoragePort: Send + Sync {
    fn insert_coaching_event(&self, event: &CoachingEventRow) -> Result<(), CoreError>;

    fn update_coaching_event_personalized(
        &self,
        event_id: &str,
        personalized_text: &str,
    ) -> Result<(), CoreError>;
}
