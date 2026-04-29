use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Json;
use oneshim_api_contracts::frames::FrameResponse;

use crate::error::ApiError;
use crate::services::frames_service::FramesQueryService;
use crate::services::web_contexts::StorageWebContext;

use super::{PaginatedResponse, TimeRangeQuery};

/// GET /api/frames?from=&to=&limit=&offset=
pub async fn get_frames(
    State(context): State<StorageWebContext>,
    Query(params): Query<TimeRangeQuery>,
) -> Result<Json<PaginatedResponse<FrameResponse>>, ApiError> {
    Ok(Json(FramesQueryService::new(context).get_frames(&params)?))
}

/// GET /api/frames/:id/image
pub async fn get_frame_image(
    State(context): State<StorageWebContext>,
    Path(frame_id): Path<i64>,
) -> Response {
    FramesQueryService::new(context)
        .get_frame_image(frame_id)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};
    use chrono::Utc;
    use oneshim_core::models::frame::FrameMetadata;
    use oneshim_storage::encryption::EncryptionKey;
    use oneshim_storage::frame_storage::FrameFileStorage;

    use oneshim_storage::sqlite::SqliteStorage;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    fn test_app_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn loopback_app(state: AppState) -> axum::Router {
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    #[test]
    fn frame_response_serializes() {
        let frame = FrameResponse {
            id: 1,
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            trigger_type: "AppSwitch".to_string(),
            app_name: "Code".to_string(),
            window_title: "main.rs".to_string(),
            importance: 0.85,
            resolution: "1920x1080".to_string(),
            file_path: Some("frames/123.webp".to_string()),
            ocr_text: None,
            image_url: Some("/api/frames/1/image".to_string()),
            tag_ids: vec![1, 3],
        };
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("Code"));
        assert!(json.contains("tag_ids"));
    }

    #[tokio::test]
    async fn get_frame_image_returns_not_found_for_nonexistent() {
        let app = loopback_app(test_app_state());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/frames/99999/image")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn get_frame_image_returns_decrypted_bytes_for_encrypted_frame_storage() {
        let data_dir = tempfile::tempdir().expect("temp data dir");
        let encryption_key = Arc::new(EncryptionKey::from_bytes([0x42; 32]));
        let frame_storage = Arc::new(
            FrameFileStorage::with_encryption(
                data_dir.path().to_path_buf(),
                100,
                7,
                Some(encryption_key),
            )
            .await
            .expect("encrypted frame storage"),
        );
        let plaintext = b"RIFF\x00\x00\x00\x00WEBPVP8 test image bytes";
        let timestamp = Utc::now();
        let relative_path = frame_storage
            .save_frame(timestamp, plaintext)
            .await
            .expect("save encrypted frame");

        let sqlite = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let frame_id = sqlite
            .save_frame_metadata(
                &FrameMetadata {
                    timestamp,
                    trigger_type: "manual".to_string(),
                    app_name: "Codex".to_string(),
                    window_title: String::new(),
                    resolution: (1920, 1080),
                    importance: 1.0,
                },
                Some(&relative_path.to_string_lossy()),
                None,
            )
            .expect("frame metadata");

        let (event_tx, _) = broadcast::channel(16);
        let mut state = AppState::with_core(sqlite, event_tx);
        state.core.frames_dir = Some(data_dir.path().to_path_buf());
        state.core.frame_storage = Some(frame_storage);
        let app = loopback_app(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/frames/{frame_id}/image"))
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        assert_eq!(bytes.as_ref(), plaintext);
    }
}
