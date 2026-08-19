use crate::error::StorageError;
use maekon_core::models::coaching::HabitStreakRow;
use rusqlite::Connection;

use super::SqliteStorage;

/// Owned parameters for an `upsert_habit_streak` write, moved into the
/// `Send + 'static` `with_conn` closure (ADR-026 PR-8).
struct OwnedHabitStreak {
    regime_label: String,
    date: String,
    minutes_logged: u32,
    target_minutes: u32,
    met: bool,
}

impl SqliteStorage {
    /// Upsert a daily habit record for a regime, keyed by the unique
    /// `(regime_label, date)` constraint.
    ///
    /// The conflict path is MONOTONIC, not a blind overwrite (#8052):
    /// `minutes_logged = MAX(existing, new)` and `met = MAX(existing, new)` so a
    /// stale/lower write (e.g. the first tick after a restart before the in-RAM
    /// counter re-hydrates, or a concurrent double-write) can never regress the
    /// day's persisted total or flip `met` back from true to false.
    ///
    /// Synchronous inherent twin — retained for the in-crate `#[cfg(test)]`
    /// suite and the web handler `#[cfg(test)]` seed path. The async
    /// `HabitStorage` trait method (ADR-026 PR-8) routes through
    /// [`Self::upsert_habit_streak_async`].
    pub fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), StorageError> {
        // Write — write_lock (skipped when deletion_flag is set, habit_streaks ∈ ALL_TABLES).
        let params = OwnedHabitStreak {
            regime_label: regime_label.to_owned(),
            date: date.to_owned(),
            minutes_logged,
            target_minutes,
            met,
        };
        self.conn
            .write_lock()
            .run((), |conn| Self::upsert_habit_streak_inner(conn, &params))
    }

    /// Async funnel for the `HabitStorage` trait method: offloads the SQLite
    /// write onto `spawn_blocking` via `with_conn` (`deletion_flag || erasing`
    /// re-checked) so the `parking_lot` guard is never held across an `.await`
    /// (ADR-026 PR-8).
    pub(crate) async fn upsert_habit_streak_async(
        &self,
        regime_label: String,
        date: String,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), StorageError> {
        let params = OwnedHabitStreak {
            regime_label,
            date,
            minutes_logged,
            target_minutes,
            met,
        };
        self.with_conn(move |conn| Self::upsert_habit_streak_inner(conn, &params))
            .await
    }

    /// Shared SQL body for both the sync twin and the async funnel.
    fn upsert_habit_streak_inner(
        conn: &Connection,
        params: &OwnedHabitStreak,
    ) -> Result<(), StorageError> {
        // #8052: MAX-guard the conflict path so a lower/stale write cannot
        // regress the day's minutes or flip `met` true->false. `minutes_logged`
        // and `met` on the RHS refer to the EXISTING row value; `?3`/`?5` are the
        // incoming write. `target_minutes` is authoritative-latest (config can
        // change intra-day). Both guarded columns are `NOT NULL`, so `MAX(..)`
        // never sees a NULL argument.
        conn.execute(
            "INSERT INTO habit_streaks (regime_label, date, minutes_logged, target_minutes, met)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(regime_label, date)
             DO UPDATE SET
                 minutes_logged = MAX(minutes_logged, ?3),
                 target_minutes = ?4,
                 met = MAX(met, ?5)",
            rusqlite::params![
                params.regime_label,
                params.date,
                params.minutes_logged,
                params.target_minutes,
                params.met
            ],
        )
        .map_err(|e| StorageError::Internal(format!("upsert_habit_streak: {e}")))?;
        Ok(())
    }

    /// Query habit streak rows for all regimes within the last `days` days,
    /// ordered by date descending then regime_label ascending.
    ///
    /// Synchronous inherent twin — see [`Self::upsert_habit_streak`]. The async
    /// trait method routes through [`Self::query_habit_streaks_async`].
    pub fn query_habit_streaks(&self, days: u32) -> Result<Vec<HabitStreakRow>, StorageError> {
        // Read — read_lock (independent of deletion_flag).
        let read = self.conn.read_lock();
        Self::query_habit_streaks_inner(read.conn(), days)
    }

    /// Async funnel for the `HabitStorage` trait method (ADR-026 PR-8).
    pub(crate) async fn query_habit_streaks_async(
        &self,
        days: u32,
    ) -> Result<Vec<HabitStreakRow>, StorageError> {
        self.with_conn_read(move |conn| Self::query_habit_streaks_inner(conn, days))
            .await
    }

    /// Shared SQL body for both the sync twin and the async funnel.
    fn query_habit_streaks_inner(
        conn: &Connection,
        days: u32,
    ) -> Result<Vec<HabitStreakRow>, StorageError> {
        let mut stmt = conn
            .prepare(
                "SELECT regime_label, date, minutes_logged, target_minutes, met
                 FROM habit_streaks
                 WHERE date >= date('now', '-' || ?1 || ' days')
                 ORDER BY date DESC, regime_label ASC",
            )
            .map_err(|e| StorageError::Internal(format!("prepare query_habit_streaks: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![days], |row| {
                Ok(HabitStreakRow {
                    regime_label: row.get(0)?,
                    date: row.get(1)?,
                    minutes_logged: row.get(2)?,
                    target_minutes: row.get(3)?,
                    met: row.get(4)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("query_habit_streaks: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(|e| StorageError::Internal(format!("row read: {e}")))?);
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_storage() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).expect("in-memory storage")
    }

    #[test]
    fn upsert_and_query_habit_streak() {
        let storage = test_storage();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        storage
            .upsert_habit_streak("deep_work", &today, 120, 120, true)
            .unwrap();
        storage
            .upsert_habit_streak("communication", &today, 30, 60, false)
            .unwrap();

        let rows = storage.query_habit_streaks(7).unwrap();
        assert_eq!(rows.len(), 2);
        // Ordered by date DESC, regime ASC — both same date, so sorted by label
        assert_eq!(rows[0].regime_label, "communication");
        assert_eq!(rows[1].regime_label, "deep_work");
    }

    #[test]
    fn upsert_advances_minutes_and_met_on_higher_write() {
        let storage = test_storage();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        storage
            .upsert_habit_streak("deep_work", &today, 60, 120, false)
            .unwrap();
        // A higher write (target reached) advances both minutes and met.
        storage
            .upsert_habit_streak("deep_work", &today, 130, 120, true)
            .unwrap();

        let rows = storage.query_habit_streaks(7).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].minutes_logged, 130);
        assert!(rows[0].met);
    }

    /// #8052: the conflict path is MAX-guarded — a lower/stale write (the shape
    /// produced by the first post-restart tick before the in-RAM counter
    /// re-hydrates, or a concurrent double-write) must NOT regress the day's
    /// persisted minutes and must NOT flip `met` back from true to false.
    #[test]
    fn upsert_does_not_regress_minutes_or_met_on_lower_write() {
        let storage = test_storage();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        // Established: 80 min, target met earlier today.
        storage
            .upsert_habit_streak("deep_work", &today, 80, 100, true)
            .unwrap();
        // Stale/lower write: fewer minutes, met=false. Must be absorbed by MAX.
        storage
            .upsert_habit_streak("deep_work", &today, 30, 100, false)
            .unwrap();

        let rows = storage.query_habit_streaks(7).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].minutes_logged, 80,
            "a lower write must not regress the day's minutes"
        );
        assert!(rows[0].met, "met must not regress from true to false");

        // A subsequent genuinely-higher write still advances the total.
        storage
            .upsert_habit_streak("deep_work", &today, 95, 100, true)
            .unwrap();
        let rows = storage.query_habit_streaks(7).unwrap();
        assert_eq!(rows[0].minutes_logged, 95);
        assert!(rows[0].met);
    }

    #[test]
    fn query_habit_streaks_respects_day_window() {
        let storage = test_storage();

        // Insert for today (should appear in 7-day window)
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        storage
            .upsert_habit_streak("deep_work", &today, 60, 120, false)
            .unwrap();

        // Insert for 30 days ago (should NOT appear in 7-day window)
        storage
            .upsert_habit_streak("deep_work", "2020-01-01", 60, 120, false)
            .unwrap();

        let rows = storage.query_habit_streaks(7).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, today);
    }

    #[test]
    fn query_empty_returns_empty() {
        let storage = test_storage();
        let rows = storage.query_habit_streaks(7).unwrap();
        assert!(rows.is_empty());
    }
}
