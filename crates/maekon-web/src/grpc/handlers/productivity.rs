//! ADR-013: GetProductivityMetrics RPC handler.

use crate::grpc::to_proto_ts;
use crate::proto::dashboard::v1::{
    GetProductivityMetricsRequest, MetricBucket, ProductivityMetricsResponse,
};
use crate::storage_port::WebStorage;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Default + hard cap for GetProductivityMetrics.
pub const DEFAULT_METRICS_SINCE_HOURS: u32 = 24;
pub const MAX_METRICS_SINCE_HOURS: u32 = 168;

pub async fn get_productivity_metrics(
    storage: Arc<dyn WebStorage>,
    req: Request<GetProductivityMetricsRequest>,
) -> Result<Response<ProductivityMetricsResponse>, Status> {
    let since_hours = match req.into_inner().since_hours {
        0 => DEFAULT_METRICS_SINCE_HOURS,
        n => n.min(MAX_METRICS_SINCE_HOURS),
    };
    let from = (chrono::Utc::now() - chrono::Duration::hours(i64::from(since_hours))).to_rfc3339();

    // ADR-026 PR-7: `list_hourly_metrics_since` is now async and offloads the
    // SQLite read onto `spawn_blocking` internally, so await it directly instead
    // of wrapping the (now-async) call in a hand-rolled `spawn_blocking`.
    let records = storage
        .list_hourly_metrics_since(&from)
        .await
        .map_err(|e| Status::internal(format!("list_hourly_metrics_since: {e}")))?;

    let buckets = records
        .into_iter()
        .map(|r| {
            // Parse the RFC3339 hour key ("YYYY-MM-DDTHH:00:00Z") produced
            // by the SQLite metrics aggregation. On parse failure, fall back
            // to None so the bucket is still emitted with an absent start
            // rather than being silently dropped.
            let start = chrono::DateTime::parse_from_rfc3339(&r.hour)
                .ok()
                .map(|dt| to_proto_ts(dt.with_timezone(&chrono::Utc)));
            MetricBucket {
                start,
                cpu_avg_pct: r.cpu_avg,
                // HourlyMetricsRecord stores bytes; convert to MB for the wire field.
                memory_avg_mb: r.memory_avg as f64 / 1_048_576.0,
                // Keystroke/click counters are not tracked in the hourly
                // metrics table (v2a); they land in v2b streaming buckets.
                active_keystrokes: 0,
                active_mouse_clicks: 0,
            }
        })
        .collect();

    Ok(Response::new(ProductivityMetricsResponse { buckets }))
}
