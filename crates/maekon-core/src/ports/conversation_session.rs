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

    /// Whether this session transmits conversation content to an external
    /// provider — a cloud HTTP API or a provider-owned CLI subprocess.
    ///
    /// External sessions MUST be wrapped in a privacy guard before any user
    /// content (prompt, history, attachments) is transmitted off-device.
    /// Mirrors [`crate::ports::llm_provider::LlmProvider::is_external`] so the
    /// same guard invariant can be enforced on the conversation (chat) path.
    ///
    /// Defaults to `false` (local / in-process, e.g. Ollama) for backwards
    /// compatibility; adapters that egress data override this to `true`.
    /// See E21 review finding B1 (chat path was previously unguarded).
    fn is_external(&self) -> bool {
        false
    }

    /// Network endpoints that may carry chat messages, history, or attachments.
    ///
    /// In-process/local sessions return an empty list. Sessions that report
    /// `is_external() == false` while still using an endpoint must expose that
    /// URL so guard decorators can independently verify that every such
    /// endpoint is loopback before allowing an unguarded local fast path.
    fn egress_endpoint_urls(&self) -> Vec<&str> {
        Vec::new()
    }

    /// Interrupt (stop) the session's in-flight turn (E21 #5017).
    ///
    /// Turn-lifecycle control for backends that expose a steerable/cancellable
    /// in-flight turn (the Codex `app-server` `turn/interrupt`). The DEFAULT
    /// returns `CoreError::InvalidArguments` — most backends (stateless HTTP
    /// request/response, local subprocess LLM, provider CLIs) have no
    /// server-side in-flight turn to interrupt, so "interrupt" is an
    /// invalid-request-for-this-state condition rather than a hard failure.
    ///
    /// The error code reuses `ValidationCode::InvalidArguments` deliberately:
    /// "no interruptible in-flight turn for this backend" is honestly an
    /// invalid-request condition, and reusing an existing wire code keeps the
    /// locked `wire_contract_snapshot` untouched (no new variant/code).
    ///
    /// # Errors
    /// Returns `CoreError::InvalidArguments` by default (no interruptible turn).
    /// Real implementations may surface transport errors (`Network` / timeout).
    async fn interrupt(&self) -> Result<(), CoreError> {
        Err(CoreError::InvalidArguments {
            code: crate::error_codes::ValidationCode::InvalidArguments,
            message: "interrupt is unsupported for this session backend (no in-flight turn)"
                .to_string(),
        })
    }

    /// Steer the session's in-flight turn with additional user input (E21
    /// #5017).
    ///
    /// Mid-turn course-correction for backends that expose a steerable in-flight
    /// turn (the Codex `app-server` `turn/steer`). The DEFAULT returns
    /// `CoreError::InvalidArguments` for the same reason as [`interrupt`].
    ///
    /// `message` carries USER content (the steering text), so a privacy-guard
    /// decorator MUST sanitize it before an external backend transmits it —
    /// exactly like [`send_message`].
    ///
    /// # Errors
    /// Returns `CoreError::InvalidArguments` by default (no steerable turn).
    async fn steer(&self, _message: &SessionMessage) -> Result<(), CoreError> {
        Err(CoreError::InvalidArguments {
            code: crate::error_codes::ValidationCode::InvalidArguments,
            message: "steer is unsupported for this session backend (no in-flight turn)"
                .to_string(),
        })
    }
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

    /// Record a successful completed turn and reset transient retry budget.
    async fn record_success(&self, session_id: &str);

    /// Report an adapter-level failure. Returns the resulting session state.
    async fn report_failure(&self, session_id: &str, error: &CoreError) -> SessionState;

    /// Terminate all active sessions during app shutdown.
    async fn shutdown_all(&self);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ai_session::{
        ConversationSessionInfo, SessionMessage, SessionState, SessionTransport,
    };
    use chrono::Utc;

    /// A minimal session that does NOT override `is_external`.
    struct LocalOnlySession;

    #[async_trait]
    impl ConversationSession for LocalOnlySession {
        async fn send_message(
            &self,
            _message: &SessionMessage,
        ) -> Result<ResponseStream, CoreError> {
            // Never exercised by these tests (they only assert `is_external`).
            Err(CoreError::Internal {
                code: crate::error_codes::InternalCode::Generic,
                message: "unused in test".to_string(),
            })
        }
        fn info(&self) -> ConversationSessionInfo {
            ConversationSessionInfo {
                session_id: "local".to_string(),
                provider_name: "local".to_string(),
                model: "m".to_string(),
                state: SessionState::Active,
                transport: SessionTransport::LocalLlm,
                created_at: Utc::now(),
                last_active: Utc::now(),
                turn_count: 0,
                title: None,
            }
        }
        fn session_id(&self) -> &str {
            "local"
        }
        fn provider_name(&self) -> &str {
            "local"
        }
    }

    /// A session that egresses data off-device overrides `is_external`.
    struct ExternalSession;

    #[async_trait]
    impl ConversationSession for ExternalSession {
        async fn send_message(
            &self,
            _message: &SessionMessage,
        ) -> Result<ResponseStream, CoreError> {
            // Never exercised by these tests (they only assert `is_external`).
            Err(CoreError::Internal {
                code: crate::error_codes::InternalCode::Generic,
                message: "unused in test".to_string(),
            })
        }
        fn info(&self) -> ConversationSessionInfo {
            ConversationSessionInfo {
                session_id: "ext".to_string(),
                provider_name: "ext".to_string(),
                model: "m".to_string(),
                state: SessionState::Active,
                transport: SessionTransport::Subprocess,
                created_at: Utc::now(),
                last_active: Utc::now(),
                turn_count: 0,
                title: None,
            }
        }
        fn session_id(&self) -> &str {
            "ext"
        }
        fn provider_name(&self) -> &str {
            "ext"
        }
        fn is_external(&self) -> bool {
            true
        }
    }

    fn user_msg() -> SessionMessage {
        SessionMessage {
            role: crate::models::ai_session::MessageRole::User,
            content: "steer me".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        }
    }

    #[test]
    fn is_external_defaults_to_false_for_local_sessions() {
        assert!(!LocalOnlySession.is_external());
    }

    #[test]
    fn is_external_can_be_overridden_to_true() {
        assert!(ExternalSession.is_external());
    }

    #[tokio::test]
    async fn interrupt_defaults_to_invalid_arguments() {
        // A backend without an in-flight-turn protocol takes the default Err.
        let err = LocalOnlySession
            .interrupt()
            .await
            .expect_err("default interrupt must Err");
        assert!(matches!(
            err,
            CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn steer_defaults_to_invalid_arguments() {
        let err = LocalOnlySession
            .steer(&user_msg())
            .await
            .expect_err("default steer must Err");
        assert!(matches!(
            err,
            CoreError::InvalidArguments {
                code: crate::error_codes::ValidationCode::InvalidArguments,
                ..
            }
        ));
    }

    /// A session overriding `interrupt`/`steer` must compile and run — proving
    /// the default is overridable for the real (CodexAppServerSession) backend.
    struct SteerableSession;

    #[async_trait]
    impl ConversationSession for SteerableSession {
        async fn send_message(
            &self,
            _message: &SessionMessage,
        ) -> Result<ResponseStream, CoreError> {
            Err(CoreError::Internal {
                code: crate::error_codes::InternalCode::Generic,
                message: "unused".to_string(),
            })
        }
        fn info(&self) -> ConversationSessionInfo {
            ConversationSessionInfo {
                session_id: "steerable".to_string(),
                provider_name: "steerable".to_string(),
                model: "m".to_string(),
                state: SessionState::Active,
                transport: SessionTransport::Subprocess,
                created_at: Utc::now(),
                last_active: Utc::now(),
                turn_count: 0,
                title: None,
            }
        }
        fn session_id(&self) -> &str {
            "steerable"
        }
        fn provider_name(&self) -> &str {
            "steerable"
        }
        async fn interrupt(&self) -> Result<(), CoreError> {
            Ok(())
        }
        async fn steer(&self, _message: &SessionMessage) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn interrupt_and_steer_can_be_overridden_ok() {
        // Contract: a ConversationSession implementation that overrides interrupt() and
        // steer() to return Ok(()) must compile and not surface a CoreError. This is
        // the dispatch path for the Codex app-server steerable backend (E21 #5017).
        // Ok(()) is genuinely the entire contract here — there is no value beyond unit
        // to pin; the test's purpose is verifying the trait default is overridable (#5594).
        SteerableSession
            .interrupt()
            .await
            .expect("overridden interrupt() must return Ok(()) for a steerable backend");
        SteerableSession
            .steer(&user_msg())
            .await
            .expect("overridden steer() must return Ok(()) for a steerable backend");
    }
}
