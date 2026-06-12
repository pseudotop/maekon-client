//! ADR-013: GetSessionStats RPC handler.

use crate::proto::dashboard::v1::{GetSessionStatsRequest, SessionStatsResponse};
use crate::storage_port::WebStorage;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Default sample size when `GetSessionStatsRequest::limit == 0`.
pub const DEFAULT_SESSION_STATS_LIMIT: usize = 1000;

pub async fn get_session_stats(
    storage: Arc<dyn WebStorage>,
    req: Request<GetSessionStatsRequest>,
) -> Result<Response<SessionStatsResponse>, Status> {
    let limit = match req.into_inner().limit {
        0 => DEFAULT_SESSION_STATS_LIMIT,
        n => n as usize,
    };

    // ADR-026 PR-5: `list_session_stats` is now async and offloads the SQLite
    // read onto the `spawn_blocking` pool internally (via the storage
    // `with_conn_read` funnel), so the hand-rolled `spawn_blocking` wrapper is
    // removed and the call is awaited directly.
    let rows = storage
        .list_session_stats(limit)
        .await
        .map_err(|e| Status::internal(format!("list_session_stats: {e}")))?;

    let total_sessions = rows.len() as u32;
    let mut ended_sessions: u32 = 0;
    let mut total_events: u64 = 0;
    let mut total_frames: u64 = 0;
    let mut total_idle_secs: u64 = 0;
    let mut total_duration_secs: i64 = 0;
    for row in &rows {
        total_events += row.total_events;
        total_frames += row.total_frames;
        total_idle_secs += row.total_idle_secs;
        if let Some(ended_at) = row.ended_at {
            ended_sessions += 1;
            let duration = (ended_at - row.started_at).num_seconds();
            if duration > 0 {
                total_duration_secs += duration;
            }
        }
    }
    let avg_duration_secs = if ended_sessions > 0 {
        total_duration_secs as f64 / f64::from(ended_sessions)
    } else {
        0.0
    };

    Ok(Response::new(SessionStatsResponse {
        total_sessions,
        ended_sessions,
        avg_duration_secs,
        total_events,
        total_frames,
        total_idle_secs,
    }))
}
