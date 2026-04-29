use std::path::Path;
use std::process::{Command as StdCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use oneshim_api_contracts::provider_specs::{subprocess_auth_probe_mode, SubprocessAuthProbeMode};

use super::{
    catalog_subprocess_transport, truncate_for_error, DetectedSubprocessCli, ProbedSubprocessCli,
    SubprocessCliAuthStatus, CLI_AUTH_PROBE_TIMEOUT_SECS,
};
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
        SubprocessAuthProbeMode::None => None,
    }
}

pub(super) fn auth_probe_runtime_for_surface(
    surface_id: &str,
) -> Result<Option<SubprocessAuthProbeRuntime>, String> {
    auth_probe_mode_for_surface(surface_id).map(auth_probe_runtime_for_mode)
}

pub(super) fn probe_cli_surface(detected: DetectedSubprocessCli) -> ProbedSubprocessCli {
    let (auth_status, auth_detail) = match auth_probe_runtime_for_surface(&detected.surface_id) {
        Ok(Some(runtime)) => {
            let probe_args =
                auth_probe_command_for_surface(&detected.surface_id).unwrap_or_default();
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
    parse_codex_auth_status(&combined)
}

pub(super) fn parse_codex_auth_status(raw: &str) -> (SubprocessCliAuthStatus, Option<String>) {
    let normalized = raw.trim();
    let lowered = normalized.to_ascii_lowercase();
    if lowered.contains("not logged in") || lowered.contains("login required") {
        return (
            SubprocessCliAuthStatus::Unauthenticated,
            Some("cli_auth_required".to_string()),
        );
    }

    if lowered.starts_with("logged in") || lowered.contains("logged in using") {
        return (
            SubprocessCliAuthStatus::Authenticated,
            Some("cli_authenticated".to_string()),
        );
    }

    (
        SubprocessCliAuthStatus::Unknown,
        Some(format!(
            "unexpected_status_output:{}",
            truncate_for_error(normalized)
        )),
    )
}

fn probe_claude_auth_status(
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

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|err| format!("probe_failed:{err}"));
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    if let Err(e) = child.kill() {
                        debug!("process kill failed: {e}");
                    }
                    if let Err(e) = child.wait() {
                        debug!("process wait failed: {e}");
                    }
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
                return Err(format!("probe_failed:{err}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_codex_logged_in_status() {
        let (status, detail) = parse_codex_auth_status("Logged in using ChatGPT");
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
    fn marks_catalog_subprocess_surfaces_as_runtime_supported() {
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
    }
}
