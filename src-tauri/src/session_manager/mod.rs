//! Session manager implementation — creates, manages, and reaps AI conversation sessions.

mod error_recovery;
pub(crate) mod factory;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use maekon_core::config::AiSessionConfig;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{ConversationSessionInfo, SessionConfig, SessionState};
use maekon_core::ports::audit_log::AuditLogPort;
use maekon_core::ports::conversation_session::{ConversationSession, SessionManager};
use maekon_core::ports::secret_store::SecretStore;

use crate::provider_adapters::ConversationContentGuard;
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

/// #6266: reaper-durable daily token ledger. The previous `check_token_budget`
/// summed tokens only across CURRENTLY-LIVE sessions, so reaping an idle session
/// discarded its usage and the "daily" budget silently reset (and never rolled
/// over by date). This counter is incremented in `accumulate_tokens` and survives
/// session removal; it resets when the (UTC) calendar date changes.
struct DailyTokenLedger {
    date: chrono::NaiveDate,
    input_tokens: u64,
    output_tokens: u64,
}

impl DailyTokenLedger {
    /// Roll the ledger over to `today`, zeroing the counters on a new day.
    fn roll_to(&mut self, today: chrono::NaiveDate) {
        if self.date != today {
            self.date = today;
            self.input_tokens = 0;
            self.output_tokens = 0;
        }
    }
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
    /// Privacy guard for external (off-device) chat sessions. When set, external
    /// sessions are wrapped so user content is sanitized before transmission
    /// (E21 #4882/#4883 — closes the chat-path PII gap, review B1).
    privacy_guard: Option<Arc<dyn ConversationContentGuard>>,
    /// Durable transparency ledger for external conversation attempts (#9077).
    /// The privacy decorator records both blocked and accepted sends without
    /// persisting user content.
    egress_ledger: Option<Arc<dyn maekon_core::ports::egress_ledger::EgressLedgerSink>>,
    /// Rollout stage for the Codex `app-server` transport (E21 #4871). `Off`
    /// (default) keeps the `codex exec` path; opt-in/default attempt app-server
    /// with graceful fallback to exec on failure.
    codex_app_server_rollout: maekon_core::config::CodexAppServerRollout,
    /// FAIL-CLOSED approval decider for Codex app-server `requestApproval`
    /// REQUESTs (E21 #4870). When set, app-server sessions wire the approval loop
    /// (policy auto-decision + audit + DefaultDenyUiHook). `None` → inbound
    /// approval requests are drained-and-dropped (still fail-closed: the server
    /// proceeds without the gated action).
    codex_approval_decider: Option<Arc<maekon_core::codex_approval::CodexApprovalDecider>>,
    /// D7 (#4812 / E20-20): the single shared workspace-wide circuit-breaker
    /// registry from the composition root, used when creating `HttpApiSession`s.
    /// Defaults to a fresh registry for standalone use (tests); the composition
    /// root overrides it with the shared Arc via `with_breaker_registry`.
    pub(crate) breaker_registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    /// Resolved Ollama base URL + default model from `AppConfig` (C2 #5722).
    /// Pre-computed at composition time by `session_wiring.rs` via
    /// `with_local_llm_target`; `None` → `create_local_llm_session` falls back
    /// to the catalog-derived loopback default.
    pub(crate) local_llm_target: Option<crate::session_manager::factory::LocalLlmTarget>,
    /// #6266: date-keyed cumulative token counter that survives session reaping
    /// (the live-session sum used previously reset every time an idle session was
    /// reaped). Used by `check_token_budget` / `get_global_token_usage`.
    daily_token_ledger: parking_lot::Mutex<DailyTokenLedger>,
    /// #8050: shared LLM-connectivity flag threaded into the `AuditingSession`
    /// decorator of every session this manager creates, so a send success/failure
    /// on ANY provider updates the tray's "Local LLM" connection status. `None`
    /// for standalone/test use; the composition root injects the shared Arc.
    llm_health_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
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
            privacy_guard: None,
            egress_ledger: None,
            codex_app_server_rollout: maekon_core::config::CodexAppServerRollout::default(),
            codex_approval_decider: None,
            breaker_registry: crate::breaker_registry::CircuitBreakerRegistry::new(),
            local_llm_target: None,
            daily_token_ledger: parking_lot::Mutex::new(DailyTokenLedger {
                date: chrono::Utc::now().date_naive(),
                input_tokens: 0,
                output_tokens: 0,
            }),
            llm_health_flag: None,
        }
    }

    /// Inject the shared LLM-connectivity flag (#8050). Every session this
    /// manager creates threads it into its `AuditingSession` decorator, so a
    /// send success/failure on any provider drives the tray's "Local LLM"
    /// connection status.
    pub(crate) fn with_llm_health_flag(mut self, flag: Arc<std::sync::atomic::AtomicBool>) -> Self {
        self.llm_health_flag = Some(flag);
        self
    }

    /// Inject the single shared workspace-wide circuit-breaker registry from the
    /// composition root (D7 #4812 / E20-20). `HttpApiSession`s created by this
    /// manager clone this one Arc, so they converge on the same breaker as the
    /// other network adapters targeting the same endpoint.
    pub fn with_breaker_registry(
        mut self,
        registry: Arc<crate::breaker_registry::CircuitBreakerRegistry>,
    ) -> Self {
        self.breaker_registry = registry;
        self
    }

    /// Wire the pre-resolved Ollama base URL + default model (C2 #5722).
    /// Called from the composition root (`session_wiring.rs`) with the result
    /// of `resolve_local_llm_target(&config.ai_provider)` so that
    /// `create_local_llm_session` can honour wizard-written custom Ollama
    /// endpoints without needing access to the full `AppConfig`.
    // #7734: narrowed from `pub` — `LocalLlmTarget` itself is `pub(crate)`
    // and the sole call site is internal to this crate (private_interfaces
    // lint fallout from the `[lib]` target enabler; behavior-neutral).
    pub(crate) fn with_local_llm_target(
        mut self,
        target: crate::session_manager::factory::LocalLlmTarget,
    ) -> Self {
        self.local_llm_target = Some(target);
        self
    }

    /// Attach the FAIL-CLOSED Codex approval decider (E21 #4870). Wired from the
    /// session bootstrap with the shared `PolicyClient`-backed policy port + the
    /// shared audit sink + a `DefaultDenyUiHook` (FU-A swaps in the Tauri hook).
    pub fn with_codex_approval_decider(
        mut self,
        decider: Arc<maekon_core::codex_approval::CodexApprovalDecider>,
    ) -> Self {
        self.codex_approval_decider = Some(decider);
        self
    }

    /// Attach the privacy guard applied to external chat sessions.
    // #7734: narrowed from `pub` — `ConversationContentGuard` itself is
    // `pub(crate)` and every call site (production + in-crate unit tests) is
    // internal to this crate (private_interfaces lint fallout from the
    // `[lib]` target enabler; behavior-neutral).
    pub(crate) fn with_privacy_guard(mut self, guard: Arc<dyn ConversationContentGuard>) -> Self {
        self.privacy_guard = Some(guard);
        self
    }

    pub(crate) fn with_egress_ledger(
        mut self,
        ledger: Arc<dyn maekon_core::ports::egress_ledger::EgressLedgerSink>,
    ) -> Self {
        self.egress_ledger = Some(ledger);
        self
    }

    /// Set the Codex `app-server` transport rollout stage (E21 #4871).
    pub fn with_codex_app_server_rollout(
        mut self,
        rollout: maekon_core::config::CodexAppServerRollout,
    ) -> Self {
        self.codex_app_server_rollout = rollout;
        self
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

    /// Record a completed successful turn and reset transient retry budget.
    pub async fn record_success(&self, session_id: &str) {
        if let Some(managed) = self.sessions.write().await.get_mut(session_id) {
            let previous = managed.state;
            managed.last_active = Instant::now();
            managed.retry_count = 0;
            managed.state = SessionState::Active;
            if previous != SessionState::Active {
                self.emit_state_change(session_id, previous, SessionState::Active, "turn success");
            }
        }
    }

    /// Accumulate token usage for a session from a completed response.
    pub async fn accumulate_tokens(&self, session_id: &str, input: u64, output: u64) {
        if let Some(managed) = self.sessions.write().await.get_mut(session_id) {
            managed.total_input_tokens += input;
            managed.total_output_tokens += output;
        }
        // #6266: also fold into the reaper-durable daily ledger so the budget is
        // not discarded when this session is later reaped. Roll over on date change.
        let mut ledger = self.daily_token_ledger.lock();
        ledger.roll_to(chrono::Utc::now().date_naive());
        ledger.input_tokens = ledger.input_tokens.saturating_add(input);
        ledger.output_tokens = ledger.output_tokens.saturating_add(output);
    }

    /// Check if the daily token budget is exhausted. Returns true if sending is allowed.
    pub async fn check_token_budget(&self, _session_id: &str) -> bool {
        let budget = self.config.daily_token_budget;
        if budget == 0 {
            return true; // unlimited
        }
        // #6266: use the date-keyed durable ledger (reaped sessions' tokens still
        // count toward today's budget) instead of summing only live sessions.
        let mut ledger = self.daily_token_ledger.lock();
        ledger.roll_to(chrono::Utc::now().date_naive());
        let total = ledger.input_tokens.saturating_add(ledger.output_tokens);
        total < budget
    }

    /// Get total token usage for the current day (for daily budget display).
    pub async fn get_global_token_usage(&self) -> (u64, u64) {
        // #6266: report the durable daily ledger, not the live-session sum.
        let mut ledger = self.daily_token_ledger.lock();
        ledger.roll_to(chrono::Utc::now().date_naive());
        (ledger.input_tokens, ledger.output_tokens)
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
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
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
            // than "something broke inside maekon".
            return Err(CoreError::ServiceUnavailable {
                code: maekon_core::error_codes::ServiceCode::Unavailable,
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
                code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
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

    async fn record_success(&self, session_id: &str) {
        SessionManagerImpl::record_success(self, session_id).await;
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
