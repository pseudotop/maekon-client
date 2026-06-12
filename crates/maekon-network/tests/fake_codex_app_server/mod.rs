//! Reusable JSONL-stdio mock `codex app-server` for integration tests (E21 #4872).
//!
//! The Codex app-server is a SUBPROCESS that speaks JSON-RPC 2.0 over
//! newline-delimited JSON (JSONL) on stdio (the `"jsonrpc"` envelope field is
//! omitted on the wire). The production `AppServerProcess::connect` /
//! `CodexAppServerSession::connect` / `::resume` all take a
//! `tokio::process::Command` by value and drive the real
//! spawn -> setpgid -> JSONL-wire -> `initialize` -> `thread/*` -> `turn/start`
//! chain. So a deterministic mock cannot be an in-process HTTP server (mockito)
//! nor the in-process `tokio::io::duplex` the inline unit tests use — it MUST be
//! a spawnable `Command` whose stdio scripts the dialog.
//!
//! This builder consolidates the EXACT `sh -c` dialect already proven by the
//! inline fakes (`fake_app_server`, `_rich`, `_stateful`, `_resume_drift` in
//! `codex_app_server_session.rs`, and the reap/connect fakes in
//! `codex_app_server.rs`): a read-line loop, `sed`-extracted `id`/`threadId`,
//! append-to-`$MAEKON_FAKE_LOG`/`$MAEKON_REQ_LOG` statefulness, and `printf`ed
//! notifications. It mirrors the in-repo harness precedent
//! `tests/fake_integration_server/mod.rs`, but yields a `Command` instead of an
//! axum `Router`.
//!
//! Manual mock only — no mockall (ADR-001 §5). Unix-only (matches every existing
//! codex fake/test, which are `#[cfg(all(test, unix))]` / `#[cfg(unix)]`; the
//! process-group reap path is unix-only). NO real network egress — stdio only.

#![cfg(unix)]

use std::path::{Path, PathBuf};

/// How the mock answers `thread/resume` — drives the continuity-guard tests.
#[derive(Debug, Clone)]
pub enum ResumeBehavior {
    /// Echo back the requested `threadId` (continuity preserved). Default.
    Echo,
    /// Always return this FIXED, different `threadId` (continuity lost).
    Drift(String),
    /// Reply with a success result but OMIT `threadId` (defensive "absence ==
    /// match" path in production `resume`).
    OmitThreadId,
}

/// How the mock answers the read-only `account/read` request — drives the E21
/// #4868 Part 1 structured-auth-probe tests. `None` (the default) emits no
/// `account/read` arm at all, so a test that never probes auth is unaffected.
#[derive(Debug, Clone)]
pub enum AccountReadBehavior {
    /// No `account/read` case arm (default).
    None,
    /// `{account:{type:"chatgpt", email, planType}, requiresOpenaiAuth:true}`.
    /// The email is included to PROVE the production probe never logs it.
    Chatgpt {
        email: &'static str,
        plan_type: &'static str,
    },
    /// `{account:{type:"apiKey"}, requiresOpenaiAuth:true}`.
    ApiKey,
    /// `{account:null, requiresOpenaiAuth:true}` — logged out.
    NotLoggedIn,
    /// `{account:null, requiresOpenaiAuth:false}` — OSS/local, no auth needed.
    NoAuthNeeded,
    /// `{account:{type:"amazonBedrock"}, requiresOpenaiAuth:true}`.
    AmazonBedrock,
    /// A malformed/unexpected result (`{account:{type:"futuretype"}}`).
    UnknownShape,
    /// A JSON-RPC error reply to `account/read`.
    RpcError { code: i64, msg: &'static str },
}

/// How the mock answers the `initialize` handshake — drives the R4 / #4871
/// fallback-input path tests.
#[derive(Debug, Clone)]
pub enum Handshake {
    /// Normal success: reply `{result:{userAgent:<configured>, ...}}`. Default.
    Ok,
    /// Reply a JSON-RPC error to `initialize` (broken handshake -> the input
    /// `create_codex_session_with_fallback` (#4871) degrades to `codex exec` on).
    RpcError { code: i64, msg: String },
    /// Inject EXTRA unknown sibling fields into the `initialize` result, to prove
    /// forward-compat raw retention (production `ServerInfo.raw`).
    OkWithExtraFields,
}

/// One scripted streaming step emitted (in order) after the `turn/start` result.
#[derive(Debug, Clone)]
pub enum TurnStep {
    /// `item/agentReasoning/delta` -> `OutboundMessage::Thinking`.
    Reasoning(&'static str),
    /// `item/agentMessage/delta` -> `OutboundMessage::Text`.
    Answer(&'static str),
    /// A tool-internal event the production drain IGNORES (not chat output).
    /// `method` is the JSON-RPC method to emit (e.g. `item/started`).
    ToolInternal(&'static str),
    /// An unrecognized notification the production drain IGNORES gracefully.
    Unknown(&'static str),
    /// `turn/completed` -> terminal `OutboundMessage::Result`. `usage` is an
    /// optional `(input_tokens, output_tokens)` pair surfaced as token usage.
    Completed { usage: Option<(u64, u64)> },
    /// v2 `thread/tokenUsage/updated` (#5071) — real codex carries per-turn usage
    /// here (in `tokenUsage.last`), NOT on `turn/completed`. The client captures
    /// it and attaches it to the terminal Result.
    TokenUsage {
        input_tokens: u64,
        output_tokens: u64,
    },
    /// A server→client `requestApproval` REQUEST (#4870), emitted mid-turn as an
    /// id-bearing line (NOT a notification). The client's reverse-request path
    /// must classify it as a `Request`, run the approval decider, and write a
    /// correlated RESPONSE line back; with `$MAEKON_REQ_LOG` set the test can
    /// then assert that `{"id":<approval_id>,"result":{...}}` round-trip.
    ApprovalRequest {
        /// The JSON-RPC method (e.g. `item/commandExecution/requestApproval`).
        method: &'static str,
        /// The server-chosen request id (distinct from the client's request ids).
        approval_id: u64,
        /// The `command` string in params (drives dangerous/policy cases).
        command: &'static str,
        /// When true, params carry a `networkApprovalContext` (default-deny case).
        with_network: bool,
    },
}

/// Builder for a deterministic JSONL-stdio mock `codex app-server`.
///
/// Defaults model the happy path: a `codex-app-server/1.0` userAgent, thread id
/// `thr_42`, `ResumeBehavior::Echo`, and a single-answer turn (`Answer("Hello")`
/// then `Completed{None}`). Override fields via the builder methods, then call
/// [`into_command`](Self::into_command) and hand the `Command` to the REAL
/// `AppServerProcess`/`CodexAppServerSession` constructors.
pub struct FakeCodexAppServer {
    /// `result.userAgent` for `initialize`. `None` omits the field entirely.
    user_agent: Option<String>,
    /// The `threadId` returned by `thread/start`.
    thread_id: String,
    /// How `thread/resume` answers.
    resume_behavior: ResumeBehavior,
    /// Scripted streaming steps for each `turn/start`.
    turn: Vec<TurnStep>,
    /// When set, every `turn/start` request line is appended here (proves
    /// thread reuse + no-history-resend across turns).
    turn_log: Option<PathBuf>,
    /// When set, EVERY inbound request/notification line is appended here
    /// (the R4 outbound-contract drift detector reads it).
    request_log: Option<PathBuf>,
    /// When `Some`, the `turn/start` result carries `{turn:{id:<this>}}` so a
    /// test can exercise the E21 #5017 turn/steer + turn/interrupt round-trip
    /// (the client captures this id and targets it). `None` (default) keeps the
    /// legacy `{result:{}}` shape (no turn id surfaced).
    turn_id: Option<String>,
    /// How `initialize` answers.
    handshake: Handshake,
    /// How the read-only `account/read` request answers (E21 #4868 Part 1).
    account_read: AccountReadBehavior,
    /// The line printed for a `--version` probe when the fake is materialized as
    /// a standalone executable (E21 #4863 R7). `Some(line)` → print `line` and
    /// exit 0; `None` → exit non-zero with no output (an "unprobeable" binary
    /// that the factory must treat as untrusted → exec fallback).
    version_line: Option<String>,
}

impl Default for FakeCodexAppServer {
    fn default() -> Self {
        Self {
            user_agent: Some("codex-app-server/1.0".to_string()),
            thread_id: "thr_42".to_string(),
            resume_behavior: ResumeBehavior::Echo,
            turn: vec![
                TurnStep::Answer("Hello"),
                TurnStep::Completed { usage: None },
            ],
            turn_log: None,
            request_log: None,
            turn_id: None,
            handshake: Handshake::Ok,
            account_read: AccountReadBehavior::None,
            version_line: Some("codex-cli 1.2.3".to_string()),
        }
    }
}

impl FakeCodexAppServer {
    /// A fresh builder with happy-path defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set (or, with `None`, omit) the `initialize` `result.userAgent`.
    pub fn user_agent(mut self, user_agent: Option<&str>) -> Self {
        self.user_agent = user_agent.map(str::to_string);
        self
    }

    /// Set the `threadId` returned by `thread/start`.
    pub fn thread_id(mut self, thread_id: &str) -> Self {
        self.thread_id = thread_id.to_string();
        self
    }

    /// Set how `thread/resume` answers.
    pub fn resume(mut self, behavior: ResumeBehavior) -> Self {
        self.resume_behavior = behavior;
        self
    }

    /// Set the scripted streaming steps for each `turn/start`.
    pub fn turn(mut self, steps: Vec<TurnStep>) -> Self {
        self.turn = steps;
        self
    }

    /// Append every `turn/start` request line to `path` (thread-reuse /
    /// no-resend proof).
    pub fn with_turn_log(mut self, path: PathBuf) -> Self {
        self.turn_log = Some(path);
        self
    }

    /// Append every inbound request/notification line to `path` (R4 outbound
    /// wire-contract drift detector).
    pub fn with_request_log(mut self, path: PathBuf) -> Self {
        self.request_log = Some(path);
        self
    }

    /// Make `turn/start` return a turn id (`{result:{turn:{id:<id>}}}`) so the
    /// E21 #5017 turn/steer + turn/interrupt path has a live turn to target.
    pub fn turn_id(mut self, id: &str) -> Self {
        self.turn_id = Some(id.to_string());
        self
    }

    /// Set how `initialize` answers (Ok / RpcError / OkWithExtraFields).
    pub fn handshake(mut self, handshake: Handshake) -> Self {
        self.handshake = handshake;
        self
    }

    /// Set how the read-only `account/read` request answers (E21 #4868 Part 1).
    pub fn account_read(mut self, behavior: AccountReadBehavior) -> Self {
        self.account_read = behavior;
        self
    }

    /// Configure the `--version` probe answer for the executable form (E21
    /// #4863 R7). `Some(line)` prints `line` (the factory's lenient extractor
    /// pulls a `MAJOR.MINOR.PATCH`/`MAJOR.MINOR` token from it) and exits 0.
    /// `None` makes the binary "unprobeable" (no output, exit 1) — the factory
    /// must then treat it as untrusted and degrade to `codex exec`.
    pub fn version_line(mut self, line: Option<&str>) -> Self {
        self.version_line = line.map(str::to_string);
        self
    }

    /// Build the spawnable `tokio::process::Command` (`sh -c <script>`).
    ///
    /// The generated script reproduces the verified inline `sh` dialect: a
    /// read-line loop that `sed`-extracts the request `id` (and, for resume, the
    /// requested `threadId`), optionally tees lines to the request/turn logs,
    /// then branches on `initialize` / `thread/start` / `thread/resume` /
    /// `turn/start`.
    pub fn into_command(self) -> tokio::process::Command {
        let mut cmd = tokio::process::Command::new("sh");

        // Wire the optional logs through the environment exactly like the inline
        // `fake_app_server_stateful` fake (`$MAEKON_FAKE_LOG`); add a sibling
        // `$MAEKON_REQ_LOG` for the full request capture.
        if let Some(turn_log) = self.turn_log.as_ref() {
            cmd.env("MAEKON_FAKE_LOG", turn_log);
        }
        if let Some(request_log) = self.request_log.as_ref() {
            cmd.env("MAEKON_REQ_LOG", request_log);
        }

        cmd.arg("-c").arg(self.build_script());
        cmd
    }

    /// Materialize the fake as a STANDALONE executable script at `path` (E21
    /// #4863 R7 wired test seam). Unlike [`into_command`] — which yields a
    /// pre-built `sh -c <script>` whose argv the spawner cannot influence — the
    /// factory spawns the resolved `executable_path` directly with its own args
    /// (`--version` for the probe, `app-server` for the JSON-RPC dialog). So the
    /// wired test needs a real on-disk binary that branches on `$1`:
    ///   * `--version`   → print the configured version line (or exit 1 when
    ///     `version_line` is `None`, modeling an unprobeable binary), then exit;
    ///   * `app-server`  → run the existing JSONL read-loop dialog.
    ///
    /// Returns the path (chmod +x'd). The log env vars (`MAEKON_FAKE_LOG` /
    /// `MAEKON_REQ_LOG`) are baked into the script via `: "${VAR:=...}"` so they
    /// survive even though the factory builds the `Command` (not this harness).
    pub fn write_executable(&self, path: &Path) -> std::io::Result<PathBuf> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let version_branch = match &self.version_line {
            Some(line) => format!("printf '%s\\n' '{line}'; exit 0"),
            None => "exit 1".to_string(),
        };
        // Bake the optional log paths so the dialog's tee still works when the
        // factory (not this harness) constructs the spawn Command.
        let mut env_preamble = String::new();
        if let Some(turn_log) = self.turn_log.as_ref() {
            env_preamble.push_str(&format!(
                "export MAEKON_FAKE_LOG='{}'\n",
                turn_log.display()
            ));
        }
        if let Some(request_log) = self.request_log.as_ref() {
            env_preamble.push_str(&format!(
                "export MAEKON_REQ_LOG='{}'\n",
                request_log.display()
            ));
        }
        let dialog = self.build_script();
        let script = format!(
            "#!/bin/sh\n{env_preamble}case \"$1\" in\n  --version) {version_branch} ;;\n  app-server) {dialog} ;;\n  *) {dialog} ;;\nesac\n"
        );

        // Write and CLOSE the file before chmod+return. An open write fd at
        // exec time makes the kernel reject the exec with ETXTBSY ("Text file
        // busy") — a parallel-test race observed on the linux runner. Scope the
        // File so its fd is dropped, and `sync_all` so the bytes are on disk
        // before the caller spawns it.
        {
            let mut file = std::fs::File::create(path)?;
            file.write_all(script.as_bytes())?;
            file.sync_all()?;
        }
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
        Ok(path.to_path_buf())
    }

    /// Compose the `sh` program text from the configured behavior.
    fn build_script(&self) -> String {
        // ── initialize branch ───────────────────────────────────────────────
        let initialize_branch = match &self.handshake {
            Handshake::Ok => format!(
                "printf '{{\"id\":%s,\"result\":{{{}}}}}\\n' \"$id\" ;;",
                self.user_agent_result_body()
            ),
            Handshake::OkWithExtraFields => format!(
                // A well-formed success with EXTRA unknown sibling fields, to
                // prove production `ServerInfo.raw` retains them (forward-compat).
                "printf '{{\"id\":%s,\"result\":{{{}{}\"futureField\":{{\"x\":1}}}}}}\\n' \"$id\" ;;",
                self.user_agent_result_body(),
                if self.user_agent.is_some() { "," } else { "" },
            ),
            Handshake::RpcError { code, msg } => format!(
                "printf '{{\"id\":%s,\"error\":{{\"code\":{code},\"message\":\"{msg}\"}}}}\\n' \"$id\" ;;"
            ),
        };

        // ── thread/resume branch ────────────────────────────────────────────
        let resume_branch = match &self.resume_behavior {
            ResumeBehavior::Echo => {
                // Echo the requested threadId back (continuity preserved).
                "tid=$(printf '%s' \"$line\" | sed -n 's/.*\"threadId\":\"\\([^\"]*\\)\".*/\\1/p'); \
                 printf '{\"id\":%s,\"result\":{\"threadId\":\"%s\"}}\\n' \"$id\" \"$tid\" ;;"
                    .to_string()
            }
            ResumeBehavior::Drift(other) => format!(
                // Always answer a DIFFERENT threadId regardless of request.
                "printf '{{\"id\":%s,\"result\":{{\"threadId\":\"{other}\"}}}}\\n' \"$id\" ;;"
            ),
            ResumeBehavior::OmitThreadId => {
                // Success with no echoed threadId (defensive "absence == match").
                "printf '{\"id\":%s,\"result\":{}}\\n' \"$id\" ;;".to_string()
            }
        };

        // ── turn/start branch (result + scripted notifications) ─────────────
        let turn_branch = self.turn_start_branch();

        // ── account/read branch (E21 #4868 Part 1) ──────────────────────────
        let account_read_arm = match self.account_read_branch() {
            Some(branch) => format!("    *'\"account/read\"'*) {branch}\n"),
            None => String::new(),
        };

        // The case-arm patterns mirror the inline fakes verbatim (substring match
        // on the quoted method token).
        format!(
            r#"while IFS= read -r line; do
  {req_log}
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) {initialize_branch}
    *'"thread/resume"'*) {resume_branch}
    *'"thread/start"'*) printf '{{"id":%s,"result":{{"threadId":"{thread_id}"}}}}\n' "$id" ;;
    *'"turn/interrupt"'*) printf '{{"id":%s,"result":{{}}}}\n' "$id" ;;
    *'"turn/steer"'*) printf '{{"id":%s,"result":{{}}}}\n' "$id" ;;
    *'"turn/start"'*) {turn_branch}
{account_read_arm}  esac
done"#,
            // Tee every inbound line to $MAEKON_REQ_LOG when requested. Quoted so a
            // missing env var simply yields an empty path -> the test never sets
            // the log and this is skipped.
            req_log = if self.request_log.is_some() {
                r#"if [ -n "$MAEKON_REQ_LOG" ]; then printf '%s\n' "$line" >> "$MAEKON_REQ_LOG"; fi"#
            } else {
                ":"
            },
            thread_id = self.thread_id,
            account_read_arm = account_read_arm,
        )
    }

    /// Render the `account/read` case-arm body per the configured behavior, or
    /// `None` to omit the arm entirely (E21 #4868 Part 1). The `printf` dialect
    /// mirrors `user_agent_result_body`: PLAIN `"` quotes inside the
    /// single-quoted format string — never `\"`. bash's printf collapses `\"`
    /// to `"`, but dash (`/bin/sh` on the ubuntu runners) emits the backslash
    /// verbatim, producing unparseable JSON and a 120 s handshake timeout.
    fn account_read_branch(&self) -> Option<String> {
        let result_or_error = match &self.account_read {
            AccountReadBehavior::None => return None,
            AccountReadBehavior::Chatgpt { email, plan_type } => format!(
                "\"account\":{{\"type\":\"chatgpt\",\"email\":\"{email}\",\"planType\":\"{plan_type}\"}},\"requiresOpenaiAuth\":true"
            ),
            AccountReadBehavior::ApiKey => {
                "\"account\":{\"type\":\"apiKey\"},\"requiresOpenaiAuth\":true".to_string()
            }
            AccountReadBehavior::NotLoggedIn => {
                "\"account\":null,\"requiresOpenaiAuth\":true".to_string()
            }
            AccountReadBehavior::NoAuthNeeded => {
                "\"account\":null,\"requiresOpenaiAuth\":false".to_string()
            }
            AccountReadBehavior::AmazonBedrock => {
                "\"account\":{\"type\":\"amazonBedrock\"},\"requiresOpenaiAuth\":true".to_string()
            }
            AccountReadBehavior::UnknownShape => {
                "\"account\":{\"type\":\"futuretype\"}".to_string()
            }
            AccountReadBehavior::RpcError { code, msg } => {
                return Some(format!(
                    "printf '{{\"id\":%s,\"error\":{{\"code\":{code},\"message\":\"{msg}\"}}}}\\n' \"$id\" ;;"
                ));
            }
        };
        Some(format!(
            "printf '{{\"id\":%s,\"result\":{{{result_or_error}}}}}\\n' \"$id\" ;;"
        ))
    }

    /// Render the `initialize` `result` body fragment carrying (or omitting) the
    /// userAgent, WITHOUT the surrounding braces — callers wrap it.
    fn user_agent_result_body(&self) -> String {
        match self.user_agent.as_deref() {
            Some(ua) => format!("\"userAgent\":\"{ua}\""),
            // No userAgent key at all -> production parses `None` and still Ok.
            None => String::new(),
        }
    }

    /// Render the `turn/start` arm: reply the request result, optionally append
    /// the request line to `$MAEKON_FAKE_LOG`, then `printf` each scripted step.
    fn turn_start_branch(&self) -> String {
        let mut parts = Vec::new();

        // Append the turn/start request line to the turn log first (matches the
        // inline `fake_app_server_stateful`), so a test can assert thread reuse +
        // that no prior-turn history was replayed.
        if self.turn_log.is_some() {
            parts.push(r#"printf '%s\n' "$line" >> "$MAEKON_FAKE_LOG""#.to_string());
        }

        // Acknowledge the turn/start request. When a turn id is configured (E21
        // #5017), embed it as `{result:{turn:{id:<id>}}}` so the client captures
        // a live turn to target with turn/steer + turn/interrupt.
        match self.turn_id.as_deref() {
            Some(turn_id) => parts.push(format!(
                "printf '{{\"id\":%s,\"result\":{{\"turn\":{{\"id\":\"{turn_id}\"}}}}}}\\n' \"$id\""
            )),
            None => parts.push("printf '{\"id\":%s,\"result\":{}}\\n' \"$id\"".to_string()),
        }

        // Emit each scripted streaming notification in order.
        for step in &self.turn {
            parts.push(Self::step_printf(step));
        }

        // `;;` terminates the case arm.
        format!("{} ;;", parts.join("; "))
    }

    /// Render one streaming step as a `printf` of its JSONL notification line.
    fn step_printf(step: &TurnStep) -> String {
        match step {
            TurnStep::Reasoning(text) => format!(
                "printf '{{\"method\":\"item/agentReasoning/delta\",\"params\":{{\"text\":\"{text}\"}}}}\\n'"
            ),
            TurnStep::Answer(text) => format!(
                "printf '{{\"method\":\"item/agentMessage/delta\",\"params\":{{\"text\":\"{text}\"}}}}\\n'"
            ),
            TurnStep::ToolInternal(method) => format!(
                "printf '{{\"method\":\"{method}\",\"params\":{{}}}}\\n'"
            ),
            TurnStep::Unknown(method) => format!(
                "printf '{{\"method\":\"{method}\",\"params\":{{\"x\":1}}}}\\n'"
            ),
            TurnStep::ApprovalRequest {
                method,
                approval_id,
                command,
                with_network,
            } => {
                // #4870: emit an id-bearing server→client REQUEST. The client's
                // reverse-request path classifies it (id + method, no
                // result/error) as a Request, decides, and writes a correlated
                // response back (which the read-loop tee logs to $MAEKON_REQ_LOG).
                let network = if *with_network {
                    ",\"networkApprovalContext\":{}"
                } else {
                    ""
                };
                format!(
                    "printf '{{\"id\":{approval_id},\"method\":\"{method}\",\"params\":{{\"command\":\"{command}\",\"cwd\":\"/tmp\",\"reason\":\"test\"{network}}}}}\\n'"
                )
            }
            TurnStep::TokenUsage {
                input_tokens,
                output_tokens,
            } => format!(
                "printf '{{\"method\":\"thread/tokenUsage/updated\",\"params\":{{\"threadId\":\"t\",\"turnId\":\"u\",\"tokenUsage\":{{\"last\":{{\"inputTokens\":{input_tokens},\"outputTokens\":{output_tokens},\"totalTokens\":0,\"cachedInputTokens\":0,\"reasoningOutputTokens\":0}}}}}}}}\\n'"
            ),
            TurnStep::Completed { usage } => match usage {
                Some((input, output)) => format!(
                    "printf '{{\"method\":\"turn/completed\",\"params\":{{\"usage\":{{\"inputTokens\":{input},\"outputTokens\":{output}}}}}}}\\n'"
                ),
                None => {
                    "printf '{\"method\":\"turn/completed\",\"params\":{}}\\n'".to_string()
                }
            },
        }
    }
}

/// A unique per-test temp path for a turn-log, so cargo's parallel test runner
/// never collides two mocks on one file. Mirrors the inline `unique_log_path`.
pub fn unique_turn_log(test_name: &str) -> PathBuf {
    unique_path("turn", test_name)
}

/// A unique per-test temp path for a request-log (R4 wire-contract capture).
pub fn unique_request_log(test_name: &str) -> PathBuf {
    unique_path("req", test_name)
}

/// Shared temp-path builder keyed on a kind + test name + this process's pid,
/// removing any stale file first.
fn unique_path(kind: &str, test_name: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "maekon_fake_codex_{}_{}_{}.log",
        kind,
        test_name,
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// Read a log file written by the mock, asserting it exists.
pub fn read_log(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("log {path:?} not written: {e}"))
}

/// Dash-safety invariant for the generated `sh` dialog (fix for the 30-test
/// 120 s handshake-timeout cascade on the ubuntu runners): the printf format
/// strings are single-quoted, so JSON quotes must be PLAIN `"`. A `\"` escape
/// is implementation-defined — bash collapses it to `"` (masking the bug on
/// macOS, where `sh` is bash), dash prints the backslash verbatim and the
/// client can never parse a response.
#[test]
fn generated_script_uses_plain_quotes_not_backslash_escapes() {
    let script = FakeCodexAppServer::new()
        .user_agent(Some("codex-app-server/7.7"))
        .account_read(AccountReadBehavior::Chatgpt {
            email: "user@example.com",
            plan_type: "plus",
        })
        .build_script();
    assert!(
        !script.contains(r#"\""#),
        "fake app-server script must not rely on printf `\\\"` escapes (dash emits them \
         verbatim, breaking the JSON wire): {script}"
    );
}
