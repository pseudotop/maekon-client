//! Audit decorator — wraps any ConversationSession with best-effort audit logging.
//! Phase 2: wired into SessionManagerImpl when adapters are created.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::Utc;

use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    ConversationSessionInfo, SessionAuditCategory, SessionAuditEntry, SessionMessage,
};
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};

pub struct AuditingSession {
    inner: Arc<dyn ConversationSession>,
    audit: Arc<dyn AuditLogPort>,
    /// #8050: optional shared LLM-connectivity flag. As the OUTERMOST decorator
    /// wrapping every chat provider (subprocess/http/local, guarded or not),
    /// this is the single provider-agnostic chokepoint that observes each send's
    /// outcome — so it records `true` on a successful send and `false` on a
    /// failed one, and the health-check loop surfaces that in the tray. `None`
    /// for sessions built without the flag (e.g. unit tests).
    llm_health_flag: Option<Arc<AtomicBool>>,
}

impl AuditingSession {
    pub fn new(inner: Arc<dyn ConversationSession>, audit: Arc<dyn AuditLogPort>) -> Self {
        Self {
            inner,
            audit,
            llm_health_flag: None,
        }
    }

    /// Attach the shared LLM-connectivity flag written on every send outcome
    /// (#8050). Wired from the session factory's `decorate_session` so it covers
    /// all providers through this one outermost decorator.
    pub(crate) fn with_llm_health_flag(mut self, flag: Option<Arc<AtomicBool>>) -> Self {
        self.llm_health_flag = flag;
        self
    }
}

#[async_trait]
impl ConversationSession for AuditingSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        let start = Instant::now();

        self.audit
            .record_session_event(SessionAuditEntry {
                timestamp: Utc::now(),
                session_id: self.session_id().to_string(),
                category: SessionAuditCategory::Message,
                event_type: "inbound".to_string(),
                provider: self.provider_name().to_string(),
                payload: serde_json::to_value(message.role).ok(),
                duration_ms: None,
            })
            .await;

        let result = self.inner.send_message(message).await;

        // #8050: record LLM connectivity from the send outcome. This is the
        // outermost decorator over every provider, so one write here covers all
        // transports. `true` on a started/successful send, `false` on a send
        // error; the flag returns to healthy on the next successful send.
        if let Some(ref flag) = self.llm_health_flag {
            flag.store(result.is_ok(), Ordering::Relaxed);
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        let event_type = if result.is_ok() {
            "outbound_started"
        } else {
            "outbound_error"
        };

        self.audit
            .record_session_event(SessionAuditEntry {
                timestamp: Utc::now(),
                session_id: self.session_id().to_string(),
                category: SessionAuditCategory::Message,
                event_type: event_type.to_string(),
                provider: self.provider_name().to_string(),
                payload: None,
                duration_ms: Some(duration_ms),
            })
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

    /// Forward the inner session's external/egress status (E21 #4869). Without
    /// this, the composed session falls back to the trait default `false`,
    /// misreporting an external (off-device) session as local to any caller
    /// that inspects the outermost decorator.
    fn is_external(&self) -> bool {
        self.inner.is_external()
    }

    /// Forward the inner session's self-heal capability (#8057 P3). Without this,
    /// a wrapped Codex app-server (which cannot self-heal) would report the trait
    /// default `true` to `recover_session` and be falsely "recovered".
    fn can_self_heal_on_retry(&self) -> bool {
        self.inner.can_self_heal_on_retry()
    }

    /// Forward interrupt to the inner session AND audit it (E21 #5017). Records a
    /// pre-event then an outcome event, then forwards the Result. MUST NOT
    /// silently drop — the decorator order is `Auditing(Guarded(inner))`, so a
    /// missing override here hits the default `InvalidArguments` and the
    /// interrupt never reaches the privacy/transport layers.
    async fn interrupt(&self) -> Result<(), CoreError> {
        self.record_event("interrupt", None).await;
        let result = self.inner.interrupt().await;
        let outcome = if result.is_ok() {
            "interrupt_ok"
        } else {
            "interrupt_error"
        };
        self.record_event(outcome, None).await;
        result
    }

    /// Forward steer to the inner session AND audit it (E21 #5017). The steering
    /// content is NOT logged (only the message role) — same payload discretion
    /// as `send_message` (the inner Guarded decorator owns content sanitization).
    async fn steer(&self, message: &SessionMessage) -> Result<(), CoreError> {
        self.record_event("steer", serde_json::to_value(message.role).ok())
            .await;
        let result = self.inner.steer(message).await;
        let outcome = if result.is_ok() {
            "steer_ok"
        } else {
            "steer_error"
        };
        self.record_event(outcome, None).await;
        result
    }

    async fn terminate(&self) {
        self.inner.terminate().await;
    }
}

impl AuditingSession {
    /// Record a single session audit entry for a turn-control event (E21 #5017),
    /// reusing the `Message` category. `payload` is an optional render-safe value
    /// (e.g. the message role) — never raw steering content.
    async fn record_event(&self, event_type: &str, payload: Option<serde_json::Value>) {
        self.audit
            .record_session_event(SessionAuditEntry {
                timestamp: Utc::now(),
                session_id: self.session_id().to_string(),
                category: SessionAuditCategory::Message,
                event_type: event_type.to_string(),
                provider: self.provider_name().to_string(),
                payload,
                duration_ms: None,
            })
            .await;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use maekon_core::models::ai_session::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct MockSession;

    #[async_trait]
    impl ConversationSession for MockSession {
        async fn send_message(&self, _: &SessionMessage) -> Result<ResponseStream, CoreError> {
            Ok(Box::pin(futures::stream::empty()))
        }
        fn info(&self) -> ConversationSessionInfo {
            ConversationSessionInfo {
                session_id: "test".to_string(),
                provider_name: "mock".to_string(),
                model: "test-model".to_string(),
                state: SessionState::Active,
                transport: SessionTransport::HttpApi,
                created_at: Utc::now(),
                last_active: Utc::now(),
                turn_count: 0,
                title: None,
            }
        }
        fn session_id(&self) -> &str {
            "test"
        }
        fn provider_name(&self) -> &str {
            "mock"
        }
        async fn interrupt(&self) -> Result<(), CoreError> {
            Ok(())
        }
        async fn steer(&self, _message: &SessionMessage) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[derive(Default)]
    pub(crate) struct MockAudit {
        call_count: AtomicU32,
    }

    #[async_trait]
    impl AuditLogPort for MockAudit {
        async fn pending_count(&self) -> usize {
            0
        }
        async fn recent_entries(&self, _: usize) -> Vec<maekon_core::models::audit::AuditEntry> {
            vec![]
        }
        async fn entries_by_status(
            &self,
            _: &maekon_core::models::audit::AuditStatus,
            _: usize,
        ) -> Vec<maekon_core::models::audit::AuditEntry> {
            vec![]
        }
        async fn entries_by_action_prefix(
            &self,
            _: &str,
            _: usize,
        ) -> Vec<maekon_core::models::audit::AuditEntry> {
            vec![]
        }
        async fn entries_by_command_id(
            &self,
            _cmd_id: &str,
            _limit: usize,
        ) -> Vec<maekon_core::models::audit::AuditEntry> {
            vec![]
        }
        async fn stats(&self) -> maekon_core::models::audit::AuditStats {
            Default::default()
        }
        async fn has_pending_batch(&self) -> bool {
            false
        }
        async fn log_event(&self, _: &str, _: &str, _: &str) {}
        async fn log_start_if(
            &self,
            _: maekon_core::models::audit::AuditLevel,
            _: &str,
            _: &str,
            _: &str,
        ) {
        }
        async fn log_complete_with_time(
            &self,
            _: maekon_core::models::audit::AuditLevel,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) {
        }
        async fn drain_batch(&self) -> Vec<maekon_core::models::audit::AuditEntry> {
            vec![]
        }
        async fn drain_all(&self) -> Vec<maekon_core::models::audit::AuditEntry> {
            vec![]
        }
        async fn record_session_event(&self, _entry: SessionAuditEntry) {
            self.call_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn audit_decorator_records_events() {
        let audit = Arc::new(MockAudit {
            call_count: AtomicU32::new(0),
        });
        let session = AuditingSession::new(Arc::new(MockSession), audit.clone());

        let msg = SessionMessage {
            screen_derived: false,
            role: MessageRole::User,
            content: "test".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };
        let _ = session.send_message(&msg).await;

        assert_eq!(audit.call_count.load(Ordering::Relaxed), 2); // inbound + outbound
    }

    #[tokio::test]
    async fn audit_decorator_forwards_and_records_interrupt() {
        // E21 #5017: interrupt forwards to inner (Ok) AND records pre+outcome
        // audit entries. A mutation dropping the override (default Err) would
        // both fail the Ok assertion and not record 2 entries.
        let audit = Arc::new(MockAudit {
            call_count: AtomicU32::new(0),
        });
        let session = AuditingSession::new(Arc::new(MockSession), audit.clone());
        session.interrupt().await.expect("forwarded to inner Ok");
        assert_eq!(
            audit.call_count.load(Ordering::Relaxed),
            2,
            "interrupt records a pre-event + an outcome event"
        );
    }

    #[tokio::test]
    async fn audit_decorator_forwards_and_records_steer() {
        let audit = Arc::new(MockAudit {
            call_count: AtomicU32::new(0),
        });
        let session = AuditingSession::new(Arc::new(MockSession), audit.clone());
        let msg = SessionMessage {
            screen_derived: false,
            role: MessageRole::User,
            content: "steer it".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };
        session.steer(&msg).await.expect("forwarded to inner Ok");
        assert_eq!(
            audit.call_count.load(Ordering::Relaxed),
            2,
            "steer records a pre-event + an outcome event"
        );
    }

    #[test]
    fn delegates_info_to_inner() {
        let audit = Arc::new(MockAudit {
            call_count: AtomicU32::new(0),
        });
        let session = AuditingSession::new(Arc::new(MockSession), audit);
        assert_eq!(session.session_id(), "test");
        assert_eq!(session.provider_name(), "mock");
    }

    /// Inner session that always fails `send_message` — used to prove the health
    /// flag flips unhealthy on a send error (#8050).
    struct FailingSession;

    #[async_trait]
    impl ConversationSession for FailingSession {
        async fn send_message(&self, _: &SessionMessage) -> Result<ResponseStream, CoreError> {
            Err(CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: "send: injected failure".to_string(),
            })
        }
        fn info(&self) -> ConversationSessionInfo {
            ConversationSessionInfo {
                session_id: "fail".to_string(),
                provider_name: "fail".to_string(),
                model: "m".to_string(),
                state: SessionState::Active,
                transport: SessionTransport::HttpApi,
                created_at: Utc::now(),
                last_active: Utc::now(),
                turn_count: 0,
                title: None,
            }
        }
        fn session_id(&self) -> &str {
            "fail"
        }
        fn provider_name(&self) -> &str {
            "fail"
        }
    }

    fn user_msg(content: &str) -> SessionMessage {
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
    async fn send_message_writes_llm_health_flag_on_success_and_failure() {
        // #8050: the AuditingSession is the outermost decorator over every
        // provider, so it is the single chokepoint that records LLM connectivity.
        // A successful send flips the shared flag healthy; a failed send flips it
        // unhealthy; the next success recovers it.
        let audit = Arc::new(MockAudit::default());
        // Start unhealthy so a successful send must actively flip it true.
        let flag = Arc::new(AtomicBool::new(false));

        let ok_session = AuditingSession::new(Arc::new(MockSession), audit.clone())
            .with_llm_health_flag(Some(flag.clone()));
        let _stream = ok_session
            .send_message(&user_msg("hi"))
            .await
            .expect("mock send is Ok");
        assert!(
            flag.load(Ordering::Relaxed),
            "a successful send must report the LLM connected"
        );

        let failing = AuditingSession::new(Arc::new(FailingSession), audit.clone())
            .with_llm_health_flag(Some(flag.clone()));
        let _ = failing.send_message(&user_msg("hi")).await;
        assert!(
            !flag.load(Ordering::Relaxed),
            "a failed send must report the LLM disconnected"
        );

        // Recovery through a healthy provider flips it back.
        let _stream = ok_session
            .send_message(&user_msg("hi again"))
            .await
            .expect("mock send is Ok");
        assert!(
            flag.load(Ordering::Relaxed),
            "a recovered send must report the LLM connected again"
        );
    }

    #[tokio::test]
    async fn send_message_without_health_flag_is_a_noop() {
        // No flag wired (unit/standalone use) → the decorator must not panic and
        // simply records audit events as before.
        let audit = Arc::new(MockAudit::default());
        let session = AuditingSession::new(Arc::new(MockSession), audit.clone());
        let _stream = session
            .send_message(&user_msg("hi"))
            .await
            .expect("send is Ok even without a health flag");
    }
}
