//! AI session REST handlers — session CRUD and SSE message streaming.

use axum::extract::{Path, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::stream::Stream;
use futures::StreamExt;
use std::convert::Infallible;
use std::time::Duration;

use oneshim_api_contracts::sessions::{AiSendMessageRequest, AiSessionPath};
use oneshim_core::models::ai_session::{
    ConversationSessionInfo, MessageRole, OutboundMessage, SessionConfig, SessionMessage,
};

use crate::error::ApiError;
use crate::services::web_contexts::AiSessionWebContext;

/// POST /api/ai/sessions — create a new AI conversation session.
pub async fn create_session(
    State(context): State<AiSessionWebContext>,
    Json(config): Json<SessionConfig>,
) -> Result<Json<ConversationSessionInfo>, ApiError> {
    let session_manager = context.session_manager.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("AI session manager is not configured".to_string())
    })?;

    let session = session_manager.create_session(config).await?;
    Ok(Json(session.info()))
}

/// GET /api/ai/sessions — list all active sessions.
pub async fn list_sessions(
    State(context): State<AiSessionWebContext>,
) -> Result<Json<Vec<ConversationSessionInfo>>, ApiError> {
    let session_manager = context.session_manager.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("AI session manager is not configured".to_string())
    })?;

    let sessions = session_manager.list_sessions().await;
    Ok(Json(sessions))
}

/// GET /api/ai/sessions/{id} — get a single session by ID.
pub async fn get_session(
    State(context): State<AiSessionWebContext>,
    Path(path): Path<AiSessionPath>,
) -> Result<Json<ConversationSessionInfo>, ApiError> {
    let session_manager = context.session_manager.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("AI session manager is not configured".to_string())
    })?;

    let session = session_manager.get_session(&path.id).await?;
    Ok(Json(session.info()))
}

/// DELETE /api/ai/sessions/{id} — terminate and remove a session.
pub async fn delete_session(
    State(context): State<AiSessionWebContext>,
    Path(path): Path<AiSessionPath>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let session_manager = context.session_manager.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("AI session manager is not configured".to_string())
    })?;

    session_manager.kill_session(&path.id).await?;
    Ok(Json(serde_json::json!({ "deleted": true })))
}

/// POST /api/ai/sessions/{id}/messages — send a message and stream the response via SSE.
pub async fn send_message(
    State(context): State<AiSessionWebContext>,
    Path(path): Path<AiSessionPath>,
    Json(req): Json<AiSendMessageRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let session_manager = context.session_manager.as_ref().ok_or_else(|| {
        ApiError::ServiceUnavailable("AI session manager is not configured".to_string())
    })?;

    let session = session_manager.get_session(&path.id).await?;

    // Keep session alive — matches Tauri path behavior.
    session_manager.touch_session(&path.id).await;

    let message = SessionMessage {
        role: MessageRole::User,
        content: req.content,
        attachments: req.attachments,
        tools: req.tools,
        context: req.context,
        response_format: req.response_format,
    };

    let mgr_for_stream = session_manager.clone();
    let session_id_for_stream = path.id.clone();
    let response_stream = match session.send_message(&message).await {
        Ok(s) => s,
        Err(err) => {
            session_manager.report_failure(&path.id, &err).await;
            return Err(err.into());
        }
    };

    // Convert ResponseStream items to SSE Events (async for report_failure).
    let sse_stream = response_stream.then(move |item| {
        let mgr = mgr_for_stream.clone();
        let sid = session_id_for_stream.clone();
        async move {
            Ok::<Event, Infallible>(match item {
                Ok(outbound) => match &outbound {
                    OutboundMessage::Text { .. } => Event::default()
                        .event("text")
                        .json_data(&outbound)
                        .unwrap_or_else(|_| {
                            Event::default().event("error").data("serialize error")
                        }),
                    OutboundMessage::Result { .. } => Event::default()
                        .event("result")
                        .json_data(&outbound)
                        .unwrap_or_else(|_| {
                            Event::default().event("error").data("serialize error")
                        }),
                    OutboundMessage::ToolUse { .. } => Event::default()
                        .event("tool_use")
                        .json_data(&outbound)
                        .unwrap_or_else(|_| {
                            Event::default().event("error").data("serialize error")
                        }),
                    OutboundMessage::Error { .. } => Event::default()
                        .event("error")
                        .json_data(&outbound)
                        .unwrap_or_else(|_| Event::default().event("error").data("unknown error")),
                    OutboundMessage::Control { .. } => Event::default()
                        .event("control")
                        .json_data(&outbound)
                        .unwrap_or_else(|_| {
                            Event::default().event("error").data("serialize error")
                        }),
                    OutboundMessage::Thinking { .. } => Event::default()
                        .event("thinking")
                        .json_data(&outbound)
                        .unwrap_or_else(|_| {
                            Event::default().event("error").data("serialize error")
                        }),
                    OutboundMessage::ToolCallDelta { .. } => Event::default()
                        .event("tool_call_delta")
                        .json_data(&outbound)
                        .unwrap_or_else(|_| {
                            Event::default().event("error").data("serialize error")
                        }),
                },
                Err(err) => {
                    let new_state = mgr.report_failure(&sid, &err).await;
                    let retryable =
                        new_state == oneshim_core::models::ai_session::SessionState::Active;
                    let error_msg = OutboundMessage::Error {
                        code: "stream".to_string(),
                        message: err.to_string(),
                        retryable,
                    };
                    Event::default()
                        .event("error")
                        .data(serde_json::to_string(&error_msg).unwrap_or_default())
                }
            })
        }
    });

    Ok(Sse::new(sse_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("ping"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppState;
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::extract::connect_info::MockConnectInfo;
    use axum::http::{Request, StatusCode};

    use oneshim_core::error::CoreError;
    use oneshim_core::models::ai_session::{ConversationSessionInfo, SessionState};
    use oneshim_core::ports::conversation_session::{ConversationSession, SessionManager};
    use oneshim_storage::sqlite::SqliteStorage;
    use std::collections::HashMap;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::sync::{broadcast, Mutex};
    use tower::ServiceExt;

    // ── Mock SessionManager ──────────────────────────────────────

    struct MockSessionManager {
        sessions: Mutex<HashMap<String, ConversationSessionInfo>>,
    }

    impl MockSessionManager {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(HashMap::new()),
            }
        }
    }

    struct MockConversationSession {
        info: ConversationSessionInfo,
    }

    #[async_trait]
    impl ConversationSession for MockConversationSession {
        async fn send_message(
            &self,
            _message: &SessionMessage,
        ) -> Result<oneshim_core::ports::conversation_session::ResponseStream, CoreError> {
            Err(CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: "not implemented in mock".to_string(),
            })
        }

        fn info(&self) -> ConversationSessionInfo {
            self.info.clone()
        }

        fn session_id(&self) -> &str {
            &self.info.session_id
        }

        fn provider_name(&self) -> &str {
            &self.info.provider_name
        }
    }

    #[async_trait]
    impl SessionManager for MockSessionManager {
        async fn create_session(
            &self,
            config: SessionConfig,
        ) -> Result<Arc<dyn ConversationSession>, CoreError> {
            let now = chrono::Utc::now();
            let info = ConversationSessionInfo {
                session_id: uuid::Uuid::new_v4().to_string(),
                provider_name: "mock".to_string(),
                model: config.model.unwrap_or_else(|| "mock-model".to_string()),
                state: SessionState::Active,
                transport: config.transport,
                created_at: now,
                last_active: now,
                turn_count: 0,
                title: None,
            };
            self.sessions
                .lock()
                .await
                .insert(info.session_id.clone(), info.clone());
            Ok(Arc::new(MockConversationSession { info }))
        }

        async fn kill_session(&self, session_id: &str) -> Result<(), CoreError> {
            self.sessions
                .lock()
                .await
                .remove(session_id)
                .map(|_| ())
                .ok_or_else(|| CoreError::NotFound {
                    code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                    resource_type: "session".to_string(),
                    id: session_id.to_string(),
                })
        }

        async fn list_sessions(&self) -> Vec<ConversationSessionInfo> {
            self.sessions.lock().await.values().cloned().collect()
        }

        async fn get_session(
            &self,
            session_id: &str,
        ) -> Result<Arc<dyn ConversationSession>, CoreError> {
            let guard = self.sessions.lock().await;
            let info = guard.get(session_id).ok_or_else(|| CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "session".to_string(),
                id: session_id.to_string(),
            })?;
            Ok(Arc::new(MockConversationSession { info: info.clone() }))
        }

        async fn recover_session(
            &self,
            session_id: &str,
        ) -> Result<Arc<dyn ConversationSession>, CoreError> {
            self.get_session(session_id).await
        }

        async fn touch_session(&self, _session_id: &str) {}

        async fn report_failure(&self, _session_id: &str, _error: &CoreError) -> SessionState {
            SessionState::Failed
        }

        async fn shutdown_all(&self) {
            self.sessions.lock().await.clear();
        }
    }

    // ── Helpers ──────────────────────────────────────────────────

    fn test_app_state() -> AppState {
        let storage = Arc::new(SqliteStorage::open_in_memory(30).expect("in-memory sqlite"));
        let (event_tx, _) = broadcast::channel(16);
        AppState::with_core(storage, event_tx)
    }

    fn test_app_state_with_session_manager() -> AppState {
        let mut state = test_app_state();
        state.session.manager = Some(Arc::new(MockSessionManager::new()));
        state
    }

    fn loopback_app(state: AppState) -> axum::Router {
        crate::WebServer::build_router(state)
            .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    // ── Tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn list_sessions_returns_empty_initially() {
        let app = loopback_app(test_app_state_with_session_manager());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ai/sessions")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json parse");
        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().expect("array").len(), 0);
    }

    #[tokio::test]
    async fn create_session_with_valid_config() {
        let app = loopback_app(test_app_state_with_session_manager());
        let body = serde_json::json!({
            "transport": "subprocess",
            "tools_enabled": false
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ai/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_string(&body).expect("serialize")))
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body bytes");
        let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json parse");
        assert!(parsed.get("session_id").is_some());
        assert_eq!(parsed["state"], "active");
        assert_eq!(parsed["transport"], "subprocess");
    }

    #[tokio::test]
    async fn create_session_returns_error_on_invalid_transport() {
        let app = loopback_app(test_app_state_with_session_manager());
        let body = r#"{"transport": "carrier_pigeon", "tools_enabled": false}"#;

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/ai/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .expect("request build"),
            )
            .await
            .expect("response");

        // Axum rejects unrecognized enum values during deserialization → 422
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn get_session_returns_not_found_for_nonexistent() {
        let app = loopback_app(test_app_state_with_session_manager());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/ai/sessions/nonexistent-id-999")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_session_returns_not_found_for_nonexistent() {
        let app = loopback_app(test_app_state_with_session_manager());
        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/api/ai/sessions/nonexistent-id-999")
                    .body(Body::empty())
                    .expect("request build"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
