//! Transient error detection, failure reporting, and session recovery.

use std::sync::Arc;

use tracing::{info, warn};

use oneshim_core::error::CoreError;
use oneshim_core::models::ai_session::SessionState;
use oneshim_core::ports::conversation_session::ConversationSession;

use super::SessionManagerImpl;

/// Classify whether an error is transient (eligible for automatic retry).
pub(super) fn is_transient_error(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Network {
            code: oneshim_core::error_codes::NetworkCode::Generic,
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
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "session".to_string(),
                id: session_id.to_string(),
            })?;

        if managed.retry_count >= self.config.max_retries {
            managed.state = SessionState::Failed;
            // Iter-97: retry exhaustion means the service is effectively
            // unavailable for this session's adapter. Wire code
            // `service.unavailable` lets telemetry distinguish "we gave up
            // after N retries" from "something broke inside oneshim".
            return Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: "max retries exceeded".into(),
            });
        }

        managed.retry_count += 1;
        managed.state = SessionState::Recovering;
        info!(
            session_id,
            retry = managed.retry_count,
            "recovering session"
        );

        // The adapter itself handles --continue/resume for session continuity
        managed.state = SessionState::Active;
        Ok(managed.session.clone())
    }
}
