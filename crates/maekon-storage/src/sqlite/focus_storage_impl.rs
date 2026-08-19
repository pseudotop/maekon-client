use async_trait::async_trait;
use chrono::{DateTime, Utc};
use maekon_core::error::CoreError;
use maekon_core::models::suggestion::Suggestion;
use maekon_core::models::work_session::{AppCategory, FocusMetrics, Interruption, WorkSession};
use maekon_core::ports::focus_storage::FocusStorage;

use super::SqliteStorage;

/// `#[async_trait]` `FocusStorage` impl (ADR-026 PR-2).
///
/// Each method delegates to a `*_async` inherent helper on `SqliteStorage`
/// that routes the SQLite work through the `with_conn`/`with_conn_skip`/
/// `with_conn_read` funnel (`spawn_blocking`). The parking_lot connection guard
/// is therefore acquired on a blocking-pool thread and never held across an
/// `.await`, removing the runtime-thread-blocking defect in `focus_analyzer`
/// while preserving the #4928 erase barrier (the funnel still re-checks
/// `deletion_flag || erasing` inside `write_lock`).
#[async_trait]
impl FocusStorage for SqliteStorage {
    async fn increment_focus_metrics(
        &self,
        date: &str,
        active_secs: u64,
        deep_work_secs: u64,
        communication_secs: u64,
        context_switches: u32,
        interruption_count: u32,
    ) -> Result<(), CoreError> {
        SqliteStorage::increment_focus_metrics_async(
            self,
            date,
            active_secs,
            deep_work_secs,
            communication_secs,
            context_switches,
            interruption_count,
        )
        .await
        .map_err(Into::into)
    }

    async fn add_deep_work_secs(&self, session_id: i64, secs: u64) -> Result<(), CoreError> {
        SqliteStorage::add_deep_work_secs_async(self, session_id, secs)
            .await
            .map_err(Into::into)
    }

    async fn record_interruption(&self, interruption: &Interruption) -> Result<i64, CoreError> {
        SqliteStorage::record_interruption_async(self, interruption)
            .await
            .map_err(Into::into)
    }

    async fn increment_work_session_interruption(&self, session_id: i64) -> Result<(), CoreError> {
        SqliteStorage::increment_work_session_interruption_async(self, session_id)
            .await
            .map_err(Into::into)
    }

    async fn resume_interruption(
        &self,
        interruption_id: i64,
        resumed_to_app: &str,
        resumed_at: DateTime<Utc>,
    ) -> Result<Option<Interruption>, CoreError> {
        SqliteStorage::resume_interruption_async(self, interruption_id, resumed_to_app, resumed_at)
            .await
            .map_err(Into::into)
    }

    async fn end_work_session(&self, session_id: i64) -> Result<(), CoreError> {
        SqliteStorage::end_work_session_async(self, session_id)
            .await
            .map_err(Into::into)
    }

    async fn start_work_session(
        &self,
        primary_app: &str,
        category: AppCategory,
    ) -> Result<WorkSession, CoreError> {
        SqliteStorage::start_work_session_async(self, primary_app, category)
            .await
            .map_err(Into::into)
    }

    async fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, CoreError> {
        SqliteStorage::get_or_create_focus_metrics_async(self, date)
            .await
            .map_err(Into::into)
    }

    async fn update_focus_metrics(
        &self,
        date: &str,
        metrics: &FocusMetrics,
    ) -> Result<(), CoreError> {
        SqliteStorage::update_focus_metrics_async(self, date, metrics)
            .await
            .map_err(Into::into)
    }

    async fn save_rule_suggestion(&self, suggestion: &Suggestion) -> Result<String, CoreError> {
        SqliteStorage::save_rule_suggestion_async(self, suggestion)
            .await
            .map_err(Into::into)
    }

    async fn mark_suggestion_shown_by_id(&self, suggestion_id: &str) -> Result<(), CoreError> {
        SqliteStorage::mark_unified_suggestion_shown_async(self, suggestion_id)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    async fn get_pending_interruption(&self) -> Result<Option<Interruption>, CoreError> {
        SqliteStorage::get_pending_interruption_async(self)
            .await
            .map_err(Into::into)
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
    use maekon_core::models::work_session::{AppCategory, Interruption};
    use maekon_core::ports::focus_storage::FocusStorage;

    use super::SqliteStorage;

    #[tokio::test]
    async fn focus_storage_port_smoke_exercises_ten_of_twelve_methods() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();

        // 1. start_work_session
        let session = <SqliteStorage as FocusStorage>::start_work_session(
            &storage,
            "VSCode",
            AppCategory::Development,
        )
        .await
        .unwrap();
        let session_id = session.id;

        // 2. add_deep_work_secs
        <SqliteStorage as FocusStorage>::add_deep_work_secs(&storage, session_id, 60)
            .await
            .unwrap();

        // 3. record_interruption
        let interruption = Interruption::new(
            0, // id assigned by DB
            "VSCode".to_string(),
            "Slack".to_string(),
            None,
        );
        let int_id = <SqliteStorage as FocusStorage>::record_interruption(&storage, &interruption)
            .await
            .unwrap();

        // 4. increment_work_session_interruption
        <SqliteStorage as FocusStorage>::increment_work_session_interruption(&storage, session_id)
            .await
            .unwrap();

        // 5. resume_interruption
        let resumed = <SqliteStorage as FocusStorage>::resume_interruption(
            &storage,
            int_id,
            "VSCode",
            Utc::now(),
        )
        .await
        .unwrap()
        .expect("pending interruption should resume");
        assert_eq!(resumed.id, int_id);

        // 6. get_pending_interruption (None after resume)
        let pending = <SqliteStorage as FocusStorage>::get_pending_interruption(&storage)
            .await
            .unwrap();
        assert!(pending.is_none(), "all interruptions resumed");

        // 7. end_work_session
        <SqliteStorage as FocusStorage>::end_work_session(&storage, session_id)
            .await
            .unwrap();

        // 8. get_or_create_focus_metrics
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let metrics =
            <SqliteStorage as FocusStorage>::get_or_create_focus_metrics(&storage, &today)
                .await
                .unwrap();

        // 9. increment_focus_metrics
        <SqliteStorage as FocusStorage>::increment_focus_metrics(
            &storage, &today, 120, 60, 30, 2, 1,
        )
        .await
        .unwrap();

        // 10. update_focus_metrics
        <SqliteStorage as FocusStorage>::update_focus_metrics(&storage, &today, &metrics)
            .await
            .unwrap();

        // All invocations above returned Ok → port impl chain is wired.
    }
}
