use crate::error::StorageError;
use oneshim_core::models::coaching::HabitStreakRow;

use super::SqliteStorage;

impl SqliteStorage {
    /// Upsert a daily habit record for a regime (INSERT OR REPLACE by unique
    /// `(regime_label, date)` constraint).
    pub fn upsert_habit_streak(
        &self,
        regime_label: &str,
        date: &str,
        minutes_logged: u32,
        target_minutes: u32,
        met: bool,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

        conn.execute(
            "INSERT INTO habit_streaks (regime_label, date, minutes_logged, target_minutes, met)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(regime_label, date)
             DO UPDATE SET minutes_logged = ?3, target_minutes = ?4, met = ?5",
            rusqlite::params![regime_label, date, minutes_logged, target_minutes, met],
        )
        .map_err(|e| StorageError::Internal(format!("upsert_habit_streak: {e}")))?;

        Ok(())
    }

    /// Query habit streak rows for all regimes within the last `days` days,
    /// ordered by date descending then regime_label ascending.
    pub fn query_habit_streaks(&self, days: u32) -> Result<Vec<HabitStreakRow>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("lock poisoned: {e}")))?;

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
    fn upsert_overwrites_existing() {
        let storage = test_storage();
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();

        storage
            .upsert_habit_streak("deep_work", &today, 60, 120, false)
            .unwrap();
        storage
            .upsert_habit_streak("deep_work", &today, 130, 120, true)
            .unwrap();

        let rows = storage.query_habit_streaks(7).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].minutes_logged, 130);
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
