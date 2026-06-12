//! ADR-013: GetRecentFrames RPC handler.

use crate::proto::dashboard::v1::{
    recent_frames_response, GetRecentFramesRequest, RecentFramesResponse,
};
use crate::storage_port::WebStorage;
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Default + hard cap for GetRecentFrames.
pub const DEFAULT_RECENT_FRAMES_LIMIT: u32 = 50;
pub const MAX_RECENT_FRAMES_LIMIT: u32 = 500;
pub const DEFAULT_RECENT_FRAMES_SINCE_HOURS: u32 = 1;
pub const MAX_RECENT_FRAMES_SINCE_HOURS: u32 = 168; // 7 days

pub async fn get_recent_frames(
    storage: Arc<dyn WebStorage>,
    pii_sanitizer: Option<Arc<dyn PiiSanitizer>>,
    req: Request<GetRecentFramesRequest>,
) -> Result<Response<RecentFramesResponse>, Status> {
    let req = req.into_inner();
    let limit = match req.limit {
        0 => DEFAULT_RECENT_FRAMES_LIMIT,
        n => n.min(MAX_RECENT_FRAMES_LIMIT),
    };
    let since_hours = match req.since_hours {
        0 => DEFAULT_RECENT_FRAMES_SINCE_HOURS,
        n => n.min(MAX_RECENT_FRAMES_SINCE_HOURS),
    };

    let to = chrono::Utc::now();
    let from = to - chrono::Duration::hours(i64::from(since_hours));

    let limit_usize = limit as usize;
    // ADR-026 PR-4: `get_frames` is now async and offloads onto the storage
    // `with_conn_read` funnel internally, so the hand-rolled `spawn_blocking`
    // wrapper is removed and the call is awaited directly.
    let records = storage
        .get_frames(from, to, limit_usize)
        .await
        .map_err(|e| Status::internal(format!("get_frames: {e}")))?;

    let frames = records
        .into_iter()
        .map(|r| recent_frames_response::FrameMetadata {
            frame_id: r.id,
            captured_at: r.timestamp,
            trigger_type: r.trigger_type,
            app_name: crate::grpc::privacy::sanitize_dashboard_text(
                r.app_name,
                pii_sanitizer.as_deref(),
            ),
            window_title: crate::grpc::privacy::sanitize_dashboard_text(
                r.window_title,
                pii_sanitizer.as_deref(),
            ),
            importance: r.importance,
            resolution_w: r.resolution_w,
            resolution_h: r.resolution_h,
        })
        .collect();

    Ok(Response::new(RecentFramesResponse { frames }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use maekon_core::config::PiiFilterLevel;
    use maekon_core::models::frame::FrameMetadata;
    use maekon_storage::sqlite::SqliteStorage;

    struct MarkerSanitizer;

    impl PiiSanitizer for MarkerSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("secret@example.com", "[EMAIL]")
                .replace("Acme Roadmap", "[TITLE]")
        }
    }

    #[tokio::test]
    async fn get_recent_frames_sanitizes_app_and_window_titles() {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("sqlite"));
        let timestamp = Utc::now() - Duration::hours(1);
        storage
            .save_frame_metadata(
                &FrameMetadata {
                    timestamp,
                    trigger_type: "timer".to_string(),
                    app_name: "secret@example.com".to_string(),
                    window_title: "Acme Roadmap".to_string(),
                    resolution: (1920, 1080),
                    importance: 0.8,
                    monitor_id: None,
                    app_bundle_id: None,
                },
                None,
                None,
            )
            .expect("seed frame");

        let response = get_recent_frames(
            storage,
            Some(Arc::new(MarkerSanitizer)),
            Request::new(GetRecentFramesRequest {
                limit: 1,
                since_hours: 24,
            }),
        )
        .await
        .expect("get frames")
        .into_inner();

        let frame = response.frames.first().expect("one frame");
        assert_eq!(frame.app_name, "[EMAIL]");
        assert_eq!(frame.window_title, "[TITLE]");
    }
}
