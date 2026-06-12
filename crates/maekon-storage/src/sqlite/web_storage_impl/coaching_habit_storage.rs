//! `CoachingQueryStorage`, `HabitStorage` implementations.
//!
//! Converted to `#[async_trait]` async per ADR-026 PR-8 (async storage
//! convergence). Each method delegates to a `*_async` inherent helper on
//! `SqliteStorage` (defined in `coaching_storage.rs` / `habit_storage.rs`)
//! that routes through the `with_conn`/`with_conn_read` funnel
//! (`spawn_blocking`), so the single-connection `parking_lot` guard is never
//! held across an `.await`. Reads use `with_conn_read` (never skipped); the
//! habit upsert uses the `with_conn` write funnel (`deletion_flag || erasing`
//! re-checked, #4928 erase barrier). The synchronous inherent twins are
//! retained because the src-tauri `get_coaching_history`/`get_habit_streaks`
//! IPC commands consume `Arc<SqliteStorage>` directly.

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::web_storage::{CoachingQueryStorage, HabitStorage};

use crate::sqlite::SqliteStorage;

// ---------------------------------------------------------------------------
// CoachingQueryStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl CoachingQueryStorage for SqliteStorage {
    async fn query_coaching_events(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<maekon_core::models::coaching::CoachingEventRow>, CoreError> {
        SqliteStorage::query_coaching_events_async(self, limit, offset)
            .await
            .map_err(Into::into)
    }

    async fn query_coaching_events_since(
        &self,
        since_date: &str,
    ) -> Result<Vec<maekon_core::models::coaching::CoachingEventRow>, CoreError> {
        SqliteStorage::query_coaching_events_since_async(self, since_date.to_owned())
            .await
            .map_err(Into::into)
    }
}

// ---------------------------------------------------------------------------
// HabitStorage
// ---------------------------------------------------------------------------

#[async_trait]
impl HabitStorage for SqliteStorage {
    async fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), CoreError> {
        SqliteStorage::upsert_habit_streak_async(
            self,
            regime_label.to_owned(),
            date.to_owned(),
            minutes_logged,
            target_minutes,
            met,
        )
        .await
        .map_err(Into::into)
    }

    async fn query_habit_streaks(
        &self,
        days: u32,
    ) -> Result<Vec<maekon_core::models::coaching::HabitStreakRow>, CoreError> {
        SqliteStorage::query_habit_streaks_async(self, days)
            .await
            .map_err(Into::into)
    }
}
