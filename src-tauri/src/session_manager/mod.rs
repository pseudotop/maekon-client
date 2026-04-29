//! Session manager implementation — creates, manages, and reaps AI conversation sessions.

mod error_recovery;
mod factory;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use oneshim_core::config::AiSessionConfig;
use oneshim_core::error::CoreError;
use oneshim_core::models::ai_session::{ConversationSessionInfo, SessionConfig, SessionState};
use oneshim_core::ports::audit_log::AuditLogPort;
use oneshim_core::ports::conversation_session::{ConversationSession, SessionManager};
use oneshim_core::ports::secret_store::SecretStore;

use crate::session_context::SessionContextAssembler;

struct ManagedSession {
    session: Arc<dyn ConversationSession>,
    state: SessionState,
    created_at: Instant,
    last_active: Instant,
    retry_count: u32,
    total_input_tokens: u64,
    total_output_tokens: u64,
}

/// Tauri event payload emitted on session state transitions.
#[derive(Debug, Clone, Serialize)]
pub struct SessionStateEvent {
    pub session_id: String,
    pub previous_state: SessionState,
    pub new_state: SessionState,
    pub reason: String,
}

pub struct SessionManagerImpl {
    sessions: RwLock<HashMap<String, ManagedSession>>,
    pub(crate) config: Arc<AiSessionConfig>,
    audit: Arc<dyn AuditLogPort>,
    context_assembler: Option<Arc<SessionContextAssembler>>,
    /// Secret store for resolving provider credentials (HttpApi sessions).
    secret_store: Option<Arc<dyn SecretStore>>,
    /// Tauri app handle for emitting session state change events.
    app_handle: Option<AppHandle>,
}

impl SessionManagerImpl {
    pub fn new(
        config: Arc<AiSessionConfig>,
        audit: Arc<dyn AuditLogPort>,
        context_assembler: Option<Arc<SessionContextAssembler>>,
    ) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            config,
            audit,
            context_assembler,
            secret_store: None,
            app_handle: None,
        }
    }

    /// Attach a secret store for resolving provider credentials.
    pub fn with_secret_store(mut self, store: Arc<dyn SecretStore>) -> Self {
        self.secret_store = Some(store);
        self
    }

    /// Attach a Tauri app handle for emitting state transition events.
    pub fn with_app_handle(mut self, handle: AppHandle) -> Self {
        self.app_handle = Some(handle);
        self
    }

    fn emit_state_change(
        &self,
        session_id: &str,
        previous: SessionState,
        new: SessionState,
        reason: &str,
    ) {
        if let Some(ref handle) = self.app_handle {
            let event = SessionStateEvent {
                session_id: session_id.to_string(),
                previous_state: previous,
                new_state: new,
                reason: reason.to_string(),
            };
            if let Err(e) = handle.emit("session-state-changed", &event) {
                debug!("emit session-state-changed failed: {e}");
            }
        }
    }

    /// Terminate all sessions (called during app shutdown).
    pub async fn shutdown_all(&self) {
        let session_ids: Vec<String> = {
            let sessions = self.sessions.read().await;
            sessions.keys().cloned().collect()
        };

        for id in session_ids {
            if let Err(err) = self.kill_session(&id).await {
                warn!(session_id = %id, "failed to terminate session during shutdown: {err}");
            }
        }

        info!("all AI sessions terminated");
    }

    /// Touch a session to reset its idle timer and mark it as Active.
    /// Called on every send_message to keep the session alive.
    pub async fn touch_session(&self, session_id: &str) {
        if let Some(managed) = self.sessions.write().await.get_mut(session_id) {
            let previous = managed.state;
            managed.last_active = Instant::now();
            managed.state = SessionState::Active;
            if previous != SessionState::Active {
                self.emit_state_change(session_id, previous, SessionState::Active, "user activity");
            }
        }
    }

    /// Accumulate token usage for a session from a completed response.
    pub async fn accumulate_tokens(&self, session_id: &str, input: u64, output: u64) {
        if let Some(managed) = self.sessions.write().await.get_mut(session_id) {
            managed.total_input_tokens += input;
            managed.total_output_tokens += output;
        }
    }

    /// Check if the daily token budget is exhausted. Returns true if sending is allowed.
    pub async fn check_token_budget(&self, _session_id: &str) -> bool {
        let budget = self.config.daily_token_budget;
        if budget == 0 {
            return true; // unlimited
        }
        let sessions = self.sessions.read().await;
        // Sum tokens across ALL sessions (daily budget is global)
        let total: u64 = sessions
            .values()
            .map(|m| m.total_input_tokens + m.total_output_tokens)
            .sum();
        total < budget
    }

    /// Get total token usage across all sessions (for daily budget display).
    pub async fn get_global_token_usage(&self) -> (u64, u64) {
        let sessions = self.sessions.read().await;
        sessions.values().fold((0, 0), |(ai, ao), m| {
            (ai + m.total_input_tokens, ao + m.total_output_tokens)
        })
    }

    /// Background task: check for idle sessions and terminate them.
    /// Two-phase idle: Active→Idle (warning) on first timeout, Idle→Terminated on second.
    pub async fn reap_idle_sessions(&self) {
        let idle_timeout = std::time::Duration::from_secs(self.config.idle_timeout_secs);
        let session_timeout = std::time::Duration::from_secs(self.config.session_timeout_secs);
        let mut to_reap: Vec<(String, &'static str)> = vec![];

        {
            let mut sessions = self.sessions.write().await;
            for (id, managed) in sessions.iter_mut() {
                // Absolute session lifetime — reap regardless of activity.
                if managed.created_at.elapsed() > session_timeout {
                    to_reap.push((id.clone(), "absolute session timeout"));
                    continue;
                }

                if managed.last_active.elapsed() > idle_timeout {
                    if managed.state == SessionState::Active {
                        // First pass: mark Active → Idle (grace period)
                        let previous = managed.state;
                        managed.state = SessionState::Idle;
                        warn!(session_id = %id, "session marked idle");
                        self.emit_state_change(id, previous, SessionState::Idle, "idle timeout");
                    } else if managed.state == SessionState::Idle {
                        // Second pass: Idle past timeout → collect for reaping
                        to_reap.push((id.clone(), "idle timeout (second phase)"));
                    }
                }
            }
        }

        for (id, reason) in to_reap {
            info!(session_id = %id, reason, "reaping session");
            if let Err(e) = self.kill_session_with_reason(&id, reason).await {
                debug!("kill_session_with_reason failed: {e}");
            }
        }
    }

    /// Internal kill that captures previous state for event emission.
    async fn kill_session_with_reason(
        &self,
        session_id: &str,
        reason: &str,
    ) -> Result<(), CoreError> {
        let removed = self.sessions.write().await.remove(session_id);
        match removed {
            Some(managed) => {
                managed.session.terminate().await;
                info!(session_id = %session_id, "session terminated");
                self.emit_state_change(session_id, managed.state, SessionState::Terminated, reason);
                Ok(())
            }
            None => Err(CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "session".to_string(),
                id: session_id.to_string(),
            }),
        }
    }

    /// Atomically check admission and insert a session under a single write lock.
    /// Prevents TOCTOU race where concurrent create_session calls both pass the count check.
    async fn admit_session(
        &self,
        session_id: String,
        managed: ManagedSession,
    ) -> Result<(), CoreError> {
        let mut sessions = self.sessions.write().await;
        if sessions.len() >= self.config.max_concurrent_sessions as usize {
            // Iter-97: capacity-limit hit is a service-availability condition
            // (transient; client can retry after an existing session ends).
            // Wire code `service.unavailable` distinguishes this from true
            // internal failures; frontend can show "try again soon" rather
            // than "something broke inside oneshim".
            return Err(CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: format!(
                    "max concurrent sessions ({}) reached",
                    self.config.max_concurrent_sessions,
                ),
            });
        }
        sessions.insert(session_id, managed);
        Ok(())
    }
}

#[async_trait]
impl SessionManager for SessionManagerImpl {
    async fn create_session(
        &self,
        config: SessionConfig,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        self.create_session_impl(config).await
    }

    async fn kill_session(&self, session_id: &str) -> Result<(), CoreError> {
        self.kill_session_with_reason(session_id, "user terminated")
            .await
    }

    async fn list_sessions(&self) -> Vec<ConversationSessionInfo> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .map(|managed| {
                let mut info = managed.session.info();
                // Override adapter's always-Active state with manager's authoritative state
                info.state = managed.state;
                info
            })
            .collect()
    }

    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        let sessions = self.sessions.read().await;
        sessions
            .get(session_id)
            .map(|m| m.session.clone())
            .ok_or_else(|| CoreError::NotFound {
                code: oneshim_core::error_codes::NotFoundCode::ResourceMissing,
                resource_type: "session".to_string(),
                id: session_id.to_string(),
            })
    }

    async fn recover_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<dyn ConversationSession>, CoreError> {
        SessionManagerImpl::recover_session(self, session_id).await
    }

    async fn touch_session(&self, session_id: &str) {
        SessionManagerImpl::touch_session(self, session_id).await;
    }

    async fn report_failure(&self, session_id: &str, error: &CoreError) -> SessionState {
        SessionManagerImpl::report_failure(self, session_id, error).await
    }

    async fn shutdown_all(&self) {
        SessionManagerImpl::shutdown_all(self).await;
    }
}

#[cfg(test)]
mod tests;
