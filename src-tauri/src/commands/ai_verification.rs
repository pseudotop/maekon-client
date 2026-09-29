//! User-started verification of the API-key (HTTP) Chat path (#12530).
//!
//! Readiness cannot tell whether a provider accepts the configured key and
//! model without calling it, so `chat.http_api` stays unverified until the
//! user asks for this call. It sends one fixed synthetic message through an
//! internal session, which carries the privacy guard, egress ledger, and audit
//! decorators Chat sessions carry, after the same daily token budget check. A
//! completed reply records the evidence that readiness compares against.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use serde::Serialize;
use tauri::command;

use maekon_core::config::AiProviderConfig;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    MessageRole, OutboundMessage, SessionConfig, SessionMessage, SessionTransport,
};
use maekon_core::ports::consent_manager::ConsentGate;
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};
use maekon_core::ports::credential_source::CredentialSource;

use crate::ai_invocation_evidence::{self, session_factory_credential, HttpChatTarget};
use crate::ai_readiness::provider_selection_apply_pending;
use crate::ipc_error::IpcError;
use crate::runtime_state::{AiSessionRuntimeState, AppState, ConfigRuntimeState};
use crate::session_manager::SessionManagerImpl;

/// Fixed, so the session context assembler adds no activity summary.
const VERIFICATION_SYSTEM_PROMPT: &str =
    "You are checking that a connection works. Follow the user message exactly.";
/// The only content this command sends: no user text, screen data, or context.
const VERIFICATION_MESSAGE: &str = "Reply with the single word OK.";
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatHttpVerificationStatus {
    Verified,
    NotConfigured,
    ConsentRequired,
    /// A saved provider change is not applied yet. Sessions still use the
    /// provider and key this process started with, so a call now would verify
    /// something other than the saved configuration.
    RestartRequired,
    BudgetExhausted,
    ProviderError,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ChatHttpVerificationResult {
    pub status: ChatHttpVerificationStatus,
}

/// Verifies the configured HTTP Chat provider with one bounded call and, on a
/// completed reply, records the evidence `chat.http_api` readiness reads.
#[command]
pub async fn verify_chat_http_provider(
    app_state: tauri::State<'_, AppState>,
    ai_state: tauri::State<'_, AiSessionRuntimeState>,
    config_state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<ChatHttpVerificationResult, IpcError> {
    let manager = ai_state
        .manager_impl()
        .ok_or_else(|| IpcError::new("service.unavailable", "session manager not available"))?;
    let evidence_path = ai_invocation_evidence::evidence_path()
        .ok_or_else(|| IpcError::new("internal.generic", "app data directory not available"))?;
    let full_text_granted =
        ConsentGate::from_ref(app_state.capture.consent_manager.as_ref()).may_extract_full_text();
    let config = config_state.config_manager().get();
    let restart_pending =
        provider_selection_apply_pending(&config.ai_provider, &app_state.config.ai_provider);

    let status = match plan_verification(
        &config.ai_provider,
        full_text_granted,
        restart_pending,
        |surface_id, url| session_factory_credential(Some(&*manager), surface_id, url),
    )
    .await
    {
        Ok(plan) => run_verification(manager, plan, &evidence_path, REPLY_TIMEOUT)
            .await
            .map_err(|error| {
                tracing::warn!(kind = ?error.kind(), "failed to record chat verification evidence");
                IpcError::new(
                    "internal.generic",
                    "could not record the verification result",
                )
            })?,
        Err(status) => status,
    };
    tracing::info!(?status, "chat verification finished");
    Ok(ChatHttpVerificationResult { status })
}

/// Session request and the fingerprint of what it will use.
pub(crate) struct VerificationPlan {
    session_config: SessionConfig,
    fingerprint: String,
}

/// Checks the preconditions and resolves the session target. Consent comes
/// first, as it does in `chat.http_api` readiness. A pending restart comes
/// next: until it applies the saved provider, `session_credential` (the
/// session factory) answers for the boot-time one. A target whose credential
/// cannot be read is not configured: a call made without a fingerprint could
/// not produce evidence, so none is made.
pub(crate) async fn plan_verification(
    ai_provider: &AiProviderConfig,
    full_text_granted: bool,
    restart_pending: bool,
    session_credential: impl FnOnce(&str, &str) -> Option<CredentialSource>,
) -> Result<VerificationPlan, ChatHttpVerificationStatus> {
    if !full_text_granted {
        return Err(ChatHttpVerificationStatus::ConsentRequired);
    }
    if restart_pending {
        return Err(ChatHttpVerificationStatus::RestartRequired);
    }
    let target =
        HttpChatTarget::resolve(ai_provider).ok_or(ChatHttpVerificationStatus::NotConfigured)?;
    let credential = session_credential(&target.surface_id, &target.url)
        .ok_or(ChatHttpVerificationStatus::NotConfigured)?;
    let fingerprint = target
        .fingerprint(&credential)
        .await
        .ok_or(ChatHttpVerificationStatus::NotConfigured)?;
    Ok(VerificationPlan {
        session_config: SessionConfig {
            transport: SessionTransport::HttpApi,
            surface_id: Some(target.surface_id),
            // Explicit, so the session uses exactly the fingerprinted model.
            model: Some(target.model),
            system_prompt: Some(VERIFICATION_SYSTEM_PROMPT.to_owned()),
            tools_enabled: false,
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        },
        fingerprint,
    })
}

/// Session-manager operations a verification needs. A seam, so the result
/// classification and the session cleanup are testable without a provider.
#[async_trait]
pub(crate) trait VerificationSessionHost: Send + Sync {
    async fn open(&self, config: SessionConfig) -> Result<Arc<dyn ConversationSession>, CoreError>;
    async fn token_budget_allows(&self, session_id: &str) -> bool;
    async fn record_usage(&self, session_id: &str, input_tokens: u64, output_tokens: u64);
    async fn close(&self, session_id: &str);
}

#[async_trait]
impl VerificationSessionHost for SessionManagerImpl {
    async fn open(&self, config: SessionConfig) -> Result<Arc<dyn ConversationSession>, CoreError> {
        self.create_internal_session(config).await
    }

    async fn token_budget_allows(&self, session_id: &str) -> bool {
        self.check_token_budget(session_id).await
    }

    async fn record_usage(&self, session_id: &str, input_tokens: u64, output_tokens: u64) {
        self.accumulate_tokens(session_id, input_tokens, output_tokens)
            .await;
    }

    async fn close(&self, session_id: &str) {
        if let Err(error) = self
            .kill_session_with_reason(session_id, "chat verification finished")
            .await
        {
            tracing::warn!(%error, "failed to close the chat verification session");
        }
    }
}

/// Closes the verification session on every exit. The normal path awaits
/// `close`; if the command future is dropped mid-call, `Drop` spawns the
/// close instead, as `RecoverySessionGuard` does.
struct SessionCloser {
    host: Arc<dyn VerificationSessionHost>,
    session_id: Option<String>,
}

impl SessionCloser {
    async fn close(mut self) {
        if let Some(session_id) = self.session_id.as_deref() {
            self.host.close(session_id).await;
        }
        self.session_id = None;
    }
}

impl Drop for SessionCloser {
    fn drop(&mut self) {
        if let Some(session_id) = self.session_id.take() {
            let host = Arc::clone(&self.host);
            tauri::async_runtime::spawn(async move { host.close(&session_id).await });
        }
    }
}

/// Opens the session, checks the budget, sends the fixed message, and drains
/// the reply. The session is closed before this returns. Evidence is written
/// only for a completed reply with text; every other outcome leaves the
/// existing record as it was. `Err` means the record could not be written.
pub(crate) async fn run_verification(
    host: Arc<dyn VerificationSessionHost>,
    plan: VerificationPlan,
    evidence_path: &Path,
    reply_timeout: Duration,
) -> std::io::Result<ChatHttpVerificationStatus> {
    let session = match host.open(plan.session_config).await {
        Ok(session) => session,
        Err(error) => {
            tracing::warn!(
                code = error.code(),
                "chat verification could not open a session"
            );
            return Ok(ChatHttpVerificationStatus::ProviderError);
        }
    };
    let closer = SessionCloser {
        host: Arc::clone(&host),
        session_id: Some(session.session_id().to_owned()),
    };
    let status = exchange(host.as_ref(), session.as_ref(), reply_timeout).await;
    closer.close().await;

    if status == ChatHttpVerificationStatus::Verified {
        let path = evidence_path.to_path_buf();
        let fingerprint = plan.fingerprint;
        tokio::task::spawn_blocking(move || {
            ai_invocation_evidence::write_evidence(&path, &fingerprint, chrono::Utc::now())
        })
        .await
        .map_err(std::io::Error::other)??;
    }
    Ok(status)
}

async fn exchange(
    host: &dyn VerificationSessionHost,
    session: &dyn ConversationSession,
    reply_timeout: Duration,
) -> ChatHttpVerificationStatus {
    let session_id = session.session_id();
    if !host.token_budget_allows(session_id).await {
        return ChatHttpVerificationStatus::BudgetExhausted;
    }
    let stream = match session.send_message(&verification_message()).await {
        Ok(stream) => stream,
        Err(error) => {
            tracing::warn!(code = error.code(), "chat verification request failed");
            return ChatHttpVerificationStatus::ProviderError;
        }
    };
    match tokio::time::timeout(reply_timeout, reply_has_text(host, session_id, stream)).await {
        Ok(true) => ChatHttpVerificationStatus::Verified,
        Ok(false) => ChatHttpVerificationStatus::ProviderError,
        Err(_) => ChatHttpVerificationStatus::Timeout,
    }
}

fn verification_message() -> SessionMessage {
    SessionMessage {
        role: MessageRole::User,
        content: VERIFICATION_MESSAGE.to_owned(),
        attachments: Vec::new(),
        tools: None,
        context: None,
        response_format: None,
        screen_derived: false,
    }
}

/// Whether the reply stream ends without an error after yielding text, read
/// the way `collect_response` in `current_context.rs` reads a reply: Text
/// frames own the body, and a terminal Result's content counts only when no
/// Text frame arrived. The reply is neither kept nor logged; only its token
/// usage is recorded against the daily budget.
async fn reply_has_text(
    host: &dyn VerificationSessionHost,
    session_id: &str,
    mut stream: ResponseStream,
) -> bool {
    let mut saw_text_frame = false;
    let mut text_has_content = false;
    let mut result_has_content = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(OutboundMessage::Text { content, .. }) => {
                saw_text_frame = true;
                text_has_content |= !content.trim().is_empty();
            }
            Ok(OutboundMessage::Result { content, usage, .. }) => {
                if !content.is_empty() {
                    result_has_content = !content.trim().is_empty();
                }
                if let Some(usage) = usage {
                    host.record_usage(session_id, usage.input_tokens, usage.output_tokens)
                        .await;
                }
            }
            Ok(OutboundMessage::Error { code, .. }) => {
                tracing::warn!(%code, "chat verification reply reported an error");
                return false;
            }
            Err(error) => {
                tracing::warn!(code = error.code(), "chat verification reply failed");
                return false;
            }
            Ok(_) => {}
        }
    }
    if saw_text_frame {
        text_has_content
    } else {
        result_has_content
    }
}

#[cfg(test)]
#[path = "ai_verification/tests.rs"]
mod tests;
