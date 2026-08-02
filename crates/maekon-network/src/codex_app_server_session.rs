//! `CodexAppServerSession` (E21 #4866) — a `ConversationSession` backed by the
//! Codex `app-server` JSON-RPC transport. Replaces the per-turn `codex exec`
//! re-spawn (which re-fed the whole history each turn) with a native `thread/*`
//! conversation: `thread/start` on connect (or `thread/resume` to re-attach an
//! existing server-side thread, [`CodexAppServerSession::resume`]), `turn/start`
//! per message.
//!
//! Streaming-event → `OutboundMessage` mapping is intentionally minimal here
//! (agentMessage deltas + a terminal Result); richer mapping is #4867. PII
//! guarding of outbound content is applied by the `GuardedConversationSession`
//! decorator (#4869), not here.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_stream::try_stream;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::Mutex as AsyncMutex;

use maekon_core::codex_approval::CodexApprovalDecider;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    ConversationSessionInfo, OutboundMessage, SessionMessage, SessionState, SessionTransport,
    TokenUsage,
};
use maekon_core::ports::codex_approval::ApprovalDecision;
use maekon_core::ports::conversation_session::{ConversationSession, ResponseStream};

use crate::codex_app_server::{
    AppServerProcess, ClientInfo, InboundRequest, Notification, ServerInfo, TransportError,
};
use crate::mutex_ext::lock_or_recover;

/// #8057 (P2-3, #4865): default max idle gap between a turn's app-server
/// notifications before the drain loop force-terminates the turn. The JSON-RPC
/// request timeout bounds request/response correlation only, NOT the
/// notification stream — a live provider that never emits `turn/completed`
/// would otherwise pin the owned notifications lock forever, deadlocking the
/// next `send_message`. Codex streams continuously, so a multi-minute gap
/// between ANY notification means the turn is wedged.
const DEFAULT_TURN_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// Map a transport-level failure onto the shared `CoreError` wire taxonomy.
fn map_transport_error(err: TransportError) -> CoreError {
    match err {
        TransportError::Timeout { timeout_ms } => CoreError::RequestTimeout {
            code: maekon_core::error_codes::NetworkCode::Timeout,
            timeout_ms,
        },
        TransportError::Rpc(rpc) => CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!("app-server rpc error {}: {}", rpc.code, rpc.message),
        },
        TransportError::Closed(msg) | TransportError::Io(msg) => CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: format!("app-server transport: {msg}"),
        },
    }
}

/// Emit a distinct, groupable warn when a mid-call request hits `-32601`
/// (method not found) — the LIVE app-server does not support `method` (E21
/// #4863/#5017). KNOWN PoC limitation: there is NO graceful re-fallback for a
/// mid-turn unsupported method (the thread is already live); the error surfaces
/// to the caller. Behavior is unchanged — this only logs.
fn warn_on_mid_call_unsupported(err: &TransportError, thread_id: &str, method: &str) {
    if let TransportError::Rpc(rpc) = err {
        if rpc.code == -32601 {
            tracing::warn!(
                rpc_code = -32601,
                thread_id = %thread_id,
                method,
                "app-server unsupported method mid-turn; no re-fallback (#4863 known limitation)"
            );
        }
    }
}

/// Conversation-thread parameters passed to `thread/start` (E21 #4866 I3).
#[derive(Debug, Clone)]
pub struct ThreadConfig {
    pub model: String,
    /// Working directory (`None` → app-server default).
    pub cwd: Option<String>,
    /// Requested sandbox policy; clamped by [`effective_sandbox`] (R6).
    pub sandbox_policy: Option<String>,
    /// Requested command-approval policy.
    pub approval_policy: Option<String>,
}

/// R6 (#4866): never weaken the conversation sandbox below the exec path's
/// `read-only` containment. Any other/unknown/absent request is clamped to
/// `read-only` (and a downgrade attempt is logged).
fn effective_sandbox(requested: Option<&str>) -> &'static str {
    match requested.map(str::trim) {
        None | Some("") | Some("read-only") => "read-only",
        Some(other) => {
            tracing::warn!(
                requested = other,
                "app-server sandbox downgrade refused; clamping to read-only (#4866 R6)"
            );
            "read-only"
        }
    }
}

/// Extract token usage from a `turn/completed` notification (E21 #4867).
///
/// The app-server is preview/unstable and its exact usage shape is not pinned
/// (see R1/R4 + the #4863 gate), so this is defensive: it accepts the usage
/// object at `params.usage` or nested at `params.turn.usage`, and reads token
/// counts under either camelCase (`inputTokens`) or snake_case (`input_tokens`)
/// keys. Returns `None` when no usage is present rather than fabricating zeros.
fn parse_usage(params: &serde_json::Value) -> Option<TokenUsage> {
    let usage = params
        .get("usage")
        .or_else(|| params.get("turn").and_then(|t| t.get("usage")))?;
    let count = |camel: &str, snake: &str| -> Option<u64> {
        usage
            .get(camel)
            .or_else(|| usage.get(snake))
            .and_then(serde_json::Value::as_u64)
    };
    let input_tokens = count("inputTokens", "input_tokens");
    let output_tokens = count("outputTokens", "output_tokens");
    // Only synthesize a TokenUsage when at least one count is present.
    if input_tokens.is_none() && output_tokens.is_none() {
        return None;
    }
    Some(TokenUsage {
        input_tokens: input_tokens.unwrap_or(0),
        output_tokens: output_tokens.unwrap_or(0),
    })
}

/// Parse a v2 `TokenUsageBreakdown` (`{inputTokens, outputTokens, ...}`) into
/// maekon's [`TokenUsage`]. Real codex 0.130.0 surfaces per-turn usage via the
/// `thread/tokenUsage/updated` notification (`tokenUsage.last`), NOT on
/// `turn/completed` (whose `Turn` carries no usage) — so `parse_usage` always
/// returned `None` against the real server. (#5071)
fn parse_token_usage_breakdown(breakdown: &serde_json::Value) -> Option<TokenUsage> {
    let count = |camel: &str, snake: &str| -> Option<u64> {
        breakdown
            .get(camel)
            .or_else(|| breakdown.get(snake))
            .and_then(serde_json::Value::as_u64)
    };
    let input_tokens = count("inputTokens", "input_tokens");
    let output_tokens = count("outputTokens", "output_tokens");
    if input_tokens.is_none() && output_tokens.is_none() {
        return None;
    }
    Some(TokenUsage {
        input_tokens: input_tokens.unwrap_or(0),
        output_tokens: output_tokens.unwrap_or(0),
    })
}

/// Extract a turn id from a `turn/start` result or a `turn/started`/`turn/*`
/// notification's params (E21 #5017). The app-server is preview/unstable so its
/// exact shape is not pinned (R1/R4): accept `turn.id` (nested), a top-level
/// `turnId` (camelCase) or `turn_id` (snake_case). Returns `None` when absent
/// rather than fabricating an id — `steer`/`interrupt` then return
/// `InvalidArguments` (graceful, no crash) against a server that does not
/// surface a turn id.
fn extract_turn_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("turn")
        .and_then(|t| t.get("id"))
        .or_else(|| value.get("turnId"))
        .or_else(|| value.get("turn_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

/// Extract the thread id from a `thread/start` / `thread/resume` RESULT, accepting
/// both shapes. The real codex app-server (v2 schema, verified live against codex
/// 0.130.0 `generate-ts`: `ThreadStartResponse { thread: Thread { id, .. }, .. }`)
/// NESTS it at `result.thread.id`; the legacy flat `threadId`/`thread_id` is kept
/// as a fallback (older builds + the #4872 fake harness). Mirrors
/// [`extract_turn_id`], which already handles the same nested-vs-flat split for
/// turn ids — connect/resume previously read only the flat `threadId`, which is
/// why the live smoke (#4863 AC1) failed against the real binary.
fn extract_thread_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("thread")
        .and_then(|t| t.get("id"))
        .or_else(|| value.get("threadId"))
        .or_else(|| value.get("thread_id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

pub struct CodexAppServerSession {
    /// Shared so the approval task (#4870) can `respond()` concurrently with the
    /// per-turn notification drain. The session holds one `Arc`; the approval
    /// task holds another. On `Drop` the session aborts the task (releasing its
    /// `Arc`), so the last `Arc` drop reaps the process group as before.
    process: Arc<AppServerProcess>,
    thread_id: String,
    model: String,
    created_at: DateTime<Utc>,
    last_active: StdMutex<DateTime<Utc>>,
    turn_count: AtomicU32,
    state: StdMutex<SessionState>,
    /// The id of the in-flight turn, captured from the `turn/start` result (or a
    /// `turn/started` notification) and cleared on `turn/completed` (E21 #5017).
    /// `None` ⇒ no steerable/interruptible turn → `steer`/`interrupt` return
    /// `InvalidArguments`. Short-critical-section `StdMutex`, consistent with
    /// `last_active`/`state`; read by a concurrent `steer`/`interrupt` while the
    /// per-turn drain writes it.
    current_turn_id: Arc<StdMutex<Option<String>>>,
    notifications: Arc<AsyncMutex<UnboundedReceiver<Notification>>>,
    /// #8057 (P2-3): per-turn idle timeout for the notification drain loop.
    /// Defaults to [`DEFAULT_TURN_IDLE_TIMEOUT`]; tests inject a short value.
    turn_idle_timeout: std::time::Duration,
    /// The spawned approval-loop task, aborted on session drop (#4870). `None`
    /// when no decider was supplied (plain #4866 sessions / tests).
    approval_task: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl CodexAppServerSession {
    /// Spawn `codex app-server`, run the `initialize` handshake, and open a NEW
    /// conversation thread (`thread/start`). The thread id becomes the session
    /// id. To re-attach an EXISTING thread instead, see
    /// [`resume`](Self::resume) (`thread/resume`).
    pub async fn connect(
        command: tokio::process::Command,
        client_info: &ClientInfo,
        thread: ThreadConfig,
    ) -> Result<Self, CoreError> {
        Self::connect_with_approval(command, client_info, thread, None).await
    }

    /// Like [`connect`](Self::connect) but wires the FAIL-CLOSED approval loop
    /// (#4870): when `decider` is `Some`, a background task drains server→client
    /// `requestApproval` REQUESTs, runs the decider, and responds on the wire.
    /// `None` preserves the plain #4866 behavior (inbound requests are drained
    /// and dropped — see [`finalize`](Self::finalize)).
    pub async fn connect_with_approval(
        command: tokio::process::Command,
        client_info: &ClientInfo,
        thread: ThreadConfig,
        decider: Option<Arc<CodexApprovalDecider>>,
    ) -> Result<Self, CoreError> {
        let (process, notifications, inbound_requests) =
            AppServerProcess::connect(command, client_info)
                .await
                .map_err(map_transport_error)?;

        // R6: the thread's sandbox is never weaker than the exec path's
        // read-only containment, regardless of what the caller requested.
        let sandbox = effective_sandbox(thread.sandbox_policy.as_deref());
        let mut params = serde_json::json!({ "model": thread.model, "sandbox": sandbox });
        if let Some(cwd) = thread.cwd.as_deref() {
            params["cwd"] = serde_json::Value::String(cwd.to_string());
        }
        // #5071: v2 `ThreadStartParams.approvalPolicy` is the `AskForApproval`
        // enum; a non-enum string is rejected by real codex with a deserialize
        // error. Only forward a recognized value; an unrecognized one is dropped
        // with a warning (the server then applies its default) rather than
        // breaking thread/start. The `granular` object form is not surfaced by
        // maekon's String config, so only the string variants are accepted.
        if let Some(approval) = thread.approval_policy.as_deref() {
            const ASK_FOR_APPROVAL: [&str; 4] = ["untrusted", "on-failure", "on-request", "never"];
            if ASK_FOR_APPROVAL.contains(&approval) {
                params["approvalPolicy"] = serde_json::Value::String(approval.to_string());
            } else {
                tracing::warn!(
                    approval_policy = approval,
                    "ignoring unrecognized codex approvalPolicy (not an AskForApproval value); \
                     server default applies (#5071)"
                );
            }
        }

        let result = process
            .request("thread/start", params)
            .await
            .map_err(map_transport_error)?;
        let thread_id = extract_thread_id(&result).ok_or_else(|| CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: "app-server thread/start did not return a threadId".to_string(),
        })?;

        Ok(Self::finalize(
            process,
            notifications,
            inbound_requests,
            decider,
            thread_id,
            thread.model,
        ))
    }

    /// Re-attach an EXISTING server-side conversation thread (E21 #4866).
    ///
    /// Spawns `codex app-server`, runs the shared `initialize` handshake, then
    /// `thread/resume` to bind to `thread_id` instead of `thread/start`. The
    /// server retains the thread's full prior context, so post-resume turns need
    /// no client-side history replay — the live-path no-resend guarantee holds
    /// across a process restart.
    ///
    /// Continuity guard: if the server echoes a DIFFERENT `threadId`, resume
    /// fails (`CoreError::Network`) rather than silently drifting onto a fresh,
    /// empty thread. (When the server omits `threadId` on success, the absence
    /// is treated as a match — the same defensive posture as `connect`.)
    ///
    /// R6: the resumed thread keeps the read-only containment that
    /// [`effective_sandbox`] clamped at `thread/start`; `resume` re-supplies
    /// neither cwd nor sandbox. If a future upstream requires them on resume,
    /// route them through [`ThreadConfig`] + [`effective_sandbox`] so the
    /// read-only clamp still holds. `turn_count` restarts at 0 (this process has
    /// issued zero NEW turns; it is not a cumulative thread-lifetime counter).
    pub async fn resume(
        command: tokio::process::Command,
        client_info: &ClientInfo,
        thread_id: String,
        model: String,
    ) -> Result<Self, CoreError> {
        Self::resume_with_approval(command, client_info, thread_id, model, None).await
    }

    /// Like [`resume`](Self::resume) but wires the #4870 approval loop (see
    /// [`connect_with_approval`](Self::connect_with_approval)).
    pub async fn resume_with_approval(
        command: tokio::process::Command,
        client_info: &ClientInfo,
        thread_id: String,
        model: String,
        decider: Option<Arc<CodexApprovalDecider>>,
    ) -> Result<Self, CoreError> {
        let (process, notifications, inbound_requests) =
            AppServerProcess::connect(command, client_info)
                .await
                .map_err(map_transport_error)?;

        let result = process
            .request(
                "thread/resume",
                serde_json::json!({ "threadId": thread_id }),
            )
            .await
            .map_err(map_transport_error)?;

        // Defensive: absence of an echoed threadId is treated as a match (same
        // posture as connect's optional reads); an explicit DIFFERENT id means
        // the server opened another thread and continuity is lost. Accepts the
        // v2 nested `thread.id` and the legacy flat `threadId` (see
        // [`extract_thread_id`]).
        let echoed_id = extract_thread_id(&result);
        let echoed = echoed_id.as_deref().unwrap_or(thread_id.as_str());
        if echoed != thread_id {
            return Err(CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message: format!(
                    "app-server thread/resume returned a different threadId \
                     (requested {thread_id}, got {echoed}); continuity not preserved"
                ),
            });
        }

        Ok(Self::finalize(
            process,
            notifications,
            inbound_requests,
            decider,
            thread_id,
            model,
        ))
    }

    /// Shared struct-tail construction for [`connect`](Self::connect) and
    /// [`resume`](Self::resume): a freshly-attached process always starts
    /// `Active` with zero NEW turns. When `decider` is `Some`, spawns the #4870
    /// approval task; otherwise drains-and-drops inbound requests so they cannot
    /// queue unboundedly (the plain #4866 posture).
    fn finalize(
        process: AppServerProcess,
        notifications: UnboundedReceiver<Notification>,
        inbound_requests: UnboundedReceiver<InboundRequest>,
        decider: Option<Arc<CodexApprovalDecider>>,
        thread_id: String,
        model: String,
    ) -> Self {
        let process = Arc::new(process);
        let approval_task = Self::spawn_approval_task(process.clone(), inbound_requests, decider);
        Self {
            process,
            thread_id,
            model,
            created_at: Utc::now(),
            last_active: StdMutex::new(Utc::now()),
            turn_count: AtomicU32::new(0),
            state: StdMutex::new(SessionState::Active),
            current_turn_id: Arc::new(StdMutex::new(None)),
            notifications: Arc::new(AsyncMutex::new(notifications)),
            turn_idle_timeout: DEFAULT_TURN_IDLE_TIMEOUT,
            approval_task: StdMutex::new(approval_task),
        }
    }

    /// #8057 (P2-3): override the per-turn notification idle timeout (default
    /// [`DEFAULT_TURN_IDLE_TIMEOUT`]). Exposed so a factory can tune it and so
    /// integration tests can inject a short value to exercise the wedged-turn
    /// termination path without waiting the production default.
    #[must_use]
    pub fn with_turn_idle_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.turn_idle_timeout = timeout;
        self
    }

    /// The parsed `initialize` response from the underlying app-server process
    /// (E21 #4863). Delegates to [`AppServerProcess::server_info`]: today only the
    /// process exposes the handshake `ServerInfo`, but the session-creating
    /// factory holds a `CodexAppServerSession` (not the inner process) and needs
    /// the `userAgent` post-connect to run the inform-only version cross-check.
    pub fn server_info(&self) -> &ServerInfo {
        self.process.server_info()
    }

    /// Spawn the FAIL-CLOSED approval loop (#4870). Runs CONCURRENTLY with the
    /// per-turn notification drain so a server turn blocked awaiting an approval
    /// response is unblocked while the same session keeps streaming deltas.
    ///
    /// SECURITY: the response payload is ONLY `{"decision":"accept"|"decline"}`
    /// — never a sandbox/permission/escalation field (I7/R6). The thread sandbox
    /// is pinned read-only once at `thread/start` ([`effective_sandbox`]); an
    /// auto-accept here structurally cannot re-open it.
    ///
    /// When `decider` is `None`, the task still drains the channel (dropping each
    /// request) so an unanswered server request cannot grow the unbounded queue;
    /// the server's turn proceeds without the gated action (fail-closed).
    fn spawn_approval_task(
        process: Arc<AppServerProcess>,
        mut inbound_requests: UnboundedReceiver<InboundRequest>,
        decider: Option<Arc<CodexApprovalDecider>>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        Some(tokio::spawn(async move {
            while let Some(req) = inbound_requests.recv().await {
                let Some(decider) = decider.as_ref() else {
                    // No decider: nothing to answer with. Dropping the request is
                    // the fail-closed posture (server proceeds without the action).
                    tracing::debug!(
                        method = %req.method,
                        "codex inbound request dropped (no approval decider wired)"
                    );
                    continue;
                };
                let decision = decider.decide(req.id, &req.method, &req.params).await;
                let payload = approval_payload(decision);
                if let Err(err) = process.respond(req.id, Ok(payload)).await {
                    // An unwritable response is safe: the server simply never
                    // sees the decision and the turn proceeds without the action.
                    tracing::warn!(
                        method = %req.method,
                        "failed to send codex approval response (fail-closed): {err:?}"
                    );
                }
            }
        }))
    }
}

/// Map an [`ApprovalDecision`] to the JSON-RPC `result` payload the app-server's
/// `requestApproval` expects. DELIBERATELY carries ONLY the verdict — no
/// sandbox/permission/escalation field (I7/R6 invariant; asserted by
/// `accept_response_carries_no_sandbox_field`).
fn approval_payload(decision: ApprovalDecision) -> serde_json::Value {
    // Decision tokens are the canonical codex app-server `requestApproval`
    // response values (camelCase), per the openai/codex app-server protocol
    // (codex-rs/app-server/README.md): command/file approvals accept
    // `accept` / `acceptForSession` / `decline` / `cancel`. We send only the
    // fail-closed binary `accept` / `decline`; `approved`/`denied` are NOT
    // recognized by the app-server and would leave the turn's approval
    // unresolved. (maekon's internal AUDIT labels still read approved/denied —
    // that is our own semantic, distinct from this wire token.)
    match decision {
        ApprovalDecision::Accept => serde_json::json!({ "decision": "accept" }),
        ApprovalDecision::Decline => serde_json::json!({ "decision": "decline" }),
    }
}

#[async_trait]
impl ConversationSession for CodexAppServerSession {
    async fn send_message(&self, message: &SessionMessage) -> Result<ResponseStream, CoreError> {
        // #6195: the notification receiver is a SINGLE channel fed by ONE
        // continuous reader task (codex_app_server.rs) that never stops per-turn,
        // and is drained positionally by this loop. If a PRIOR turn's stream was
        // abandoned before `turn/completed` (e.g. the caller dropped the stream
        // after an interrupt, or the turn errored mid-flight), the notifications
        // that prior turn never consumed stay queued and would BLEED into THIS
        // turn — stale deltas/usage attributed to the wrong turn.
        //
        // Fix: acquire the notifications lock and drain any already-queued
        // notifications EAGERLY, BEFORE issuing `turn/start`. At this point the
        // server has been told nothing about this turn, so anything queued is
        // unambiguously stale (from a prior/abandoned turn) — never this turn's
        // own events, which the server only emits AFTER it receives `turn/start`.
        // The same guard is then held across the recv loop below, so this turn
        // consumes only its OWN notifications.
        //
        // (A per-notification turnId would let us filter instead, but the
        // app-server does NOT carry one on every notification — e.g. agentMessage
        // deltas, and the fake harness emits none — so a pre-turn positional
        // drain is the robust choice; see extract_turn_id's note on the unpinned
        // notification shape.)
        let mut guard = self.notifications.clone().lock_owned().await;
        let mut drained_stale = 0u32;
        while guard.try_recv().is_ok() {
            drained_stale += 1;
        }
        if drained_stale > 0 {
            tracing::debug!(
                drained_stale,
                thread_id = %self.thread_id,
                "drained stale app-server notifications from a prior/abandoned turn (#6195)"
            );
        }

        // Begin the turn (the server streams its work via notifications). Issued
        // AFTER the drain above, so every notification the server emits for this
        // turn arrives strictly after this point and is delivered (not drained).
        let start_result = self
            .process
            .request(
                // v2 `TurnStartParams.input` is `Array<UserInput>` (verified live
                // against codex 0.130.0 generate-ts), NOT a bare string — a string
                // is rejected with `-32600 invalid type … expected a sequence`. A
                // text turn is one `{type:"text", text, text_elements:[]}` item.
                "turn/start",
                serde_json::json!({
                    "threadId": self.thread_id,
                    "input": [{ "type": "text", "text": message.content, "text_elements": [] }],
                }),
            )
            .await
            .map_err(|err| {
                // E21 #4863 (observability only): a mid-turn `-32601` (method not
                // found) means a LIVE session's app-server does not support
                // `turn/start`. Unlike a connect-time `-32601` — which bubbles to
                // the #4871 arm and degrades to `codex exec` — a mid-turn one has
                // NO graceful re-fallback (the thread is already live); it surfaces
                // a hard turn error to the chat caller. KNOWN PoC limitation
                // (documented in docs/qa/provider-cli-compatibility-matrix.md);
                // mid-turn re-fallback is a deferred follow-up. This branch only
                // emits a distinct, groupable warn — behavior is unchanged.
                if let TransportError::Rpc(ref rpc) = err {
                    if rpc.code == -32601 {
                        tracing::warn!(
                            rpc_code = -32601,
                            thread_id = %self.thread_id,
                            "app-server unsupported method mid-turn (turn/start); \
                             no re-fallback (#4863 known limitation)"
                        );
                    }
                }
                map_transport_error(err)
            })?;

        self.turn_count.fetch_add(1, Ordering::Relaxed);
        *lock_or_recover(&self.last_active, "codex_app_server_session.last_active") = Utc::now();

        // E21 #5017: capture the in-flight turn id so a concurrent
        // steer/interrupt can target it. Prefer the turn/start result; the drain
        // loop below also captures it from a `turn/started` notification if the
        // result did not carry one. Cleared on `turn/completed`.
        if let Some(turn_id) = extract_turn_id(&start_result) {
            *lock_or_recover(
                &self.current_turn_id,
                "codex_app_server_session.current_turn_id",
            ) = Some(turn_id);
        }

        // Drain this turn's notification stream, normalizing app-server events to
        // `OutboundMessage` (E21 #4867). Mapping policy:
        //   - `item/agentMessage/delta`  → Text  (assistant answer, streamed)
        //   - `item/agentReasoning/delta` → Thinking (chain-of-thought, streamed)
        //   - `turn/completed`           → Result (terminal, with token usage)
        //   - command-execution / plan / diff / item lifecycle → IGNORED (debug
        //     log only). Surfacing those is the approval workflow's job (#4870);
        //     mapping them to chat output here would leak tool-internals into the
        //     answer stream.
        //   - anything else              → graceful debug log, never panic.
        let current_turn_id = self.current_turn_id.clone();
        let turn_idle_timeout = self.turn_idle_timeout;
        // `guard` (the pre-drained, still-held owned notifications lock) is moved
        // into the stream so this turn keeps exclusive access from the drain
        // point through the recv loop — no other turn can interleave on the
        // single channel, and the stale-drain baseline established above holds.
        let stream: ResponseStream = Box::pin(try_stream! {
            let mut guard = guard;
            let mut accumulated = String::new();
            // #5071: per-turn token usage arrives via `thread/tokenUsage/updated`
            // (`tokenUsage.last`) DURING the turn, not on `turn/completed`. Capture
            // the latest and attach it to the terminal Result.
            let mut latest_usage: Option<TokenUsage> = None;
            loop {
                // #8057 (P2-3, #4865): bound the wait for EACH notification so a
                // wedged provider (alive, but no `turn/completed`) cannot block
                // this `recv` forever holding the owned lock. On idle-elapse,
                // yield a terminal error and break so the guard drops.
                let note = match tokio::time::timeout(turn_idle_timeout, guard.recv()).await {
                    Ok(Some(note)) => note,
                    // Channel closed (process died mid-turn): end the drain; the
                    // command layer's P2-2 guard emits a synthetic terminal.
                    Ok(None) => break,
                    Err(_elapsed) => {
                        *lock_or_recover(
                            &current_turn_id,
                            "codex_app_server_session.current_turn_id",
                        ) = None;
                        tracing::warn!(
                            idle_timeout_secs = turn_idle_timeout.as_secs(),
                            "codex turn idle timeout — terminating turn (#4865)"
                        );
                        yield OutboundMessage::Error {
                            code: "turn_timeout".to_string(),
                            message: format!(
                                "codex turn produced no notification for {}s",
                                turn_idle_timeout.as_secs()
                            ),
                            retryable: true,
                        };
                        break;
                    }
                };
                match note.method.as_str() {
                    // E21 #5017: a `turn/started` notification is a fallback
                    // source for the turn id when turn/start's result omitted it.
                    "turn/started" => {
                        if let Some(turn_id) = extract_turn_id(&note.params) {
                            *lock_or_recover(
                                &current_turn_id,
                                "codex_app_server_session.current_turn_id",
                            ) = Some(turn_id);
                        }
                    }
                    "item/agentMessage/delta" => {
                        // v2 `AgentMessageDeltaNotification` carries the streamed
                        // chunk in `delta` (verified live against codex 0.130.0);
                        // the legacy/fake shape used `text`. Accept both.
                        if let Some(text) = note.params.get("delta")
                            .or_else(|| note.params.get("text"))
                            .and_then(|v| v.as_str())
                        {
                            accumulated.push_str(text);
                            yield OutboundMessage::Text { content: text.to_string(), done: false };
                        }
                    }
                    // Real codex 0.130.0 (v2) emits reasoning as
                    // `item/reasoning/textDelta` + `item/reasoning/summaryTextDelta`
                    // (chunk in `delta`); the prior `item/agentReasoning/delta` was
                    // a FABRICATED method the real binary never sends (only the fake
                    // echoed it) → reasoning was silently dropped. Match all three;
                    // `item/agentReasoning/delta` retained for the fake harness.
                    // (#4861 epic review)
                    "item/reasoning/textDelta"
                    | "item/reasoning/summaryTextDelta"
                    | "item/agentReasoning/delta" => {
                        if let Some(text) = note.params.get("delta")
                            .or_else(|| note.params.get("text"))
                            .and_then(|v| v.as_str())
                        {
                            yield OutboundMessage::Thinking { content: text.to_string(), done: false };
                        }
                    }
                    // #5071: real codex per-turn usage. v2
                    // `ThreadTokenUsageUpdatedNotification = {threadId, turnId,
                    // tokenUsage: ThreadTokenUsage{total, last, ...}}`; `last` is
                    // this turn's `TokenUsageBreakdown`.
                    "thread/tokenUsage/updated" => {
                        if let Some(usage) = note
                            .params
                            .get("tokenUsage")
                            .and_then(|tu| tu.get("last"))
                            .and_then(parse_token_usage_breakdown)
                        {
                            latest_usage = Some(usage);
                        }
                    }
                    "turn/completed" => {
                        // E21 #5017: the turn is over — clear the in-flight id so
                        // a late steer/interrupt returns InvalidArguments (no
                        // active turn) rather than targeting a dead turn.
                        *lock_or_recover(
                            &current_turn_id,
                            "codex_app_server_session.current_turn_id",
                        ) = None;
                        yield OutboundMessage::Result {
                            content: accumulated.clone(),
                            done: true,
                            // Prefer the usage captured from tokenUsage/updated
                            // (real codex); fall back to turn/completed.usage (the
                            // fake harness shape) for back-compat.
                            usage: latest_usage.clone().or_else(|| parse_usage(&note.params)),
                        };
                        break;
                    }
                    // Tool-internal / planning events: intentionally not surfaced
                    // to the chat answer stream (approval routing is #4870).
                    "item/started"
                    | "item/completed"
                    | "item/commandExecution/outputDelta"
                    | "turn/plan/updated"
                    | "turn/diff/updated" => {
                        tracing::debug!(method = %note.method, "app-server event ignored (not chat output; see #4870)");
                    }
                    other => {
                        tracing::debug!(method = %other, "unknown app-server notification ignored (graceful)");
                    }
                }
            }
        });
        Ok(stream)
    }

    fn info(&self) -> ConversationSessionInfo {
        ConversationSessionInfo {
            session_id: self.thread_id.clone(),
            provider_name: "codex".to_string(),
            model: self.model.clone(),
            state: *lock_or_recover(&self.state, "codex_app_server_session.state"),
            transport: SessionTransport::Subprocess,
            created_at: self.created_at,
            last_active: *lock_or_recover(
                &self.last_active,
                "codex_app_server_session.last_active",
            ),
            turn_count: self.turn_count.load(Ordering::Relaxed),
            title: None,
        }
    }

    fn session_id(&self) -> &str {
        &self.thread_id
    }

    fn provider_name(&self) -> &str {
        "codex"
    }

    fn is_external(&self) -> bool {
        // app-server egresses chat content off-device → must be guarded (#4869).
        true
    }

    fn can_self_heal_on_retry(&self) -> bool {
        // #8057 (P3, #4872): this backend owns ONE long-lived app-server process.
        // Once it dies the held handle is permanently dead — a retry that merely
        // flips state back to Active would re-fail on the next send. Report false
        // so `recover_session` surfaces an honest "re-create the session" error
        // instead of a misleading "recovered" transition. In-place thread rebuild
        // via `resume` is tracked in #4872.
        false
    }

    /// Interrupt (cancel) the in-flight turn via the app-server `turn/interrupt`
    /// JSON-RPC on the LIVE thread (E21 #5017). Sent on the same shared
    /// `AppServerProcess` writer (locked per-call) as the send_message turn, so
    /// it can be issued WHILE the turn's notification stream is still draining.
    ///
    /// SECURITY (R6 / is_external preserved): the request params carry ONLY
    /// `{threadId, turnId}` — never a sandbox/cwd/approval/escalation field — so
    /// it structurally cannot re-open the read-only clamp pinned at thread/start
    /// (same argument as the approval payload guard). is_external stays true.
    ///
    /// # Errors
    /// `InvalidArguments` when no turn is in flight; transport errors map via
    /// [`map_transport_error`]. A mid-call `-32601` (app-server lacks
    /// `turn/interrupt`) is a KNOWN #4863 PoC limitation — surfaced as an error,
    /// no re-fallback.
    async fn interrupt(&self) -> Result<(), CoreError> {
        let turn_id = lock_or_recover(
            &self.current_turn_id,
            "codex_app_server_session.current_turn_id",
        )
        .clone();
        let Some(turn_id) = turn_id else {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: "no active turn to interrupt".to_string(),
            });
        };
        self.process
            .request(
                "turn/interrupt",
                serde_json::json!({ "threadId": self.thread_id, "turnId": turn_id }),
            )
            .await
            .map(|_| ())
            .map_err(|err| {
                warn_on_mid_call_unsupported(&err, &self.thread_id, "turn/interrupt");
                map_transport_error(err)
            })
    }

    /// Steer the in-flight turn with additional user input via the app-server
    /// `turn/steer` JSON-RPC on the LIVE thread (E21 #5017).
    ///
    /// SECURITY: `message.content` is USER content; the `GuardedConversationSession`
    /// decorator sanitizes it before this adapter is reached (fail-closed). The
    /// request params carry `{threadId, expectedTurnId, input:[{type:text,...}]}`
    /// — NO sandbox/cwd/approval field — so R6 + is_external are untouched.
    ///
    /// # Errors
    /// `InvalidArguments` when no turn is in flight; transport errors map via
    /// [`map_transport_error`]; mid-call `-32601` is the known PoC limitation.
    async fn steer(&self, message: &SessionMessage) -> Result<(), CoreError> {
        let turn_id = lock_or_recover(
            &self.current_turn_id,
            "codex_app_server_session.current_turn_id",
        )
        .clone();
        let Some(turn_id) = turn_id else {
            return Err(CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                message: "no active turn to steer".to_string(),
            });
        };
        self.process
            .request(
                // v2 `TurnSteerParams.input` is `Array<UserInput>`; the text item
                // REQUIRES `text_elements` (same v2 UserInput shape as turn/start,
                // verified against codex 0.130.0 generate-ts). Omitting it makes a
                // real server reject the steer with a deserialize error — the fake
                // harness (printf echo) never caught it. (#4861 epic review)
                "turn/steer",
                serde_json::json!({
                    "threadId": self.thread_id,
                    "expectedTurnId": turn_id,
                    "input": [{ "type": "text", "text": message.content, "text_elements": [] }],
                }),
            )
            .await
            .map(|_| ())
            .map_err(|err| {
                warn_on_mid_call_unsupported(&err, &self.thread_id, "turn/steer");
                map_transport_error(err)
            })
    }

    async fn terminate(&self) {
        *lock_or_recover(&self.state, "codex_app_server_session.state") = SessionState::Terminated;
        // Dropping the process (on session drop) reaps the group; nothing else
        // to do synchronously here.
    }
}

impl Drop for CodexAppServerSession {
    fn drop(&mut self) {
        // Abort the approval task so it releases its `Arc<AppServerProcess>`
        // clone; the session's own `Arc` then drops, reaping the process group
        // (the `AppServerProcess` Drop). Without this the task's Arc would keep
        // the process alive until the (unbounded) request channel closed.
        if let Some(handle) = lock_or_recover(
            &self.approval_task,
            "codex_app_server_session.approval_task",
        )
        .take()
        {
            handle.abort();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use futures::StreamExt;
    use maekon_core::models::ai_session::MessageRole;

    /// A loop-based fake `codex app-server`: dispatches by JSON-RPC method,
    /// echoes the request id, and for turn/start emits an agentMessage delta
    /// plus turn/completed. Exits on stdin EOF.
    fn fake_app_server() -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id" ;;
    *'"thread/start"'*) printf '{"id":%s,"result":{"threadId":"thr_42"}}\n' "$id" ;;
    *'"turn/start"'*) printf '{"id":%s,"result":{}}\n' "$id"; printf '{"method":"item/agentMessage/delta","params":{"text":"Hello"}}\n'; printf '{"method":"turn/completed","params":{}}\n' ;;
  esac
done"#,
        );
        cmd
    }

    fn client_info() -> ClientInfo {
        ClientInfo {
            name: "maekon".to_string(),
            title: "Maekon".to_string(),
            version: "0.0.1".to_string(),
        }
    }

    fn thread_config() -> ThreadConfig {
        ThreadConfig {
            model: "gpt-5.4".to_string(),
            cwd: None,
            sandbox_policy: None,
            approval_policy: None,
        }
    }

    #[test]
    fn accept_response_carries_no_sandbox_field() {
        // I7/R6 mutation guard: the approval response payload carries ONLY the
        // verdict — never a sandbox/permission/escalation field. An auto-accept
        // therefore structurally cannot downgrade the read-only thread sandbox.
        for decision in [ApprovalDecision::Accept, ApprovalDecision::Decline] {
            let payload = approval_payload(decision);
            let serialized = serde_json::to_string(&payload)
                .unwrap()
                .to_ascii_lowercase();
            assert!(
                !serialized.contains("sandbox"),
                "approval payload must not carry a sandbox field: {serialized}"
            );
            assert!(
                !serialized.contains("permission"),
                "approval payload must not carry a permission field: {serialized}"
            );
            assert!(
                !serialized.contains("escalat"),
                "approval payload must not carry an escalation field: {serialized}"
            );
            assert!(
                serialized.contains("decision"),
                "approval payload must carry the verdict: {serialized}"
            );
        }
        // Canonical codex app-server tokens (camelCase), NOT approved/denied.
        assert_eq!(
            approval_payload(ApprovalDecision::Accept)["decision"],
            "accept"
        );
        assert_eq!(
            approval_payload(ApprovalDecision::Decline)["decision"],
            "decline"
        );
    }

    #[test]
    fn effective_sandbox_never_weaker_than_read_only() {
        // R6: default + any downgrade request resolve to read-only.
        assert_eq!(effective_sandbox(None), "read-only");
        assert_eq!(effective_sandbox(Some("read-only")), "read-only");
        assert_eq!(effective_sandbox(Some("workspace-write")), "read-only");
        assert_eq!(effective_sandbox(Some("danger-full-access")), "read-only");
        assert_eq!(effective_sandbox(Some("")), "read-only");
    }

    #[tokio::test]
    async fn connect_opens_thread_and_exposes_it_as_session_id() {
        let session =
            CodexAppServerSession::connect(fake_app_server(), &client_info(), thread_config())
                .await
                .expect("connect + thread/start");
        assert_eq!(session.session_id(), "thr_42");
        assert!(session.is_external());
        assert_eq!(session.provider_name(), "codex");
        assert_eq!(session.info().turn_count, 0);
    }

    #[tokio::test]
    async fn send_message_starts_turn_and_streams_to_result() {
        let session =
            CodexAppServerSession::connect(fake_app_server(), &client_info(), thread_config())
                .await
                .unwrap();

        let msg = SessionMessage {
            screen_derived: false,
            role: MessageRole::User,
            content: "hi".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };
        let outputs: Vec<_> = session
            .send_message(&msg)
            .await
            .expect("turn/start")
            .map(|r| r.expect("stream item"))
            .collect()
            .await;

        // A streamed Text("Hello") delta followed by a terminal Result("Hello").
        assert!(outputs
            .iter()
            .any(|m| matches!(m, OutboundMessage::Text { content, .. } if content == "Hello")));
        assert!(outputs.iter().any(
            |m| matches!(m, OutboundMessage::Result { content, done, .. } if content == "Hello" && *done)
        ));
        assert_eq!(session.info().turn_count, 1);
    }

    #[test]
    fn parse_usage_reads_camel_and_snake_case_and_nesting() {
        // camelCase at params.usage
        let camel = serde_json::json!({ "usage": { "inputTokens": 12, "outputTokens": 34 } });
        let u = parse_usage(&camel).expect("camelCase usage");
        assert_eq!((u.input_tokens, u.output_tokens), (12, 34));

        // snake_case at params.usage
        let snake = serde_json::json!({ "usage": { "input_tokens": 5, "output_tokens": 7 } });
        let u = parse_usage(&snake).expect("snake_case usage");
        assert_eq!((u.input_tokens, u.output_tokens), (5, 7));

        // nested under params.turn.usage
        let nested = serde_json::json!({ "turn": { "usage": { "inputTokens": 1 } } });
        let u = parse_usage(&nested).expect("nested usage");
        assert_eq!((u.input_tokens, u.output_tokens), (1, 0));

        // absent → None (never fabricate zeros)
        assert!(parse_usage(&serde_json::json!({})).is_none());
        assert!(parse_usage(&serde_json::json!({ "usage": {} })).is_none());
    }

    /// Fake app-server that emits, for one turn: a reasoning delta, two answer
    /// deltas, several tool-internal/unknown notifications that MUST be ignored,
    /// and a terminal `turn/completed` carrying token usage.
    fn fake_app_server_rich() -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id" ;;
    *'"thread/start"'*) printf '{"id":%s,"result":{"threadId":"thr_42"}}\n' "$id" ;;
    *'"turn/start"'*)
      printf '{"id":%s,"result":{}}\n' "$id"
      printf '{"method":"item/agentReasoning/delta","params":{"text":"pondering"}}\n'
      printf '{"method":"item/started","params":{"itemId":"i1"}}\n'
      printf '{"method":"item/agentMessage/delta","params":{"text":"Hello"}}\n'
      printf '{"method":"item/commandExecution/outputDelta","params":{"chunk":"ls -la"}}\n'
      printf '{"method":"item/agentMessage/delta","params":{"text":" world"}}\n'
      printf '{"method":"turn/plan/updated","params":{"steps":[]}}\n'
      printf '{"method":"some/unknown/event","params":{"x":1}}\n'
      printf '{"method":"turn/completed","params":{"usage":{"inputTokens":12,"outputTokens":34}}}\n'
      ;;
  esac
done"#,
        );
        cmd
    }

    /// Build a `SessionMessage` carrying `content` as a user turn.
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

    /// Drain a `send_message` response stream to completion (discarding output).
    async fn drain(session: &CodexAppServerSession, content: &str) {
        let _outputs: Vec<_> = session
            .send_message(&user_msg(content))
            .await
            .expect("turn/start")
            .map(|r| r.expect("stream item"))
            .collect()
            .await;
    }

    /// A unique per-test temp path for the stateful fake's turn-log, so cargo's
    /// parallel test runner never collides two fakes on the same file.
    fn unique_log_path(test_name: &str) -> std::path::PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "maekon_fake_app_server_{}_{}.log",
            test_name,
            std::process::id()
        ));
        // Start from a clean slate in case a prior run left a stale file.
        let _ = std::fs::remove_file(&path);
        path
    }

    /// A STATEFUL fake `codex app-server` that APPENDS every `turn/start`
    /// request line to `$MAEKON_FAKE_LOG`, then replies with a result, one
    /// answer delta echoing a per-turn marker, and `turn/completed`. Lets a test
    /// prove the persistent thread is reused and that NO history is re-sent.
    ///
    /// `thread/resume` echoes back the SAME threadId it was handed (continuity
    /// preserved), so the same command doubles as a resume round-trip fake.
    fn fake_app_server_stateful(log_path: &std::path::Path) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.env("MAEKON_FAKE_LOG", log_path);
        cmd.arg("-c").arg(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id" ;;
    *'"thread/start"'*) printf '{"id":%s,"result":{"threadId":"thr_42"}}\n' "$id" ;;
    *'"thread/resume"'*)
      tid=$(printf '%s' "$line" | sed -n 's/.*"threadId":"\([^"]*\)".*/\1/p')
      printf '{"id":%s,"result":{"threadId":"%s"}}\n' "$id" "$tid" ;;
    *'"turn/start"'*)
      printf '%s\n' "$line" >> "$MAEKON_FAKE_LOG"
      printf '{"id":%s,"result":{}}\n' "$id"
      printf '{"method":"item/agentMessage/delta","params":{"text":"ack"}}\n'
      printf '{"method":"turn/completed","params":{}}\n'
      ;;
  esac
done"#,
        );
        cmd
    }

    /// A fake whose `thread/resume` ALWAYS returns a DIFFERENT threadId,
    /// regardless of the requested id — used to prove the continuity guard.
    fn fake_app_server_resume_drift() -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id" ;;
    *'"thread/resume"'*) printf '{"id":%s,"result":{"threadId":"thr_other"}}\n' "$id" ;;
  esac
done"#,
        );
        cmd
    }

    #[tokio::test]
    async fn multi_turn_reuses_thread_without_resending_history() {
        // AC1: turn 2 must target the SAME persistent thread_id as turn 1, and
        // the turn/start `input` for turn 2 must be ONLY the new message — the
        // adapter never re-injects turn-1 history (the server owns context).
        let log = unique_log_path("multi_turn_no_resend");
        let session = CodexAppServerSession::connect(
            fake_app_server_stateful(&log),
            &client_info(),
            thread_config(),
        )
        .await
        .expect("connect + thread/start");

        drain(&session, "first question").await;
        drain(&session, "second question").await;

        let logged = std::fs::read_to_string(&log).expect("turn-log written");
        let lines: Vec<&str> = logged.lines().collect();
        assert_eq!(lines.len(), 2, "exactly two turn/start requests: {logged}");

        // Both turns reuse the same persistent thread.
        assert!(
            lines[0].contains(r#""threadId":"thr_42""#),
            "turn 1 targets thr_42: {}",
            lines[0]
        );
        assert!(
            lines[1].contains(r#""threadId":"thr_42""#),
            "turn 2 targets thr_42: {}",
            lines[1]
        );

        // Turn 2 carries ONLY the new message — no turn-1 history replay.
        assert!(
            lines[1].contains("second question"),
            "turn 2 sends the new message: {}",
            lines[1]
        );
        assert!(
            !lines[1].contains("first question"),
            "turn 2 must NOT re-send turn-1 history: {}",
            lines[1]
        );

        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn turn_count_advances_across_two_turns() {
        // AC2: turn_count progresses 0 -> 1 -> 2 across two send_message calls
        // on the same session (existing tests stop at 1).
        let session =
            CodexAppServerSession::connect(fake_app_server(), &client_info(), thread_config())
                .await
                .unwrap();
        assert_eq!(session.info().turn_count, 0);
        drain(&session, "one").await;
        assert_eq!(session.info().turn_count, 1);
        drain(&session, "two").await;
        assert_eq!(session.info().turn_count, 2);
    }

    #[tokio::test]
    async fn info_reflects_active_then_terminated_and_subprocess_transport() {
        // AC2: state is Active on a live session and Terminated after
        // terminate(); transport is always Subprocess.
        let session =
            CodexAppServerSession::connect(fake_app_server(), &client_info(), thread_config())
                .await
                .unwrap();
        assert_eq!(session.info().state, SessionState::Active);
        assert_eq!(session.info().transport, SessionTransport::Subprocess);
        session.terminate().await;
        assert_eq!(session.info().state, SessionState::Terminated);
    }

    #[tokio::test]
    async fn resume_reattaches_same_thread_and_stays_external() {
        // resume(): re-attaching an existing thread via thread/resume yields
        // session_id == the supplied thread_id, preserves is_external()==true
        // and provider_name()=="codex", and starts at turn_count==0.
        let log = unique_log_path("resume_reattach");
        let session = CodexAppServerSession::resume(
            fake_app_server_stateful(&log),
            &client_info(),
            "thr_resumed".to_string(),
            "gpt-5.4".to_string(),
        )
        .await
        .expect("resume re-attaches the thread");

        assert_eq!(session.session_id(), "thr_resumed");
        assert!(session.is_external());
        assert_eq!(session.provider_name(), "codex");
        assert_eq!(session.info().turn_count, 0);
        assert_eq!(session.info().state, SessionState::Active);
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn resume_rejects_thread_id_drift() {
        // resume() continuity guard: if the server opens a DIFFERENT thread on
        // resume (continuity lost), resume() returns Err rather than silently
        // drifting to a fresh empty thread.
        let result = CodexAppServerSession::resume(
            fake_app_server_resume_drift(),
            &client_info(),
            "thr_wanted".to_string(),
            "gpt-5.4".to_string(),
        )
        .await;

        let err = result
            .err()
            .expect("resume must reject a different threadId");
        match err {
            CoreError::Network { message, .. } => {
                assert!(
                    message.contains("thr_wanted") && message.contains("thr_other"),
                    "mismatch error names both threadIds: {message}"
                );
            }
            other => panic!("expected Err(Network) on threadId drift, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn resume_after_first_turn_sends_only_new_message() {
        // AC1 cross-restart bridge: after resume() the FIRST turn/start still
        // sends only the new message (no client-side history replay), matching
        // the live-path no-resend guarantee. A resumed session counts NEW turns
        // from 0.
        let log = unique_log_path("resume_first_turn");
        let session = CodexAppServerSession::resume(
            fake_app_server_stateful(&log),
            &client_info(),
            "thr_42".to_string(),
            "gpt-5.4".to_string(),
        )
        .await
        .expect("resume");

        drain(&session, "after restart").await;
        assert_eq!(session.info().turn_count, 1);

        let logged = std::fs::read_to_string(&log).expect("turn-log written");
        let lines: Vec<&str> = logged.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "exactly one turn/start after resume: {logged}"
        );
        assert!(
            lines[0].contains(r#""threadId":"thr_42""#),
            "turn targets the resumed thread: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("after restart"),
            "turn sends the new message: {}",
            lines[0]
        );
        let _ = std::fs::remove_file(&log);
    }

    /// A STATEFUL fake whose `turn/start` result carries a turn id (`turn_7`)
    /// and which logs+answers `turn/steer` + `turn/interrupt` requests to
    /// `$MAEKON_FAKE_LOG`, so a test can assert the wire send + correlate.
    fn fake_app_server_steerable(log_path: &std::path::Path) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.env("MAEKON_FAKE_LOG", log_path);
        cmd.arg("-c").arg(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id" ;;
    *'"thread/start"'*) printf '{"id":%s,"result":{"threadId":"thr_42"}}\n' "$id" ;;
    *'"turn/interrupt"'*) printf '%s\n' "$line" >> "$MAEKON_FAKE_LOG"; printf '{"id":%s,"result":{}}\n' "$id" ;;
    *'"turn/steer"'*) printf '%s\n' "$line" >> "$MAEKON_FAKE_LOG"; printf '{"id":%s,"result":{}}\n' "$id" ;;
    *'"turn/start"'*)
      printf '{"id":%s,"result":{"turn":{"id":"turn_7"}}}\n' "$id"
      printf '{"method":"item/agentMessage/delta","params":{"text":"hi"}}\n'
      printf '{"method":"turn/completed","params":{}}\n'
      ;;
  esac
done"#,
        );
        cmd
    }

    #[tokio::test]
    async fn interrupt_before_any_turn_is_invalid_arguments() {
        // E21 #5017: with no in-flight turn, interrupt/steer return
        // InvalidArguments (not a crash, not a silent no-op).
        let session =
            CodexAppServerSession::connect(fake_app_server(), &client_info(), thread_config())
                .await
                .unwrap();
        let err = session
            .interrupt()
            .await
            .expect_err("no active turn → interrupt Err");
        assert!(matches!(
            err,
            CoreError::InvalidArguments {
                code: maekon_core::error_codes::ValidationCode::InvalidArguments,
                ..
            }
        ));
        let err = session
            .steer(&user_msg("go left"))
            .await
            .expect_err("no active turn → steer Err");
        assert!(matches!(err, CoreError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn interrupt_after_turn_sends_turn_interrupt_with_thread_and_turn_id() {
        // E21 #5017: after a turn whose turn/start result carried turn_7, a
        // subsequent interrupt sends turn/interrupt carrying {threadId, turnId}.
        // Drains the turn first (which then clears current_turn_id on
        // turn/completed), so we re-prime by re-capturing: assert the wire send
        // happens for a turn whose id is still live. To keep the id live we DO
        // NOT drain to completion here — we hold the stream open.
        let log = unique_log_path("interrupt_sends");
        let session = CodexAppServerSession::connect(
            fake_app_server_steerable(&log),
            &client_info(),
            thread_config(),
        )
        .await
        .unwrap();

        // Start a turn but keep the stream undrained so current_turn_id stays Some.
        let _stream = session.send_message(&user_msg("work")).await.unwrap();
        // The turn id is captured from the turn/start result synchronously.
        session.interrupt().await.expect("interrupt sends");

        let logged = std::fs::read_to_string(&log).expect("interrupt logged");
        let line = logged
            .lines()
            .find(|l| l.contains(r#""turn/interrupt""#))
            .expect("turn/interrupt captured");
        assert!(
            line.contains(r#""threadId":"thr_42""#),
            "carries threadId: {line}"
        );
        assert!(
            line.contains(r#""turnId":"turn_7""#),
            "carries turnId: {line}"
        );
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn steer_after_turn_sends_turn_steer_with_input_text() {
        // E21 #5017: steer sends turn/steer with expectedTurnId + an input list
        // whose text == the message content.
        let log = unique_log_path("steer_sends");
        let session = CodexAppServerSession::connect(
            fake_app_server_steerable(&log),
            &client_info(),
            thread_config(),
        )
        .await
        .unwrap();
        let _stream = session.send_message(&user_msg("work")).await.unwrap();
        session
            .steer(&user_msg("actually do X"))
            .await
            .expect("steer sends");

        let logged = std::fs::read_to_string(&log).expect("steer logged");
        let line = logged
            .lines()
            .find(|l| l.contains(r#""turn/steer""#))
            .expect("turn/steer captured");
        assert!(
            line.contains(r#""expectedTurnId":"turn_7""#),
            "carries expectedTurnId: {line}"
        );
        assert!(
            line.contains("actually do X"),
            "carries the input text: {line}"
        );
        // SECURITY (R6): the steer/interrupt request lines carry NO
        // sandbox/permission/escalation field — same structural guard as the
        // approval payload. (Mutation that adds one fails this.)
        let lower = logged.to_ascii_lowercase();
        assert!(!lower.contains("sandbox"), "no sandbox field: {logged}");
        assert!(
            !lower.contains("permission"),
            "no permission field: {logged}"
        );
        assert!(!lower.contains("escalat"), "no escalation field: {logged}");
        let _ = std::fs::remove_file(&log);
    }

    #[tokio::test]
    async fn streams_reasoning_answer_and_usage_ignoring_tool_and_unknown_events() {
        let session =
            CodexAppServerSession::connect(fake_app_server_rich(), &client_info(), thread_config())
                .await
                .unwrap();

        let msg = SessionMessage {
            screen_derived: false,
            role: MessageRole::User,
            content: "hi".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };
        let outputs: Vec<_> = session
            .send_message(&msg)
            .await
            .expect("turn/start")
            .map(|r| r.expect("stream item"))
            .collect()
            .await;

        // Exactly: Thinking, Text("Hello"), Text(" world"), Result — tool-internal
        // and unknown notifications yield nothing.
        assert_eq!(
            outputs.len(),
            4,
            "ignored/unknown events must not yield; got {outputs:?}"
        );
        assert!(matches!(
            &outputs[0],
            OutboundMessage::Thinking { content, done } if content == "pondering" && !done
        ));
        assert!(matches!(
            &outputs[1],
            OutboundMessage::Text { content, done } if content == "Hello" && !done
        ));
        assert!(matches!(
            &outputs[2],
            OutboundMessage::Text { content, done } if content == " world" && !done
        ));
        // Terminal Result accumulates only the answer deltas and carries usage.
        match &outputs[3] {
            OutboundMessage::Result {
                content,
                done,
                usage,
            } => {
                assert_eq!(content, "Hello world");
                assert!(done);
                let u = usage.as_ref().expect("usage mapped from turn/completed");
                assert_eq!((u.input_tokens, u.output_tokens), (12, 34));
            }
            other => panic!("expected terminal Result, got {other:?}"),
        }
    }

    /// A fake whose `thread/start` reply is PRECEDED on the wire by a stale
    /// `item/agentMessage/delta` ("STALE") — simulating notifications left
    /// queued by a prior/abandoned turn before the next `send_message` runs.
    /// Because the reader task processes wire lines in order, the stale delta is
    /// queued onto the shared receiver BEFORE the awaited `thread/start` result
    /// unblocks `connect` — so by the time the first `send_message` drains, the
    /// stale delta is deterministically already present (no timing race). The
    /// turn itself then emits a clean "fresh" delta + `turn/completed`.
    fn fake_app_server_with_stale_preturn_notification() -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(
            r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"id":%s,"result":{"userAgent":"fake/1"}}\n' "$id" ;;
    *'"thread/start"'*)
      printf '{"method":"item/agentMessage/delta","params":{"text":"STALE"}}\n'
      printf '{"id":%s,"result":{"threadId":"thr_42"}}\n' "$id" ;;
    *'"turn/start"'*)
      printf '{"id":%s,"result":{}}\n' "$id"
      printf '{"method":"item/agentMessage/delta","params":{"text":"fresh"}}\n'
      printf '{"method":"turn/completed","params":{}}\n'
      ;;
  esac
done"#,
        );
        cmd
    }

    #[tokio::test]
    async fn send_message_drains_stale_preturn_notifications_so_they_do_not_bleed_in() {
        // #6195 regression: a notification left queued before the turn starts
        // (here a "STALE" delta emitted at connect-time, standing in for one a
        // prior/abandoned turn never consumed) MUST NOT bleed into this turn's
        // stream. send_message drains the already-queued backlog before its recv
        // loop, so the turn yields only its OWN "fresh" delta + Result("fresh").
        let session = CodexAppServerSession::connect(
            fake_app_server_with_stale_preturn_notification(),
            &client_info(),
            thread_config(),
        )
        .await
        .expect("connect + thread/start");

        let outputs: Vec<_> = session
            .send_message(&user_msg("go"))
            .await
            .expect("turn/start")
            .map(|r| r.expect("stream item"))
            .collect()
            .await;

        // The stale "STALE" delta was drained — it never appears in this turn.
        assert!(
            !outputs.iter().any(|m| matches!(
                m,
                OutboundMessage::Text { content, .. } if content == "STALE"
            )),
            "stale pre-turn notification must be drained, not bleed in: {outputs:?}"
        );
        // The turn still delivers its own fresh delta and a terminal Result whose
        // accumulated content is ONLY the fresh delta (no stale prefix).
        assert!(
            outputs.iter().any(|m| matches!(
                m,
                OutboundMessage::Text { content, .. } if content == "fresh"
            )),
            "the current turn's own delta must still be delivered: {outputs:?}"
        );
        match outputs.last().expect("at least a terminal Result") {
            OutboundMessage::Result { content, done, .. } => {
                assert_eq!(content, "fresh", "Result must accumulate only this turn");
                assert!(done);
            }
            other => panic!("expected terminal Result last, got {other:?}"),
        }
    }
}
