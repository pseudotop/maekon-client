//! ADR-013: GetFocusStats RPC handler.

use crate::proto::dashboard::v1::{FocusStatsResponse, GetFocusStatsRequest};
use crate::storage_port::WebStorage;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Default + hard cap for GetFocusStats.
pub const DEFAULT_FOCUS_DAYS: u32 = 7;
pub const MAX_FOCUS_DAYS: u32 = 90;

pub async fn get_focus_stats(
    storage: Arc<dyn WebStorage>,
    req: Request<GetFocusStatsRequest>,
) -> Result<Response<FocusStatsResponse>, Status> {
    let days = match req.into_inner().days {
        0 => DEFAULT_FOCUS_DAYS,
        n => n.min(MAX_FOCUS_DAYS),
    };

    let days_usize = days as usize;
    // ADR-026 PR-5: `get_recent_focus_metrics` is now async and offloads the
    // SQLite read onto the `spawn_blocking` pool internally (via the storage
    // `with_conn_read` funnel), so the hand-rolled `spawn_blocking` wrapper is
    // removed and the call is awaited directly.
    let records = storage
        .get_recent_focus_metrics(days_usize)
        .await
        .map_err(|e| Status::internal(format!("get_recent_focus_metrics: {e}")))?;

    let bucket_count = records.len() as u32;
    let mut total_active_secs: u64 = 0;
    let mut total_deep_work_secs: u64 = 0;
    let mut total_communication_secs: u64 = 0;
    let mut total_interruptions: u32 = 0;
    let mut focus_score_sum: f32 = 0.0;
    let mut longest_focus_secs: u64 = 0;
    for (_date, m) in &records {
        total_active_secs += m.total_active_secs;
        total_deep_work_secs += m.deep_work_secs;
        total_communication_secs += m.communication_secs;
        total_interruptions = total_interruptions.saturating_add(m.interruption_count);
        focus_score_sum += m.focus_score;
        if m.max_focus_duration_secs > longest_focus_secs {
            longest_focus_secs = m.max_focus_duration_secs;
        }
    }
    let avg_focus_score = if bucket_count > 0 {
        focus_score_sum / bucket_count as f32
    } else {
        0.0
    };

    Ok(Response::new(FocusStatsResponse {
        bucket_count,
        total_active_secs,
        total_deep_work_secs,
        total_communication_secs,
        total_interruptions,
        avg_focus_score,
        longest_focus_secs,
    }))
}
