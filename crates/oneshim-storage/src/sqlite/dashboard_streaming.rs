//! SQLite impls for v2b dashboard streaming queries.
//!
//! `aggregate_metrics_window` averages `system_metrics` rows in a
//! half-open `[from, to)` interval; `fetch_dashboard_event_source`
//! returns canonical rows for Frame signals (Idle / AiRuntimeStatus
//! are latest-state and served directly from the RealtimeEvent payload
//! at the grpc handler — never reach this impl).

use chrono::{DateTime, Utc};
use rusqlite::params;

use oneshim_core::error::CoreError;
use oneshim_core::error_codes::{InternalCode, NotFoundCode, StorageCode};
use oneshim_core::models::dashboard_streaming::{
    DashboardEventRecord, DashboardEventSignal, MetricBucketRecord,
};

use super::SqliteStorage;

impl oneshim_core::ports::web_storage::DashboardStreamingStorage for SqliteStorage {
    fn aggregate_metrics_window(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
    ) -> Result<MetricBucketRecord, CoreError> {
        let conn = self.conn.lock().map_err(|_| CoreError::Internal {
            code: InternalCode::Generic,
            message: "metrics mutex poisoned".to_string(),
        })?;

        let from_s = from.to_rfc3339();
        let to_s = to.to_rfc3339();

        let mut stmt = conn
            .prepare(
                "SELECT AVG(cpu_usage), AVG(memory_used), COUNT(*)
                 FROM system_metrics
                 WHERE timestamp >= ?1 AND timestamp < ?2",
            )
            .map_err(|e| CoreError::Storage {
                code: StorageCode::Failed,
                message: format!("prepare aggregate_metrics: {e}"),
            })?;

        let (cpu_avg, mem_avg_bytes, _count): (Option<f64>, Option<f64>, i64) = stmt
            .query_row(params![from_s, to_s], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| CoreError::Storage {
                code: StorageCode::Failed,
                message: format!("aggregate_metrics query: {e}"),
            })?;

        // Keystroke / mouse counters: SQLite `system_metrics` schema does not
        // carry them today. Zero-initialise for now; a future schema migration
        // can backfill.
        Ok(MetricBucketRecord {
            start: from,
            cpu_avg_pct: cpu_avg.unwrap_or(0.0),
            memory_avg_mb: mem_avg_bytes.unwrap_or(0.0) / (1024.0 * 1024.0),
            active_keystrokes: 0,
            active_mouse_clicks: 0,
        })
    }

    fn fetch_dashboard_event_source(
        &self,
        signal: &DashboardEventSignal,
    ) -> Result<DashboardEventRecord, CoreError> {
        match signal {
            DashboardEventSignal::Frame(id) => self.fetch_frame_event(*id),
            DashboardEventSignal::Idle | DashboardEventSignal::AiRuntimeStatus => {
                Err(CoreError::Internal {
                    code: InternalCode::Generic,
                    message: "fetch_dashboard_event_source invoked with non-Frame signal \
                              (Idle / AiRuntimeStatus must be served from event payload)"
                        .to_string(),
                })
            }
        }
    }
}

impl SqliteStorage {
    fn fetch_frame_event(&self, frame_id: i64) -> Result<DashboardEventRecord, CoreError> {
        let conn = self.conn.lock().map_err(|_| CoreError::Internal {
            code: InternalCode::Generic,
            message: "frames mutex poisoned".to_string(),
        })?;

        // frames table uses `timestamp` (not `captured_at`) — confirmed in migration v01.
        let row = conn
            .query_row(
                "SELECT timestamp, app_name, window_title, importance, trigger_type
                 FROM frames
                 WHERE id = ?1",
                params![frame_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, f64>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => CoreError::NotFound {
                    code: NotFoundCode::ResourceMissing,
                    resource_type: "frame".to_string(),
                    id: frame_id.to_string(),
                },
                other => CoreError::Storage {
                    code: StorageCode::Failed,
                    message: format!("fetch_frame: {other}"),
                },
            })?;

        let occurred_at = DateTime::parse_from_rfc3339(&row.0)
            .map_err(|e| CoreError::Storage {
                code: StorageCode::Failed,
                message: format!("frame timestamp parse: {e}"),
            })?
            .with_timezone(&Utc);

        Ok(DashboardEventRecord::Frame {
            frame_id,
            occurred_at,
            app_name: row.1,
            window_title: row.2,
            importance: row.3 as f32,
            trigger_type: row.4,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::SqliteStorage;
    use chrono::{Duration, TimeZone, Utc};
    use oneshim_core::error::CoreError;
    use oneshim_core::models::dashboard_streaming::DashboardEventSignal;
    use oneshim_core::models::system::SystemMetrics;
    use oneshim_core::ports::storage::MetricsStorage;
    use oneshim_core::ports::web_storage::DashboardStreamingStorage;

    fn in_memory() -> SqliteStorage {
        SqliteStorage::open_in_memory(30).expect("open_in_memory")
    }

    #[tokio::test]
    async fn aggregate_metrics_window_empty_returns_zero_bucket() {
        let storage = in_memory();
        let now = Utc::now();
        let bucket = storage
            .aggregate_metrics_window(now - Duration::seconds(60), now)
            .expect("aggregate returns Ok");

        assert_eq!(bucket.cpu_avg_pct, 0.0);
        assert_eq!(bucket.memory_avg_mb, 0.0);
        assert_eq!(bucket.active_keystrokes, 0);
        assert_eq!(bucket.active_mouse_clicks, 0);
    }

    #[tokio::test]
    async fn aggregate_metrics_window_averages_two_rows() {
        let storage = in_memory();
        let now = Utc.with_ymd_and_hms(2026, 4, 21, 12, 0, 0).unwrap();

        let m1 = SystemMetrics {
            timestamp: now - Duration::seconds(50),
            cpu_usage: 40.0,
            memory_used: 4 * 1024 * 1024 * 1024,
            memory_total: 16 * 1024 * 1024 * 1024,
            disk_used: 0,
            disk_total: 0,
            network: None,
            typing_wpm: 0.0,
        };
        let m2 = SystemMetrics {
            timestamp: now - Duration::seconds(20),
            cpu_usage: 60.0,
            memory_used: 8 * 1024 * 1024 * 1024,
            memory_total: 16 * 1024 * 1024 * 1024,
            disk_used: 0,
            disk_total: 0,
            network: None,
            typing_wpm: 0.0,
        };
        storage.save_metrics(&m1).await.expect("save m1");
        storage.save_metrics(&m2).await.expect("save m2");

        let bucket = storage
            .aggregate_metrics_window(now - Duration::seconds(60), now)
            .expect("aggregate ok");

        assert!((bucket.cpu_avg_pct - 50.0).abs() < 0.5);
        assert!((bucket.memory_avg_mb - 6144.0).abs() < 32.0);
    }

    #[tokio::test]
    async fn fetch_frame_returns_not_found_for_missing_id() {
        let storage = in_memory();
        let err = storage
            .fetch_dashboard_event_source(&DashboardEventSignal::Frame(999999))
            .expect_err("should be NotFound");
        assert!(
            matches!(err, CoreError::NotFound { .. }),
            "expected CoreError::NotFound, got {err:?}"
        );
    }

    #[test]
    fn fetch_idle_returns_internal_error() {
        let storage = in_memory();
        let err = storage
            .fetch_dashboard_event_source(&DashboardEventSignal::Idle)
            .expect_err("Idle signal must error — served from payload");
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "expected CoreError::Internal for Idle, got {err:?}"
        );
    }

    #[test]
    fn fetch_ai_runtime_status_returns_internal_error() {
        let storage = in_memory();
        let err = storage
            .fetch_dashboard_event_source(&DashboardEventSignal::AiRuntimeStatus)
            .expect_err("AiRuntimeStatus signal must error — served from payload");
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "expected CoreError::Internal for AiRuntimeStatus, got {err:?}"
        );
    }
}
