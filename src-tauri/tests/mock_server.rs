use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{atomic::AtomicU64, Arc};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[derive(Debug, Default)]
pub struct MockServerState {
    pub request_count: AtomicU64,
    pub sessions: RwLock<HashMap<String, SessionInfo>>,
    pub tokens: RwLock<HashMap<String, TokenInfo>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub user_id: String,
    pub client_id: String,
    pub created_at: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TokenInfo {
    pub access_token: String,
    pub expires_at: i64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    #[serde(alias = "email")]
    pub identifier: String,
    pub password: String,
    #[serde(default)]
    pub organization_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoginResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub expires_in: i64,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SessionCreateRequest {
    pub client_id: String,
    pub metadata: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionCreateResponse {
    pub session_id: String,
    pub user_id: String,
    pub client_id: String,
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct BatchSyncRequest {
    pub events: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BatchSyncResponse {
    pub synced_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub session_active: bool,
}

pub struct MockServer {
    pub addr: String,
    pub port: u16,
    pub state: Arc<MockServerState>,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl MockServer {
    pub async fn start() -> Self {
        Self::start_on_port(0).await // 0 = random port
    }

    pub async fn start_on_port(port: u16) -> Self {
        let state = Arc::new(MockServerState::default());
        let app = create_router(state.clone());

        let listener = TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .expect("failed to bind port");

        let local_addr = listener.local_addr().unwrap();
        let actual_port = local_addr.port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = shutdown_rx.await;
                })
                .await
                .expect("server execution failure");
        });

        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Self {
            addr: format!("http://127.0.0.1:{}", actual_port),
            port: actual_port,
            state,
            shutdown_tx: Some(shutdown_tx),
        }
    }

    pub fn url(&self) -> &str {
        &self.addr
    }

    pub fn request_count(&self) -> u64 {
        self.state
            .request_count
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn session_count(&self) -> usize {
        self.state.sessions.read().len()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn create_router(state: Arc<MockServerState>) -> Router {
    Router::new()
        .route("/api/v1/auth/tokens", post(handle_login)) // login
        .route("/api/v1/auth/tokens/refresh", post(handle_refresh)) // token refresh
        .route("/user_context/sessions/", post(handle_create_session))
        .route(
            "/user_context/sessions/{session_id}/heartbeat",
            post(handle_health),
        )
        .route("/user_context/batches", post(handle_batch_sync))
        .route("/user_context/suggestions/feedback", post(handle_feedback))
        .route("/user_context/suggestions/stream", get(handle_sse_stream))
        // U2-sse-e2e: 프로덕션 `SseStreamClient::stream_url`이 실제로 GET 하는 경로.
        // (`crates/maekon-network/src/sse_client.rs::stream_url`는
        //  `{base_url}/user_context/sessions/stream?session_id=...`를 호출한다.)
        // 실제 `suggestion` SSE 이벤트를 흘려보내 receiver→queue→notifier 체인을 구동한다.
        .route(
            "/user_context/sessions/stream",
            get(handle_suggestion_sse_stream),
        )
        .with_state(state)
}

async fn handle_login(
    State(state): State<Arc<MockServerState>>,
    Json(req): Json<LoginRequest>,
) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    if req.identifier.is_empty() || req.password.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "Invalid credentials"})),
        )
            .into_response();
    }

    let access_token = format!("mock_access_{}", uuid::Uuid::new_v4());
    let refresh_token = format!("mock_refresh_{}", uuid::Uuid::new_v4());

    state.tokens.write().insert(
        access_token.clone(),
        TokenInfo {
            access_token: access_token.clone(),
            expires_at: chrono::Utc::now().timestamp() + 3600,
        },
    );

    Json(LoginResponse {
        access_token,
        refresh_token,
        token_type: "Bearer".to_string(),
        expires_in: 3600,
    })
    .into_response()
}

async fn handle_refresh(State(state): State<Arc<MockServerState>>) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let access_token = format!("mock_access_{}", uuid::Uuid::new_v4());

    Json(LoginResponse {
        access_token,
        refresh_token: format!("mock_refresh_{}", uuid::Uuid::new_v4()),
        token_type: "Bearer".to_string(),
        expires_in: 3600,
    })
}

async fn handle_create_session(
    State(state): State<Arc<MockServerState>>,
    Json(req): Json<SessionCreateRequest>,
) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let session_id = format!("session_{}", uuid::Uuid::new_v4());
    let user_id = format!("user_{}", uuid::Uuid::new_v4());

    let session = SessionInfo {
        session_id: session_id.clone(),
        user_id: user_id.clone(),
        client_id: req.client_id.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    state.sessions.write().insert(session_id.clone(), session);

    Json(SessionCreateResponse {
        session_id,
        user_id,
        client_id: req.client_id,
        permissions: vec!["read".to_string(), "write".to_string()],
    })
}

async fn handle_health(State(state): State<Arc<MockServerState>>) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    Json(HealthResponse {
        status: "ok".to_string(),
        session_active: true,
    })
}

async fn handle_batch_sync(
    State(state): State<Arc<MockServerState>>,
    Json(req): Json<BatchSyncRequest>,
) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let count = req.events.len();

    Json(BatchSyncResponse {
        synced_count: count,
        failed_count: 0,
    })
}

async fn handle_feedback(State(state): State<Arc<MockServerState>>) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    Json(serde_json::json!({"status": "ok"}))
}

async fn handle_sse_stream(State(state): State<Arc<MockServerState>>) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    let body = "event: ping\ndata: {\"type\":\"ping\"}\n\n";

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream")],
        body,
    )
}

/// U2-sse-e2e: 실제 `suggestion` 이벤트를 방출하는 SSE 스트림 핸들러.
///
/// 프로덕션 `SseStreamClient::connect`가 `Eventsource`로 파싱하는 텍스트 형식을 그대로
/// 내보낸다. 이벤트 순서:
///   1. `connection` — `SseEvent::Connected` 로 파싱됨 (`session_id` 필요)
///   2. `suggestion` — `SseEvent::Suggestion(Suggestion)` 로 파싱됨 (JSON 본문은
///      `maekon_core::models::suggestion::Suggestion` serde 계약을 따름:
///      enum 은 SCREAMING_SNAKE_CASE)
///   3. `close`      — `SseEvent::Close` → receiver 의 run() 루프가 종료됨
///
/// `close` 이벤트로 스트림을 끝맺으므로 receiver 가 재연결 루프에 빠지지 않고
/// 자연스럽게 run() 을 반환한다. 단일 정적 본문이지만 실제 HTTP/SSE 전송 경로를
/// 통과하므로 in-memory 테스트가 아닌 라이브 e2e 다.
async fn handle_suggestion_sse_stream(
    State(state): State<Arc<MockServerState>>,
) -> impl IntoResponse {
    state
        .request_count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    // Suggestion serde 계약과 정확히 일치하는 JSON 본문을 직렬화로 생성한다.
    // (필드명/enum 표기를 손으로 적지 않고 serde_json::json! 으로 구성)
    let suggestion_json = serde_json::json!({
        "suggestion_id": "u2-sse-e2e-1",
        "suggestion_type": "WORK_GUIDANCE",
        "content": "라이브 SSE 경로로 전달된 제안",
        "priority": "HIGH",
        "confidence_score": 0.91,
        "relevance_score": 0.88,
        "is_actionable": true,
        "created_at": "2026-06-03T10:00:00Z",
        "source": "LLM_SERVER"
    })
    .to_string();

    // SSE 프레임: 각 이벤트는 `event:`/`data:` 라인 + 빈 줄로 구분된다.
    let body = format!(
        "event: connection\ndata: {{\"session_id\":\"u2-sse-session\"}}\n\n\
         event: suggestion\ndata: {suggestion_json}\n\n\
         event: close\ndata: \n\n"
    );

    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream")],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_server_starts() {
        let server = MockServer::start().await;
        assert!(!server.url().is_empty());
        assert!(server.port > 0);
    }

    #[tokio::test]
    async fn test_login_endpoint() {
        let server = MockServer::start().await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/api/v1/auth/tokens", server.url()))
            .json(&serde_json::json!({
                "email": "test@example.com",
                "password": "test-password-placeholder"
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let body: LoginResponse = resp.json().await.unwrap();
        assert!(body.access_token.starts_with("mock_access_"));
        assert_eq!(body.token_type, "Bearer");
    }

    #[tokio::test]
    async fn test_session_creation() {
        let server = MockServer::start().await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/user_context/sessions/", server.url()))
            .json(&serde_json::json!({
                "client_id": "test_client_123"
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let body: SessionCreateResponse = resp.json().await.unwrap();
        assert!(body.session_id.starts_with("session_"));
        assert_eq!(body.client_id, "test_client_123");
        assert_eq!(server.session_count(), 1);
    }

    #[tokio::test]
    async fn test_batch_sync() {
        let server = MockServer::start().await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("{}/user_context/batches", server.url()))
            .json(&serde_json::json!({
                "events": [
                    {"type": "window_change", "app": "Chrome"},
                    {"type": "window_change", "app": "Slack"}
                ]
            }))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let body: BatchSyncResponse = resp.json().await.unwrap();
        assert_eq!(body.synced_count, 2);
        assert_eq!(body.failed_count, 0);
    }

    #[tokio::test]
    async fn test_health_check() {
        let server = MockServer::start().await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "{}/user_context/sessions/test_session_1/heartbeat",
                server.url()
            ))
            .send()
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);

        let body: HealthResponse = resp.json().await.unwrap();
        assert_eq!(body.status, "ok");
        assert!(body.session_active);
    }

    #[tokio::test]
    async fn test_request_counting() {
        let server = MockServer::start().await;
        assert_eq!(server.request_count(), 0);

        let client = reqwest::Client::new();

        let _ = client
            .post(format!(
                "{}/user_context/sessions/s1/heartbeat",
                server.url()
            ))
            .send()
            .await;
        let _ = client
            .post(format!(
                "{}/user_context/sessions/s2/heartbeat",
                server.url()
            ))
            .send()
            .await;
        let _ = client
            .post(format!(
                "{}/user_context/sessions/s3/heartbeat",
                server.url()
            ))
            .send()
            .await;

        assert_eq!(server.request_count(), 3);
    }
}
