use crate::error::StorageError;
use chrono::Utc;
#[allow(deprecated)]
use oneshim_core::models::work_session::FocusMetrics;
use tracing::debug;

use super::super::SqliteStorage;

impl SqliteStorage {
    // --------------------------------------------------------
    // --------------------------------------------------------

    pub fn get_or_create_today_focus_metrics(&self) -> Result<FocusMetrics, StorageError> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        self.get_or_create_focus_metrics(&today)
    }

    pub fn get_or_create_focus_metrics(&self, date: &str) -> Result<FocusMetrics, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let result = conn.query_row(
            "SELECT total_active_secs, deep_work_secs, communication_secs, context_switches,
                    interruption_count, avg_focus_duration_secs, max_focus_duration_secs, focus_score
             FROM focus_metrics WHERE date = ?1",
            rusqlite::params![date],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, f32>(7)?,
                ))
            },
        );

        let (period_start, period_end) = Self::date_to_period_range(date);
        let period = oneshim_core::types::TimeWindow::new(period_start, period_end)
            .expect("date_to_period_range produces valid window");

        match result {
            Ok((
                total_active_secs,
                deep_work_secs,
                communication_secs,
                context_switches,
                interruption_count,
                avg_focus_duration_secs,
                max_focus_duration_secs,
                focus_score,
            )) => Ok(FocusMetrics {
                period,
                total_active_secs,
                deep_work_secs,
                communication_secs,
                context_switches,
                interruption_count,
                avg_focus_duration_secs,
                max_focus_duration_secs,
                focus_score,
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                conn.execute(
                    "INSERT INTO focus_metrics (date) VALUES (?1)",
                    rusqlite::params![date],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to create focus metric: {e}"))
                })?;

                FocusMetrics::new(period_start, period_end).map_err(|e| {
                    StorageError::Internal(format!(
                        "date_to_period_range produced invalid window: {e}"
                    ))
                })
            }
            Err(e) => Err(StorageError::Internal(format!(
                "Failed to query focus metric: {e}"
            ))),
        }
    }

    pub fn update_focus_metrics(
        &self,
        date: &str,
        metrics: &FocusMetrics,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE focus_metrics SET
                total_active_secs = ?1,
                deep_work_secs = ?2,
                communication_secs = ?3,
                context_switches = ?4,
                interruption_count = ?5,
                avg_focus_duration_secs = ?6,
                max_focus_duration_secs = ?7,
                focus_score = ?8,
                updated_at = datetime('now')
             WHERE date = ?9",
            rusqlite::params![
                metrics.total_active_secs as i64,
                metrics.deep_work_secs as i64,
                metrics.communication_secs as i64,
                metrics.context_switches as i64,
                metrics.interruption_count as i64,
                metrics.avg_focus_duration_secs as i64,
                metrics.max_focus_duration_secs as i64,
                metrics.focus_score,
                date,
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to update focus metric: {e}")))?;

        debug!(
            "focus metrics updated: date={}, score={:.2}",
            date, metrics.focus_score
        );
        Ok(())
    }

    pub fn increment_focus_metrics(
        &self,
        date: &str,
        total_active_secs: u64,
        deep_work_secs: u64,
        communication_secs: u64,
        context_switches: u32,
        interruption_count: u32,
    ) -> Result<(), StorageError> {
        let _ = self.get_or_create_focus_metrics(date)?;

        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE focus_metrics SET
                total_active_secs = total_active_secs + ?1,
                deep_work_secs = deep_work_secs + ?2,
                communication_secs = communication_secs + ?3,
                context_switches = context_switches + ?4,
                interruption_count = interruption_count + ?5,
                updated_at = datetime('now')
             WHERE date = ?6",
            rusqlite::params![
                total_active_secs as i64,
                deep_work_secs as i64,
                communication_secs as i64,
                context_switches as i64,
                interruption_count as i64,
                date,
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to increment focus metric: {e}")))?;

        Ok(())
    }

    pub fn get_recent_focus_metrics(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT date, total_active_secs, deep_work_secs, communication_secs, context_switches,
                        interruption_count, avg_focus_duration_secs, max_focus_duration_secs, focus_score
                 FROM focus_metrics ORDER BY date DESC LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![days as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u32>(5)?,
                    row.get::<_, u64>(6)?,
                    row.get::<_, u64>(7)?,
                    row.get::<_, f32>(8)?,
                ))
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut results = Vec::new();
        for row in rows {
            let (
                date,
                total_active_secs,
                deep_work_secs,
                communication_secs,
                context_switches,
                interruption_count,
                avg_focus_duration_secs,
                max_focus_duration_secs,
                focus_score,
            ) = row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?;

            let (period_start, period_end) = Self::date_to_period_range(&date);
            let period = oneshim_core::types::TimeWindow::new(period_start, period_end)
                .expect("date_to_period_range produces valid window");

            results.push((
                date,
                FocusMetrics {
                    period,
                    total_active_secs,
                    deep_work_secs,
                    communication_secs,
                    context_switches,
                    interruption_count,
                    avg_focus_duration_secs,
                    max_focus_duration_secs,
                    focus_score,
                },
            ));
        }

        Ok(results)
    }
}
