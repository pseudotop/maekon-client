// ADR-013 file split (#11218) — DONE. `mod tests` now lives in
// `windows_tests.rs`, attached with `#[path]`.
//
// The marker this replaces existed so the sixth cap raise would not be silent.
// It was raised silently anyway (1645 -> 1697, #11222), and the change that
// would have made it seven (#11213's ACL fixture work) is what triggered the
// split instead. History: 1595 -> 1603 -> 1606 -> 1611 -> 1645 -> 1697, each
// time to exactly the new line count, which returns headroom to zero — the
// ratchet erosion #9636 named.
//
// This file is now ~970 lines against a 900-line giant threshold, so it is not
// finished shrinking; the remaining growth pressure is production code, not
// tests, and splitting that needs a Windows host to verify. Raising the cap
// again should be read as a signal to split, not as routine.
//! Windows sandbox — Job Object resource limits + restricted token enforcement.
//!
//! **Enforcement model (subprocess):**
//! The sandbox spawns the `maekon-sandbox-worker` binary as a child process
//! inside a Win32 Job Object with configured resource limits. A restricted
//! token is created via `CreateRestrictedToken` and **applied** to the worker
//! by launching it with `CreateProcessAsUserW` (the `windows-sandbox` feature
//! path no longer uses `tokio::process::Command`), so the child runs under the
//! restricted token instead of inheriting the parent's unrestricted token.
//!
//! **Dual code paths (`windows-sandbox` feature):**
//! - **Enabled**: Real Win32 API calls — `CreateJobObjectW`, `SetInformationJobObject`,
//!   `CreateRestrictedToken`, `CreateProcessAsUserW` (restricted-token launch),
//!   `AssignProcessToJobObject`.
//! - **Disabled**: Log-only stubs that describe what *would* be enforced.
//!
//! **Enforcement status:**
//! - **Resource limits**: Enforced via Job Object (`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`).
//! - **Process isolation**: Enforced via subprocess model (child inherits Job Object).
//! - **Token restriction**: Enforced (feature on) — the restricted token demotes
//!   the `BUILTIN\Administrators` group to DENY-ONLY via `SidsToDisable` (#7071)
//!   whenever the policy sets `disable_admin_sid` (every profile), and adds
//!   `DISABLE_MAX_PRIVILEGE` for privilege-stripping profiles; the worker is then
//!   spawned under that token via `CreateProcessAsUserW` (raw spawn wrapped in
//!   `spawn_blocking`, with manual Job Object assignment + pipe wiring). Integrity
//!   is deliberately NOT lowered: the worker's sole purpose is to synthesize input
//!   (mouse/keyboard via `enigo`), and a Low-integrity process is blocked by UIPI
//!   from sending input to normal medium-integrity windows — lowering integrity
//!   would break the only reachable (`Permissive`) automation path. The deny-only
//!   admin SID delivers the privilege containment without that functional
//!   regression, so `privilege_restriction` is now backed by a real control.
//!   Standard/Strict additionally map `disable_most_sids` to the Windows Write
//!   Restricted Code SID (`S-1-5-33`) through `SidsToRestrict` plus the
//!   `WRITE_RESTRICTED` flag (#7979). Windows applies the second restricting-SID
//!   access check to writes while preserving DLL/executable reads needed to start
//!   the worker.
//! - **Filesystem isolation**: Not enforced (Job Objects do not isolate FS).
//! - **Syscall filtering**: Not available on Windows.
//! - **Network isolation**: Not enforced (would require Windows Firewall rules).
//!
//! Because Windows cannot enforce filesystem/syscall/network isolation, the
//! `Standard`/`Strict` profiles still fail CLOSED here (see
//! `missing_required_containment_for_profile`); only `Permissive` reaches the
//! spawn path, and it now runs under the restricted token.

use async_trait::async_trait;

use crate::error::AutomationError;
use crate::sandbox::ipc;
use crate::sandbox::win_limits::{
    build_job_limits, build_token_restrictions, missing_required_containment_for_profile,
    JobObjectLimits, TokenRestrictions,
};
use maekon_core::config::SandboxConfig;
use maekon_core::error::CoreError;
use maekon_core::models::automation::AutomationAction;
use maekon_core::ports::sandbox::{Sandbox, SandboxCapabilities};

// ── RAII handle wrapper ─────────────────────────────────────────────

/// RAII wrapper for Win32 HANDLE values. Calls `CloseHandle` on drop.
///
#[cfg(feature = "windows-sandbox")]
type Win32Handle = windows_sys::Win32::Foundation::HANDLE;

#[cfg(feature = "windows-sandbox")]
struct OwnedHandle(Win32Handle);

#[cfg(feature = "windows-sandbox")]
// SAFETY: Win32 HANDLE values are process-local opaque references. This wrapper
// owns exactly one handle and only transfers ownership so it can be used and
// closed from the async task after creation on a blocking thread.
unsafe impl Send for OwnedHandle {}

#[cfg(feature = "windows-sandbox")]
impl OwnedHandle {
    /// Returns `true` when the handle is neither null nor INVALID_HANDLE_VALUE.
    fn is_valid(&self) -> bool {
        !self.0.is_null() && self.0 != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
    }

    /// Transfer the raw handle out of this wrapper WITHOUT closing it; the caller
    /// takes over ownership (and the obligation to close it exactly once). Used to
    /// hand a pipe end to `std::fs::File`, which then owns and closes it on drop.
    fn into_raw(self) -> Win32Handle {
        let raw = self.0;
        std::mem::forget(self);
        raw
    }
}

#[cfg(feature = "windows-sandbox")]
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.is_valid() {
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}

// ── WindowsSandbox ──────────────────────────────────────────────────

pub struct WindowsSandbox {
    is_available: bool,
}

impl Default for WindowsSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsSandbox {
    pub fn new() -> Self {
        Self {
            is_available: check_windows_sandbox_support(),
        }
    }
}

#[async_trait]
impl Sandbox for WindowsSandbox {
    fn platform(&self) -> &str {
        "windows"
    }

    fn is_available(&self) -> bool {
        self.is_available
    }

    async fn execute_sandboxed(
        &self,
        action: &AutomationAction,
        config: &SandboxConfig,
    ) -> Result<(), CoreError> {
        if !self.is_available {
            return Err(CoreError::SandboxUnsupported {
                code: maekon_core::error_codes::SandboxCode::UnsupportedPlatform,
                message: "Windows sandbox not available on this platform".to_string(),
            });
        }

        // #6422: Windows currently enforces Job Object resource limits only. A
        // Standard/Strict profile advertises filesystem/syscall/network containment, so
        // fail CLOSED instead of auditing a resource-limited child as sandbox-contained.
        let capabilities = self.capabilities();
        if let Some(missing) = missing_required_containment_for_profile(
            config.profile,
            capabilities.filesystem_isolation,
            capabilities.syscall_filtering,
            capabilities.network_isolation,
            capabilities.privilege_restriction,
        ) {
            tracing::error!(
                profile = ?config.profile,
                missing_capabilities = %missing,
                "Windows sandbox refusing profile because requested containment is unavailable"
            );
            return Err(CoreError::SandboxUnsupported {
                code: maekon_core::error_codes::SandboxCode::UnsupportedPlatform,
                message: format!(
                    "Windows sandbox profile {:?} requires containment capabilities that are not \
                     enforced on this platform: {missing}. Refusing to run with Job Object \
                     resource limits only.",
                    config.profile
                ),
            });
        }

        let worker_path = ipc::resolve_worker_path()?;
        let request = ipc::SandboxRequest {
            action: action.clone(),
        };
        let request_json =
            serde_json::to_string(&request).map_err(|e| CoreError::SandboxExecution {
                code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!("serialize: {e}"),
            })?;

        let job_limits = build_job_limits(config);
        let token_restrictions = build_token_restrictions(config);

        let timeout_ms = if config.max_cpu_time_ms > 0 {
            config.max_cpu_time_ms + 5000
        } else {
            60_000
        };

        tracing::debug!(
            max_memory = job_limits.max_memory_bytes,
            max_cpu_ms = job_limits.max_cpu_time_ms,
            max_processes = job_limits.max_processes,
            disable_admin = token_restrictions.disable_admin_sid,
            timeout_ms,
            action = %super::redact_action(action),
            "Windows sandbox spawning worker subprocess"
        );

        // ── Feature path: spawn the worker UNDER the restricted token ──────
        //
        // All Win32 work (Job Object creation, restricted token creation, the
        // CreateProcessAsUserW launch, Job Object assignment, pipe I/O and the
        // bounded wait) is synchronous, so it runs in a single `spawn_blocking`
        // task. `tokio::process::Command` is intentionally NOT used here: it has
        // no token-injection API, which is exactly why the token was previously
        // created but never applied. `run_worker_under_restricted_token` launches
        // the worker with `CreateProcessAsUserW(restricted_token, …)` so the child
        // runs under the restricted token instead of the parent's full token.
        #[cfg(feature = "windows-sandbox")]
        {
            let outcome = tokio::task::spawn_blocking(move || {
                run_worker_under_restricted_token(
                    &worker_path,
                    request_json.as_bytes(),
                    &job_limits,
                    &token_restrictions,
                    timeout_ms,
                )
            })
            .await
            .map_err(|e| CoreError::SandboxExecution {
                code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!("thread join failed: {e}"),
            })?
            .map_err(CoreError::from)?;

            if outcome.timed_out {
                return Err(CoreError::ExecutionTimeout {
                    code: maekon_core::error_codes::SandboxCode::Timeout,
                    timeout_ms,
                });
            }

            if outcome.exit_code != 0 {
                let stderr = String::from_utf8_lossy(&outcome.stderr);
                return Err(CoreError::SandboxExecution {
                    code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!(
                        "child exited code {} -- stderr: {}",
                        outcome.exit_code,
                        stderr.trim()
                    ),
                });
            }

            let response = ipc::parse_worker_response(&outcome.stdout)?;
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
                "Windows sandbox execution completed via worker (restricted token applied)"
            );
            return Ok(());
        }

        // ── Stub path: `windows-sandbox` feature disabled ─────────────────
        //
        // Unreachable in practice — `check_windows_sandbox_support()` is false
        // without the feature, so `is_available()` is false and the early return
        // at the top of this method fires first (and the factory hands back a
        // `FailClosedSandbox`). It is kept only so the crate compiles without the
        // feature; it provides NO Job Object or restricted-token enforcement.
        #[cfg(not(feature = "windows-sandbox"))]
        {
            create_job_object(&job_limits)?;
            create_restricted_token(&token_restrictions)?;

            // `kill_on_drop(true)` reaps the child if this `Child` is dropped
            // (e.g. on timeout). `env_clear()` keeps parent secrets out of the
            // worker environment (#6827).
            let mut cmd = tokio::process::Command::new(worker_path);
            cmd.kill_on_drop(true);
            cmd.env_clear();
            cmd.stdin(std::process::Stdio::piped());
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());

            let mut child = cmd.spawn().map_err(|e| CoreError::SandboxExecution {
                code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!("spawn failed: {e}"),
            })?;

            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                stdin
                    .write_all(request_json.as_bytes())
                    .await
                    .map_err(|e| CoreError::SandboxExecution {
                        code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                        message: format!("stdin write: {e}"),
                    })?;
                stdin
                    .write_all(b"\n")
                    .await
                    .map_err(|e| CoreError::SandboxExecution {
                        code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                        message: format!("stdin newline: {e}"),
                    })?;
                drop(stdin);
            }

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
                        message: format!("wait failed: {e}"),
                    })
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
                return Err(CoreError::SandboxExecution {
                    code: maekon_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!(
                        "child exited {} -- stderr: {}",
                        output.status,
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
                "Windows sandbox execution completed via worker"
            );
            return Ok(());
        }
    }

    /// Report capabilities based on actual enforcement availability.
    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            filesystem_isolation: false, // Job Objects do not isolate filesystem
            syscall_filtering: false,    // Not available on Windows
            network_isolation: false,    // Would need Windows Firewall rules
            #[cfg(feature = "windows-sandbox")]
            resource_limits: true, // ENFORCED via Job Object
            #[cfg(not(feature = "windows-sandbox"))]
            resource_limits: false,
            #[cfg(feature = "windows-sandbox")]
            process_isolation: true, // ENFORCED via subprocess + Job Object
            #[cfg(not(feature = "windows-sandbox"))]
            process_isolation: false,
            // ENFORCED (feature on): the worker is spawned under the restricted
            // token via CreateProcessAsUserW, which demotes the Administrators
            // group to deny-only (SidsToDisable, #7071) and strips privileges for
            // privilege-stripping profiles, so the worker no longer inherits the
            // parent's full token. (Integrity is intentionally not lowered — see
            // the module-level "Token restriction" note: it would break UIPI input.)
            #[cfg(feature = "windows-sandbox")]
            privilege_restriction: true,
            #[cfg(not(feature = "windows-sandbox"))]
            privilege_restriction: false,
        }
    }
}

// `JobObjectLimits` / `TokenRestrictions` + their builders now live in the
// cfg-free `super::win_limits` module (#5138); the Win32 enforcement below
// consumes them via the import at the top of this file.

fn check_windows_sandbox_support() -> bool {
    // review4 A2: the Job-Object enforcement below is gated behind the
    // `windows-sandbox` cargo feature. Without it the adapter compiles only the
    // no-op execution path, so reporting "available" on any Windows build let an
    // enabled sandbox run actions uncontained while claiming success. Require the
    // feature so a feature-less build reports unavailable → the factory fails
    // closed instead of silently degrading.
    cfg!(target_os = "windows") && cfg!(feature = "windows-sandbox")
}

// ── Win32 enforcement (feature = "windows-sandbox") ─────────────────

/// Create a Win32 Job Object with configured resource limits.
///
/// Configures `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` with memory, CPU time,
/// and active process count limits based on the sandbox profile.
#[cfg(feature = "windows-sandbox")]
fn create_job_object(limits: &JobObjectLimits) -> Result<OwnedHandle, AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::JobObjects::*;

    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreateJobObjectW failed: error {err}"
        )));
    }
    let job = OwnedHandle(handle);

    // Always configure at minimum JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE so that
    // closing this Job Object handle (on scope-drop or process exit) sends
    // TerminateProcess to every process still assigned to the job. This prevents
    // orphaned worker trees when the parent crashes or the timeout fires.
    {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE is always set unconditionally.
        let mut limit_flags: u32 = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

        if limits.max_memory_bytes > 0 {
            info.ProcessMemoryLimit = limits.max_memory_bytes as usize;
            limit_flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        }
        if limits.max_cpu_time_ms > 0 {
            // Convert milliseconds to 100-nanosecond units
            info.BasicLimitInformation.PerJobUserTimeLimit =
                (limits.max_cpu_time_ms as i64) * 10_000;
            limit_flags |= JOB_OBJECT_LIMIT_JOB_TIME;
        }
        if limits.max_processes > 0 {
            info.BasicLimitInformation.ActiveProcessLimit = limits.max_processes;
            limit_flags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        }
        info.BasicLimitInformation.LimitFlags = limit_flags;

        let ret = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ret == 0 {
            let err = unsafe { GetLastError() };
            return Err(AutomationError::SandboxEnforcement(format!(
                "SetInformationJobObject failed: error {err}"
            )));
        }
    }

    tracing::debug!(
        memory = limits.max_memory_bytes,
        cpu_ms = limits.max_cpu_time_ms,
        processes = limits.max_processes,
        "Job Object created with limits"
    );
    Ok(job)
}

/// Log-only stub when `windows-sandbox` feature is disabled.
#[cfg(not(feature = "windows-sandbox"))]
fn create_job_object(limits: &JobObjectLimits) -> Result<(), AutomationError> {
    tracing::debug!(
        memory = limits.max_memory_bytes,
        cpu_ms = limits.max_cpu_time_ms,
        processes = limits.max_processes,
        "Job Object (enforcement requires windows-sandbox feature)"
    );
    Ok(())
}

#[cfg(feature = "windows-sandbox")]
#[path = "windows_restricted_token.rs"]
mod restricted_token;

#[cfg(feature = "windows-sandbox")]
use restricted_token::create_restricted_token;

#[cfg(all(test, feature = "windows-sandbox"))]
use restricted_token::{build_logon_sid, build_user_sid, build_write_restricted_code_sid};

/// Log-only stub when `windows-sandbox` feature is disabled.
///
/// Token restriction is unenforced in this path: without the feature the worker
/// is launched via `tokio::process::Command`, which cannot apply a token. (The
/// full Win32 implementation above DOES apply the token via `CreateProcessAsUserW`.)
#[cfg(not(feature = "windows-sandbox"))]
fn create_restricted_token(restrictions: &TokenRestrictions) -> Result<(), AutomationError> {
    tracing::debug!(
        disable_admin = restrictions.disable_admin_sid,
        remove_privs = restrictions.remove_privileges,
        restrict_most_sids = restrictions.disable_most_sids,
        "Restricted Token stub (windows-sandbox feature disabled; token restriction unenforced)"
    );
    Ok(())
}

/// Assign a child process to a Job Object for resource limit enforcement.
///
/// Must be called after spawning the child but before it exits, so the Job
/// Object limits apply for the lifetime of the child. The worker is created
/// SUSPENDED and assigned here before being resumed, so it cannot run (and thus
/// cannot escape the job) before the limits bind.
#[cfg(feature = "windows-sandbox")]
fn assign_process_to_job(
    job: &OwnedHandle,
    child_handle: Win32Handle,
) -> Result<(), AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

    // SAFETY: `job.0` is a valid Job Object handle and `child_handle` is a valid
    // process handle (just returned by CreateProcessAsUserW). Neither is consumed.
    let ret = unsafe { AssignProcessToJobObject(job.0, child_handle) };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "AssignProcessToJobObject failed: error {err}"
        )));
    }
    tracing::debug!("Child process assigned to Job Object");
    Ok(())
}

// ── Restricted-token worker launch (feature = "windows-sandbox") ─────
//
// The restricted token created above is APPLIED to the worker here, closing the
// gap where the token was built but the worker still inherited the parent's full
// token. The launch path is raw Win32 (`CreateProcessAsUserW`) because
// `tokio::process::Command` exposes no token-injection API. The whole launch is
// synchronous and is driven from a `spawn_blocking` task by `execute_sandboxed`.

/// Result of running the worker under the restricted token.
#[cfg(feature = "windows-sandbox")]
struct WorkerOutcome {
    /// `true` when the worker exceeded its timeout and was terminated.
    timed_out: bool,
    /// The worker process exit code (`1` is synthesized on timeout).
    exit_code: u32,
    /// Captured stdout bytes (the worker writes its SandboxResponse JSON here).
    stdout: Vec<u8>,
    /// Captured stderr bytes (surfaced in error messages).
    stderr: Vec<u8>,
}

/// Create an anonymous pipe whose two ends are inheritable by a child process.
///
/// Returns `(read_end, write_end)`. The caller marks the parent-retained end
/// non-inheritable via [`set_handle_non_inheritable`] so only the intended end
/// crosses into the child.
#[cfg(feature = "windows-sandbox")]
fn create_inheritable_pipe() -> Result<(OwnedHandle, OwnedHandle), AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let mut read: Win32Handle = std::ptr::null_mut();
    let mut write: Win32Handle = std::ptr::null_mut();
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1, // TRUE: the child must be able to inherit the pipe ends
    };

    // SAFETY: `read`/`write` are valid out-pointers; `security` lives for the
    // call. CreatePipe writes two fresh, owned handles on success.
    let ret = unsafe { CreatePipe(&mut read, &mut write, &security, 0) };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreatePipe failed: error {err}"
        )));
    }
    Ok((OwnedHandle(read), OwnedHandle(write)))
}

/// Clear `HANDLE_FLAG_INHERIT` on a handle so it is NOT inherited by the child.
///
/// Applied to the parent-retained pipe ends: if the child inherited the parent's
/// write end of the stdin pipe (or read ends of stdout/stderr) it would never see
/// EOF, deadlocking the I/O.
#[cfg(feature = "windows-sandbox")]
fn set_handle_non_inheritable(handle: &OwnedHandle) -> Result<(), AutomationError> {
    use windows_sys::Win32::Foundation::{GetLastError, SetHandleInformation, HANDLE_FLAG_INHERIT};

    // SAFETY: `handle.0` is a valid owned Win32 handle for the duration of the call.
    let ret = unsafe { SetHandleInformation(handle.0, HANDLE_FLAG_INHERIT, 0) };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "SetHandleInformation failed: error {err}"
        )));
    }
    Ok(())
}

/// Build the Job Object + restricted token and launch the worker under the token.
///
/// Creates the resource-limited Job Object and the PRIMARY restricted token, then
/// hands them to [`spawn_process_with_token`] which performs the actual
/// `CreateProcessAsUserW` launch, Job Object assignment, pipe I/O and bounded
/// wait. The worker environment is cleared (`clear_env = true`) so parent secrets
/// never cross the sandbox boundary (#6827). The job/token are held alive for the
/// whole call so the limits/token bind for the worker's entire lifetime.
#[cfg(feature = "windows-sandbox")]
fn run_worker_under_restricted_token(
    worker_path: &std::path::Path,
    request_bytes: &[u8],
    job_limits: &JobObjectLimits,
    token_restrictions: &TokenRestrictions,
    timeout_ms: u64,
) -> Result<WorkerOutcome, AutomationError> {
    use std::os::windows::ffi::OsStrExt;

    let job = create_job_object(job_limits)?;
    let token = create_restricted_token(token_restrictions)?;

    // Launch by absolute application name with a null command line (the worker
    // takes no arguments). The name is a NUL-terminated wide string.
    let application_name: Vec<u16> = worker_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let current_directory: Option<Vec<u16>> = worker_path.parent().map(|parent| {
        parent
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    });

    spawn_process_with_token(
        &application_name,
        None,
        current_directory.as_deref(),
        &job,
        &token,
        request_bytes,
        timeout_ms,
        true, // clear_env: keep parent secrets out of the worker (#6827)
    )
}

/// Launch a process under `token`, wired to anonymous pipes, assigned to `job`,
/// and wait for it (bounded by `timeout_ms`). Returns the captured stdout/stderr
/// and exit status. Factored out so the FFI launch path can be exercised by a
/// Windows CI runtime test against a known system binary (`windows.rs` is only
/// compiled on Windows, so this is the only place the launch path can be tested).
///
/// `application_name` is a NUL-terminated wide string. `command_line`, when
/// `Some`, is a NUL-terminated mutable wide buffer (CreateProcess may modify it).
/// `current_directory`, when `Some`, is a NUL-terminated wide string.
/// When `clear_env` is true the child is launched with an empty environment.
#[cfg(feature = "windows-sandbox")]
fn spawn_process_with_token(
    application_name: &[u16],
    command_line: Option<&mut [u16]>,
    current_directory: Option<&[u16]>,
    job: &OwnedHandle,
    token: &OwnedHandle,
    stdin_bytes: &[u8],
    timeout_ms: u64,
    clear_env: bool,
) -> Result<WorkerOutcome, AutomationError> {
    use std::io::{Read, Write};
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::Foundation::{GetLastError, WAIT_FAILED, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        CreateProcessAsUserW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, ResumeThread, TerminateProcess,
        UpdateProcThreadAttribute, WaitForSingleObject, CREATE_SUSPENDED,
        CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, EXTENDED_STARTUPINFO_PRESENT,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
    };

    struct ProcThreadAttributeListGuard(LPPROC_THREAD_ATTRIBUTE_LIST);

    impl Drop for ProcThreadAttributeListGuard {
        fn drop(&mut self) {
            // SAFETY: The guarded pointer was initialized by
            // InitializeProcThreadAttributeList and remains valid until this drop.
            unsafe { DeleteProcThreadAttributeList(self.0) };
        }
    }

    // Anonymous pipes. The child end of each pipe is inheritable; the
    // parent-retained end is marked non-inheritable so EOF propagates correctly.
    let (stdin_read, stdin_write) = create_inheritable_pipe()?;
    let (stdout_read, stdout_write) = create_inheritable_pipe()?;
    let (stderr_read, stderr_write) = create_inheritable_pipe()?;
    set_handle_non_inheritable(&stdin_write)?;
    set_handle_non_inheritable(&stdout_read)?;
    set_handle_non_inheritable(&stderr_read)?;

    // Redirect the child's std handles to the child-side pipe ends.
    // SAFETY: STARTUPINFOEXW is plain-old-data; zeroing then filling fields is valid.
    let mut startup_ex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup_ex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup_ex.StartupInfo.hStdInput = stdin_read.0;
    startup_ex.StartupInfo.hStdOutput = stdout_write.0;
    startup_ex.StartupInfo.hStdError = stderr_write.0;

    // bInheritHandles must be TRUE for STARTF_USESTDHANDLES, but unrestricted
    // inheritance can leak unrelated runner/app handles into the restricted child.
    // STARTUPINFOEX lets us allow-list only the three pipe ends the child needs.
    let inheritable_handles = [stdin_read.0, stdout_write.0, stderr_write.0];
    let mut attribute_list_size = 0usize;
    // SAFETY: The first call intentionally probes the required buffer size.
    unsafe {
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attribute_list_size)
    };
    if attribute_list_size == 0 {
        return Err(AutomationError::SandboxEnforcement(
            "InitializeProcThreadAttributeList size probe returned zero".to_string(),
        ));
    }
    let mut attribute_list_storage = vec![0u8; attribute_list_size];
    let attribute_list = attribute_list_storage.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
    // SAFETY: `attribute_list_storage` is sized from the probe above and stays
    // alive until after CreateProcessAsUserW returns.
    let initialized = unsafe {
        InitializeProcThreadAttributeList(attribute_list, 1, 0, &mut attribute_list_size)
    };
    if initialized == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "InitializeProcThreadAttributeList failed: error {err}"
        )));
    }
    let _attribute_list_guard = ProcThreadAttributeListGuard(attribute_list);
    // SAFETY: `inheritable_handles` contains valid inheritable pipe handles and
    // remains alive until after CreateProcessAsUserW consumes the attribute list.
    let updated = unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            inheritable_handles.as_ptr() as *const core::ffi::c_void,
            std::mem::size_of_val(&inheritable_handles),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if updated == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "UpdateProcThreadAttribute(PROC_THREAD_ATTRIBUTE_HANDLE_LIST) failed: error {err}"
        )));
    }
    startup_ex.lpAttributeList = attribute_list;

    // CREATE_SUSPENDED: assign to the Job Object before the child runs.
    //
    // DETACHED_PROCESS, not CREATE_NO_WINDOW (#10288): both hide the console
    // window, but CREATE_NO_WINDOW still *allocates* a console for the child,
    // and that allocation goes through conhost/CSRSS. A restricted token cannot
    // complete it, so the child dies during initialization and surfaces as
    // STATUS_DLL_INIT_FAILED (0xC0000142) — the whole of #10288. DETACHED_PROCESS
    // allocates no console at all, which is what this worker actually wants: all
    // three std handles are redirected to the pipes above, so it never needs one.
    //
    // Measured on Windows 11 client (26200.9168) with the production token
    // shapes, full spawn path, console flag as the only variable:
    //   Permissive      CREATE_NO_WINDOW 0xC0000142 | DETACHED_PROCESS exit 7
    //   Standard/Strict CREATE_NO_WINDOW 0xC0000142 | DETACHED_PROCESS exit 7
    // The token restrictions are unchanged by this fix; containment is identical.
    let mut creation_flags = CREATE_SUSPENDED | DETACHED_PROCESS | EXTENDED_STARTUPINFO_PRESENT;

    // Empty Unicode environment block (two NUL WCHARs) when clearing the env.
    let empty_env: [u16; 2] = [0, 0];
    let environment: *const core::ffi::c_void = if clear_env {
        creation_flags |= CREATE_UNICODE_ENVIRONMENT;
        empty_env.as_ptr() as *const core::ffi::c_void
    } else {
        std::ptr::null()
    };

    let command_line_ptr: *mut u16 = match command_line {
        Some(buffer) => buffer.as_mut_ptr(),
        None => std::ptr::null_mut(),
    };
    let current_directory_ptr: *const u16 = match current_directory {
        Some(buffer) => buffer.as_ptr(),
        None => std::ptr::null(),
    };

    // SAFETY: PROCESS_INFORMATION is plain-old-data; zeroing is a valid init.
    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    // SAFETY: `token.0` is a valid PRIMARY restricted token (opened with
    // TOKEN_ASSIGN_PRIMARY | TOKEN_DUPLICATE | TOKEN_QUERY). `application_name` is
    // NUL-terminated wide; `command_line_ptr` is null or a NUL-terminated mutable
    // wide buffer. `startup_ex`/`process_info` are valid in/out pointers and the
    // allow-listed std handles in `startup_ex` are live for the call.
    // `environment` is null or points to a valid double-NUL-terminated block.
    // `current_directory_ptr` is null or a NUL-terminated wide directory string.
    // CreateProcessAsUserW does not take ownership of any handle passed to it.
    let created = unsafe {
        CreateProcessAsUserW(
            token.0,
            application_name.as_ptr(),
            command_line_ptr,
            std::ptr::null(),
            std::ptr::null(),
            1, // bInheritHandles = TRUE: child inherits the std* pipe ends
            creation_flags,
            environment,
            current_directory_ptr,
            (&startup_ex as *const STARTUPINFOEXW).cast::<STARTUPINFOW>(),
            &mut process_info,
        )
    };
    if created == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreateProcessAsUserW failed: error {err} (restricted-token launch)"
        )));
    }
    // RAII for the process + primary-thread handles returned by CreateProcess.
    let process = OwnedHandle(process_info.hProcess);
    let thread = OwnedHandle(process_info.hThread);

    // The child holds its own inherited copies of the child-side pipe ends; close
    // the parent's copies so the parent's read ends see EOF when the child exits
    // and the child's stdin sees EOF when we close our write end.
    drop(stdin_read);
    drop(stdout_write);
    drop(stderr_write);

    // Constrain the still-suspended child to the Job Object BEFORE it runs, then
    // resume it. Terminate on any failure so a suspended child never leaks.
    if let Err(e) = assign_process_to_job(job, process.0) {
        // SAFETY: `process.0` is a valid, still-suspended process handle.
        unsafe { TerminateProcess(process.0, 1) };
        return Err(e);
    }
    // SAFETY: `thread.0` is the valid primary-thread handle from CreateProcess.
    let resume = unsafe { ResumeThread(thread.0) };
    if resume == u32::MAX {
        let err = unsafe { GetLastError() };
        // SAFETY: `process.0` is valid; terminate the child that never resumed.
        unsafe { TerminateProcess(process.0, 1) };
        return Err(AutomationError::SandboxEnforcement(format!(
            "ResumeThread failed: error {err}"
        )));
    }
    drop(thread); // primary-thread handle no longer needed

    // Wrap the parent-side pipe ends in std::fs::File so safe std I/O closes them
    // on drop. Each raw handle is transferred out of its OwnedHandle exactly once
    // (`into_raw` forgets the wrapper), so File owns and closes it exactly once.
    // SAFETY: each handle is a unique, owned, open pipe end produced above.
    let mut stdin_file = unsafe { std::fs::File::from_raw_handle(stdin_write.into_raw() as _) };
    let stdout_file = unsafe { std::fs::File::from_raw_handle(stdout_read.into_raw() as _) };
    let stderr_file = unsafe { std::fs::File::from_raw_handle(stderr_read.into_raw() as _) };

    // Drain stdout/stderr on dedicated threads so a full pipe buffer cannot
    // deadlock against our stdin write / wait. (`File` is `Send`.)
    let stdout_reader = std::thread::spawn(move || {
        let mut file = stdout_file;
        let mut buffer = Vec::new();
        let _ = file.read_to_end(&mut buffer);
        buffer
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut file = stderr_file;
        let mut buffer = Vec::new();
        let _ = file.read_to_end(&mut buffer);
        buffer
    });

    // Send the request, then close stdin so the worker observes EOF on its one
    // line of input.
    let write_result = stdin_file
        .write_all(stdin_bytes)
        .and_then(|()| stdin_file.write_all(b"\n"));
    drop(stdin_file);
    if let Err(e) = write_result {
        // SAFETY: `process.0` is valid; reap the child we can no longer feed.
        unsafe { TerminateProcess(process.0, 1) };
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(AutomationError::SandboxExecution(format!(
            "stdin write: {e}"
        )));
    }

    // Wait for the child, bounded by the timeout (clamped to the u32 API range).
    let wait_ms = timeout_ms.min(u32::MAX as u64) as u32;
    // SAFETY: `process.0` is a valid process handle.
    let wait = unsafe { WaitForSingleObject(process.0, wait_ms) };
    if wait == WAIT_FAILED {
        let err = unsafe { GetLastError() };
        // SAFETY: `process.0` is valid; reap before returning.
        unsafe { TerminateProcess(process.0, 1) };
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        return Err(AutomationError::SandboxExecution(format!(
            "WaitForSingleObject failed: error {err}"
        )));
    }
    let timed_out = wait == WAIT_TIMEOUT;
    if timed_out {
        // SAFETY: `process.0` is valid. Terminate the timed-out worker; the Job
        // Object's KILL_ON_JOB_CLOSE (on `job` drop) also reaps any descendants.
        unsafe { TerminateProcess(process.0, 1) };
    }

    // The reader threads finish once the child's pipe ends hit EOF (on exit or
    // termination). Joining here guarantees full capture before we return.
    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    let exit_code = if timed_out {
        1
    } else {
        let mut code: u32 = 0;
        // SAFETY: `process.0` is valid; `code` is a valid out-pointer.
        let ok = unsafe { GetExitCodeProcess(process.0, &mut code) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(AutomationError::SandboxExecution(format!(
                "GetExitCodeProcess failed: error {err}"
            )));
        }
        code
    };

    // `process`/`token`/`job` (held by the caller) drop here → handles closed;
    // closing the job reaps any stragglers via KILL_ON_JOB_CLOSE.
    Ok(WorkerOutcome {
        timed_out,
        exit_code,
        stdout,
        stderr,
    })
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
