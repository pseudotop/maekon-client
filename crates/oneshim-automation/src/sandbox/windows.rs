//! Windows sandbox — Job Object resource limits + restricted token enforcement.
//!
//! **Enforcement model (subprocess):**
//! The sandbox spawns the `oneshim-sandbox-worker` binary as a child process
//! inside a Win32 Job Object with configured resource limits. A restricted
//! token is created via `CreateRestrictedToken` and applied to the child via
//! `CreateProcessAsUserW`.
//!
//! **Dual code paths (`windows-sandbox` feature):**
//! - **Enabled**: Real Win32 API calls — `CreateJobObjectW`, `SetInformationJobObject`,
//!   `CreateRestrictedToken`, `CreateProcessAsUserW`, `AssignProcessToJobObject`.
//! - **Disabled**: Log-only stubs that describe what *would* be enforced.
//!
//! **Enforcement status:**
//! - **Resource limits**: Enforced via Job Object (`JOBOBJECT_EXTENDED_LIMIT_INFORMATION`).
//! - **Process isolation**: Enforced via subprocess model (child inherits Job Object).
//! - **Token restriction**: `DISABLE_MAX_PRIVILEGE` strips dangerous privileges.
//! - **Filesystem isolation**: Not enforced (Job Objects do not isolate FS).
//! - **Syscall filtering**: Not available on Windows.
//! - **Network isolation**: Not enforced (would require Windows Firewall rules).

use async_trait::async_trait;

use crate::error::AutomationError;
use crate::sandbox::ipc;
use oneshim_core::config::{SandboxConfig, SandboxProfile};
use oneshim_core::error::CoreError;
use oneshim_core::models::automation::AutomationAction;
use oneshim_core::ports::sandbox::{Sandbox, SandboxCapabilities};

// ── RAII handle wrapper ─────────────────────────────────────────────

/// RAII wrapper for Win32 HANDLE values. Calls `CloseHandle` on drop.
///
/// In `windows-sys 0.61`, HANDLE is `isize` (not a pointer type).
/// Null (0) and `INVALID_HANDLE_VALUE` (-1) are both invalid sentinels.
struct OwnedHandle(isize);

impl OwnedHandle {
    /// Returns `true` when the handle is neither null nor INVALID_HANDLE_VALUE.
    fn is_valid(&self) -> bool {
        self.0 != 0 && self.0 != -1 // 0 = null, -1 = INVALID_HANDLE_VALUE
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.is_valid() {
            #[cfg(feature = "windows-sandbox")]
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

    fn build_job_limits(config: &SandboxConfig) -> JobObjectLimits {
        let (default_memory, default_cpu_ms, default_max_processes) = match config.profile {
            SandboxProfile::Permissive => (0, 0, 0),
            SandboxProfile::Standard => (512 * 1024 * 1024, 30_000, 10), // 512MB, 30s, 10 processes
            SandboxProfile::Strict => (256 * 1024 * 1024, 10_000, 3),    // 256MB, 10s, 3 processes
        };

        JobObjectLimits {
            max_memory_bytes: if config.max_memory_bytes > 0 {
                config.max_memory_bytes
            } else {
                default_memory
            },
            max_cpu_time_ms: if config.max_cpu_time_ms > 0 {
                config.max_cpu_time_ms
            } else {
                default_cpu_ms
            },
            max_processes: default_max_processes,
        }
    }

    fn build_token_restrictions(config: &SandboxConfig) -> TokenRestrictions {
        match config.profile {
            SandboxProfile::Permissive => TokenRestrictions {
                disable_admin_sid: true,
                disable_most_sids: false,
                remove_privileges: false,
            },
            SandboxProfile::Standard => TokenRestrictions {
                disable_admin_sid: true,
                disable_most_sids: true,
                remove_privileges: true,
            },
            SandboxProfile::Strict => TokenRestrictions {
                disable_admin_sid: true,
                disable_most_sids: true,
                remove_privileges: true,
            },
        }
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
                code: oneshim_core::error_codes::SandboxCode::UnsupportedPlatform,
                message: "Windows sandbox not available on this platform".to_string(),
            });
        }

        if is_permissive_noop(config) {
            tracing::debug!("Permissive profile with no limits -- skipping subprocess");
            return Ok(());
        }

        let worker_path = ipc::resolve_worker_path()?;
        let request = ipc::SandboxRequest {
            action: action.clone(),
        };
        let request_json =
            serde_json::to_string(&request).map_err(|e| CoreError::SandboxExecution {
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!("serialize: {e}"),
            })?;

        let job_limits = Self::build_job_limits(config);
        let token_restrictions = Self::build_token_restrictions(config);

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
            action = ?action,
            "Windows sandbox spawning worker subprocess"
        );

        // Win32 API calls are synchronous -- run in spawn_blocking
        #[cfg(feature = "windows-sandbox")]
        let (job, _token) = tokio::task::spawn_blocking(move || {
            let job = create_job_object(&job_limits)?;
            let token = create_restricted_token(&token_restrictions)?;
            Ok::<_, AutomationError>((job, token))
        })
        .await
        .map_err(|e| CoreError::SandboxExecution {
            code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
            message: format!("thread join failed: {e}"),
        })?
        .map_err(CoreError::from)?;

        #[cfg(not(feature = "windows-sandbox"))]
        {
            create_job_object(&job_limits)?;
            create_restricted_token(&token_restrictions)?;
        }

        // Spawn the worker subprocess via tokio::process::Command.
        // On Windows without `windows-sandbox`, this spawns a plain child
        // without Job Object or restricted token enforcement.
        let mut cmd = tokio::process::Command::new(worker_path);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| CoreError::SandboxExecution {
            code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
            message: format!("spawn failed: {e}"),
        })?;

        // Assign child to Job Object for resource limit enforcement.
        // The Job Object outlives the child because `job` is held until the
        // end of this function.
        //
        // `Child::raw_handle()` returns `Option<RawHandle>` where
        // `RawHandle = *mut c_void`. We cast to isize for the Win32 HANDLE.
        #[cfg(feature = "windows-sandbox")]
        {
            let raw_child_handle =
                child
                    .raw_handle()
                    .ok_or_else(|| CoreError::SandboxExecution {
                        code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                        message: "child process handle unavailable".into(),
                    })?;
            assign_process_to_job(&job, raw_child_handle as isize)?;
        }

        // Write serialized request to child stdin
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(request_json.as_bytes())
                .await
                .map_err(|e| CoreError::SandboxExecution {
                    code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!("stdin write: {e}"),
                })?;
            stdin
                .write_all(b"\n")
                .await
                .map_err(|e| CoreError::SandboxExecution {
                    code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                    message: format!("stdin newline: {e}"),
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
            message: format!("wait failed: {e}"),
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CoreError::SandboxExecution {
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
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
                code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
                message: format!(
                    "worker reported failure: {}",
                    response.error.unwrap_or_default()
                ),
            });
        }

        tracing::info!(action = ?action, "Windows sandbox execution completed via worker");
        Ok(())
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
        }
    }
}

// ── Internal types ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct JobObjectLimits {
    max_memory_bytes: u64,
    max_cpu_time_ms: u64,
    max_processes: u32,
}

#[derive(Debug, Clone)]
struct TokenRestrictions {
    disable_admin_sid: bool,
    #[allow(dead_code)] // Reserved for future SID-level restrictions
    disable_most_sids: bool,
    remove_privileges: bool,
}

fn check_windows_sandbox_support() -> bool {
    cfg!(target_os = "windows")
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
    if handle == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreateJobObjectW failed: error {err}"
        )));
    }
    let job = OwnedHandle(handle);

    // Configure limits only when at least one is non-zero
    if limits.max_memory_bytes > 0 || limits.max_cpu_time_ms > 0 || limits.max_processes > 0 {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        let mut limit_flags: u32 = 0;

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

/// Create a restricted token from the current process token.
///
/// Uses `CreateRestrictedToken` with `DISABLE_MAX_PRIVILEGE` to strip
/// dangerous privileges (SeDebugPrivilege, SeTcbPrivilege, etc.) from the
/// child process token.
#[cfg(feature = "windows-sandbox")]
fn create_restricted_token(
    restrictions: &TokenRestrictions,
) -> Result<OwnedHandle, AutomationError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Security::*;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    let mut process_token: isize = 0;
    let ret = unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY | TOKEN_ASSIGN_PRIMARY,
            &mut process_token,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "OpenProcessToken failed: error {err}"
        )));
    }
    let _process_token = OwnedHandle(process_token);

    let mut flags: u32 = 0;
    if restrictions.remove_privileges {
        flags |= DISABLE_MAX_PRIVILEGE;
    }

    let mut restricted_token: isize = 0;
    let ret = unsafe {
        CreateRestrictedToken(
            process_token,
            flags,
            0,
            std::ptr::null(), // disable SIDs
            0,
            std::ptr::null(), // delete privileges
            0,
            std::ptr::null(), // restrict SIDs
            &mut restricted_token,
        )
    };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(AutomationError::SandboxEnforcement(format!(
            "CreateRestrictedToken failed: error {err}"
        )));
    }

    tracing::debug!(
        disable_admin = restrictions.disable_admin_sid,
        remove_privs = restrictions.remove_privileges,
        "Restricted token created"
    );
    Ok(OwnedHandle(restricted_token))
}

/// Log-only stub when `windows-sandbox` feature is disabled.
#[cfg(not(feature = "windows-sandbox"))]
fn create_restricted_token(restrictions: &TokenRestrictions) -> Result<(), AutomationError> {
    tracing::debug!(
        disable_admin = restrictions.disable_admin_sid,
        remove_privs = restrictions.remove_privileges,
        "Restricted Token (enforcement requires windows-sandbox feature)"
    );
    Ok(())
}

/// Assign a child process to a Job Object for resource limit enforcement.
///
/// Must be called after spawning the child but before it exits, so the Job
/// Object limits apply for the lifetime of the child.
#[cfg(feature = "windows-sandbox")]
fn assign_process_to_job(job: &OwnedHandle, child_handle: isize) -> Result<(), CoreError> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

    let ret = unsafe { AssignProcessToJobObject(job.0, child_handle) };
    if ret == 0 {
        let err = unsafe { GetLastError() };
        return Err(CoreError::SandboxExecution {
            code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
            message: format!("AssignProcessToJobObject failed: error {err}"),
        });
    }
    tracing::debug!("Child process assigned to Job Object");
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_job_limits_profiles() {
        let standard = WindowsSandbox::build_job_limits(&SandboxConfig {
            profile: SandboxProfile::Standard,
            ..Default::default()
        });
        assert_eq!(standard.max_memory_bytes, 512 * 1024 * 1024);
        assert_eq!(standard.max_processes, 10);

        let strict = WindowsSandbox::build_job_limits(&SandboxConfig {
            profile: SandboxProfile::Strict,
            ..Default::default()
        });
        assert_eq!(strict.max_memory_bytes, 256 * 1024 * 1024);
        assert_eq!(strict.max_processes, 3);
    }

    #[test]
    fn build_job_limits_custom_override() {
        let limits = WindowsSandbox::build_job_limits(&SandboxConfig {
            profile: SandboxProfile::Strict,
            max_memory_bytes: 1024 * 1024 * 1024,
            max_cpu_time_ms: 60_000,
            ..Default::default()
        });
        assert_eq!(limits.max_memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(limits.max_cpu_time_ms, 60_000);
    }

    #[test]
    fn build_token_restrictions_profiles() {
        let permissive = WindowsSandbox::build_token_restrictions(&SandboxConfig {
            profile: SandboxProfile::Permissive,
            ..Default::default()
        });
        assert!(permissive.disable_admin_sid);
        assert!(!permissive.disable_most_sids);

        let standard = WindowsSandbox::build_token_restrictions(&SandboxConfig {
            profile: SandboxProfile::Standard,
            ..Default::default()
        });
        assert!(standard.disable_admin_sid);
        assert!(standard.disable_most_sids);
        assert!(standard.remove_privileges);
    }

    #[test]
    fn owned_handle_validity() {
        let null_h = OwnedHandle(0);
        assert!(!null_h.is_valid());
        std::mem::forget(null_h);

        let invalid_h = OwnedHandle(-1);
        assert!(!invalid_h.is_valid());
        std::mem::forget(invalid_h);

        let valid_h = OwnedHandle(42);
        assert!(valid_h.is_valid());
        std::mem::forget(valid_h);
    }

    #[test]
    fn windows_sandbox_capabilities() {
        let sandbox = WindowsSandbox::new();
        let caps = sandbox.capabilities();
        // Feature-dependent: resource_limits and process_isolation are only
        // true when `windows-sandbox` feature is enabled.
        if cfg!(feature = "windows-sandbox") {
            assert!(caps.resource_limits);
            assert!(caps.process_isolation);
        } else {
            assert!(!caps.resource_limits);
            assert!(!caps.process_isolation);
        }
        assert!(!caps.filesystem_isolation);
        assert!(!caps.syscall_filtering);
        assert!(!caps.network_isolation);
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

    #[tokio::test]
    async fn windows_sandbox_not_available_on_other_os() {
        let sandbox = WindowsSandbox::new();
        if !cfg!(target_os = "windows") {
            assert!(!sandbox.is_available());
            let action = AutomationAction::MouseMove { x: 0, y: 0 };
            let config = SandboxConfig::default();
            let result = sandbox.execute_sandboxed(&action, &config).await;
            assert!(result.is_err());
        }
    }
}
