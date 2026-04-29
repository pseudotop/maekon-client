use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;

use crate::services::stream_service::RealtimeStreamQueryService;
use crate::services::web_contexts::RealtimeStreamWebContext;

/// GET /api/stream
pub async fn event_stream(
    State(context): State<RealtimeStreamWebContext>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let sse_stream = RealtimeStreamQueryService::new(context).event_stream();

    Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    )
}

#[cfg(test)]
mod tests {
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};
    use oneshim_api_contracts::stream::{
        AiRuntimeStatus, FrameUpdate, IdleUpdate, MetricsUpdate, RealtimeEvent,
    };

    use oneshim_storage::sqlite::SqliteStorage;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    fn test_app_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn loopback_app() -> axum::Router {
        let state = test_app_state();
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    #[tokio::test]
    async fn stream_endpoint_returns_sse_content_type() {
        let app = loopback_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .expect("content-type header should be present")
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("text/event-stream"),
            "expected text/event-stream but got: {content_type}"
        );
    }

    #[test]
    fn realtime_event_metrics_serializes() {
        let event = RealtimeEvent::Metrics(MetricsUpdate {
            timestamp: "2026-03-21T10:00:00Z".to_string(),
            cpu_usage: 55.0,
            memory_percent: 72.5,
            memory_used: 12_000_000_000,
            memory_total: 16_000_000_000,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"metrics\""));
        assert!(json.contains("\"cpu_usage\":55.0"));
    }

    #[test]
    fn realtime_event_frame_serializes() {
        let event = RealtimeEvent::Frame(FrameUpdate {
            id: 42,
            timestamp: "2026-03-21T10:00:00Z".to_string(),
            app_name: "Terminal".to_string(),
            window_title: "zsh".to_string(),
            importance: 0.7,
            trigger_type: "active_change".to_string(),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"frame\""));
        assert!(json.contains("\"app_name\":\"Terminal\""));
    }

    #[test]
    fn realtime_event_idle_serializes() {
        let event = RealtimeEvent::Idle(IdleUpdate {
            is_idle: false,
            idle_secs: 0,
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"idle\""));
        assert!(json.contains("\"is_idle\":false"));
    }

    #[test]
    fn realtime_event_ai_runtime_status_serializes() {
        let event = RealtimeEvent::AiRuntimeStatus(AiRuntimeStatus {
            ocr_source: "remote".to_string(),
            llm_source: "local".to_string(),
            ocr_fallback_reason: None,
            llm_fallback_reason: Some("API key missing".to_string()),
        });
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ai_runtime_status\""));
        assert!(json.contains("\"llm_source\":\"local\""));
    }

    #[test]
    fn realtime_event_ping_serializes() {
        let event = RealtimeEvent::Ping;
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ping\""));
    }
}
