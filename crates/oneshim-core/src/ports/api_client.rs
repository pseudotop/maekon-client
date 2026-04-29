//! Server API client port — defines the contract for all HTTP/gRPC
//! communication with the connected server (session, batch upload, heartbeat).
//! Implemented by `HttpApiClient` and `UnifiedClient` in `oneshim-network`.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::event::EventBatch;
use crate::models::frame::ContextUpload;
use crate::models::suggestion::{Suggestion, SuggestionFeedback};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionCreateResponse {
    pub session_id: String,
    pub user_id: String,
    pub client_id: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[async_trait]
/// All methods route HTTP responses through the canonical semantic status
/// mapping (see `docs/guides/http-status-error-mapping.md`). Common wire codes:
/// `auth.failed` (401/403), `not_found.resource_missing` (404),
/// `network.timeout` (408/504 / reqwest timeout), `network.rate_limit` (429),
/// `service.unavailable` (502/503), `network.generic` (other non-2xx or
/// connection-level failures).
pub trait ApiClient: Send + Sync {
    /// Create a server session for the given client.
    async fn create_session(&self, client_id: &str) -> Result<SessionCreateResponse, CoreError>;

    /// End an active server session.
    async fn end_session(&self, session_id: &str) -> Result<(), CoreError>;

    /// Upload an event batch to the server.
    async fn upload_batch(&self, batch: &EventBatch) -> Result<(), CoreError>;

    /// Upload context data (frames + metadata) to the server.
    async fn upload_context(&self, upload: &ContextUpload) -> Result<(), CoreError>;

    /// Send suggestion feedback (accept/reject) to the server.
    async fn send_feedback(&self, feedback: &SuggestionFeedback) -> Result<(), CoreError>;

    /// Send a session heartbeat to keep the server session alive.
    async fn send_heartbeat(&self, session_id: &str) -> Result<(), CoreError>;
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum SseEvent {
    Connected {
        session_id: String,
    },
    Suggestion(Suggestion),
    Update(serde_json::Value),
    Heartbeat {
        timestamp: chrono::DateTime<chrono::Utc>,
    },
    Error(String),
    Close,
}

#[async_trait]
pub trait SseClient: Send + Sync {
    /// Connect to the server SSE stream for real-time suggestion delivery.
    ///
    /// # Errors
    /// This method runs a connect/retry loop; transient failures are logged
    /// and the loop reconnects with exponential backoff. The only `Err`
    /// returned is `CoreError::Auth` (wire: `auth.failed`) when the session
    /// token is rejected — re-auth required. All other failure classes
    /// (transport, 5xx, server restart) are handled internally via
    /// reconnect.
    async fn connect(
        &self,
        session_id: &str,
        tx: tokio::sync::mpsc::Sender<SseEvent>,
    ) -> Result<(), CoreError>;
}
