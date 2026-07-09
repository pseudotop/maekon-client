use std::io::Read;
use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use maekon_api_contracts::provider_specs::{subprocess_auth_probe_mode, SubprocessAuthProbeMode};
// #5032: the codex `app-server` JSON-RPC probe (`AppServerProcess`) lives in
// `maekon-network`, only compiled when `analysis` pulls in `dep:maekon-network`.
// `block_on_account_read` is gated on `analysis`; the `probe_codex_account_read`
// dispatcher has a `not(analysis)` body returning `Unknown` so this file compiles
// under `--no-default-features`. No behaviour change when `analysis` is enabled.
#[cfg(feature = "analysis")]
use maekon_network::codex_app_server::{AppServerProcess, ClientInfo};

use super::{
    catalog_subprocess_transport, truncate_for_error, DetectedSubprocessCli, ProbedSubprocessCli,
    SubprocessCliAuthStatus, CLI_AUTH_PROBE_TIMEOUT_SECS,
};
// #5032: only used by the `analysis`-gated `probe_codex_account_read` timeout.
#[cfg(feature = "analysis")]
use super::CLI_APP_SERVER_AUTH_PROBE_TIMEOUT_SECS;
use tracing::debug;

#[derive(Clone, Copy)]
pub(super) struct SubprocessAuthProbeRuntime {
    pub(super) probe: fn(&Path, &[String]) -> (SubprocessCliAuthStatus, Option<String>),
}

pub(super) fn auth_probe_mode_for_surface(
    surface_id: &str,
) -> Result<SubprocessAuthProbeMode, String> {
    subprocess_auth_probe_mode(surface_id)
}

fn auth_probe_command_for_surface(surface_id: &str) -> Result<Vec<String>, String> {
    Ok(catalog_subprocess_transport(surface_id)?
        .auth_probe_command
        .clone())
}

/// The launch args the account/read probe uses to spawn `codex app-server`.
///
/// The fn-pointer probe runtime contract is `fn(&Path, &[String])` and carries no
/// `surface_id`, so the catalog `app_server_args` (e.g. `["app-server"]`) are
/// resolved here and threaded through the existing args vector for the
/// [`SubprocessAuthProbeMode::CodexAccountReadJson`] mode. This keeps the probe
/// catalog-driven without widening the runtime signature.
fn app_server_args_for_surface(surface_id: &str) -> Result<Vec<String>, String> {
    Ok(catalog_subprocess_transport(surface_id)?
        .app_server_args
        .clone())
}

/// The args a probe runtime receives depend on its mode: the structured
/// account/read probe needs the app-server launch args; the legacy text/JSON
/// probes need the configured `auth_probe_command`.
fn probe_args_for_surface(
    surface_id: &str,
    mode: SubprocessAuthProbeMode,
) -> Result<Vec<String>, String> {
    match mode {
        SubprocessAuthProbeMode::CodexAccountReadJson => app_server_args_for_surface(surface_id),
        _ => auth_probe_command_for_surface(surface_id),
    }
}

fn auth_probe_runtime_for_mode(
    mode: SubprocessAuthProbeMode,
) -> Option<SubprocessAuthProbeRuntime> {
    match mode {
        SubprocessAuthProbeMode::CodexLoginStatusText => Some(SubprocessAuthProbeRuntime {
            probe: probe_codex_auth_status,
        }),
        SubprocessAuthProbeMode::ClaudeAuthStatusJson => Some(SubprocessAuthProbeRuntime {
            probe: probe_claude_auth_status,
        }),
        // E21 #4868 Part 1: structured read-only account/read probe for the codex
        // app-server surface.
        SubprocessAuthProbeMode::CodexAccountReadJson => Some(SubprocessAuthProbeRuntime {
            probe: probe_codex_account_read,
        }),
        SubprocessAuthProbeMode::None => None,
    }
}

pub(super) fn auth_probe_runtime_for_surface(
    surface_id: &str,
) -> Result<Option<SubprocessAuthProbeRuntime>, String> {
    auth_probe_mode_for_surface(surface_id).map(auth_probe_runtime_for_mode)
}

pub(super) fn probe_cli_surface(detected: DetectedSubprocessCli) -> ProbedSubprocessCli {
    let mode = auth_probe_mode_for_surface(&detected.surface_id);
    let (auth_status, auth_detail) = match auth_probe_runtime_for_surface(&detected.surface_id) {
        Ok(Some(runtime)) => {
            let probe_args = mode
                .ok()
                .and_then(|mode| probe_args_for_surface(&detected.surface_id, mode).ok())
                .unwrap_or_default();
            (runtime.probe)(&detected.executable_path, &probe_args)
        }
        Ok(None) => (
            SubprocessCliAuthStatus::Unknown,
            Some("auth_status_probe_not_implemented".to_string()),
        ),
        Err(error) => (
            SubprocessCliAuthStatus::Unknown,
            Some(format!("probe_spec_error:{error}")),
        ),
    };

    ProbedSubprocessCli {
        detected,
        auth_status,
        auth_detail,
    }
}

fn probe_codex_auth_status(
    executable_path: &Path,
    args: &[String],
) -> (SubprocessCliAuthStatus, Option<String>) {
    let output = match run_probe_command_with_timeout(
        executable_path,
        args,
        Duration::from_secs(CLI_AUTH_PROBE_TIMEOUT_SECS),
    ) {
        Ok(output) => output,
        Err(detail) => return (SubprocessCliAuthStatus::Unknown, Some(detail)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");
    if !output.status.success() {
        if let Some(classified) = classify_auth_status_text(&combined) {
            return classified;
        }
        return (
            SubprocessCliAuthStatus::Unknown,
            Some(nonzero_probe_detail(output.status.code(), &combined)),
        );
    }
    parse_codex_auth_status(&combined)
}

pub(super) fn parse_codex_auth_status(raw: &str) -> (SubprocessCliAuthStatus, Option<String>) {
    let normalized = raw.trim();
    if let Some(classified) = classify_auth_status_text(normalized) {
        return classified;
    }

    (
        SubprocessCliAuthStatus::Unknown,
        Some(format!(
            "unexpected_status_output:{}",
            truncate_for_error(&sanitize_auth_probe_output(normalized))
        )),
    )
}

// ── Codex app-server structured auth probe (E21 #4868 Part 1) ────────────────

/// Sync probe for the codex app-server surface: spawns `codex app-server`, runs
/// the `initialize` handshake, and issues a single read-only `account/read`
/// JSON-RPC request, then maps the structured response to a
/// [`SubprocessCliAuthStatus`].
///
/// `app_server_args` (the catalog `["app-server"]`) arrive via `args` — see
/// [`probe_args_for_surface`]. This NEVER touches the ChatGPT OAuth token; the
/// CLI owns OAuth and `account/read` is the ADR-025-blessed READ-ONLY path. Every
/// failure (runtime build, connect/handshake error, request error, timeout,
/// unrecognized shape) degrades to `Unknown` with a sanitized detail; it never
/// panics and never flips the surface to a wrong authenticated/unauthenticated
/// verdict.
#[cfg(feature = "analysis")]
fn probe_codex_account_read(
    executable_path: &Path,
    args: &[String],
) -> (SubprocessCliAuthStatus, Option<String>) {
    block_on_account_read(
        executable_path,
        args,
        Duration::from_secs(CLI_APP_SERVER_AUTH_PROBE_TIMEOUT_SECS),
    )
}

/// #5032: `not(analysis)` fallback. The `account/read` probe needs the
/// `maekon-network` app-server JSON-RPC client, which is not compiled in this
/// build. Degrade to `Unknown` — exactly how every real failure path of the
/// `analysis` build (runtime build / connect / timeout error) already reports —
/// so surface selection treats it as "auth status not verifiable" rather than a
/// wrong authenticated/unauthenticated verdict.
#[cfg(not(feature = "analysis"))]
fn probe_codex_account_read(
    _executable_path: &Path,
    _args: &[String],
) -> (SubprocessCliAuthStatus, Option<String>) {
    (
        SubprocessCliAuthStatus::Unknown,
        Some("account_read_unavailable_no_analysis_feature".to_string()),
    )
}

/// Bridge the sync fn-pointer probe contract to the async `AppServerProcess` API.
///
/// The three probe call sites run on heterogeneous threads (two `spawn_blocking`
/// workers + one direct sync caller in `build_automation_runtime` with NO ambient
/// runtime). A `block_in_place` + `Handle::block_on` path would PANIC on the
/// no-runtime caller, so — matching the established `main.rs` / telemetry-shutdown
/// precedent — we build a DEDICATED current-thread runtime per probe and
/// `block_on` it. That is valid from ANY thread (we never call
/// `Handle::current()` and never nest on the same runtime), keeping ONE uniform
/// panic-free path across all callers.
#[cfg(feature = "analysis")]
fn block_on_account_read(
    executable_path: &Path,
    args: &[String],
    timeout: Duration,
) -> (SubprocessCliAuthStatus, Option<String>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            return (
                SubprocessCliAuthStatus::Unknown,
                Some(format!("account_read_runtime_error:{err}")),
            );
        }
    };

    let executable_path = executable_path.to_path_buf();
    let args = args.to_vec();

    runtime.block_on(async move {
        let attempt = tokio::time::timeout(timeout, async {
            let mut command = tokio::process::Command::new(&executable_path);
            command.args(&args);
            // `name` MUST be "maekon": the initialize clientInfo records the
            // mechanism for OpenAI usage attribution.
            let info = ClientInfo {
                name: "maekon".to_string(),
                title: "Maekon".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            };
            // Connect runs the initialize handshake. `_notifications` and the
            // inbound-request channel are dropped immediately — account/read is a
            // single request/response and the probe never starts a turn.
            let (process, _notifications, _inbound) =
                AppServerProcess::connect(command, &info).await?;
            // Read-only: refreshToken is NEVER true (true is the ADR-025
            // token-refresh boundary, legal-gated Part 2).
            let result = process
                .request("account/read", serde_json::json!({"refreshToken": false}))
                .await;
            // Drop `process` here (end of block) → reaps the spawned app-server
            // process group even on the error path.
            result
        })
        .await;

        match attempt {
            Ok(Ok(value)) => parse_account_read_status(&value),
            Ok(Err(transport_err)) => (
                SubprocessCliAuthStatus::Unknown,
                Some(format!(
                    "account_read_transport_error:{}",
                    truncate_for_error(&sanitize_auth_probe_output(&format!("{transport_err:?}")))
                )),
            ),
            Err(_elapsed) => (
                SubprocessCliAuthStatus::Unknown,
                Some(format!("account_read_timeout:{}ms", timeout.as_millis())),
            ),
        }
    })
}

/// Map the `account/read` JSON-RPC RESULT to a [`SubprocessCliAuthStatus`].
///
/// Authoritative codex schema (`GetAccountResponse`): `{ account: Account|null,
/// requiresOpenaiAuth: bool }`. We discriminate on the RESPONSE `account.type`
/// (camelCase `"apiKey"`/`"chatgpt"`/`"amazonBedrock"`) — NOT `authMode`, which
/// lives only in the `account/updated` notification we never receive on a single
/// request/response.
///
/// Defensive by construction: codex may add `account.type` values across binary
/// versions and we cannot run `generate-json-schema` against the user's binary,
/// so every unrecognized/absent field maps to `Unknown` (never panics, never
/// unwraps on shape). It MUST NOT call `classify_auth_status_text` — that text
/// classifier maps "subscription"/"plan"/"tier" → Unsupported, which would
/// wrongly gate out an authenticated ChatGPT-subscription user.
///
/// The ChatGPT-vs-API-key distinction (SCOPE item 3) is carried in the detail
/// token (`cli_authenticated_chatgpt` vs `cli_authenticated_apikey`); both map to
/// `Authenticated` so selection/readiness treat them identically.
// #5032: only reached from the `analysis`-gated `block_on_account_read` (and the
// unit tests). `cfg(test)` keeps it compiled for the test module under any
// feature set; otherwise it follows the `analysis` gate so it is not dead code
// under `--no-default-features`.
#[cfg(any(feature = "analysis", test))]
pub(super) fn parse_account_read_status(
    value: &serde_json::Value,
) -> (SubprocessCliAuthStatus, Option<String>) {
    let Some(object) = value.as_object() else {
        return (
            SubprocessCliAuthStatus::Unknown,
            Some("account_read_unexpected_shape".to_string()),
        );
    };

    let account = object.get("account");
    let requires = object
        .get("requiresOpenaiAuth")
        .and_then(serde_json::Value::as_bool);

    // 1) account is an object → discriminate on `type`.
    if let Some(account_object) = account.and_then(serde_json::Value::as_object) {
        let acct_type = account_object
            .get("type")
            .and_then(serde_json::Value::as_str);
        return match acct_type {
            Some("chatgpt") => (
                SubprocessCliAuthStatus::Authenticated,
                // Plan tier is deliberately kept OUT of the gating decision and
                // the detail (email/account.email MUST never be logged).
                Some("cli_authenticated_chatgpt".to_string()),
            ),
            // Accept the canonical camelCase plus a defensive lowercase fallback.
            Some("apiKey") | Some("apikey") => (
                SubprocessCliAuthStatus::Authenticated,
                Some("cli_authenticated_apikey".to_string()),
            ),
            Some("amazonBedrock") => (
                // Explicit map so an unsupported Bedrock account does not silently
                // fall to Unknown (ADR-019 Bedrock non-support).
                SubprocessCliAuthStatus::Unsupported,
                Some("cli_auth_unsupported".to_string()),
            ),
            _ => (
                SubprocessCliAuthStatus::Unknown,
                Some("account_read_unknown_account_type".to_string()),
            ),
        };
    }

    // 2) account is null/absent → combine with requiresOpenaiAuth.
    match requires {
        // OSS/local model: no OpenAI auth needed → the surface is usable.
        Some(false) => (
            SubprocessCliAuthStatus::Authenticated,
            Some("cli_no_auth_required".to_string()),
        ),
        Some(true) => (
            SubprocessCliAuthStatus::Unauthenticated,
            Some("cli_auth_required".to_string()),
        ),
        None => (
            SubprocessCliAuthStatus::Unknown,
            Some("account_read_missing_requires_field".to_string()),
        ),
    }
}

fn probe_claude_auth_status(
    executable_path: &Path,
    args: &[String],
) -> (SubprocessCliAuthStatus, Option<String>) {
    probe_claude_auth_status_with_timeout(
        executable_path,
        args,
        Duration::from_secs(CLI_AUTH_PROBE_TIMEOUT_SECS),
    )
}

fn probe_claude_auth_status_with_timeout(
    executable_path: &Path,
    args: &[String],
    timeout: Duration,
) -> (SubprocessCliAuthStatus, Option<String>) {
    let output = match run_probe_command_with_timeout(executable_path, args, timeout) {
        Ok(output) => output,
        Err(detail) => return (SubprocessCliAuthStatus::Unknown, Some(detail)),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        if let Some(classified) = classify_auth_status_text(&stderr) {
            return classified;
        }
        return (
            SubprocessCliAuthStatus::Unknown,
            Some(nonzero_probe_detail(output.status.code(), &stderr)),
        );
    }
    parse_claude_auth_status(&stdout)
}

pub(super) fn parse_claude_auth_status(raw: &str) -> (SubprocessCliAuthStatus, Option<String>) {
    let normalized = raw.trim();
    let value = match serde_json::from_str::<serde_json::Value>(normalized) {
        Ok(value) => value,
        Err(err) => {
            return (
                SubprocessCliAuthStatus::Unknown,
                Some(format!("invalid_status_json:{err}")),
            );
        }
    };

    let hint_text = auth_hint_text_from_json(&value);
    if let Some(classified) = classify_auth_status_text(&hint_text) {
        return classified;
    }

    match value.get("loggedIn").and_then(|value| value.as_bool()) {
        Some(true) => (
            SubprocessCliAuthStatus::Authenticated,
            Some("cli_authenticated".to_string()),
        ),
        Some(false) => (
            SubprocessCliAuthStatus::Unauthenticated,
            Some("cli_auth_required".to_string()),
        ),
        None => (
            SubprocessCliAuthStatus::Unknown,
            Some("missing_loggedIn_field".to_string()),
        ),
    }
}

fn classify_auth_status_text(raw: &str) -> Option<(SubprocessCliAuthStatus, Option<String>)> {
    let lowered = raw.to_ascii_lowercase();
    if lowered.contains("expired")
        || lowered.contains("revoked")
        || lowered.contains("stale")
        || lowered.contains("credential cache")
    {
        return Some((
            SubprocessCliAuthStatus::StaleSession,
            Some("cli_auth_stale".to_string()),
        ));
    }

    if lowered.contains("unsupported")
        || lowered.contains("subscription")
        || lowered.contains("provider mode")
        || lowered.contains("plan")
        || lowered.contains("tier")
    {
        return Some((
            SubprocessCliAuthStatus::Unsupported,
            Some("cli_auth_unsupported".to_string()),
        ));
    }

    if lowered.contains("interactive")
        || lowered.contains("browser")
        || lowered.contains("prompt")
        || lowered.contains("continue login")
    {
        return Some((
            SubprocessCliAuthStatus::InteractiveRequired,
            Some("cli_auth_interactive_required".to_string()),
        ));
    }

    if lowered.contains("not logged in")
        || lowered.contains("login required")
        || lowered.contains("sign in")
        || lowered.contains("not authenticated")
    {
        return Some((
            SubprocessCliAuthStatus::Unauthenticated,
            Some("cli_auth_required".to_string()),
        ));
    }

    if lowered.starts_with("logged in") || lowered.contains("logged in using") {
        return Some((
            SubprocessCliAuthStatus::Authenticated,
            Some("cli_authenticated".to_string()),
        ));
    }

    None
}

fn auth_hint_text_from_json(value: &serde_json::Value) -> String {
    [
        "reason",
        "message",
        "error",
        "detail",
        "status",
        "authStatus",
        "subscription",
        "plan",
        "providerMode",
    ]
    .into_iter()
    .filter_map(|key| value.get(key).and_then(|value| value.as_str()))
    .collect::<Vec<_>>()
    .join(" ")
}

fn nonzero_probe_detail(code: Option<i32>, raw: &str) -> String {
    let code = code
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "probe_failed:exit:{code}:{}",
        truncate_for_error(&sanitize_auth_probe_output(raw.trim()))
    )
}

fn sanitize_auth_probe_output(raw: &str) -> String {
    raw.split_whitespace()
        .map(redact_sensitive_token)
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_sensitive_token(token: &str) -> String {
    let sensitive = token.trim_matches(|value: char| {
        matches!(
            value,
            ',' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
        )
    });
    let lowered = sensitive.to_ascii_lowercase();
    let replacement = if looks_like_email(sensitive) {
        Some("<redacted-email>")
    } else if lowered.starts_with("org_") || lowered.starts_with("organization_") {
        Some("<redacted-org>")
    } else if lowered.starts_with("gho_")
        || lowered.starts_with("ghp_")
        || lowered.starts_with("sk-")
        || lowered.starts_with("sk_")
    {
        Some("<redacted-token>")
    } else {
        None
    };

    replacement
        .map(|value| token.replace(sensitive, value))
        .unwrap_or_else(|| token.to_string())
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

fn run_probe_command_with_timeout(
    executable_path: &Path,
    args: &[String],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut child = StdCommand::new(executable_path)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("probe_failed:{err}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "probe_failed:stdout pipe missing".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "probe_failed:stderr pipe missing".to_string())?;
    let stdout_reader = thread::spawn(move || {
        let mut reader = stdout;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map(|_| buf)
    });
    let stderr_reader = thread::spawn(move || {
        let mut reader = stderr;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map(|_| buf)
    });

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_reader
                    .join()
                    .map_err(|_| "probe_failed:stdout reader panicked".to_string())?
                    .map_err(|err| format!("probe_failed:{err}"))?;
                let stderr = stderr_reader
                    .join()
                    .map_err(|_| "probe_failed:stderr reader panicked".to_string())?
                    .map_err(|err| format!("probe_failed:{err}"))?;
                return Ok(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    if let Err(e) = child.kill() {
                        debug!("process kill failed: {e}");
                    }
                    if let Err(e) = child.wait() {
                        debug!("process wait failed: {e}");
                    }
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(format!("probe_timeout:{}ms", timeout.as_millis()));
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                if let Err(e) = child.kill() {
                    debug!("process kill failed: {e}");
                }
                if let Err(e) = child.wait() {
                    debug!("process wait failed: {e}");
                }
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("probe_failed:{err}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn parses_codex_logged_in_status() {
        let (status, detail) = parse_codex_auth_status("Logged in using ChatGPT");
        assert_eq!(status, SubprocessCliAuthStatus::Authenticated);
        assert_eq!(detail.as_deref(), Some("cli_authenticated"));
    }

    #[test]
    fn parses_codex_auth_edge_states_without_pii() {
        let (status, detail) =
            parse_codex_auth_status("Session expired for alice@example.com; login required");
        assert_eq!(status, SubprocessCliAuthStatus::StaleSession);
        assert_eq!(detail.as_deref(), Some("cli_auth_stale"));

        let (status, detail) =
            parse_codex_auth_status("Current subscription plan does not support headless CLI use");
        assert_eq!(status, SubprocessCliAuthStatus::Unsupported);
        assert_eq!(detail.as_deref(), Some("cli_auth_unsupported"));

        let (status, detail) =
            parse_codex_auth_status("Open the browser to continue interactive login");
        assert_eq!(status, SubprocessCliAuthStatus::InteractiveRequired);
        assert_eq!(detail.as_deref(), Some("cli_auth_interactive_required"));

        let (status, detail) =
            parse_codex_auth_status("Active account alice@example.com in org org_123");
        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        let detail = detail.expect("unknown output detail should be present");
        assert!(detail.starts_with("unexpected_status_output:"));
        assert!(!detail.contains("alice@example.com"));
        assert!(!detail.contains("org_123"));
    }

    #[test]
    fn parses_claude_logged_in_status_json() {
        let (status, detail) =
            parse_claude_auth_status(r#"{"loggedIn":true,"authMethod":"oauth"}"#);
        assert_eq!(status, SubprocessCliAuthStatus::Authenticated);
        assert_eq!(detail.as_deref(), Some("cli_authenticated"));
    }

    #[test]
    fn parses_claude_logged_out_status_json() {
        let (status, detail) =
            parse_claude_auth_status(r#"{"loggedIn":false,"authMethod":"none"}"#);
        assert_eq!(status, SubprocessCliAuthStatus::Unauthenticated);
        assert_eq!(detail.as_deref(), Some("cli_auth_required"));
    }

    #[test]
    fn parses_claude_auth_edge_states_without_pii() {
        let (status, detail) = parse_claude_auth_status(
            r#"{"loggedIn":false,"reason":"Expired session for alice@example.com"}"#,
        );
        assert_eq!(status, SubprocessCliAuthStatus::StaleSession);
        assert_eq!(detail.as_deref(), Some("cli_auth_stale"));

        let (status, detail) = parse_claude_auth_status(
            r#"{"loggedIn":true,"message":"Plan unsupported for headless CLI"}"#,
        );
        assert_eq!(status, SubprocessCliAuthStatus::Unsupported);
        assert_eq!(detail.as_deref(), Some("cli_auth_unsupported"));

        let (status, detail) =
            parse_claude_auth_status(r#"{"loggedIn":false,"status":"interactive_required"}"#);
        assert_eq!(status, SubprocessCliAuthStatus::InteractiveRequired);
        assert_eq!(detail.as_deref(), Some("cli_auth_interactive_required"));
    }

    #[test]
    fn parses_claude_invalid_status_json_as_unknown() {
        let (status, detail) = parse_claude_auth_status("not-json");
        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        assert!(detail
            .as_deref()
            .is_some_and(|value| value.starts_with("invalid_status_json:")));
    }

    #[test]
    fn fake_claude_auth_cli_preserves_argv_and_reports_states() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_claude_auth_cli(temp_dir.path());
        let spaced_arg = "literal arg with spaces".to_string();

        let test_timeout = Duration::from_secs(10);
        let (status, detail) = probe_claude_auth_status_with_timeout(
            &executable_path,
            &["success".to_string(), spaced_arg.clone()],
            test_timeout,
        );
        assert_eq!(status, SubprocessCliAuthStatus::Authenticated);
        assert_eq!(detail.as_deref(), Some("cli_authenticated"));

        let (status, detail) = probe_claude_auth_status_with_timeout(
            &executable_path,
            &["failure".to_string(), spaced_arg.clone()],
            test_timeout,
        );
        assert_eq!(status, SubprocessCliAuthStatus::Unauthenticated);
        assert_eq!(detail.as_deref(), Some("cli_auth_required"));

        let (status, detail) = probe_claude_auth_status_with_timeout(
            &executable_path,
            &["invalid".to_string(), spaced_arg],
            test_timeout,
        );
        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        assert!(detail
            .as_deref()
            .is_some_and(|value| value.starts_with("invalid_status_json:")));
    }

    #[test]
    fn fake_auth_probe_nonzero_stderr_is_sanitized() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_claude_auth_cli(temp_dir.path());

        let (status, detail) = probe_claude_auth_status_with_timeout(
            &executable_path,
            &[
                "stderr-pii".to_string(),
                "literal arg with spaces".to_string(),
            ],
            Duration::from_secs(10),
        );

        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        let detail = detail.expect("probe failure detail should be present");
        assert!(detail.starts_with("probe_failed:exit:"));
        assert!(!detail.contains("alice@example.com"));
        assert!(!detail.contains("org_123"));
    }

    #[test]
    fn fake_auth_probe_timeout_is_bounded() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_claude_auth_cli(temp_dir.path());

        let detail = run_probe_command_with_timeout(
            &executable_path,
            &["sleep".to_string()],
            Duration::from_millis(50),
        )
        .expect_err("fake auth probe should time out");

        assert!(detail.starts_with("probe_timeout:"));
    }

    #[test]
    fn fake_auth_probe_large_stdout_does_not_timeout() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let executable_path = write_fake_claude_auth_cli(temp_dir.path());

        let output = run_probe_command_with_timeout(
            &executable_path,
            &[
                "stdout-big".to_string(),
                "literal arg with spaces".to_string(),
            ],
            Duration::from_secs(10),
        )
        .expect("large stdout probe should drain pipes and exit");

        assert!(output.status.success());
        assert!(output.stdout.len() > 100_000);
    }

    #[test]
    fn marks_headless_catalog_subprocess_surfaces_as_runtime_supported() {
        use super::super::runtime::runtime_supported_for_surface;

        assert!(runtime_supported_for_surface(
            "provider_surface.openai.subprocess_cli"
        ));
        assert!(runtime_supported_for_surface(
            "provider_surface.anthropic.subprocess_cli"
        ));
        assert!(runtime_supported_for_surface(
            "provider_surface.google.subprocess_cli"
        ));
        assert!(!runtime_supported_for_surface(
            "provider_surface.google.antigravity_cli"
        ));
    }

    #[test]
    fn resolves_auth_probe_runtime_from_catalog_mode() {
        assert!(
            auth_probe_runtime_for_surface("provider_surface.openai.subprocess_cli")
                .expect("openai probe runtime should resolve")
                .is_some()
        );
        assert!(
            auth_probe_runtime_for_surface("provider_surface.google.subprocess_cli")
                .expect("google probe runtime should resolve")
                .is_none()
        );
        // E21 #4868 Part 1: the app-server surface now resolves a runtime via the
        // new account/read probe mode.
        assert!(
            auth_probe_runtime_for_surface("provider_surface.openai.codex_app_server")
                .expect("codex app-server probe runtime should resolve")
                .is_some()
        );
    }

    #[test]
    fn account_read_probe_resolves_app_server_args_not_login_status() {
        // E21 #4868 Part 1: the structured account/read probe MUST spawn
        // `codex app-server` (app_server_args), NOT the text-probe `login status`
        // command. Guards the mode→args dispatch in probe_args_for_surface so a
        // branch swap (app_server_args ↔ auth_probe_command) is caught — the
        // direct-call probe tests pass ["app-server"] explicitly and cannot.
        let app_server_args = probe_args_for_surface(
            "provider_surface.openai.codex_app_server",
            SubprocessAuthProbeMode::CodexAccountReadJson,
        )
        .expect("app-server probe args resolve");
        assert_eq!(app_server_args, vec!["app-server".to_string()]);

        // The exec surface's text probe still resolves the login-status command.
        let text_args = probe_args_for_surface(
            "provider_surface.openai.subprocess_cli",
            SubprocessAuthProbeMode::CodexLoginStatusText,
        )
        .expect("exec text probe args resolve");
        assert!(
            text_args.iter().any(|arg| arg == "status"),
            "exec text probe should use the login-status command, got {text_args:?}"
        );
        assert!(
            !text_args.iter().any(|arg| arg == "app-server"),
            "exec text probe must NOT spawn app-server, got {text_args:?}"
        );
    }

    // ── parse_account_read_status (E21 #4868 Part 1) ─────────────────────────

    fn account_read_value(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).expect("valid account/read fixture json")
    }

    #[test]
    fn account_read_chatgpt_is_authenticated_without_pii() {
        // ChatGPT-subscription account → Authenticated. The detail carries the
        // chatgpt distinction but NEVER the account email (PII).
        let value = account_read_value(
            r#"{"account":{"type":"chatgpt","email":"alice@example.com","planType":"pro"},"requiresOpenaiAuth":true}"#,
        );
        let (status, detail) = parse_account_read_status(&value);
        assert_eq!(status, SubprocessCliAuthStatus::Authenticated);
        let detail = detail.expect("chatgpt detail present");
        assert_eq!(detail, "cli_authenticated_chatgpt");
        assert!(!detail.contains("alice@example.com"));
        assert!(!detail.contains("pro"));
    }

    #[test]
    fn account_read_apikey_is_authenticated_with_distinct_detail() {
        // API-key account → Authenticated, distinct from the ChatGPT detail
        // token (SCOPE item 3: distinguish auth modes).
        let value =
            account_read_value(r#"{"account":{"type":"apiKey"},"requiresOpenaiAuth":true}"#);
        let (status, detail) = parse_account_read_status(&value);
        assert_eq!(status, SubprocessCliAuthStatus::Authenticated);
        assert_eq!(detail.as_deref(), Some("cli_authenticated_apikey"));
    }

    #[test]
    fn account_read_not_logged_in_is_unauthenticated() {
        // account:null + requiresOpenaiAuth:true → Unauthenticated.
        let value = account_read_value(r#"{"account":null,"requiresOpenaiAuth":true}"#);
        let (status, detail) = parse_account_read_status(&value);
        assert_eq!(status, SubprocessCliAuthStatus::Unauthenticated);
        assert_eq!(detail.as_deref(), Some("cli_auth_required"));
    }

    #[test]
    fn account_read_no_auth_required_is_authenticated() {
        // LOAD-BEARING: account:null + requiresOpenaiAuth:false → Authenticated
        // (OSS/local model). A mutation treating account:null as always
        // Unauthenticated FAILS here — proves the requiresOpenaiAuth split.
        let value = account_read_value(r#"{"account":null,"requiresOpenaiAuth":false}"#);
        let (status, detail) = parse_account_read_status(&value);
        assert_eq!(status, SubprocessCliAuthStatus::Authenticated);
        assert_eq!(detail.as_deref(), Some("cli_no_auth_required"));
    }

    #[test]
    fn account_read_amazon_bedrock_is_unsupported() {
        let value =
            account_read_value(r#"{"account":{"type":"amazonBedrock"},"requiresOpenaiAuth":true}"#);
        let (status, detail) = parse_account_read_status(&value);
        assert_eq!(status, SubprocessCliAuthStatus::Unsupported);
        assert_eq!(detail.as_deref(), Some("cli_auth_unsupported"));
    }

    #[test]
    fn account_read_unknown_account_type_is_unknown() {
        let value =
            account_read_value(r#"{"account":{"type":"futuretype"},"requiresOpenaiAuth":true}"#);
        let (status, detail) = parse_account_read_status(&value);
        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        assert_eq!(detail.as_deref(), Some("account_read_unknown_account_type"));
    }

    #[test]
    fn account_read_defensive_shapes_are_unknown_without_panic() {
        // Non-object value.
        let (status, detail) = parse_account_read_status(&serde_json::Value::Null);
        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        assert_eq!(detail.as_deref(), Some("account_read_unexpected_shape"));

        // Empty object: account absent + requiresOpenaiAuth absent.
        let (status, detail) = parse_account_read_status(&account_read_value("{}"));
        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        assert_eq!(
            detail.as_deref(),
            Some("account_read_missing_requires_field")
        );

        // account:{} with no type, requiresOpenaiAuth present.
        let value = account_read_value(r#"{"account":{},"requiresOpenaiAuth":true}"#);
        let (status, detail) = parse_account_read_status(&value);
        assert_eq!(status, SubprocessCliAuthStatus::Unknown);
        assert_eq!(detail.as_deref(), Some("account_read_unknown_account_type"));
    }

    fn write_fake_claude_auth_cli(base_dir: &Path) -> PathBuf {
        use std::process::Command as StdCommand;

        let bin_dir = base_dir.join("Claude Code").join("bin");
        std::fs::create_dir_all(&bin_dir).expect("fake cli dir");
        let source_path = bin_dir.join("fake_claude_auth.rs");
        let executable_path = bin_dir.join(if cfg!(windows) {
            "claude-auth.exe"
        } else {
            "claude-auth"
        });
        std::fs::write(
            &source_path,
            r##"
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|value| value.as_str()) {
        Some("sleep") => {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
        Some(mode) => {
            if args.get(1).map(|value| value.as_str()) != Some("literal arg with spaces") {
                eprintln!("argv was not preserved");
                std::process::exit(91);
            }
            match mode {
                "success" => println!("{}", r#"{"loggedIn":true,"authMethod":"oauth"}"#),
                "failure" => println!("{}", r#"{"loggedIn":false,"authMethod":"none"}"#),
                "invalid" => println!("not-json"),
                "stdout-big" => {
                    for _ in 0..200_000 {
                        print!("x");
                    }
                }
                "stderr-pii" => {
                    eprintln!("active account alice@example.com in org org_123");
                    std::process::exit(42);
                }
                _ => {
                    eprintln!("unknown fake auth mode");
                    std::process::exit(92);
                }
            }
        }
        None => {
            eprintln!("missing fake auth mode");
            std::process::exit(93);
        }
    }
}
"##,
        )
        .expect("fake cli source");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let status = StdCommand::new(rustc)
            .arg(&source_path)
            .arg("-o")
            .arg(&executable_path)
            .status()
            .expect("compile fake auth cli");
        assert!(status.success(), "fake auth cli should compile");
        executable_path
    }

    /// End-to-end bridge test (E21 #4868 Part 1): drives the REAL
    /// `probe_codex_account_read` → `block_on_account_read` → `AppServerProcess`
    /// spawn + `initialize` + `account/read` path against a fake app-server, and
    /// guards the ADR-025 read-only boundary. Unix-only (the fake is a `sh`
    /// JSONL server, mirroring the #4872 harness), like the integration suite.
    #[cfg(unix)]
    #[test]
    fn account_read_probe_bridge_sends_refresh_token_false_and_maps_chatgpt() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let (executable_path, request_log) = write_fake_codex_app_server(temp_dir.path());

        let (status, detail) =
            probe_codex_account_read(&executable_path, &["app-server".to_string()]);

        // The structured account/read result is mapped, not the text grep.
        assert_eq!(status, SubprocessCliAuthStatus::Authenticated);
        assert_eq!(detail.as_deref(), Some("cli_authenticated_chatgpt"));

        let captured = std::fs::read_to_string(&request_log).expect("request log written");
        // The probe actually issued account/read (not a hardcoded mapping).
        assert!(
            captured.contains("\"account/read\""),
            "account/read request was never sent: {captured}"
        );
        // ADR-025 boundary: the read-only probe MUST send refreshToken:false and
        // MUST NEVER send refreshToken:true (true is the legal-gated Part-2
        // token-refresh). This guard fails if production flips the literal.
        assert!(
            captured.contains("\"refreshToken\":false"),
            "account/read must send refreshToken:false: {captured}"
        );
        assert!(
            !captured.contains("\"refreshToken\":true"),
            "account/read sent refreshToken:true — ADR-025 Part-2 violation: {captured}"
        );
    }

    /// Fake `codex app-server`: a `sh` JSONL stdio server that echoes the request
    /// `id`, answers `initialize` + `account/read` (ChatGPT account), and tees
    /// every inbound request line to a log file so the test can assert the
    /// `account/read` params. Mirrors the #4872 `FakeCodexAppServer` sh dialect.
    #[cfg(unix)]
    fn write_fake_codex_app_server(base_dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let executable_path = base_dir.join("fake-codex");
        let request_log = base_dir.join("account_read_requests.jsonl");
        let script = format!(
            r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> "{log}"
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{{"id":%s,"result":{{"userAgent":"codex/test"}}}}\n' "$id" ;;
    *'"account/read"'*) printf '{{"id":%s,"result":{{"account":{{"type":"chatgpt","email":"u@example.com","planType":"plus"}},"requiresOpenaiAuth":true}}}}\n' "$id" ;;
  esac
done
"#,
            log = request_log.display()
        );
        std::fs::write(&executable_path, script).expect("write fake codex app-server");
        std::fs::set_permissions(&executable_path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake codex app-server");
        (executable_path, request_log)
    }
}
