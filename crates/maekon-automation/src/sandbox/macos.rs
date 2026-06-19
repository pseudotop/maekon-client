//! macOS sandbox enforcement via Seatbelt (`sandbox-exec`).
//!
//! Generates SBPL (Seatbelt Profile Language) profiles based on the
//! [`SandboxConfig`] and executes automation actions within a sandboxed
//! child process using:
//! `/usr/bin/sandbox-exec -p <profile> -- maekon-sandbox-worker`
//!
//! The action is written to the worker's stdin as a JSON-encoded
//! [`SandboxRequest`] and the result is read from stdout as a
//! [`SandboxResponse`].
//!
//! **Containment policy** (#6443 F6): the deny-by-default profiles scope
//! `process-exec` to the single worker-binary literal and do not grant
//! `process-fork` (see [`super::sbpl::generate_sbpl_profile`]). `sandbox-exec`
//! must still be allowed to exec the worker itself — that is the one authorised
//! exec — but the worker, a single-shot stdin→stdout process, can no longer exec
//! a shell or any other binary.
//!
//! **`sandbox-exec` deprecation**: `/usr/bin/sandbox-exec` is deprecated by Apple
//! (since macOS 10.10) yet remains functional and is still used in production by
//! Chromium, Bazel, and Nix. The long-term replacement is the App Sandbox via
//! signed `.app`-bundle entitlements, which is a build/signing/Info.plist change
//! rather than a profile-string change and is out of scope here; it is tracked as
//! the remaining roadmap item on #6443 F6. Until then this adapter fails closed
//! when `sandbox-exec` is absent (see [`super::create_platform_sandbox`]).
//!
//! **Resource limits**: `apply_resource_limits()` logs the configured values
//! but does **not** call `setrlimit(2)`. The sandbox-exec model spawns a
//! child via Seatbelt, and there is no hook to inject `setrlimit` into the
//! child before exec. `capabilities()` therefore reports `resource_limits: false`.
//! Filesystem and network isolation ARE enforced by the SBPL profile.

use async_trait::async_trait;
use std::path::Path;
use std::process::Command;

use crate::error::AutomationError;
use crate::sandbox::ipc;
use maekon_core::config::SandboxConfig;
use maekon_core::error::CoreError;
use maekon_core::models::automation::AutomationAction;
use maekon_core::ports::sandbox::{Sandbox, SandboxCapabilities};

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

    /// Build the `sandbox-exec` command line for the given SBPL profile.
    ///
    /// Returns `(sandbox_exec_path, args)` where `args` is:
    /// `["-p", profile, "--", worker_binary_path]`.
    ///
    /// `worker_path` is resolved by the caller and passed in (rather than
    /// resolved here) so the exact same path is woven into the profile's
    /// `process-exec` literal and used as the exec target — the SBPL constraint
    /// and the real exec must reference the identical path (#6443 F6).
    ///
    /// The action is no longer passed as a command-line argument; it is
    /// written to the worker's stdin as a JSON-encoded [`SandboxRequest`].
    fn build_sandbox_command(
        &self,
        profile: &str,
        worker_path: &Path,
    ) -> Result<(String, Vec<String>), CoreError> {
        let exec_path = self
            .sandbox_exec_path
            .as_deref()
            .ok_or_else(|| CoreError::SandboxUnsupported {
                code: maekon_core::error_codes::SandboxCode::UnsupportedPlatform,
                message: "sandbox-exec not found".to_string(),
            })?
            .to_string();

        let args = vec![
            "-p".to_string(),
            profile.to_string(),
            "--".to_string(),
            worker_path.to_string_lossy().to_string(),
        ];

        Ok((exec_path, args))
    }
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
                code: maekon_core::error_codes::SandboxCode::UnsupportedPlatform,
                message: "sandbox-exec not found on this system".to_string(),
            });
        }

        // Resolve the worker path up front and canonicalize it so the profile's
        // `process-exec` literal and the actual exec target are the same canonical
        // path Seatbelt matches against (#6443 F6). If canonicalization fails the
        // raw path is used for both (kept consistent); the spawn below then
        // surfaces any genuinely-missing worker as a clear error.
        let worker_path = ipc::resolve_worker_path()?;
        let worker_path = worker_path.canonicalize().unwrap_or(worker_path);

        let profile = super::sbpl::generate_sbpl_profile(config, &worker_path.to_string_lossy());
        tracing::debug!(
            profile_type = %config.profile as u8,
            sbpl_len = profile.len(),
            action = %super::redact_action(action),
            "macOS Seatbelt sandbox: generated SBPL profile"
        );

        apply_resource_limits(config).map_err(CoreError::from)?;

        let (exec_path, args) = self.build_sandbox_command(&profile, &worker_path)?;

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
                code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
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
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| CoreError::SandboxExecution {
                code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!("failed to spawn sandbox-exec: {}", e),
            })?;

        // Write serialized request to child stdin
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| CoreError::SandboxExecution {
                    code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!("stdin write: {}", e),
                })?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| CoreError::SandboxExecution {
                    code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!("stdin newline: {}", e),
                })?;
            drop(stdin);
        }

        // `wait_with_output()` reads stdout/stderr to EOF and waits for exit.
        // On timeout it is dropped, consuming `child`; `kill_on_drop(true)` (set on
        // the builder above) then SIGKILLs the sandbox-exec worker tree as that
        // Child drops — no orphan (finding #5967: orphan-on-timeout).
        let output = match tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Err(CoreError::SandboxExecution {
                    code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!("wait failed: {}", e),
                });
            }
            Err(_elapsed) => {
                return Err(CoreError::ExecutionTimeout {
                    code: maekon_core::error_codes::SandboxCode::Timeout,
                    timeout_ms,
                });
            }
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let exit_code = output.status.code().unwrap_or(-1);
            tracing::error!(
                exit_code,
                stderr = %stderr,
                "sandbox-exec exited with non-zero status"
            );
            return Err(CoreError::SandboxExecution {
                code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
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
                code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!(
                    "worker reported failure: {}",
                    response.error.unwrap_or_default()
                ),
            });
        }

        tracing::info!(
            action = %super::redact_action(action),
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
    use crate::sandbox::is_permissive_noop;
    use maekon_core::config::SandboxProfile;

    // SBPL profile-generation tests moved to the cfg-free `super::super::sbpl`
    // module (#5120) so the security policy is verified on every OS.

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
        let worker = Path::new("/opt/maekon/maekon-sandbox-worker");
        let (exec_path, args) = sandbox
            .build_sandbox_command(profile, worker)
            .expect("command builds when exec path is present");
        assert_eq!(exec_path, "/usr/bin/sandbox-exec");
        assert_eq!(args[0], "-p");
        assert_eq!(args[1], profile);
        assert_eq!(args[2], "--");
        // The caller-supplied worker path is the exec target verbatim, so it
        // matches the profile's process-exec literal (#6443 F6).
        assert_eq!(args[3], "/opt/maekon/maekon-sandbox-worker");
    }

    #[test]
    fn build_sandbox_command_without_exec_path_fails() {
        let sandbox = MacOsSandbox::with_exec_path(None);
        let worker = Path::new("/opt/maekon/maekon-sandbox-worker");
        let err = sandbox
            .build_sandbox_command("(version 1)\n", worker)
            .unwrap_err();
        assert!(
            matches!(err, CoreError::SandboxUnsupported { .. }),
            "missing exec path must produce SandboxUnsupported, got: {err:?}"
        );
    }

    /// #6443 F6 — real-`sandbox-exec` enforcement proof.
    ///
    /// The cfg-free `sbpl` unit tests assert the profile *string* scopes
    /// `process-exec` to the worker literal. This test closes the loop on macOS:
    /// it runs the actual `/usr/bin/sandbox-exec` to prove that
    /// `(allow process-exec (literal "<path>"))` — the rule form the generator
    /// emits for the deny-by-default tiers — genuinely blocks execution of any
    /// binary other than the authorised one. A pre-#6443 blanket
    /// `(allow process-exec)` would let the worker exec an arbitrary shell.
    ///
    /// The base is `(allow default)` + a blanket `(deny process-exec*)` so the
    /// test isolates the `process-exec` literal from unrelated file-read/dyld
    /// concerns (a deny-default profile can fail to map the dyld shared cache on
    /// some macOS versions, which would confound "exec denied" with "binary
    /// failed to load"). The literal rule under test is byte-identical to the
    /// generator's. There is no macOS CI runner, so this runs locally / on any
    /// future macOS leg; it skips cleanly where `sandbox-exec` is unavailable.
    #[test]
    fn process_exec_literal_constraint_is_enforced_by_real_sandbox_exec() {
        let sandbox_exec = "/usr/bin/sandbox-exec";
        let authorised = "/usr/bin/true"; // exec authorised → must run (exit 0)
        let forbidden = "/bin/echo"; // not authorised → exec must be denied
        if !Path::new(sandbox_exec).exists()
            || !Path::new(authorised).exists()
            || !Path::new(forbidden).exists()
        {
            eprintln!("skipping: sandbox-exec or stand-in system binaries not found");
            return;
        }

        // Build the literal rule exactly as generate_sbpl_profile does, but on an
        // allow-default base scoped to process-exec only.
        let authorised_canon = std::fs::canonicalize(authorised)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| authorised.to_string());
        let profile = format!(
            "(version 1)\n(allow default)\n(deny process-exec*)\n(allow process-exec (literal \"{authorised_canon}\"))\n"
        );

        let run = |target: &str| {
            Command::new(sandbox_exec)
                .args(["-p", profile.as_str(), "--", target])
                .output()
                .expect("sandbox-exec spawns")
        };

        let allowed = run(&authorised_canon);
        let denied = run(forbidden);

        assert!(
            allowed.status.success(),
            "exec of the authorised literal must be allowed; stderr: {}",
            String::from_utf8_lossy(&allowed.stderr)
        );
        assert!(
            !denied.status.success(),
            "exec of a non-authorised binary must be denied by the process-exec \
             literal, but it succeeded (stdout: {:?})",
            String::from_utf8_lossy(&denied.stdout)
        );
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

        let err = sandbox
            .execute_sandboxed(&action, &config)
            .await
            .unwrap_err();
        assert!(
            matches!(err, CoreError::SandboxUnsupported { .. }),
            "missing exec path must produce SandboxUnsupported, got: {err:?}"
        );
        assert!(
            err.to_string().contains("sandbox-exec not found"),
            "SandboxUnsupported message must mention sandbox-exec, got: {err}"
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
