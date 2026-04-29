//! Conversation session ports — defines contracts for AI conversation
//! session management across CLI subprocess, HTTP API, and local LLM backends.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;

use crate::error::CoreError;
use crate::models::ai_session::{
    ConversationSessionInfo, OutboundMessage, SessionConfig, SessionMessage, SessionState,
};

/// Streaming response from a conversation session.
pub type ResponseStream = Pin<Box<dyn Stream<Item = Result<OutboundMessage, CoreError>> + Send>>;

#[async_trait]
pub trait ConversationSession: Send + Sync {
    /// Send a message with optional attachments, receive streaming response.
    ///
    /// # Errors
    /// Routed through ADR-019 wire codes:
    /// - Subprocess CLI returning an empty body → `CoreError::Analysis`
    ///   (wire: `provider.analysis_failed`; iter-106 re-route from
    ///   Internal).
    /// - HTTP streaming errors → canonical semantic HTTP status mapping
    ///   (wire: `auth.failed` / `network.timeout` / `network.rate_limit` /
    ///   `service.unavailable` / `provider.analysis_failed`). See
    ///   `docs/guides/http-status-error-mapping.md`.
    /// - True intra-process failures (tokio JoinError, lock poisoning,
    ///   pipe setup) remain `CoreError::Internal` (wire: `internal.generic`).
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError>;

    /// Current session info (synchronous — returns locally held state).
    fn info(&self) -> ConversationSessionInfo;

    /// Unique session identifier.
    fn session_id(&self) -> &str;

    /// Provider display name.
    fn provider_name(&self) -> &str;

    /// Gracefully terminate the session, releasing provider resources.
    /// Default implementation is a no-op for backwards compatibility.
    async fn terminate(&self) {}
}

#[async_trait]
pub trait SessionManager: Send + Sync {
    /// Create a new session with the given provider.
    ///
    /// # Errors
    /// Returns `CoreError::ServiceUnavailable` if max concurrent sessions
    /// reached (iter-97: was `Internal` before; capacity is a transient
    /// service-availability condition). Returns `CoreError::NotFound` if
    /// subprocess CLI surface detection fails (iter-94). Returns
    /// `CoreError::InvalidArguments` if required config fields are missing
    /// in the request payload. Returns `CoreError::Auth` if credentials
    /// unavailable.
    async fn create_session(
        &self,
        config: SessionConfig,
    ) -> Result<Arc<dyn ConversationSession>, CoreError>;

    /// Terminate a session.
    ///
    /// # Errors
    /// Returns `CoreError::NotFound` (wire: `not_found.resource_missing`)
    /// if the session ID is not found. Iter-94: previously documented as
    /// `Internal`, but the implementation emits `NotFound` consistent with
    /// the catalog-miss pattern.
    async fn kill_session(&self, session_id: &str) -> Result<(), CoreError>;

    /// List active sessions.
    async fn list_sessions(&self) -> Vec<ConversationSessionInfo>;

    /// Retrieve a session by ID.
    ///
    /// # Errors
    /// Returns `CoreError::NotFound` (wire: `not_found.resource_missing`)
    /// if the session ID is not found.
    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn ConversationSession>, CoreError>;

    /// Recover a failed session when retry budget remains.
    async fn recover_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn ConversationSession>, CoreError>;

    /// Reset idle timer for a session (keeps it alive during active use).
    async fn touch_session(&self, session_id: &str);

    /// Report an adapter-level failure. Returns the resulting session state.
    async fn report_failure(&self, session_id: &str, error: &CoreError) -> SessionState;

    /// Terminate all active sessions during app shutdown.
    async fn shutdown_all(&self);
}
