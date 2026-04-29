use crate::error::StorageError;
use chrono::{DateTime, Utc};
#[allow(deprecated)]
use oneshim_core::models::work_session::{AppCategory, Interruption, SessionState, WorkSession};
use oneshim_core::types::TimeWindow;
use tracing::debug;

use super::super::{FocusInterruptionRecord, FocusWorkSessionRecord, SqliteStorage};

/// Serialize an enum to its SQL string representation using serde.
/// Produces consistent casing (e.g. "FocusReminder") instead of Debug
/// format which may differ between enum variants.
pub(crate) fn enum_to_sql_str<T: serde::Serialize>(val: &T) -> String {
    serde_json::to_string(val)
        .unwrap_or_default()
        .trim_matches('"')
        .to_string()
}

impl SqliteStorage {
    // --------------------------------------------------------
    // --------------------------------------------------------

    pub fn start_work_session(
        &self,
        primary_app: &str,
        category: AppCategory,
    ) -> Result<WorkSession, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let now = Utc::now();
        let category_str = enum_to_sql_str(&category);

        conn.execute(
            "INSERT INTO work_sessions (started_at, primary_app, category, state)
             VALUES (?1, ?2, ?3, 'active')",
            rusqlite::params![now.to_rfc3339(), primary_app, category_str],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to start work session: {e}")))?;

        let id = conn.last_insert_rowid();
        debug!(
            "work session started: id={}, app={}, category={:?}",
            id, primary_app, category
        );

        Ok(WorkSession {
            id,
            started_at: now,
            ended_at: None,
            primary_app: primary_app.to_string(),
            category,
            state: SessionState::Active,
            interruption_count: 0,
            deep_work_secs: 0,
            duration_secs: 0,
        })
    }

    pub fn get_active_work_session(&self) -> Result<Option<WorkSession>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let result = conn.query_row(
            "SELECT id, started_at, primary_app, category, interruption_count, deep_work_secs, duration_secs
             FROM work_sessions WHERE state = 'active' ORDER BY id DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, u64>(5)?,
                    row.get::<_, u64>(6)?,
                ))
            },
        );

        match result {
            Ok((
                id,
                started_str,
                primary_app,
                category_str,
                interruption_count,
                deep_work_secs,
                duration_secs,
            )) => {
                let started_at = DateTime::parse_from_rfc3339(&started_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                let category = Self::parse_app_category(&category_str);

                Ok(Some(WorkSession {
                    id,
                    started_at,
                    ended_at: None,
                    primary_app,
                    category,
                    state: SessionState::Active,
                    interruption_count,
                    deep_work_secs,
                    duration_secs,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::Internal(format!(
                "Failed to query work session: {e}"
            ))),
        }
    }

    pub fn end_work_session(&self, session_id: i64) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let now = Utc::now();
        let now_str = now.to_rfc3339();

        let duration_secs: i64 = conn
            .query_row(
                "UPDATE work_sessions
                 SET ended_at = ?1,
                     state = 'completed',
                     duration_secs = CAST((julianday(?1) - julianday(started_at)) * 86400 AS INTEGER)
                 WHERE id = ?2
                 RETURNING duration_secs",
                rusqlite::params![now_str, session_id],
                |row| row.get(0),
            )
            .map_err(|e| StorageError::Internal(format!("Failed to end work session: {e}")))?;

        debug!(
            "work session ended: id={}, duration={}s",
            session_id, duration_secs
        );
        Ok(())
    }

    pub fn increment_work_session_interruption(&self, session_id: i64) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE work_sessions SET interruption_count = interruption_count + 1 WHERE id = ?1",
            rusqlite::params![session_id],
        )
        .map_err(|e| {
            StorageError::Internal(format!("Failed to increment interruption count: {e}"))
        })?;

        Ok(())
    }

    pub fn add_deep_work_secs(&self, session_id: i64, secs: u64) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE work_sessions SET deep_work_secs = deep_work_secs + ?1 WHERE id = ?2",
            rusqlite::params![secs as i64, session_id],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to increment deep_work_secs: {e}")))?;

        Ok(())
    }

    // --------------------------------------------------------
    // --------------------------------------------------------

    pub fn get_app_durations_by_date(
        &self,
        from: &str,
        to: &str,
    ) -> Result<Vec<(String, i64)>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT primary_app, SUM(duration_secs) as total_secs
                 FROM work_sessions
                 WHERE state = 'completed'
                   AND started_at >= ?1 AND started_at < ?2
                 GROUP BY primary_app
                 ORDER BY total_secs DESC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare SQL: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| StorageError::Internal(format!("Query failed: {e}")))?;

        let result: Vec<_> = rows.flatten().collect();

        Ok(result)
    }

    pub fn get_daily_active_secs(
        &self,
        window: &TimeWindow,
    ) -> Result<Vec<(String, i64)>, StorageError> {
        let (from, to) = window.to_sql_pair();
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        // NG6: half-open `started_at < ?2` preserved (intentional — work_sessions
        // started_at is an instant; closing the upper bound would double-count at
        // day rollovers).
        let mut stmt = conn
            .prepare(
                "SELECT DATE(started_at) as day, SUM(duration_secs) as total_secs
                 FROM work_sessions
                 WHERE state = 'completed'
                   AND started_at >= ?1 AND started_at < ?2
                 GROUP BY day
                 ORDER BY day",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare SQL: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| StorageError::Internal(format!("Query failed: {e}")))?;

        let result: Vec<_> = rows.flatten().collect();

        Ok(result)
    }

    // --------------------------------------------------------
    // --------------------------------------------------------

    pub fn record_interruption(&self, interruption: &Interruption) -> Result<i64, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "INSERT INTO interruptions (interrupted_at, from_app, from_category, to_app, to_category, snapshot_frame_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                interruption.interrupted_at.to_rfc3339(),
                interruption.from_app,
                enum_to_sql_str(&interruption.from_category),
                interruption.to_app,
                enum_to_sql_str(&interruption.to_category),
                interruption.snapshot_frame_id,
            ],
        )
        .map_err(|e| StorageError::Internal(format!("Failed to record interruption: {e}")))?;

        let id = conn.last_insert_rowid();
        debug!(
            "interruption recorded: {} -> {}",
            interruption.from_app, interruption.to_app
        );
        Ok(id)
    }

    pub fn record_interruption_resume(
        &self,
        interruption_id: i64,
        resumed_to_app: &str,
    ) -> Result<(), StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        conn.execute(
            "UPDATE interruptions SET resumed_at = ?1, resumed_to_app = ?2 WHERE id = ?3",
            rusqlite::params![Utc::now().to_rfc3339(), resumed_to_app, interruption_id],
        )
        .map_err(|e| {
            StorageError::Internal(format!("Failed to record interruption resume: {e}"))
        })?;

        Ok(())
    }

    pub fn get_pending_interruption(&self) -> Result<Option<Interruption>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let result = conn.query_row(
            "SELECT id, interrupted_at, from_app, from_category, to_app, to_category, snapshot_frame_id
             FROM interruptions WHERE resumed_at IS NULL ORDER BY id DESC LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<i64>>(6)?,
                ))
            },
        );

        match result {
            Ok((
                id,
                interrupted_at_str,
                from_app,
                from_category_str,
                to_app,
                to_category_str,
                snapshot_frame_id,
            )) => {
                let interrupted_at = DateTime::parse_from_rfc3339(&interrupted_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());

                Ok(Some(Interruption {
                    id,
                    interrupted_at,
                    from_app,
                    from_category: Self::parse_app_category(&from_category_str),
                    to_app,
                    to_category: Self::parse_app_category(&to_category_str),
                    snapshot_frame_id,
                    resumed_at: None,
                    resumed_to_app: None,
                    duration_secs: None,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(StorageError::Internal(format!(
                "Failed to query interruptions: {e}"
            ))),
        }
    }

    pub fn list_work_sessions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusWorkSessionRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, started_at, ended_at, primary_app, category, state,
                        interruption_count, deep_work_secs, duration_secs
                 FROM work_sessions
                 WHERE started_at >= ?1 AND started_at <= ?2
                 ORDER BY started_at DESC
                 LIMIT ?3",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to, limit as i64], |row| {
                Ok(FocusWorkSessionRecord {
                    id: row.get(0)?,
                    started_at: row.get(1)?,
                    ended_at: row.get(2)?,
                    primary_app: row.get(3)?,
                    category: row.get(4)?,
                    state: row.get(5)?,
                    interruption_count: row.get(6)?,
                    deep_work_secs: row.get(7)?,
                    duration_secs: row.get(8)?,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    pub fn list_interruptions(
        &self,
        from: &str,
        to: &str,
        limit: usize,
    ) -> Result<Vec<FocusInterruptionRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, interrupted_at, from_app, from_category, to_app, to_category,
                        resumed_at, resumed_to_app,
                        CASE WHEN resumed_at IS NOT NULL
                             THEN CAST((julianday(resumed_at) - julianday(interrupted_at)) * 86400 AS INTEGER)
                             ELSE NULL END as duration_secs
                 FROM interruptions
                 WHERE interrupted_at >= ?1 AND interrupted_at <= ?2
                 ORDER BY interrupted_at DESC
                 LIMIT ?3",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from, to, limit as i64], |row| {
                Ok(FocusInterruptionRecord {
                    id: row.get(0)?,
                    interrupted_at: row.get(1)?,
                    from_app: row.get(2)?,
                    from_category: row.get(3)?,
                    to_app: row.get(4)?,
                    to_category: row.get(5)?,
                    resumed_at: row.get(6)?,
                    resumed_to_app: row.get(7)?,
                    duration_secs: row.get::<_, Option<i64>>(8)?.map(|v| v as u64),
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut records = Vec::new();
        for row in rows {
            records
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }
        Ok(records)
    }

    // --------------------------------------------------------
    // --------------------------------------------------------

    pub(super) fn date_to_period_range(date: &str) -> (DateTime<Utc>, DateTime<Utc>) {
        use chrono::NaiveDate;

        if let Ok(naive_date) = NaiveDate::parse_from_str(date, "%Y-%m-%d") {
            let start = naive_date
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc())
                .unwrap_or_else(Utc::now);
            let end = naive_date
                .and_hms_opt(23, 59, 59)
                .map(|dt| dt.and_utc())
                .unwrap_or_else(Utc::now);
            (start, end)
        } else {
            let now = Utc::now();
            let start = now
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .map(|dt| dt.and_utc())
                .unwrap_or(now);
            let end = now
                .date_naive()
                .and_hms_opt(23, 59, 59)
                .map(|dt| dt.and_utc())
                .unwrap_or(now);
            (start, end)
        }
    }

    pub(crate) fn parse_app_category(s: &str) -> AppCategory {
        // Try serde deserialization first (handles snake_case from enum_to_sql_str)
        if let Ok(cat) = serde_json::from_str::<AppCategory>(&format!("\"{s}\"")) {
            return cat;
        }
        // Fallback for legacy Debug-format strings (PascalCase)
        match s {
            "Communication" => AppCategory::Communication,
            "Development" => AppCategory::Development,
            "Documentation" => AppCategory::Documentation,
            "Browser" => AppCategory::Browser,
            "Design" => AppCategory::Design,
            "Media" => AppCategory::Media,
            "System" => AppCategory::System,
            _ => AppCategory::Other,
        }
    }
}
