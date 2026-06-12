use crate::error::StorageError;
use chrono::Utc;
#[allow(deprecated)]
use maekon_core::models::work_session::FocusMetrics;
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
        let (skip_start, skip_end) = Self::date_to_period_range(date);
        // 신규 행 INSERT 가능성이 있으므로 write_lock 사용. deletion_flag set 시
        // 빈 메트릭(해당 기간)을 반환한다(erase 중 호출 시 무해).
        let skip = FocusMetrics::new(skip_start, skip_end).map_err(|e| {
            StorageError::Internal(format!("date_to_period_range produced invalid window: {e}"))
        })?;
        self.conn.write_lock().run(skip, move |conn| {
            Self::get_or_create_focus_metrics_inner(conn, date)
        })
    }

    pub fn update_focus_metrics(
        &self,
        date: &str,
        metrics: &FocusMetrics,
    ) -> Result<(), StorageError> {
        // 쓰기 — write_lock(deletion_flag set 시 스킵, focus_metrics ∈ ALL_TABLES).
        // analyze_periodic(bare async)에서 호출되나 parking_lot 동기 락이라 panic 없음(B2 해소).
        self.conn.write_lock().run((), |conn| {
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
        })
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

        // 쓰기 — write_lock(deletion_flag set 시 스킵).
        self.conn.write_lock().run((), |conn| {
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
            .map_err(|e| {
                StorageError::Internal(format!("Failed to increment focus metric: {e}"))
            })?;

            Ok(())
        })
    }

    // --------------------------------------------------------
    // Async variants (ADR-026 PR-2) — route through `with_conn*` (spawn_blocking)
    // so the parking_lot guard is held on a blocking-pool thread, never across
    // an `.await`. The sync variants above remain for the still-sync web
    // sub-traits (`FocusQueryStorage`) + tests/benches until PR-4..N converts
    // them. Both paths honour the #4928 erase barrier (write_lock checks
    // `deletion_flag || erasing`).
    // --------------------------------------------------------

    /// Async `get_or_create_focus_metrics` over the write funnel.
    ///
    /// 신규 행 INSERT 가능성이 있으므로 `with_conn`(write funnel). deletion_flag
    /// set 시 빈 메트릭(해당 기간)을 반환한다(erase 중 호출 시 무해).
    pub(crate) async fn get_or_create_focus_metrics_async(
        &self,
        date: &str,
    ) -> Result<FocusMetrics, StorageError> {
        // owned move into the Send + 'static closure.
        let date = date.to_string();
        let (skip_start, skip_end) = Self::date_to_period_range(&date);
        let skip = FocusMetrics::new(skip_start, skip_end).map_err(|e| {
            StorageError::Internal(format!("date_to_period_range produced invalid window: {e}"))
        })?;
        self.with_conn_skip(skip, move |conn| {
            Self::get_or_create_focus_metrics_inner(conn, &date)
        })
        .await
    }

    /// Async `update_focus_metrics` over the write funnel.
    pub(crate) async fn update_focus_metrics_async(
        &self,
        date: &str,
        metrics: &FocusMetrics,
    ) -> Result<(), StorageError> {
        // owned move into the Send + 'static closure.
        let date = date.to_string();
        let metrics = metrics.clone();
        self.with_conn(move |conn| {
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
        })
        .await
    }

    /// Async `increment_focus_metrics` over the write funnel.
    ///
    /// get-or-create 와 increment 를 단일 `with_conn` 클로저(단일 락 획득) 안에서
    /// 수행해 erase 의 wipe 에 대해 원자적이다.
    pub(crate) async fn increment_focus_metrics_async(
        &self,
        date: &str,
        total_active_secs: u64,
        deep_work_secs: u64,
        communication_secs: u64,
        context_switches: u32,
        interruption_count: u32,
    ) -> Result<(), StorageError> {
        // owned move into the Send + 'static closure.
        let date = date.to_string();
        self.with_conn(move |conn| {
            // 행 보장(없으면 INSERT) 후 누적 UPDATE — 동일 락 안에서 수행.
            Self::get_or_create_focus_metrics_inner(conn, &date)?;
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
            .map_err(|e| {
                StorageError::Internal(format!("Failed to increment focus metric: {e}"))
            })?;

            Ok(())
        })
        .await
    }

    /// Shared get-or-create body, runnable on a borrowed `&Connection` (no lock
    /// management). Used by the sync `get_or_create_focus_metrics` and both
    /// async variants so the SQL stays single-sourced.
    fn get_or_create_focus_metrics_inner(
        conn: &rusqlite::Connection,
        date: &str,
    ) -> Result<FocusMetrics, StorageError> {
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
        let period = maekon_core::types::TimeWindow::new(period_start, period_end)
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

    pub fn get_recent_focus_metrics(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, StorageError> {
        // 읽기 — read_lock(deletion_flag 무관).
        let read = self.conn.read_lock();
        Self::get_recent_focus_metrics_inner(read.conn(), days)
    }

    /// Async `get_recent_focus_metrics` over the read funnel (ADR-026 PR-5).
    pub(crate) async fn get_recent_focus_metrics_async(
        &self,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, StorageError> {
        self.with_conn_read(move |conn| Self::get_recent_focus_metrics_inner(conn, days))
            .await
    }

    /// Shared `get_recent_focus_metrics` body, runnable on a borrowed
    /// `&Connection` (no lock management). Used by the sync inherent method
    /// (storage benches) and the async variant so the SQL stays single-sourced.
    fn get_recent_focus_metrics_inner(
        conn: &rusqlite::Connection,
        days: usize,
    ) -> Result<Vec<(String, FocusMetrics)>, StorageError> {
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
            let period = maekon_core::types::TimeWindow::new(period_start, period_end)
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
