//! Audit decorator — wraps any ConversationSession with best-effort audit logging.
//! Phase 2: wired into SessionManagerImpl when adapters are created.

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
}

impl AuditingSession {
    pub fn new(inner: Arc<dyn ConversationSession>, audit: Arc<dyn AuditLogPort>) -> Self {
        Self { inner, audit }
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
}
