//! macOS sandbox enforcement via Seatbelt (`sandbox-exec`).
//!
//! Generates SBPL (Seatbelt Profile Language) profiles based on the
//! [`SandboxConfig`] and executes automation actions within a sandboxed
//! child process using:
//! `/usr/bin/sandbox-exec -p <profile> -- oneshim-sandbox-worker`
//!
//! The action is written to the worker's stdin as a JSON-encoded
//! [`SandboxRequest`] and the result is read from stdout as a
//! [`SandboxResponse`].
//!
//! **Resource limits**: `apply_resource_limits()` logs the configured values
//! but does **not** call `setrlimit(2)`. The sandbox-exec model spawns a
//! child via Seatbelt, and there is no hook to inject `setrlimit` into the
//! child before exec. `capabilities()` therefore reports `resource_limits: false`.
//! Filesystem and network isolation ARE enforced by the SBPL profile.

use async_trait::async_trait;
use std::process::Command;

use crate::error::AutomationError;
use crate::sandbox::ipc;
use oneshim_core::config::{SandboxConfig, SandboxProfile};
use oneshim_core::error::CoreError;
use oneshim_core::models::automation::AutomationAction;
use oneshim_core::ports::sandbox::{Sandbox, SandboxCapabilities};

pub struct MacOsSandbox {
    sandbox_exec_path: Option<String>,
}

impl Default for MacOsSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl MacOsSandbox {
    pub fn new() -> Self {
        let path = find_sandbox_exec();
        Self {
            sandbox_exec_path: path,
        }
    }

    /// Create a sandbox with an explicit path to `sandbox-exec`.
    /// Useful for testing with a mock binary or non-standard install location.
    #[cfg(test)]
    fn with_exec_path(path: Option<String>) -> Self {
        Self {
            sandbox_exec_path: path,
        }
    }

    fn generate_sbpl_profile(config: &SandboxConfig) -> String {
        let mut rules = String::new();
        rules.push_str("(version 1)\n");

        match config.profile {
            SandboxProfile::Permissive => {
                rules.push_str("(allow default)\n");
                rules.push_str("(deny file-write* (subpath \"/System\"))\n");
                rules.push_str("(deny file-write* (subpath \"/usr\"))\n");
            }
            SandboxProfile::Standard => {
                rules.push_str("(deny default)\n");
                rules.push_str("(allow process-exec)\n");
                rules.push_str("(allow process-fork)\n");
                rules.push_str("(allow sysctl-read)\n");
                rules.push_str("(allow mach-lookup)\n");

                rules.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
                rules.push_str("(allow file-read* (subpath \"/System/Library\"))\n");
                rules.push_str("(allow file-read* (subpath \"/Library/Frameworks\"))\n");
                rules.push_str("(allow file-read* (subpath \"/dev\"))\n");

                for path in &config.allowed_read_paths {
                    let escaped = escape_sbpl_path(path);
                    rules.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", escaped));
                }

                for path in &config.allowed_write_paths {
                    let escaped = escape_sbpl_path(path);
                    rules.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", escaped));
                }

                if !config.allow_network {
                    rules.push_str("(deny network*)\n");
                } else {
                    rules.push_str("(allow network*)\n");
                }
            }
            SandboxProfile::Strict => {
                rules.push_str("(deny default)\n");
                rules.push_str("(allow process-exec)\n");
                rules.push_str("(allow sysctl-read)\n");

                rules.push_str("(allow file-read* (subpath \"/usr/lib\"))\n");
                rules.push_str("(allow file-read* (subpath \"/dev/null\"))\n");
                rules.push_str("(allow file-read* (subpath \"/dev/urandom\"))\n");

                for path in &config.allowed_read_paths {
                    let escaped = escape_sbpl_path(path);
                    rules.push_str(&format!("(allow file-read* (subpath \"{}\"))\n", escaped));
                }

                rules.push_str("(deny network*)\n");
            }
        }

        rules
    }

    /// Build the `sandbox-exec` command line for the given SBPL profile.
    ///
    /// Returns `(sandbox_exec_path, args)` where `args` is:
    /// `["-p", profile, "--", worker_binary_path]`.
    ///
    /// The action is no longer passed as a command-line argument; it is
    /// written to the worker's stdin as a JSON-encoded [`SandboxRequest`].
    fn build_sandbox_command(&self, profile: &str) -> Result<(String, Vec<String>), CoreError> {
        let exec_path = self
            .sandbox_exec_path
            .as_deref()
            .ok_or_else(|| CoreError::SandboxUnsupported {
                code: oneshim_core::error_codes::SandboxCode::UnsupportedPlatform,
                message: "sandbox-exec not found".to_string(),
            })?
            .to_string();

        let worker_path = ipc::resolve_worker_path()?;

        let args = vec![
            "-p".to_string(),
            profile.to_string(),
            "--".to_string(),
            worker_path.to_string_lossy().to_string(),
        ];

        Ok((exec_path, args))
    }
}

/// Returns `true` when the config is Permissive with no custom resource limits,
/// meaning subprocess sandboxing can be skipped entirely.
fn is_permissive_noop(config: &SandboxConfig) -> bool {
    matches!(config.profile, SandboxProfile::Permissive)
        && config.max_memory_bytes == 0
        && config.max_cpu_time_ms == 0
}

#[async_trait]
impl Sandbox for MacOsSandbox {
    fn platform(&self) -> &str {
        "macos"
    }

    fn is_available(&self) -> bool {
        self.sandbox_exec_path.is_some()
    }

    async fn execute_sandboxed(
        &self,
        action: &AutomationAction,
        config: &SandboxConfig,
    ) -> Result<(), CoreError> {
        if !self.is_available() {
            return Err(CoreError::SandboxUnsupported {
                code: oneshim_core::error_codes::SandboxCode::UnsupportedPlatform,
                message: "sandbox-exec not found on this system".to_string(),
            });
        }

        if is_permissive_noop(config) {
            tracing::debug!("Permissive profile with no limits — skipping subprocess");
            return Ok(());
        }

        let profile = Self::generate_sbpl_profile(config);
        tracing::debug!(
            profile_type = %config.profile as u8,
            sbpl_len = profile.len(),
            action = ?action,
            "macOS Seatbelt sandbox: generated SBPL profile"
        );

        apply_resource_limits(config).map_err(CoreError::from)?;

        let (exec_path, args) = self.build_sandbox_command(&profile)?;

        tracing::debug!(
            sandbox_exec = %exec_path,
            args_count = args.len(),
            "invoking sandbox-exec with worker binary"
        );

        let request = ipc::SandboxRequest {
            action: action.clone(),
        };
        let request_json =
            serde_json::to_string(&request).map_err(|e| CoreError::SandboxExecution {
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!("failed to serialize action: {}", e),
            })?;

        let timeout_ms = if config.max_cpu_time_ms > 0 {
            config.max_cpu_time_ms + 5000
        } else {
            60_000
        };

        let mut child = tokio::process::Command::new(&exec_path)
            .args(&args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| CoreError::SandboxExecution {
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!("failed to spawn sandbox-exec: {}", e),
            })?;

        // Write serialized request to child stdin
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| CoreError::SandboxExecution {
                    code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!("stdin write: {}", e),
                })?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| CoreError::SandboxExecution {
                    code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!("stdin newline: {}", e),
                })?;
            drop(stdin);
        }

        let output = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            child.wait_with_output(),
        )
        .await
        .map_err(|_| CoreError::ExecutionTimeout {
            code: oneshim_core::error_codes::SandboxCode::Timeout,
            timeout_ms,
        })?
        .map_err(|e| CoreError::SandboxExecution {
            code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
            message: format!("wait failed: {}", e),
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code().unwrap_or(-1);
            tracing::error!(
                exit_code,
                stderr = %stderr,
                "sandbox-exec exited with non-zero status"
            );
            return Err(CoreError::SandboxExecution {
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!(
                    "sandbox-exec failed (exit {}): {}",
                    exit_code,
                    stderr.trim()
                ),
            });
        }

        let response = ipc::parse_worker_response(&output.stdout)?;
        if !response.success {
            return Err(CoreError::SandboxExecution {
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!(
                    "worker reported failure: {}",
                    response.error.unwrap_or_default()
                ),
            });
        }

        tracing::info!(
            action = ?action,
            sbpl_len = profile.len(),
            "macOS sandboxed action execution completed"
        );

        Ok(())
    }

    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            filesystem_isolation: self.is_available(),
            syscall_filtering: false, // macOS has no syscall filtering support
            network_isolation: self.is_available(),
            // Resource limits require the child process to call setrlimit(2)
            // before exec. sandbox-exec does not support injecting setrlimit
            // into the child, so apply_resource_limits() is a no-op log.
            resource_limits: false,
            process_isolation: self.is_available(),
        }
    }
}

fn find_sandbox_exec() -> Option<String> {
    let default_path = "/usr/bin/sandbox-exec";
    if std::path::Path::new(default_path).exists() {
        return Some(default_path.to_string());
    }

    if let Ok(output) = Command::new("which").arg("sandbox-exec").output() {
        if output.status.success() {
            if let Ok(path) = String::from_utf8(output.stdout) {
                let path = path.trim().to_string();
                if !path.is_empty() {
                    return Some(path);
                }
            }
        }
    }

    None
}

fn escape_sbpl_path(path: &str) -> String {
    path.replace('\\', "\\\\").replace('"', "\\\"")
}

fn apply_resource_limits(config: &SandboxConfig) -> Result<(), AutomationError> {
    if config.max_memory_bytes > 0 {
        tracing::debug!(
            max_memory = config.max_memory_bytes,
            "configuring memory limit (macOS)"
        );
    }

    if config.max_cpu_time_ms > 0 {
        tracing::debug!(
            max_cpu_ms = config.max_cpu_time_ms,
            "configuring CPU time limit (macOS)"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_permissive_profile() {
        let config = SandboxConfig {
            profile: SandboxProfile::Permissive,
            ..Default::default()
        };
        let profile = MacOsSandbox::generate_sbpl_profile(&config);
        assert!(profile.contains("(version 1)"));
        assert!(profile.contains("(allow default)"));
    }

    #[test]
    fn generate_standard_profile() {
        let config = SandboxConfig {
            profile: SandboxProfile::Standard,
            allowed_read_paths: vec!["/tmp/test".to_string()],
            allow_network: false,
            ..Default::default()
        };
        let profile = MacOsSandbox::generate_sbpl_profile(&config);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(deny network*)"));
        assert!(profile.contains("/tmp/test"));
    }

    #[test]
    fn generate_strict_profile() {
        let config = SandboxConfig {
            profile: SandboxProfile::Strict,
            ..Default::default()
        };
        let profile = MacOsSandbox::generate_sbpl_profile(&config);
        assert!(profile.contains("(deny default)"));
        assert!(profile.contains("(deny network*)"));
        assert!(!profile.contains("(allow network*)"));
    }

    #[test]
    fn escape_sbpl_path_special_chars() {
        assert_eq!(escape_sbpl_path("/normal/path"), "/normal/path");
        assert_eq!(
            escape_sbpl_path("/path/with \"quotes\""),
            "/path/with \\\"quotes\\\""
        );
    }

    #[tokio::test]
    async fn macos_sandbox_available() {
        let sandbox = MacOsSandbox::new();
        if sandbox.is_available() {
            assert_eq!(sandbox.platform(), "macos");
            let caps = sandbox.capabilities();
            assert!(caps.filesystem_isolation);
            assert!(caps.network_isolation);
        }
    }

    #[test]
    fn build_sandbox_command_uses_worker() {
        let sandbox = MacOsSandbox::with_exec_path(Some("/usr/bin/sandbox-exec".to_string()));
        let profile = "(version 1)\n(allow default)\n";
        if let Ok((exec_path, args)) = sandbox.build_sandbox_command(profile) {
            assert_eq!(exec_path, "/usr/bin/sandbox-exec");
            assert_eq!(args[0], "-p");
            assert_eq!(args[1], profile);
            assert_eq!(args[2], "--");
            assert!(args[3].contains("oneshim-sandbox-worker"));
        }
    }

    #[test]
    fn build_sandbox_command_without_exec_path_fails() {
        let sandbox = MacOsSandbox::with_exec_path(None);
        let result = sandbox.build_sandbox_command("(version 1)\n");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn execute_sandboxed_without_exec_path_returns_unsupported() {
        let sandbox = MacOsSandbox::with_exec_path(None);
        let action = AutomationAction::KeyType {
            text: "test".to_string(),
        };
        let config = SandboxConfig {
            profile: SandboxProfile::Standard,
            ..Default::default()
        };

        let result = sandbox.execute_sandboxed(&action, &config).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("sandbox-exec not found"),
            "expected SandboxUnsupported, got: {}",
            err
        );
    }

    #[test]
    fn permissive_no_limits_is_noop() {
        let config = SandboxConfig {
            profile: SandboxProfile::Permissive,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
            ..Default::default()
        };
        assert!(is_permissive_noop(&config));

        // Permissive with memory limit is NOT noop
        let config_with_mem = SandboxConfig {
            profile: SandboxProfile::Permissive,
            max_memory_bytes: 1024,
            max_cpu_time_ms: 0,
            ..Default::default()
        };
        assert!(!is_permissive_noop(&config_with_mem));

        // Standard profile is NOT noop (even with zero limits)
        let config_standard = SandboxConfig {
            profile: SandboxProfile::Standard,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
            ..Default::default()
        };
        assert!(!is_permissive_noop(&config_standard));
    }
}
