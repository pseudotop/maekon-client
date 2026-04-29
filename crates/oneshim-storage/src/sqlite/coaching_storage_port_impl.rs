use oneshim_core::error::CoreError;
use oneshim_core::models::coaching::CoachingEventRow;
use oneshim_core::ports::coaching_storage::CoachingStoragePort;

use super::SqliteStorage;

impl CoachingStoragePort for SqliteStorage {
    fn insert_coaching_event(&self, event: &CoachingEventRow) -> Result<(), CoreError> {
        SqliteStorage::insert_coaching_event(self, event).map_err(CoreError::from)
    }

    fn update_coaching_event_personalized(
        &self,
        event_id: &str,
        personalized_text: &str,
    ) -> Result<(), CoreError> {
        SqliteStorage::update_coaching_event_personalized(self, event_id, personalized_text)
            .map_err(CoreError::from)
    }
}

#[cfg(test)]
mod tests {
    //! Smoke test for CoachingStoragePort trait impl.
    //! Thin delegator: both methods forward to coaching_storage.rs impls.
    //! Deep coverage at coaching_storage.rs sibling tests; this test only
    //! verifies the port impl chain doesn't panic and propagates errors.

    use chrono::Utc;
    use oneshim_core::models::coaching::CoachingEventRow;
    use oneshim_core::ports::coaching_storage::CoachingStoragePort;

    use super::SqliteStorage;

    fn sample_event(id: &str) -> CoachingEventRow {
        CoachingEventRow {
            event_id: id.to_string(),
            trigger_type: "test_trigger".to_string(),
            profile_name: "default".to_string(),
            regime_id: None,
            message_template: "original".to_string(),
            personalized_message: None,
            shown_at: Utc::now().to_rfc3339(),
            dismissed_at: None,
            dismiss_action: None,
            feedback_type: None,
            feedback_score: None,
        }
    }

    #[test]
    fn coaching_storage_port_smoke_exercises_both_methods() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let event = sample_event("coach-smoke-001");

        // Method 1: insert_coaching_event
        <SqliteStorage as CoachingStoragePort>::insert_coaching_event(&storage, &event).unwrap();

        // Method 2: update_coaching_event_personalized
        <SqliteStorage as CoachingStoragePort>::update_coaching_event_personalized(
            &storage,
            "coach-smoke-001",
            "personalized-variant",
        )
        .unwrap();
    }
}
