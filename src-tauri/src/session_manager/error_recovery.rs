//! Transient error detection, failure reporting, and session recovery.

use std::sync::Arc;

use tracing::{info, warn};

use maekon_core::error::CoreError;
use maekon_core::models::ai_session::SessionState;
use maekon_core::ports::conversation_session::ConversationSession;

use super::SessionManagerImpl;

/// Classify whether an error is transient (eligible for automatic retry).
pub(super) fn is_transient_error(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: _
        } | CoreError::RequestTimeout { .. }
            | CoreError::RateLimit { .. }
            | CoreError::ServiceUnavailable { .. }
    )
}

impl SessionManagerImpl {
    /// Report an adapter-level failure to the manager.
    /// Auto-recovers transient errors if retries remain; marks permanent errors as Failed.
    /// Returns the resulting session state.
    pub async fn report_failure(&self, session_id: &str, error: &CoreError) -> SessionState {
        let mut sessions = self.sessions.write().await;
        let Some(managed) = sessions.get_mut(session_id) else {
            return SessionState::Terminated;
        };

        let previous = managed.state;

        if is_transient_error(error) && managed.retry_count < self.config.max_retries {
            managed.retry_count += 1;
            managed.state = SessionState::Active;
            info!(
                session_id,
                retry = managed.retry_count,
                error = %error,
                "auto-recovered transient session error"
            );
            self.emit_state_change(session_id, previous, SessionState::Active, "auto-recovery");
            SessionState::Active
        } else {
            managed.state = SessionState::Failed;
            warn!(
                session_id,
                error = %error,
                retries = managed.retry_count,
                "session marked failed"
            );
            self.emit_state_change(
                session_id,
                previous,
                SessionState::Failed,
                &error.to_string(),
            );
            SessionState::Failed
        }
    }

    /// Attempt to recover a session after a stream error.
    /// Increments retry_count and transitions state through Recovering → Active.
    /// Returns the session Arc for re-use, or an error if max retries exceeded.
    pub async fn recover_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let mut sessions = self.sessions.write().await;
        let managed = sessions
            .get_mut(session_id)
            .ok_or_else(|| CoreError::NotFound {
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "session".to_string(),
                id: session_id.to_string(),
            })?;

        if managed.retry_count >= self.config.max_retries {
            managed.state = SessionState::Failed;
            // Iter-97: retry exhaustion means the service is effectively
            // unavailable for this session's adapter. Wire code
            // `service.unavailable` lets telemetry distinguish "we gave up
            // after N retries" from "something broke inside maekon".
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "max retries exceeded".into(),
            });
        }

        // #8057 (P3, #4872): a backend that owns ONE long-lived process (the
        // Codex app-server) cannot be recovered by flipping state back to Active
        // — the held handle stays dead and the next send re-fails. Surface an
        // honest error and mark Failed rather than returning a false "recovered"
        // session. Re-creating the session (or #4872 thread resume) is the fix.
        if !managed.session.can_self_heal_on_retry() {
            managed.state = SessionState::Failed;
            warn!(
                session_id,
                "session backend is not recoverable in place (long-lived process); \
                 re-create the session (#4872)"
            );
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
                message: "session backend cannot be recovered in place; re-create the session"
                    .into(),
            });
        }

        managed.retry_count += 1;
        managed.state = SessionState::Recovering;
        info!(
            session_id,
            retry = managed.retry_count,
            "recovering session"
        );

        // NOTE: this self-heal only holds for per-turn re-spawn adapters (Claude
        // re-runs the CLI with --continue/resume on each turn, so a transient
        // process death is transparently re-established on the next message).
        // The codex app-server adapter holds ONE long-lived process; it has NO
        // per-call re-spawn, so a dead AppServerProcess Arc returned here is NOT
        // self-healing. Re-attaching its thread via CodexAppServerSession::resume
        // (thread/resume) is deferred to the SessionManager mock-harness (#4872),
        // which can persist the spawn Command/SessionConfig needed to rebuild it.
        managed.state = SessionState::Active;
        Ok(managed.session.clone())
    }
}
