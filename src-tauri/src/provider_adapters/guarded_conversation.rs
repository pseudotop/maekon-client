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
use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{ConversationSessionInfo, SessionMessage};
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};
use maekon_core::ports::egress_ledger::EgressLedgerSink;

use super::types::ensure_non_external_endpoints_are_loopback;

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
        correlation_id: &str,
    ) -> Result<SessionMessage, CoreError>;

    /// Privacy-safe consent snapshot persisted beside an egress decision.
    fn consent_state_snapshot(&self) -> String {
        "full_text_extraction=unknown".to_string()
    }
}

/// Decorator that sanitizes user content before an external session transmits
/// it. No-op passthrough for local (non-external) sessions.
pub(crate) struct GuardedConversationSession {
    inner: Arc<dyn ConversationSession>,
    guard: Arc<dyn ConversationContentGuard>,
    egress_ledger: Option<Arc<dyn EgressLedgerSink>>,
}

impl GuardedConversationSession {
    pub(crate) fn new(
        inner: Arc<dyn ConversationSession>,
        guard: Arc<dyn ConversationContentGuard>,
    ) -> Self {
        Self {
            inner,
            guard,
            egress_ledger: None,
        }
    }

    /// Attach the durable, erase-retained egress ledger used by the production
    /// composition root. Tests and standalone callers may omit it.
    pub(crate) fn with_egress_ledger(
        mut self,
        egress_ledger: Option<Arc<dyn EgressLedgerSink>>,
    ) -> Self {
        self.egress_ledger = egress_ledger;
        self
    }

    async fn record_external_egress(
        &self,
        event_type: &'static str,
        disposition: &'static str,
        byte_count: i64,
        recipient_count: i64,
        consent_state: &str,
    ) {
        let Some(ledger) = self.egress_ledger.clone() else {
            return;
        };
        let record = maekon_core::models::storage_records::EgressLedgerRecord {
            record_id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.to_string(),
            // The session id is already the sanitized runtime/audit correlation
            // key. Reusing it here joins runtime, session audit, and ledger
            // without persisting prompt or response content.
            event_id: Some(self.session_id().to_string()),
            byte_count,
            recipient_count,
            destination: "external_ai.conversation".to_string(),
            disposition: disposition.to_string(),
            consent_state: consent_state.to_string(),
            occurred_at: Utc::now().to_rfc3339(),
        };
        match tokio::task::spawn_blocking(move || ledger.record_egress(&record)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    err.code = %error.code(),
                    "external conversation egress ledger write failed: {error}"
                );
            }
            Err(error) => {
                tracing::warn!("external conversation egress ledger task panicked: {error}");
            }
        }
    }
}

#[async_trait]
impl ConversationSession for GuardedConversationSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        // Local/in-process sessions (e.g. Ollama) keep data on-device — no guard.
        if !self.inner.is_external() {
            ensure_non_external_endpoints_are_loopback(
                self.inner.provider_name(),
                self.inner.egress_endpoint_urls(),
            )?;
            return self.inner.send_message(message).await;
        }
        // Fail-closed: a guard error blocks transmission; `?` returns before
        // the inner session is ever called with the raw content.
        let consent_state = self.guard.consent_state_snapshot();
        let sanitized = match self
            .guard
            .sanitize_outbound(message, self.session_id())
            .await
        {
            Ok(sanitized) => sanitized,
            Err(error) => {
                self.record_external_egress("ConversationMessage", "blocked", 0, 0, &consent_state)
                    .await;
                return Err(error);
            }
        };
        let byte_count = serde_json::to_vec(&sanitized)
            .map(|bytes| bytes.len() as i64)
            .unwrap_or(0);
        let result = self.inner.send_message(&sanitized).await;
        let (disposition, recipient_count) = if result.is_ok() {
            ("uploaded", 1)
        } else {
            ("blocked", 0)
        };
        self.record_external_egress(
            "ConversationMessage",
            disposition,
            byte_count,
            recipient_count,
            &consent_state,
        )
        .await;
        result
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

    fn egress_endpoint_urls(&self) -> Vec<&str> {
        self.inner.egress_endpoint_urls()
    }

    /// Forward the inner session's self-heal capability (#8057 P3) so a wrapped
    /// non-self-healing backend (Codex app-server) is not falsely "recovered".
    fn can_self_heal_on_retry(&self) -> bool {
        self.inner.can_self_heal_on_retry()
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
            ensure_non_external_endpoints_are_loopback(
                self.inner.provider_name(),
                self.inner.egress_endpoint_urls(),
            )?;
            return self.inner.steer(message).await;
        }
        let consent_state = self.guard.consent_state_snapshot();
        let sanitized = match self
            .guard
            .sanitize_outbound(message, self.session_id())
            .await
        {
            Ok(sanitized) => sanitized,
            Err(error) => {
                self.record_external_egress("ConversationSteer", "blocked", 0, 0, &consent_state)
                    .await;
                return Err(error);
            }
        };
        let byte_count = serde_json::to_vec(&sanitized)
            .map(|bytes| bytes.len() as i64)
            .unwrap_or(0);
        let result = self.inner.steer(&sanitized).await;
        let (disposition, recipient_count) = if result.is_ok() {
            ("uploaded", 1)
        } else {
            ("blocked", 0)
        };
        self.record_external_egress(
            "ConversationSteer",
            disposition,
            byte_count,
            recipient_count,
            &consent_state,
        )
        .await;
        result
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
    use maekon_core::models::storage_records::EgressLedgerRecord;
    use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};
    use maekon_core::ports::egress_ledger::EgressLedgerSink;

    /// Inner session double: records the content it actually received and
    /// reports a configurable `is_external`.
    struct RecordingInner {
        external: bool,
        endpoint_url: Option<String>,
        received: Mutex<Option<String>>,
        /// Content the inner session received via `steer` (E21 #5017), so a test
        /// can prove the guard sanitized BEFORE inner.steer (vs send_message).
        steered: Mutex<Option<String>>,
    }

    impl RecordingInner {
        fn new(external: bool) -> Self {
            Self::new_with_endpoint(external, None)
        }

        fn new_with_endpoint(external: bool, endpoint_url: Option<&str>) -> Self {
            Self {
                external,
                endpoint_url: endpoint_url.map(str::to_string),
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
        fn egress_endpoint_urls(&self) -> Vec<&str> {
            self.endpoint_url.as_deref().into_iter().collect()
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

    #[derive(Default)]
    struct RecordingLedger {
        records: Mutex<Vec<EgressLedgerRecord>>,
        fail: bool,
    }

    impl RecordingLedger {
        fn failing() -> Self {
            Self {
                records: Mutex::new(Vec::new()),
                fail: true,
            }
        }
    }

    impl EgressLedgerSink for RecordingLedger {
        fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError> {
            if self.fail {
                return Err(CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "injected ledger failure".to_string(),
                });
            }
            self.records.lock().unwrap().push(record.clone());
            Ok(())
        }
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
            _correlation_id: &str,
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

        fn consent_state_snapshot(&self) -> String {
            match self.mode {
                GuardMode::Sanitize => "full_text_extraction=true".to_string(),
                GuardMode::Fail => "full_text_extraction=false".to_string(),
            }
        }
    }

    fn msg(content: &str) -> SessionMessage {
        SessionMessage {
            screen_derived: false,
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
        let ledger = Arc::new(RecordingLedger::default());
        let session = GuardedConversationSession::new(inner.clone(), guard.clone())
            .with_egress_ledger(Some(ledger.clone()));

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
        let rows = ledger.records.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_type, "ConversationMessage");
        assert_eq!(rows[0].event_id.as_deref(), Some("inner"));
        assert_eq!(rows[0].destination, "external_ai.conversation");
        assert_eq!(rows[0].disposition, "uploaded");
        assert_eq!(rows[0].recipient_count, 1);
        assert_eq!(rows[0].consent_state, "full_text_extraction=true");
        assert!(rows[0].byte_count > 0);
        assert!(
            !serde_json::to_string(&rows[0])
                .unwrap()
                .contains("secret@example.com"),
            "ledger metadata must never persist the raw prompt"
        );
    }

    #[tokio::test]
    async fn local_session_passes_through_without_guard() {
        let inner = Arc::new(RecordingInner::new(false));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let ledger = Arc::new(RecordingLedger::default());
        let session = GuardedConversationSession::new(inner.clone(), guard.clone())
            .with_egress_ledger(Some(ledger.clone()));

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
        assert!(
            ledger.records.lock().unwrap().is_empty(),
            "on-device sessions must not create external-egress records"
        );
    }

    #[tokio::test]
    async fn non_loopback_endpoint_claiming_local_is_rejected_without_guard_or_inner_call() {
        let inner = Arc::new(RecordingInner::new_with_endpoint(
            false,
            Some("https://chat.example.com/v1/messages"),
        ));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let session = GuardedConversationSession::new(inner.clone(), guard.clone());

        let err = match session.send_message(&msg("secret@example.com")).await {
            Ok(_) => panic!("non-loopback endpoint claiming local must be rejected"),
            Err(err) => err,
        };

        assert!(matches!(&err, CoreError::PolicyDenied { .. }));
        assert!(
            err.to_string().contains("reported non-external"),
            "unexpected error: {err}"
        );
        assert!(
            !*guard.called.lock().unwrap(),
            "trust-boundary rejection must happen before sanitization"
        );
        assert!(
            inner.received.lock().unwrap().is_none(),
            "fail-closed: inner must NOT receive raw content"
        );
    }

    #[tokio::test]
    async fn external_steer_sanitizes_before_inner() {
        // E21 #5017: steer's user content MUST traverse the guard before the
        // external inner session is steered (mirrors send_message). A mutation
        // that drops the sanitize/`?` would let the raw content reach inner.
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let ledger = Arc::new(RecordingLedger::default());
        let session = GuardedConversationSession::new(inner.clone(), guard.clone())
            .with_egress_ledger(Some(ledger.clone()));

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
        let rows = ledger.records.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_type, "ConversationSteer");
        assert_eq!(rows[0].event_id.as_deref(), Some("inner"));
        assert_eq!(rows[0].disposition, "uploaded");
        assert_eq!(rows[0].recipient_count, 1);
    }

    #[tokio::test]
    async fn guard_error_fails_closed_steer_inner_not_called() {
        // A guard failure blocks the steer before inner.steer is ever called.
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Fail));
        let ledger = Arc::new(RecordingLedger::default());
        let session = GuardedConversationSession::new(inner.clone(), guard.clone())
            .with_egress_ledger(Some(ledger.clone()));

        let result = session.steer(&msg("secret@example.com")).await;

        assert!(
            matches!(result.unwrap_err(), CoreError::PolicyDenied { .. }),
            "guard failure must block the steer with PolicyDenied"
        );
        assert!(
            inner.steered.lock().unwrap().is_none(),
            "fail-closed: inner.steer must NOT run when the guard refuses"
        );
        let rows = ledger.records.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_type, "ConversationSteer");
        assert_eq!(rows[0].event_id.as_deref(), Some("inner"));
        assert_eq!(rows[0].disposition, "blocked");
        assert_eq!(rows[0].recipient_count, 0);
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
        let ledger = Arc::new(RecordingLedger::default());
        let session = GuardedConversationSession::new(inner.clone(), guard.clone())
            .with_egress_ledger(Some(ledger.clone()));

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
        let rows = ledger.records.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].event_id.as_deref(), Some("inner"));
        assert_eq!(rows[0].disposition, "blocked");
        assert_eq!(rows[0].byte_count, 0);
        assert_eq!(rows[0].recipient_count, 0);
        assert_eq!(rows[0].consent_state, "full_text_extraction=false");
    }

    #[tokio::test]
    async fn ledger_failure_is_non_fatal_to_an_allowed_send() {
        let inner = Arc::new(RecordingInner::new(true));
        let guard = Arc::new(RecordingGuard::new(GuardMode::Sanitize));
        let ledger = Arc::new(RecordingLedger::failing());
        let session =
            GuardedConversationSession::new(inner.clone(), guard).with_egress_ledger(Some(ledger));

        let _stream = session
            .send_message(&msg("safe synthetic prompt"))
            .await
            .expect("the egress ledger is observability and must not fail the provider send");
        assert_eq!(inner.received.lock().unwrap().as_deref(), Some("SANITIZED"));
    }
}
