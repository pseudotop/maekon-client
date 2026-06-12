//! Codex `app-server` approval ports (E21 #4870).
//!
//! The Codex `app-server` gates risky turn actions (command execution, file
//! changes, network access) behind a server→client `requestApproval` REQUEST.
//! Maekon must answer accept/decline, FAIL-CLOSED, audit-log every decision, and
//! route unmatched requests to a UI confirmation.
//!
//! These ports keep the decision LOGIC architecture-clean: the policy lookup
//! lives in `maekon-automation` (`PolicyClient`) and the audit sink in
//! `maekon-automation`/`maekon-storage`, neither of which a sibling adapter crate
//! (`maekon-network`) may depend on (hexagonal rule). So the decider here is
//! defined against these traits, and the binary crate (`src-tauri`) wires the
//! concrete `PolicyClient` + `AuditLogPort` behind them.
//!
//! Security invariants (ties to server ADR-025 "approval-gated turns" + I7/R6):
//! - The decision is FAIL-CLOSED: any error / no policy match / timeout →
//!   require UI approval or decline, NEVER a silent accept.
//! - Auto-accept of a command MUST NOT downgrade the thread sandbox — the
//!   decision payload carries ONLY the verdict, never a sandbox/escalation field
//!   (the read-only clamp is pinned once at `thread/start`; see
//!   `effective_sandbox`).
//! - Network approval is DEFAULT-DENY + explicit per-policy opt-in.
//! - Dangerous commands are blocked before any policy lookup.

use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::oneshot;

/// The matched-policy verdict for a requested process, returned by an
/// [`ApprovalPolicyPort`] implementation (which `src-tauri` backs with the
/// automation `PolicyClient`). The decider maps this to an
/// [`ApprovalDecision`] FAIL-CLOSED.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    /// No policy matched the process → the decider escalates to the UI hook
    /// (which is fail-closed: the default hook auto-declines).
    NoMatch,
    /// A policy matched: execute without prompting (only honored when the
    /// command is non-dangerous and any network requirement is satisfied).
    Auto {
        policy_id: String,
        /// `true` only when the matched policy explicitly opted into network
        /// access (`allow_network == Some(true)`). DEFAULT-DENY: `None`/`false`
        /// → a network-context request is declined.
        allow_network: bool,
    },
    /// A policy matched but requires user confirmation → escalate to the UI hook.
    Confirm { policy_id: String },
    /// A policy matched and explicitly blocks the process → decline.
    Block { policy_id: String },
}

/// Outbound port: resolve the policy verdict for a process name + args.
///
/// `src-tauri` implements this over `maekon_automation::policy::PolicyClient`
/// (`get_policy_for_process` + `validate_args` + `ExecutionPolicy.confirmation`
/// + `allow_network`). Errors are surfaced so the decider can treat them as
/// fail-closed (decline); the port MUST NOT itself paper over an error as
/// `NoMatch` if it cannot distinguish them.
#[async_trait]
pub trait ApprovalPolicyPort: Send + Sync {
    /// Resolve the verdict for `process_name` with `args`. A returned `Err`
    /// (any reason) is treated by the decider as FAIL-CLOSED (decline).
    ///
    /// # Errors
    /// Implementation-defined; the decider declines on any error.
    async fn verdict_for(
        &self,
        process_name: &str,
        args: &[String],
    ) -> Result<PolicyVerdict, String>;
}

/// The context handed to the UI hook for an UNMATCHED / confirm-required
/// approval. Carries only non-secret, render-safe fields.
///
/// `Serialize` (camelCase) so the real Tauri UI hook (`CodexUiApprovalHook`,
/// E21 #5044) can emit this struct verbatim as the `codex:approval-request`
/// event payload. SECURITY: the render fields below are render-SAFE summaries
/// only (process name, argv tail, diff LINE COUNT, network host) — never file
/// contents, secrets, or the raw diff. The eventual decision payload still
/// carries ONLY the verdict (see the module-level invariant note).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiApprovalContext {
    /// The originating request id (for correlating the eventual UI answer).
    pub request_id: u64,
    /// A short human label of what is being approved (e.g. `command: rm file`).
    pub summary: String,
    /// The approval kind tag (`command` / `file_change`).
    pub kind: String,
    /// The program name for a command approval (e.g. `git`). `None` for
    /// file-change / unknown kinds.
    #[serde(default)]
    pub process_name: Option<String>,
    /// The argv tail (args after the program). Empty for non-command kinds.
    #[serde(default)]
    pub args: Vec<String>,
    /// The number of diff lines for a file-change approval. `None` for command
    /// kinds. A COUNT only — never the diff body (no file contents leak).
    #[serde(default)]
    pub diff_line_count: Option<usize>,
    /// The network host for a network-grant approval. Currently always `None`
    /// in this slice: network is DEFAULT-DENY *before* UI escalation (the
    /// decider declines a network-context request rather than escalating it),
    /// so UI approval never grants network. Kept for forward-compat with a
    /// future network-grant modal panel.
    #[serde(default)]
    pub network_host: Option<String>,
}

impl UiApprovalContext {
    /// Construct a minimal context with no render detail (back-compat helper for
    /// the fail-closed default hook and tests that only assert the verdict path).
    #[must_use]
    pub fn summary_only(request_id: u64, summary: String, kind: String) -> Self {
        Self {
            request_id,
            summary,
            kind,
            process_name: None,
            args: Vec::new(),
            diff_line_count: None,
            network_host: None,
        }
    }
}

/// Outbound port: escalate an unmatched/confirm-required approval to a UI
/// confirmation, returning a channel that resolves to the user's answer
/// (`true` = approve, `false` = decline). Mirrors
/// `AutomationController::request_confirmation`'s oneshot + timeout shape.
///
/// The DEFAULT implementation ([`DefaultDenyUiHook`]) resolves `false`
/// immediately — the system is FAIL-CLOSED and fully functional headless until
/// the frontend modal wiring (FU-A) replaces it with a real Tauri hook.
#[async_trait]
pub trait UiApprovalHook: Send + Sync {
    /// Request a UI confirmation. Returns a `oneshot::Receiver<bool>` that
    /// resolves to the user's decision. A dropped sender (channel closed) is
    /// treated by the decider as FAIL-CLOSED (decline).
    async fn request_approval(&self, ctx: UiApprovalContext) -> oneshot::Receiver<bool>;
}

/// Fail-closed default UI hook: every escalation auto-declines. Shipping this as
/// the default keeps the whole approval loop functional and SAFE headless (no
/// GUI), with the real modal wiring deferred to FU-A as a purely additive swap.
#[derive(Debug, Default, Clone, Copy)]
pub struct DefaultDenyUiHook;

#[async_trait]
impl UiApprovalHook for DefaultDenyUiHook {
    async fn request_approval(&self, _ctx: UiApprovalContext) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        // Resolve immediately to a decline — UNMATCHED command approvals are
        // declined (the turn proceeds without the action) rather than silently
        // accepted. This is the fail-closed invariant.
        let _ = tx.send(false);
        rx
    }
}

/// The verdict the decider returns for an approval request. Deliberately has NO
/// `Default` and NO `accept` fallback — `Accept` is reachable ONLY from the two
/// explicit success branches in [`CodexApprovalDecider::decide`]. Any mutation
/// that flips the catch-all to `Accept` must edit those branches and will trip a
/// fail-closed test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Accept,
    Decline,
}

/// The default UI escalation timeout (mirrors `AutomationController`'s 30s
/// confirmation budget). A UI that never answers within this window → decline.
pub const DEFAULT_UI_APPROVAL_TIMEOUT: Duration = Duration::from_secs(30);
