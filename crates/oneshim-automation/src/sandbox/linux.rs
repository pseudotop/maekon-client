//! Linux sandbox — Landlock, seccomp, and resource limits enforcement.
//!
//! **Enforcement model (subprocess):**
//! The sandbox spawns the `oneshim-sandbox-worker` binary as a child process.
//! Landlock, seccomp-BPF, and resource limits are applied in the child's
//! `pre_exec` hook (after fork, before exec). This ensures constraints never
//! leak into the parent process or tokio thread pool.
//!
//! **Enforcement status:**
//! - **Resource limits**: Enforced via `setrlimit(2)` — always available.
//! - **Landlock**: Enforced when `linux-sandbox` feature is enabled and kernel >= 5.13.
//!   Uses ABI v3 with graceful fallback if unsupported.
//! - **seccomp-BPF**: Enforced when `linux-sandbox` feature is enabled.
//!   Uses deny-list approach: default ALLOW, blocks network/process syscalls
//!   based on `SeccompAllowlist` flags. Denied calls return EPERM.

use async_trait::async_trait;

use crate::error::AutomationError;
use crate::sandbox::ipc;
use oneshim_core::config::{SandboxConfig, SandboxProfile};
use oneshim_core::error::CoreError;
use oneshim_core::models::automation::AutomationAction;
use oneshim_core::ports::sandbox::{Sandbox, SandboxCapabilities};

/// Linux sandbox adapter.
///
/// Detects Landlock availability at construction time by probing
/// `/sys/kernel/security/landlock`. Even when Landlock is unavailable,
/// seccomp and resource-limit rules are still constructed (but not yet
/// enforced -- see module-level docs).
pub struct LinuxSandbox {
    landlock_available: bool,
}

impl Default for LinuxSandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl LinuxSandbox {
    pub fn new() -> Self {
        let landlock_available = check_landlock_support();
        Self { landlock_available }
    }

    fn build_landlock_rules(config: &SandboxConfig) -> LandlockRules {
        let mut rules = LandlockRules::default();

        // Always allow the worker binary for subprocess model
        if let Ok(path) = ipc::resolve_worker_path() {
            rules.read_paths.push(path.to_string_lossy().to_string());
            if let Some(dir) = path.parent() {
                rules.read_paths.push(dir.to_string_lossy().to_string());
            }
        }

        match config.profile {
            SandboxProfile::Permissive => {
                rules.read_paths.extend_from_slice(&[
                    "/usr".to_string(),
                    "/lib".to_string(),
                    "/lib64".to_string(),
                    "/etc".to_string(),
                ]);
                rules.read_paths.extend(config.allowed_read_paths.clone());
                rules.write_paths.extend(config.allowed_write_paths.clone());
            }
            SandboxProfile::Standard => {
                rules
                    .read_paths
                    .extend_from_slice(&["/usr/lib".to_string(), "/lib".to_string()]);
                rules.read_paths.extend(config.allowed_read_paths.clone());
            }
            SandboxProfile::Strict => {
                rules.read_paths.push("/usr/lib".to_string());
                rules.read_paths.extend(config.allowed_read_paths.clone());
            }
        }

        rules
    }

    #[cfg(feature = "linux-sandbox")]
    fn build_seccomp_allowlist(config: &SandboxConfig) -> SeccompAllowlist {
        let mut allowlist = SeccompAllowlist::default();

        match config.profile {
            SandboxProfile::Permissive => {
                allowlist.allow_basic = true;
                allowlist.allow_network = config.allow_network;
                allowlist.allow_process = true;
            }
            SandboxProfile::Standard => {
                allowlist.allow_basic = true;
                allowlist.allow_network = false;
                allowlist.allow_process = false;
            }
            SandboxProfile::Strict => {
                allowlist.allow_basic = true;
                allowlist.allow_network = false;
                allowlist.allow_process = false;
            }
        }

        allowlist
    }

    fn build_resource_limits(config: &SandboxConfig) -> ResourceLimits {
        let (default_memory, default_cpu_ms) = match config.profile {
            SandboxProfile::Permissive => (0, 0),
            SandboxProfile::Standard => (512 * 1024 * 1024, 30_000), // 512MB, 30s
            SandboxProfile::Strict => (256 * 1024 * 1024, 10_000),   // 256MB, 10s
        };

        ResourceLimits {
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
impl Sandbox for LinuxSandbox {
    fn platform(&self) -> &str {
        "linux"
    }

    /// Returns `true` when resource limits are enforceable (always on Linux)
    /// or when Landlock is available.
    fn is_available(&self) -> bool {
        true // Resource limits (setrlimit) are always available on Linux
    }

    async fn execute_sandboxed(
        &self,
        action: &AutomationAction,
        config: &SandboxConfig,
    ) -> Result<(), CoreError> {
        if is_permissive_noop(config) {
            tracing::debug!("Permissive profile with no limits — skipping subprocess");
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

        // Build BPF program before fork — heap allocation is unsafe post-fork.
        #[cfg(feature = "linux-sandbox")]
        let bpf_program = build_seccomp_bpf(&Self::build_seccomp_allowlist(config))?;

        let landlock_rules = Self::build_landlock_rules(config);
        let resource_limits = Self::build_resource_limits(config);
        let landlock_avail = self.landlock_available;

        let timeout_ms = if config.max_cpu_time_ms > 0 {
            config.max_cpu_time_ms + 5000
        } else {
            60_000
        };

        tracing::debug!(
            landlock_available = landlock_avail,
            read_paths = landlock_rules.read_paths.len(),
            write_paths = landlock_rules.write_paths.len(),
            timeout_ms,
            action = ?action,
            "Linux sandbox spawning worker subprocess"
        );

        let mut cmd = tokio::process::Command::new(&worker_path);
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // SAFETY: pre_exec runs after fork, before exec. The child gets its own
        // address space so constraints apply only to the child. BPF program is
        // pre-built (no heap allocation post-fork). Only apply_filter (prctl)
        // and setrlimit run post-fork.
        #[cfg(target_os = "linux")]
        {
            unsafe {
                cmd.pre_exec(move || {
                    #[cfg(feature = "linux-sandbox")]
                    {
                        if landlock_avail {
                            apply_landlock_rules_sync(&landlock_rules)?;
                        }
                        apply_seccomp_bpf_sync(&bpf_program)?;
                    }
                    #[cfg(not(feature = "linux-sandbox"))]
                    {
                        let _ = landlock_avail;
                        let _ = &landlock_rules;
                    }
                    apply_resource_limits_sync(&resource_limits)?;
                    Ok(())
                });
            }
        }

        let mut child = cmd.spawn().map_err(|e| CoreError::SandboxExecution {
            code: oneshim_core::error_codes::SandboxCode::ExecutionFailed,
            message: format!("spawn failed: {e}"),
        })?;

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
                message: format!("child exited {} — stderr: {}", output.status, stderr.trim()),
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

        tracing::info!(action = ?action, "Linux sandbox execution completed via worker");
        Ok(())
    }

    /// Report capabilities based on actual enforcement availability.
    fn capabilities(&self) -> SandboxCapabilities {
        SandboxCapabilities {
            #[cfg(feature = "linux-sandbox")]
            filesystem_isolation: self.landlock_available,
            #[cfg(not(feature = "linux-sandbox"))]
            filesystem_isolation: false,
            #[cfg(feature = "linux-sandbox")]
            syscall_filtering: true,
            #[cfg(not(feature = "linux-sandbox"))]
            syscall_filtering: false,
            #[cfg(feature = "linux-sandbox")]
            network_isolation: true, // via seccomp socket deny
            #[cfg(not(feature = "linux-sandbox"))]
            network_isolation: false,
            resource_limits: true,   // setrlimit always available
            process_isolation: true, // subprocess model: constraints in child pre_exec
        }
    }
}

#[derive(Debug, Default, Clone)]
struct LandlockRules {
    read_paths: Vec<String>,
    write_paths: Vec<String>,
}

#[derive(Debug, Default)]
struct SeccompAllowlist {
    allow_basic: bool,
    allow_network: bool,
    allow_process: bool,
}

#[derive(Debug, Clone)]
struct ResourceLimits {
    max_memory_bytes: u64,
    max_cpu_time_ms: u64,
}

fn check_landlock_support() -> bool {
    std::path::Path::new("/sys/kernel/security/landlock").exists()
}

// ── Pre-fork build functions ──────────────────────────────────────

/// Build a seccomp BPF program (heap-allocates). Must be called BEFORE fork.
///
/// The resulting `BpfProgram` is then passed into the `pre_exec` closure where
/// only `apply_filter` (a prctl syscall) runs — no heap allocation post-fork.
#[cfg(feature = "linux-sandbox")]
fn build_seccomp_bpf(
    allowlist: &SeccompAllowlist,
) -> Result<seccompiler::BpfProgram, AutomationError> {
    use seccompiler::{SeccompAction, SeccompFilter, SeccompRule};
    use std::collections::BTreeMap;

    let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
    let deny = vec![SeccompRule::new(vec![])
        .map_err(|e| AutomationError::SandboxInit(format!("seccomp rule: {e}")))?];

    // Block network syscalls when not allowed
    if !allowlist.allow_network {
        for &nr in &[
            libc::SYS_socket,
            libc::SYS_connect,
            libc::SYS_bind,
            libc::SYS_listen,
            libc::SYS_accept,
            libc::SYS_accept4,
            libc::SYS_sendto,
            libc::SYS_recvfrom,
            libc::SYS_sendmsg,
            libc::SYS_recvmsg,
            libc::SYS_shutdown,
            libc::SYS_setsockopt,
            libc::SYS_getsockopt,
        ] {
            rules.insert(nr, deny.clone());
        }
    }

    // Block process creation/signal syscalls when not allowed.
    // NOTE: SYS_execve and SYS_execveat are intentionally NOT blocked —
    // the child must call execve to start the sandbox-worker binary.
    // Landlock restricts which executables are reachable from the child.
    if !allowlist.allow_process {
        for &nr in &[
            libc::SYS_clone,
            libc::SYS_fork,
            libc::SYS_vfork,
            libc::SYS_kill,
            libc::SYS_tkill,
            libc::SYS_tgkill,
        ] {
            rules.insert(nr, deny.clone());
        }
    }

    if rules.is_empty() {
        tracing::debug!("seccomp: no syscalls to block (all categories allowed)");
        // Return an empty allow-all program
        let filter = SeccompFilter::new(
            BTreeMap::new(),
            SeccompAction::Allow,
            SeccompAction::Allow,
            std::env::consts::ARCH.try_into().map_err(|_| {
                AutomationError::SandboxEnforcement("unsupported arch for seccomp".into())
            })?,
        )
        .map_err(|e| AutomationError::SandboxEnforcement(format!("seccomp filter build: {e}")))?;
        return filter
            .try_into()
            .map_err(|e| AutomationError::SandboxEnforcement(format!("seccomp BPF compile: {e}")));
    }

    let filter = SeccompFilter::new(
        rules,
        SeccompAction::Allow,                     // default: allow
        SeccompAction::Errno(libc::EPERM as u32), // denied → EPERM
        std::env::consts::ARCH.try_into().map_err(|_| {
            AutomationError::SandboxEnforcement("unsupported arch for seccomp".into())
        })?,
    )
    .map_err(|e| AutomationError::SandboxEnforcement(format!("seccomp filter build: {e}")))?;

    filter
        .try_into()
        .map_err(|e| AutomationError::SandboxEnforcement(format!("seccomp BPF compile: {e}")))
}

// ── Post-fork sync wrappers (for pre_exec) ────────────────────────

/// Apply a pre-built seccomp BPF program. Only calls prctl — no allocation.
#[cfg(feature = "linux-sandbox")]
fn apply_seccomp_bpf_sync(bpf: &seccompiler::BpfProgram) -> std::io::Result<()> {
    seccompiler::apply_filter(bpf).map_err(|e| std::io::Error::other(format!("seccomp apply: {e}")))
}

/// Apply Landlock filesystem isolation (sync, returns io::Result for pre_exec).
#[cfg(feature = "linux-sandbox")]
fn apply_landlock_rules_sync(rules: &LandlockRules) -> std::io::Result<()> {
    use landlock::{
        path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr,
        RulesetStatus, ABI,
    };

    let abi = ABI::V3;
    let read_access = AccessFs::from_read(abi);
    let write_access = AccessFs::from_all(abi);

    let ruleset = Ruleset::default()
        .handle_access(write_access)
        .map_err(|e| std::io::Error::other(format!("Landlock ruleset: {e}")))?
        .create()
        .map_err(|e| std::io::Error::other(format!("Landlock create: {e}")))?;

    // Add path rules — add_rule() takes self by value (builder pattern in landlock 0.4.x)
    let read_rules: Vec<_> = path_beneath_rules(
        rules
            .read_paths
            .iter()
            .filter(|p| std::path::Path::new(p).exists()),
        read_access,
    )
    .filter_map(|r| r.ok())
    .collect();

    let write_rules: Vec<_> = path_beneath_rules(
        rules
            .write_paths
            .iter()
            .filter(|p| std::path::Path::new(p).exists()),
        write_access,
    )
    .filter_map(|r| r.ok())
    .collect();

    let mut ruleset = ruleset;
    for r in read_rules.into_iter().chain(write_rules) {
        ruleset = ruleset
            .add_rule(r)
            .map_err(|e| std::io::Error::other(format!("Landlock add_rule: {e}")))?;
    }

    match ruleset.restrict_self() {
        Ok(status) => {
            let enforced = status.ruleset == RulesetStatus::FullyEnforced;
            tracing::info!(
                enforced,
                read = rules.read_paths.len(),
                write = rules.write_paths.len(),
                "Landlock filesystem isolation applied (pre_exec)"
            );
        }
        Err(e) => {
            tracing::warn!("Landlock restrict_self failed: {e} — continuing without FS isolation");
        }
    }

    Ok(())
}

/// Apply resource limits via setrlimit(2) (sync, returns io::Result for pre_exec).
fn apply_resource_limits_sync(limits: &ResourceLimits) -> std::io::Result<()> {
    if limits.max_memory_bytes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: limits.max_memory_bytes,
            rlim_max: limits.max_memory_bytes,
        };
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlim) };
        if ret != 0 {
            let errno = std::io::Error::last_os_error();
            return Err(std::io::Error::other(format!(
                "setrlimit RLIMIT_AS failed: {errno}"
            )));
        }
    }
    if limits.max_cpu_time_ms > 0 {
        let cpu_secs = limits.max_cpu_time_ms / 1000;
        if cpu_secs > 0 {
            let rlim = libc::rlimit {
                rlim_cur: cpu_secs,
                rlim_max: cpu_secs,
            };
            let ret = unsafe { libc::setrlimit(libc::RLIMIT_CPU, &rlim) };
            if ret != 0 {
                let errno = std::io::Error::last_os_error();
                return Err(std::io::Error::other(format!(
                    "setrlimit RLIMIT_CPU failed: {errno}"
                )));
            }
        }
    }
    Ok(())
}

// ── Original enforcement functions (retained for non-subprocess use) ──

/// Apply Landlock filesystem isolation rules.
///
/// When the `linux-sandbox` feature is enabled and kernel supports Landlock,
/// restricts filesystem access to the configured read/write paths.
/// Otherwise logs the rules and returns `Ok(())`.
#[allow(dead_code)]
fn apply_landlock_rules(rules: &LandlockRules) -> Result<(), AutomationError> {
    #[cfg(feature = "linux-sandbox")]
    {
        use landlock::{
            path_beneath_rules, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr,
            RulesetStatus, ABI,
        };

        let abi = ABI::V3;
        let read_access = AccessFs::from_read(abi);
        let write_access = AccessFs::from_all(abi);

        let ruleset = Ruleset::default()
            .handle_access(write_access)
            .map_err(|e| AutomationError::SandboxEnforcement(format!("Landlock ruleset: {e}")))?
            .create()
            .map_err(|e| AutomationError::SandboxEnforcement(format!("Landlock create: {e}")))?;

        // Add path rules — add_rule() takes self by value (builder pattern in landlock 0.4.x)
        let read_rules: Vec<_> = path_beneath_rules(
            rules
                .read_paths
                .iter()
                .filter(|p| std::path::Path::new(p).exists()),
            read_access,
        )
        .filter_map(|r| r.ok())
        .collect();

        let write_rules: Vec<_> = path_beneath_rules(
            rules
                .write_paths
                .iter()
                .filter(|p| std::path::Path::new(p).exists()),
            write_access,
        )
        .filter_map(|r| r.ok())
        .collect();

        let mut ruleset = ruleset;
        for r in read_rules.into_iter().chain(write_rules) {
            ruleset = ruleset.add_rule(r).map_err(|e| {
                AutomationError::SandboxEnforcement(format!("Landlock add_rule: {e}"))
            })?;
        }

        match ruleset.restrict_self() {
            Ok(status) => {
                let enforced = status.ruleset == RulesetStatus::FullyEnforced;
                tracing::info!(
                    enforced,
                    read = rules.read_paths.len(),
                    write = rules.write_paths.len(),
                    "Landlock filesystem isolation applied"
                );
            }
            Err(e) => {
                tracing::warn!(
                    "Landlock restrict_self failed: {e} — continuing without FS isolation"
                );
            }
        }

        Ok(())
    }

    #[cfg(not(feature = "linux-sandbox"))]
    {
        tracing::debug!(
            read = rules.read_paths.len(),
            write = rules.write_paths.len(),
            "Landlock rules built (enforcement requires linux-sandbox feature)"
        );
        Ok(())
    }
}

/// Apply seccomp-BPF syscall filtering.
///
/// When `linux-sandbox` feature is enabled, installs a BPF filter that denies
/// network and/or process syscalls based on the allowlist. Default action is
/// ALLOW — only explicitly blocked categories are denied (returns EPERM).
#[allow(dead_code)]
fn apply_seccomp_filter(allowlist: &SeccompAllowlist) -> Result<(), AutomationError> {
    #[cfg(feature = "linux-sandbox")]
    {
        use seccompiler::{apply_filter, BpfProgram, SeccompAction, SeccompFilter, SeccompRule};
        use std::collections::BTreeMap;

        // Default ALLOW — deny specific dangerous syscall categories
        let mut rules: BTreeMap<i64, Vec<SeccompRule>> = BTreeMap::new();
        let deny = vec![SeccompRule::new(vec![])
            .map_err(|e| AutomationError::SandboxInit(format!("seccomp rule: {e}")))?];

        // Block network syscalls when not allowed
        if !allowlist.allow_network {
            for &nr in &[
                libc::SYS_socket,
                libc::SYS_connect,
                libc::SYS_bind,
                libc::SYS_listen,
                libc::SYS_accept,
                libc::SYS_accept4,
                libc::SYS_sendto,
                libc::SYS_recvfrom,
                libc::SYS_sendmsg,
                libc::SYS_recvmsg,
                libc::SYS_shutdown,
                libc::SYS_setsockopt,
                libc::SYS_getsockopt,
            ] {
                rules.insert(nr, deny.clone());
            }
        }

        // Block process creation/signal syscalls when not allowed.
        // NOTE: SYS_execve and SYS_execveat are intentionally NOT blocked —
        // the child must call execve to start the sandbox-worker binary.
        // Landlock restricts which executables are reachable from the child.
        if !allowlist.allow_process {
            for &nr in &[
                libc::SYS_clone,
                libc::SYS_fork,
                libc::SYS_vfork,
                libc::SYS_kill,
                libc::SYS_tkill,
                libc::SYS_tgkill,
            ] {
                rules.insert(nr, deny.clone());
            }
        }

        if rules.is_empty() {
            tracing::debug!("seccomp: no syscalls to block (all categories allowed)");
            return Ok(());
        }

        let filter: BpfProgram = SeccompFilter::new(
            rules,
            SeccompAction::Allow,                     // default: allow
            SeccompAction::Errno(libc::EPERM as u32), // denied → EPERM
            std::env::consts::ARCH.try_into().map_err(|_| {
                AutomationError::SandboxEnforcement("unsupported arch for seccomp".into())
            })?,
        )
        .map_err(|e| AutomationError::SandboxEnforcement(format!("seccomp filter build: {e}")))?
        .try_into()
        .map_err(|e| AutomationError::SandboxEnforcement(format!("seccomp BPF compile: {e}")))?;

        apply_filter(&filter)
            .map_err(|e| AutomationError::SandboxEnforcement(format!("seccomp apply: {e}")))?;

        tracing::info!(
            blocked_network = !allowlist.allow_network,
            blocked_process = !allowlist.allow_process,
            rules_count = filter.len(),
            "seccomp-BPF filter applied"
        );

        Ok(())
    }

    #[cfg(not(feature = "linux-sandbox"))]
    {
        tracing::debug!(
            basic = allowlist.allow_basic,
            network = allowlist.allow_network,
            process = allowlist.allow_process,
            "seccomp filter (enforcement requires linux-sandbox feature)"
        );
        Ok(())
    }
}

/// Apply resource limits via `setrlimit(2)`.
///
/// Sets `RLIMIT_AS` (virtual memory) and `RLIMIT_CPU` (CPU seconds) on the
/// current process. Only effective when applied before exec in a child process.
#[allow(dead_code)]
fn apply_resource_limits(limits: &ResourceLimits) -> Result<(), AutomationError> {
    if limits.max_memory_bytes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: limits.max_memory_bytes,
            rlim_max: limits.max_memory_bytes,
        };
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlim) };
        if ret != 0 {
            let errno = std::io::Error::last_os_error();
            tracing::warn!("setrlimit RLIMIT_AS failed: {errno}");
        } else {
            tracing::debug!(max_memory = limits.max_memory_bytes, "RLIMIT_AS set");
        }
    }
    if limits.max_cpu_time_ms > 0 {
        let cpu_secs = limits.max_cpu_time_ms / 1000;
        if cpu_secs > 0 {
            let rlim = libc::rlimit {
                rlim_cur: cpu_secs,
                rlim_max: cpu_secs,
            };
            let ret = unsafe { libc::setrlimit(libc::RLIMIT_CPU, &rlim) };
            if ret != 0 {
                let errno = std::io::Error::last_os_error();
                tracing::warn!("setrlimit RLIMIT_CPU failed: {errno}");
            } else {
                tracing::debug!(cpu_secs, "RLIMIT_CPU set");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_landlock_rules_permissive() {
        let config = SandboxConfig {
            profile: SandboxProfile::Permissive,
            allowed_read_paths: vec!["/home/user/data".to_string()],
            allowed_write_paths: vec!["/tmp/output".to_string()],
            ..Default::default()
        };
        let rules = LinuxSandbox::build_landlock_rules(&config);
        assert!(rules.read_paths.contains(&"/usr".to_string()));
        assert!(rules.read_paths.contains(&"/home/user/data".to_string()));
        assert!(rules.write_paths.contains(&"/tmp/output".to_string()));
    }

    #[test]
    fn build_landlock_rules_standard() {
        let config = SandboxConfig {
            profile: SandboxProfile::Standard,
            ..Default::default()
        };
        let rules = LinuxSandbox::build_landlock_rules(&config);
        assert!(rules.read_paths.contains(&"/usr/lib".to_string()));
        assert!(rules.write_paths.is_empty());
    }

    #[test]
    #[cfg(feature = "linux-sandbox")]
    fn build_seccomp_allowlist_profiles() {
        let permissive = LinuxSandbox::build_seccomp_allowlist(&SandboxConfig {
            profile: SandboxProfile::Permissive,
            allow_network: true,
            ..Default::default()
        });
        assert!(permissive.allow_network);
        assert!(permissive.allow_process);

        let standard = LinuxSandbox::build_seccomp_allowlist(&SandboxConfig {
            profile: SandboxProfile::Standard,
            ..Default::default()
        });
        assert!(!standard.allow_network);
        assert!(!standard.allow_process);
    }

    #[test]
    fn build_resource_limits_defaults() {
        let standard = LinuxSandbox::build_resource_limits(&SandboxConfig {
            profile: SandboxProfile::Standard,
            ..Default::default()
        });
        assert_eq!(standard.max_memory_bytes, 512 * 1024 * 1024);
        assert_eq!(standard.max_cpu_time_ms, 30_000);

        let strict = LinuxSandbox::build_resource_limits(&SandboxConfig {
            profile: SandboxProfile::Strict,
            ..Default::default()
        });
        assert_eq!(strict.max_memory_bytes, 256 * 1024 * 1024);
        assert_eq!(strict.max_cpu_time_ms, 10_000);
    }

    #[test]
    fn build_resource_limits_custom_override() {
        let limits = LinuxSandbox::build_resource_limits(&SandboxConfig {
            profile: SandboxProfile::Strict,
            max_memory_bytes: 1024 * 1024 * 1024, // 1GB
            max_cpu_time_ms: 60_000,              // 60s
            ..Default::default()
        });
        assert_eq!(limits.max_memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(limits.max_cpu_time_ms, 60_000);
    }

    #[tokio::test]
    async fn linux_sandbox_execute_permissive_noop() {
        let sandbox = LinuxSandbox::new();
        let action = AutomationAction::KeyType {
            text: "hello".to_string(),
        };
        // Permissive with zero limits → fast-path, no subprocess needed
        let config = SandboxConfig {
            profile: SandboxProfile::Permissive,
            max_memory_bytes: 0,
            max_cpu_time_ms: 0,
            ..Default::default()
        };
        let result = sandbox.execute_sandboxed(&action, &config).await;
        assert!(result.is_ok());
    }

    #[test]
    fn linux_process_isolation_capability() {
        let sandbox = LinuxSandbox::new();
        let caps = sandbox.capabilities();
        assert!(caps.process_isolation);
        assert!(caps.resource_limits);
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

    #[test]
    fn landlock_rules_include_worker_binary() {
        // If the worker binary is available, it should appear in read_paths
        let config = SandboxConfig {
            profile: SandboxProfile::Standard,
            ..Default::default()
        };
        let rules = LinuxSandbox::build_landlock_rules(&config);
        // The worker binary path is included when resolve_worker_path succeeds.
        // In test environments the binary may not be built, so we check that
        // the profile-based rules are present regardless.
        assert!(rules.read_paths.contains(&"/usr/lib".to_string()));

        // When worker IS found, its path should be in read_paths
        if let Ok(worker_path) = ipc::resolve_worker_path() {
            let path_str = worker_path.to_string_lossy().to_string();
            assert!(
                rules.read_paths.contains(&path_str),
                "worker binary path should be in read_paths"
            );
        }
    }
}
