//! Privacy guard decorator for the conversation (chat) path — E21 #4882/#4883.
//!
//! Wraps an external `ConversationSession` so user content (prompt + history +
//! attachments) is sanitized before it leaves the device, closing the chat-path
//! PII gap surfaced by E21 review B1. Mirrors `GuardedLlmProvider` on the chat
//! trait and follows the `AuditingSession` decorator precedent (port-injected
//! collaborator). The guard collaborator is a narrow, src-tauri-local port per
//! ADR-001 §7 (both implementor and consumer live in src-tauri).

use std::sync::Arc;

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{ConversationSessionInfo, SessionMessage};
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};

/// Narrow, src-tauri-local guard port: sanitize a chat message before it is
/// transmitted to an external provider. A single method (Interface Segregation)
/// keeps the decorator decoupled from the broad OCR/LLM guard surface and makes
/// the decorator's branch logic trivially mockable in unit tests.
///
/// Implemented by [`super::types::ExternalOcrPrivacyGuard`]. Both implementor
/// and the only consumer ([`GuardedConversationSession`]) live in src-tauri, so
/// per ADR-001 §7 this port stays here rather than in maekon-core.
#[async_trait]
pub(crate) trait ConversationContentGuard: Send + Sync {
    /// Sanitize an outbound message before external transmission.
    ///
    /// Returns `Err` (fail-closed) if safe transmission cannot be ensured —
    /// the caller MUST NOT transmit the original content in that case.
    async fn sanitize_outbound(
        &self,
        message: &SessionMessage,
    ) -> Result<SessionMessage, CoreError>;
}

/// Decorator that sanitizes user content before an external session transmits
/// it. No-op passthrough for local (non-external) sessions.
pub(crate) struct GuardedConversationSession {
    inner: Arc<dyn ConversationSession>,
    guard: Arc<dyn ConversationContentGuard>,
}

impl GuardedConversationSession {
    pub(crate) fn new(
        inner: Arc<dyn ConversationSession>,
        guard: Arc<dyn ConversationContentGuard>,
    ) -> Self {
        Self { inner, guard }
    }
}

#[async_trait]
impl ConversationSession for GuardedConversationSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        // Local/in-process sessions (e.g. Ollama) keep data on-device — no guard.
        if !self.inner.is_external() {
            return self.inner.send_message(message).await;
        }
        // Fail-closed: a guard error blocks transmission; `?` returns before
        // the inner session is ever called with the raw content.
        let sanitized = self.guard.sanitize_outbound(message).await?;
        self.inner.send_message(&sanitized).await
    }

    fn info(&self) -> ConversationSessionInfo {
        self.inner.info()
    }

    fn session_id(&self) -> &str {
        self.inner.session_id()
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn is_external(&self) -> bool {
        self.inner.is_external()
    }

    /// Forward interrupt to the inner session (E21 #5017). Interrupt carries NO
    /// user content (just a turn-cancel), so no privacy guard is required — but
    /// it MUST be forwarded, else the privacy decorator silently swallows it
    /// (default `InvalidArguments`) and the feature is inert on the guarded path.
    async fn interrupt(&self) -> Result<(), CoreError> {
        self.inner.interrupt().await
    }

    /// Forward steer to the inner session, FAIL-CLOSED through the privacy guard
    /// (E21 #5017). Steer's `message` is USER content (the steering text) that
    /// would egress off-device, so for an external session it MUST traverse the
    /// same sanitize-before-transmit path as `send_message` — a guard error
    /// blocks the steer before the inner session is ever called. Local sessions
    /// keep data on-device and pass through unguarded.
    async fn steer(&self, message: &SessionMessage) -> Result<(), CoreError> {
        if !self.inner.is_external() {
            return self.inner.steer(message).await;
        }
        let sanitized = self.guard.sanitize_outbound(message).await?;
        self.inner.steer(&sanitized).await
    }

    async fn terminate(&self) {
        self.inner.terminate().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{ConversationContentGuard, GuardedConversationSession};

    use std::sync::Arc;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use chrono::Utc;
    use maekon_core::error::CoreError;
    use maekon_core::models::ai_session::{
        ConversationSessionInfo, MessageRole, SessionMessage, SessionState, SessionTransport,
    };
    use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};

    /// Inner session double: records the content it actually received and
    /// reports a configurable `is_external`.
    struct RecordingInner {
        external: bool,
        received: Mutex<Option<String>>,
        /// Content the inner session received via `steer` (E21 #5017), so a test
        /// can prove the guard sanitized BEFORE inner.steer (vs send_message).
        steered: Mutex<Option<String>>,
    }

    impl RecordingInner {
        fn new(external: bool) -> Self {
            Self {
                external,
                received: Mutex::new(None),
                steered: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl ConversationSession for RecordingInner {
        async fn send_message(
            &self,
            message: &SessionMessage,
        ) -> Result<ResponseStream, CoreError> {
            *self.received.lock().unwrap() = Some(message.content.clone());
            Ok(Box::pin(futures::stream::empty()))
        }
        fn info(&self) -> ConversationSessionInfo {
            ConversationSessionInfo {
                session_id: "inner".to_string(),
                provider_name: "inner".to_string(),
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
            "inner"
        }
        fn provider_name(&self) -> &str {
            "inner"
        }
        fn is_external(&self) -> bool {
            self.external
        }
        async fn steer(&self, message: &SessionMessage) -> Result<(), CoreError> {
            *self.steered.lock().unwrap() = Some(message.content.clone());
            Ok(())
        }
    }

    enum GuardMode {
        /// Replace content with a sentinel to prove sanitize ran.
        Sanitize,
        /// Fail-closed: refuse to sanitize.
        Fail,
    }

    struct RecordingGuard {
        mode: GuardMode,
        called: Mutex<bool>,
    }

    impl RecordingGuard {
        fn new(mode: GuardMode) -> Self {
            Self {
                mode,
                called: Mutex::new(false),
            }
        }
    }

    #[async_trait]
    impl ConversationContentGuard for RecordingGuard {
        async fn sanitize_outbound(
            &self,
            message: &SessionMessage,
        ) -> Result<SessionMessage, CoreError> {
            *self.called.lock().unwrap() = true;
            match self.mode {
                GuardMode::Sanitize => {
                    let mut sanitized = message.clone();
                    sanitized.content = "SANITIZED".to_string();
                    Ok(sanitized)
                }
                GuardMode::Fail => Err(CoreError::PolicyDenied {
                    code: maekon_core::error_codes::PolicyCode::Denied,
                    message: "guard fail-closed".to_string(),
                }),
            }
        }
    }

    fn msg(content: &str) -> SessionMessage {
        SessionMessage {
            role: MessageRole::User,
            content: content.to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        }
    }

    #[tokio::test]
    async fn external_session_sanitizes_before_inner() {
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        // send_message must succeed; the guard sanitizes before inner is called.
        let _stream = session
            .send_message(&msg("secret@example.com"))
            .await
            .expect("send_message through a sanitizing guard must return Ok");
        assert!(*guard.called.lock().unwrap(), "guard must be invoked");
        assert_eq!(
            inner.received.lock().unwrap().as_deref(),
            Some("SANITIZED"),
            "inner must receive sanitized content, not the raw prompt"
        );
    }

    #[tokio::test]
    async fn local_session_passes_through_without_guard() {
        let inner = Arc::new(RecordingInner::new(false));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        // Local sessions bypass the guard entirely; send_message must succeed.
        let _stream = session
            .send_message(&msg("local-data"))
            .await
            .expect("send_message on a local session must return Ok without invoking the guard");
        assert!(
            !*guard.called.lock().unwrap(),
            "guard must NOT run for local (non-external) sessions"
        );
        assert_eq!(
            inner.received.lock().unwrap().as_deref(),
            Some("local-data"),
            "local inner receives the original content untouched"
        );
    }

    #[tokio::test]
    async fn external_steer_sanitizes_before_inner() {
        // E21 #5017: steer's user content MUST traverse the guard before the
        // external inner session is steered (mirrors send_message). A mutation
        // that drops the sanitize/`?` would let the raw content reach inner.
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        // steer must succeed; the guard sanitizes the user content before inner.steer.
        session
            .steer(&msg("secret@example.com"))
            .await
            .expect("steer through a sanitizing guard must return Ok");
        assert!(
            *guard.called.lock().unwrap(),
            "guard must run for external steer"
        );
        assert_eq!(
            inner.steered.lock().unwrap().as_deref(),
            Some("SANITIZED"),
            "inner.steer must receive sanitized content, not the raw steering text"
        );
    }

    #[tokio::test]
    async fn guard_error_fails_closed_steer_inner_not_called() {
        // A guard failure blocks the steer before inner.steer is ever called.
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Fail));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        let result = session.steer(&msg("secret@example.com")).await;

        assert!(
            matches!(result.unwrap_err(), CoreError::PolicyDenied { .. }),
            "guard failure must block the steer with PolicyDenied"
        );
        assert!(
            inner.steered.lock().unwrap().is_none(),
            "fail-closed: inner.steer must NOT run when the guard refuses"
        );
    }

    #[tokio::test]
    async fn local_steer_passes_through_without_guard() {
        let inner = Arc::new(RecordingInner::new(false));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        // Local sessions bypass the guard for steer too; must succeed.
        session
            .steer(&msg("local-steer"))
            .await
            .expect("steer on a local session must return Ok without invoking the guard");
        assert!(
            !*guard.called.lock().unwrap(),
            "guard must NOT run for local (non-external) steer"
        );
        assert_eq!(
            inner.steered.lock().unwrap().as_deref(),
            Some("local-steer"),
            "local inner receives the original steering content untouched"
        );
    }

    #[tokio::test]
    async fn interrupt_forwards_to_inner() {
        // interrupt carries no user content → forwarded directly. The default
        // RecordingInner.interrupt is the trait default (InvalidArguments), so a
        // forwarded interrupt surfaces that — proving it reached inner (a
        // dropped override would surface the SAME default but never touch inner;
        // here the point is the guarded decorator does not short-circuit Ok).
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        let err = session.interrupt().await.expect_err("inner default Errs");
        assert!(matches!(err, CoreError::InvalidArguments { .. }));
        assert!(
            !*guard.called.lock().unwrap(),
            "interrupt carries no user content → no guard"
        );
    }

    #[tokio::test]
    async fn guard_error_fails_closed_inner_not_called() {
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Fail));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        let result = session.send_message(&msg("secret@example.com")).await;

        assert!(
            matches!(
                result.err().expect("guard must return Err on fail-closed"),
                CoreError::PolicyDenied { .. }
            ),
            "guard failure must block transmission with PolicyDenied"
        );
        assert!(
            inner.received.lock().unwrap().is_none(),
            "fail-closed: inner must NOT receive any content when the guard refuses"
        );
    }
}
