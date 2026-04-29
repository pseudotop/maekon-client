use oneshim_core::error::CoreError;
use oneshim_core::models::suggestion::Suggestion;
use oneshim_core::models::work_session::{AppCategory, FocusMetrics, Interruption, WorkSession};
use oneshim_core::ports::focus_storage::FocusStorage;

use super::SqliteStorage;

impl FocusStorage for SqliteStorage {
    fn increment_focus_metrics(
        &self,
        date: &str,
        active_secs: u64,
        deep_work_secs: u64,
        communication_secs: u64,
        context_switches: u32,
        interruption_count: u32,
    ) -> Result<(), CoreError> {
        SqliteStorage::increment_focus_metrics(
            self,
            date,
            active_secs,
            deep_work_secs,
            communication_secs,
            context_switches,
            interruption_count,
        )
        .map_err(Into::into)
    }

    fn add_deep_work_secs(&self, session_id: i64, secs: u64) -> Result<(), CoreError> {
        SqliteStorage::add_deep_work_secs(self, session_id, secs).map_err(Into::into)
    }

    fn record_interruption(&self, interruption: &Interruption) -> Result<i64, CoreError> {
        SqliteStorage::record_interruption(self, interruption).map_err(Into::into)
    }

    fn increment_work_session_interruption(&self, session_id: i64) -> Result<(), CoreError> {
        SqliteStorage::increment_work_session_interruption(self, session_id).map_err(Into::into)
    }

    fn record_interruption_resume(
        &self,
        interruption_id: i64,
        resumed_to_app: &str,
    ) -> Result<(), CoreError> {
        SqliteStorage::record_interruption_resume(self, interruption_id, resumed_to_app)
            .map_err(Into::into)
    }

    fn end_work_session(&self, session_id: i64) -> Result<(), CoreError> {
        SqliteStorage::end_work_session(self, session_id).map_err(Into::into)
    }

    fn start_work_session(
        &self,
        primary_app: &str,
        category: AppCategory,
    ) -> Result<WorkSession, CoreError> {
        SqliteStorage::start_work_session(self, primary_app, category).map_err(Into::into)
    }

    fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError> {
        SqliteStorage::get_or_create_focus_metrics(self, date).map_err(Into::into)
    }

    fn update_focus_metrics(&self, date: &str, metrics: &FocusMetrics) -> Result<(), CoreError> {
        SqliteStorage::update_focus_metrics(self, date, metrics).map_err(Into::into)
    }

    fn save_rule_suggestion(&self, suggestion: &Suggestion) -> Result<String, CoreError> {
        SqliteStorage::save_rule_suggestion_sync(self, suggestion).map_err(Into::into)
    }

    fn mark_suggestion_shown_by_id(&self, suggestion_id: &str) -> Result<(), CoreError> {
        SqliteStorage::mark_unified_suggestion_shown(self, suggestion_id).map_err(Into::into)
    }

    fn get_pending_interruption(&self) -> Result<Option<Interruption>, CoreError> {
        SqliteStorage::get_pending_interruption(self).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    //! Smoke test for FocusStorage trait impl.
    //! Thin delegator over 12 methods; underlying impls covered at
    //! work_sessions.rs + focus_metrics.rs + suggestions.rs sibling tests
    //! and port_contract_tests.rs. This smoke exercises 10 of 12 port
    //! methods in sequence to verify the impl chain is wired correctly.
    //! Methods 11-12 (save_rule_suggestion, mark_suggestion_shown_by_id)
    //! require heavy Suggestion fixture — deferred per spec.

    use chrono::Utc;
    use oneshim_core::models::work_session::{AppCategory, Interruption};
    use oneshim_core::ports::focus_storage::FocusStorage;

    use super::SqliteStorage;

    #[test]
    fn focus_storage_port_smoke_exercises_ten_of_twelve_methods() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // 1. start_work_session
        let session = <SqliteStorage as FocusStorage>::start_work_session(
            &storage,
            "VSCode",
            AppCategory::Development,
        )
        .unwrap();
        let session_id = session.id;

        // 2. add_deep_work_secs
        <SqliteStorage as FocusStorage>::add_deep_work_secs(&storage, session_id, 60).unwrap();

        // 3. record_interruption
        let interruption = Interruption::new(
            0, // id assigned by DB
            "VSCode".to_string(),
            "Slack".to_string(),
            None,
        );
        let int_id =
            <SqliteStorage as FocusStorage>::record_interruption(&storage, &interruption).unwrap();

        // 4. increment_work_session_interruption
        <SqliteStorage as FocusStorage>::increment_work_session_interruption(&storage, session_id)
            .unwrap();

        // 5. record_interruption_resume
        <SqliteStorage as FocusStorage>::record_interruption_resume(&storage, int_id, "VSCode")
            .unwrap();

        // 6. get_pending_interruption (None after resume)
        let pending = <SqliteStorage as FocusStorage>::get_pending_interruption(&storage).unwrap();
        assert!(pending.is_none(), "all interruptions resumed");

        // 7. end_work_session
        <SqliteStorage as FocusStorage>::end_work_session(&storage, session_id).unwrap();

        // 8. get_or_create_focus_metrics
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let metrics =
            <SqliteStorage as FocusStorage>::get_or_create_focus_metrics(&storage, &today).unwrap();

        // 9. increment_focus_metrics
        <SqliteStorage as FocusStorage>::increment_focus_metrics(
            &storage, &today, 120, 60, 30, 2, 1,
        )
        .unwrap();

        // 10. update_focus_metrics
        <SqliteStorage as FocusStorage>::update_focus_metrics(&storage, &today, &metrics).unwrap();

        // All invocations above returned Ok → port impl chain is wired.
    }
}
