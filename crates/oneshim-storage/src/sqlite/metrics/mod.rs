use async_trait::async_trait;
use chrono::{DateTime, Duration, Timelike, Utc};
use oneshim_core::error::CoreError;
use oneshim_core::models::activity::{
    IdlePeriod, ProcessSnapshot, ProcessSnapshotEntry, SessionStats,
};
use oneshim_core::models::system::{NetworkInfo, SystemMetrics};
use oneshim_core::ports::storage::MetricsStorage;
use tracing::{debug, info};

use super::{HourlyMetricsRecord, SqliteStorage};
use crate::error::StorageError;

impl SqliteStorage {
    pub fn list_session_stats(&self, limit: usize) -> Result<Vec<SessionStats>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT session_id, started_at, ended_at, total_events, total_frames, total_idle_secs
                 FROM session_stats
                 ORDER BY started_at DESC
                 LIMIT ?1",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![limit as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut sessions = Vec::new();
        for row in rows {
            let (session_id, started_str, ended_str, events, frames, idle) =
                row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?;

            let started_at = DateTime::parse_from_rfc3339(&started_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let ended_at = ended_str.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .map(|dt| dt.with_timezone(&Utc))
                    .ok()
            });

            sessions.push(SessionStats {
                session_id,
                started_at,
                ended_at,
                total_events: events as u64,
                total_frames: frames as u64,
                total_idle_secs: idle as u64,
            });
        }

        Ok(sessions)
    }

    pub fn list_hourly_metrics_since(
        &self,
        from_hour: &str,
    ) -> Result<Vec<HourlyMetricsRecord>, StorageError> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| StorageError::Internal(format!("Failed to acquire lock: {e}")))?;

        let mut stmt = conn
            .prepare(
                "SELECT hour, cpu_avg, cpu_max, memory_avg, memory_max, sample_count
                 FROM system_metrics_hourly
                 WHERE hour >= ?1
                 ORDER BY hour ASC",
            )
            .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

        let rows = stmt
            .query_map(rusqlite::params![from_hour], |row| {
                Ok(HourlyMetricsRecord {
                    hour: row.get(0)?,
                    cpu_avg: row.get(1)?,
                    cpu_max: row.get(2)?,
                    memory_avg: row.get::<_, i64>(3)? as u64,
                    memory_max: row.get::<_, i64>(4)? as u64,
                    sample_count: row.get::<_, i64>(5)? as u64,
                })
            })
            .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?;

        let mut result = Vec::new();
        for row in rows {
            result
                .push(row.map_err(|e| StorageError::Internal(format!("Failed to read row: {e}")))?);
        }

        Ok(result)
    }
}

#[async_trait]
impl MetricsStorage for SqliteStorage {
    // --------------------------------------------------------
    // --------------------------------------------------------

    async fn save_metrics(&self, metrics: &SystemMetrics) -> Result<(), CoreError> {
        let timestamp = metrics.timestamp.to_rfc3339();
        let cpu_usage = metrics.cpu_usage;
        let memory_used = metrics.memory_used as i64;
        let memory_total = metrics.memory_total as i64;
        let disk_used = metrics.disk_used as i64;
        let disk_total = metrics.disk_total as i64;
        let (upload, download) = metrics
            .network
            .as_ref()
            .map(|n| (n.upload_speed as i64, n.download_speed as i64))
            .unwrap_or((0, 0));
        let memory_used_mb = metrics.memory_used / 1_048_576;

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO system_metrics (timestamp, cpu_usage, memory_used, memory_total, disk_used, disk_total, network_upload, network_download)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    timestamp,
                    cpu_usage,
                    memory_used,
                    memory_total,
                    disk_used,
                    disk_total,
                    upload,
                    download,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save system metrics: {e}")))?;

            debug!(
                "system metrics saved: CPU {:.1}%, memory {}MB",
                cpu_usage,
                memory_used_mb
            );
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn get_metrics(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<SystemMetrics>, CoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT timestamp, cpu_usage, memory_used, memory_total, disk_used, disk_total, network_upload, network_download
                     FROM system_metrics
                     WHERE timestamp >= ?1 AND timestamp <= ?2
                     ORDER BY timestamp DESC
                     LIMIT ?3",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

            let metrics = stmt
                .query_map(rusqlite::params![from_str, to_str, limit as i64], |row| {
                    let ts_str: String = row.get(0)?;
                    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    let upload: i64 = row.get(6)?;
                    let download: i64 = row.get(7)?;

                    Ok(SystemMetrics {
                        timestamp,
                        cpu_usage: row.get(1)?,
                        memory_used: row.get::<_, i64>(2)? as u64,
                        memory_total: row.get::<_, i64>(3)? as u64,
                        disk_used: row.get::<_, i64>(4)? as u64,
                        disk_total: row.get::<_, i64>(5)? as u64,
                        network: Some(NetworkInfo {
                            upload_speed: upload as u64,
                            download_speed: download as u64,
                            is_connected: upload > 0 || download > 0,
                        }),
                        typing_wpm: 0.0,
                    })
                })
                .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(metrics)
        })
        .await
        .map_err(Into::into)
    }

    async fn aggregate_hourly_metrics(&self, hour: DateTime<Utc>) -> Result<(), CoreError> {
        let hour_start = hour
            .with_minute(0)
            .and_then(|dt| dt.with_second(0))
            .and_then(|dt| dt.with_nanosecond(0))
            .unwrap_or(hour);
        let hour_end = hour_start + Duration::hours(1);

        let hour_str = hour_start.format("%Y-%m-%dT%H:00:00Z").to_string();
        let from_str = hour_start.to_rfc3339();
        let to_str = hour_end.to_rfc3339();

        self.with_conn(move |conn| {
            // AVG() in SQLite always returns REAL, so both memory columns here must
            // be read as Option<f64> — reading as Option<i64> raises
            // InvalidColumnType and was silently swallowed by the `_` arm below
            // (found via Phase 5-D8 PR1 Task 3 test authoring).
            let result: Result<(f64, f64, f64, i64, i64), rusqlite::Error> = conn.query_row(
                "SELECT AVG(cpu_usage), MAX(cpu_usage), AVG(memory_used), MAX(memory_used), COUNT(*)
                 FROM system_metrics
                 WHERE timestamp >= ?1 AND timestamp < ?2",
                rusqlite::params![from_str, to_str],
                |row| {
                    Ok((
                        row.get::<_, Option<f64>>(0)?.unwrap_or(0.0),
                        row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
                        row.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                        row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                        row.get(4)?,
                    ))
                },
            );

            match result {
                Ok((cpu_avg, cpu_max, memory_avg, memory_max, count)) if count > 0 => {
                    // memory_avg is REAL (from AVG()); the target column is INTEGER,
                    // so truncate for storage.
                    let memory_avg_i64 = memory_avg as i64;
                    conn.execute(
                        "INSERT OR REPLACE INTO system_metrics_hourly (hour, cpu_avg, cpu_max, memory_avg, memory_max, sample_count)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        rusqlite::params![hour_str, cpu_avg, cpu_max, memory_avg_i64, memory_max, count],
                    )
                    .map_err(|e| {
                        StorageError::Internal(format!("Failed to save hourly aggregate: {e}"))
                    })?;
                    debug!("hour: {} ({count}items )", hour_str);
                }
                Ok(_) => {
                    debug!("hour: {} (data none)", hour_str);
                }
                Err(e) => {
                    // Propagate genuine query errors rather than silently
                    // swallowing — the previous `_` arm masked the
                    // InvalidColumnType bug fixed in this commit.
                    return Err(StorageError::Internal(format!(
                        "Failed to aggregate hourly metrics for {hour_str}: {e}"
                    )));
                }
            }

            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn cleanup_old_metrics(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        let cutoff = before.to_rfc3339();

        self.with_conn(move |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM system_metrics WHERE timestamp < ?1",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to delete stale metrics: {e}"))
                })?;

            if deleted > 0 {
                info!("{deleted}items delete");
            }
            Ok(deleted)
        })
        .await
        .map_err(Into::into)
    }

    // --------------------------------------------------------
    // --------------------------------------------------------

    async fn save_process_snapshot(&self, snapshot: &ProcessSnapshot) -> Result<(), CoreError> {
        let timestamp = snapshot.timestamp.to_rfc3339();
        let process_count = snapshot.processes.len();
        let data = serde_json::to_string(&snapshot.processes)?;

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO process_snapshots (timestamp, snapshot_data) VALUES (?1, ?2)",
                rusqlite::params![timestamp, data],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save process snapshot: {e}")))?;

            debug!("process snapshot saved: {} processes", process_count);
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn get_process_snapshots(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> Result<Vec<ProcessSnapshot>, CoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT timestamp, snapshot_data FROM process_snapshots
                     WHERE timestamp >= ?1 AND timestamp <= ?2
                     ORDER BY timestamp DESC
                     LIMIT ?3",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

            let snapshots = stmt
                .query_map(rusqlite::params![from_str, to_str, limit as i64], |row| {
                    let ts_str: String = row.get(0)?;
                    let data: String = row.get(1)?;

                    let timestamp = DateTime::parse_from_rfc3339(&ts_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    let processes: Vec<ProcessSnapshotEntry> =
                        serde_json::from_str(&data).unwrap_or_default();

                    Ok(ProcessSnapshot {
                        timestamp,
                        processes,
                    })
                })
                .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(snapshots)
        })
        .await
        .map_err(Into::into)
    }

    async fn cleanup_old_process_snapshots(
        &self,
        before: DateTime<Utc>,
    ) -> Result<usize, CoreError> {
        let cutoff = before.to_rfc3339();

        self.with_conn(move |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM process_snapshots WHERE timestamp < ?1",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to delete stale snapshots: {e}"))
                })?;

            if deleted > 0 {
                info!("{deleted}items delete");
            }
            Ok(deleted)
        })
        .await
        .map_err(Into::into)
    }

    // --------------------------------------------------------
    // --------------------------------------------------------

    async fn start_idle_period(&self, start_time: DateTime<Utc>) -> Result<i64, CoreError> {
        let start_time_str = start_time.to_rfc3339();

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO idle_periods (start_time) VALUES (?1)",
                rusqlite::params![start_time_str],
            )
            .map_err(|e| {
                StorageError::Internal(format!("idle period started record failure: {e}"))
            })?;

            let id = conn.last_insert_rowid();
            debug!("idle period started: id={}", id);
            Ok(id)
        })
        .await
        .map_err(Into::into)
    }

    async fn end_idle_period(&self, id: i64, end_time: DateTime<Utc>) -> Result<(), CoreError> {
        let end_time_str = end_time.to_rfc3339();

        self.with_conn(move |conn| {
            let duration_secs: i64 = conn
                .query_row(
                    "UPDATE idle_periods
                     SET end_time = ?1,
                         duration_secs = CAST((julianday(?1) - julianday(start_time)) * 86400 AS INTEGER)
                     WHERE id = ?2
                     RETURNING duration_secs",
                    rusqlite::params![end_time_str, id],
                    |row| row.get(0),
                )
                .map_err(|e| {
                    StorageError::Internal(format!("idle period ended record failure: {e}"))
                })?;

            debug!("idle period ended: id={}, duration={}s", id, duration_secs);
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn get_ongoing_idle_period(&self) -> Result<Option<(i64, IdlePeriod)>, CoreError> {
        self.with_conn(move |conn| {
            let result: Result<(i64, String), rusqlite::Error> = conn.query_row(
                "SELECT id, start_time FROM idle_periods WHERE end_time IS NULL ORDER BY id DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            );

            match result {
                Ok((id, start_str)) => {
                    let start_time = DateTime::parse_from_rfc3339(&start_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    Ok(Some((
                        id,
                        IdlePeriod {
                            start_time,
                            end_time: None,
                            duration_secs: None,
                        },
                    )))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(StorageError::Internal(format!(
                    "진행 중 idle period query failure: {e}"
                ))),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn get_idle_periods(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<Vec<IdlePeriod>, CoreError> {
        let from_str = from.to_rfc3339();
        let to_str = to.to_rfc3339();

        self.with_conn(move |conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT start_time, end_time, duration_secs FROM idle_periods
                     WHERE start_time >= ?1 AND start_time <= ?2
                     ORDER BY start_time DESC",
                )
                .map_err(|e| StorageError::Internal(format!("Failed to prepare query: {e}")))?;

            let periods = stmt
                .query_map(rusqlite::params![from_str, to_str], |row| {
                    let start_str: String = row.get(0)?;
                    let end_str: Option<String> = row.get(1)?;
                    let duration: Option<i64> = row.get(2)?;

                    let start_time = DateTime::parse_from_rfc3339(&start_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    let end_time = end_str.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .map(|dt| dt.with_timezone(&Utc))
                            .ok()
                    });

                    Ok(IdlePeriod {
                        start_time,
                        end_time,
                        duration_secs: duration.map(|d| d as u64),
                    })
                })
                .map_err(|e| StorageError::Internal(format!("Failed to execute query: {e}")))?
                .filter_map(|r| r.ok())
                .collect();

            Ok(periods)
        })
        .await
        .map_err(Into::into)
    }

    async fn cleanup_old_idle_periods(&self, before: DateTime<Utc>) -> Result<usize, CoreError> {
        let cutoff = before.to_rfc3339();

        self.with_conn(move |conn| {
            let deleted = conn
                .execute(
                    "DELETE FROM idle_periods WHERE start_time < ?1 AND end_time IS NOT NULL",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| {
                    StorageError::Internal(format!("Failed to delete stale idle periods: {e}"))
                })?;

            if deleted > 0 {
                info!("idle period {deleted}items delete");
            }
            Ok(deleted)
        })
        .await
        .map_err(Into::into)
    }

    // --------------------------------------------------------
    // --------------------------------------------------------

    async fn upsert_session(&self, stats: &SessionStats) -> Result<(), CoreError> {
        let session_id = stats.session_id.clone();
        let started_at = stats.started_at.to_rfc3339();
        let ended_at = stats.ended_at.map(|dt| dt.to_rfc3339());
        let total_events = stats.total_events as i64;
        let total_frames = stats.total_frames as i64;
        let total_idle_secs = stats.total_idle_secs as i64;

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO session_stats (session_id, started_at, ended_at, total_events, total_frames, total_idle_secs)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(session_id) DO UPDATE SET
                    ended_at = excluded.ended_at,
                    total_events = excluded.total_events,
                    total_frames = excluded.total_frames,
                    total_idle_secs = excluded.total_idle_secs",
                rusqlite::params![
                    session_id,
                    started_at,
                    ended_at,
                    total_events,
                    total_frames,
                    total_idle_secs,
                ],
            )
            .map_err(|e| StorageError::Internal(format!("Failed to save session stats: {e}")))?;

            debug!("session save: {}", session_id);
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn get_session(&self, session_id: &str) -> Result<Option<SessionStats>, CoreError> {
        let session_id = session_id.to_string();

        self.with_conn(move |conn| {
            let result: Result<(String, Option<String>, i64, i64, i64), rusqlite::Error> = conn
                .query_row(
                    "SELECT started_at, ended_at, total_events, total_frames, total_idle_secs
                     FROM session_stats WHERE session_id = ?1",
                    rusqlite::params![session_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                );

            match result {
                Ok((started_str, ended_str, events, frames, idle)) => {
                    let started_at = DateTime::parse_from_rfc3339(&started_str)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    let ended_at = ended_str.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .map(|dt| dt.with_timezone(&Utc))
                            .ok()
                    });

                    Ok(Some(SessionStats {
                        session_id,
                        started_at,
                        ended_at,
                        total_events: events as u64,
                        total_frames: frames as u64,
                        total_idle_secs: idle as u64,
                    }))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(StorageError::Internal(format!(
                    "session query failure: {e}"
                ))),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn end_session(
        &self,
        session_id: &str,
        ended_at: DateTime<Utc>,
    ) -> Result<(), CoreError> {
        let session_id = session_id.to_string();
        let ended_at_str = ended_at.to_rfc3339();

        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE session_stats SET ended_at = ?1 WHERE session_id = ?2",
                rusqlite::params![ended_at_str, session_id],
            )
            .map_err(|e| StorageError::Internal(format!("session ended record failure: {e}")))?;

            debug!("session ended: {}", session_id);
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn increment_session_counters(
        &self,
        session_id: &str,
        events: u64,
        frames: u64,
        idle_secs: u64,
    ) -> Result<(), CoreError> {
        let session_id = session_id.to_string();

        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE session_stats SET
                    total_events = total_events + ?1,
                    total_frames = total_frames + ?2,
                    total_idle_secs = total_idle_secs + ?3
                 WHERE session_id = ?4",
                rusqlite::params![events as i64, frames as i64, idle_secs as i64, session_id],
            )
            .map_err(|e| {
                StorageError::Internal(format!("Failed to increment session counter: {e}"))
            })?;

            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests;
