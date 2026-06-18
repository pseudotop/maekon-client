pub mod ipc;
mod noop;
// Cfg-free macOS SBPL profile generation (#5120) — the sandbox security policy
// is pure string construction, kept here so it is testable on any OS.
mod sbpl;
// Cfg-free Windows Job-Object limits + token-restriction policy (#5138), same
// rationale (the Win32 enforcement stays in the target-gated `windows` module).
mod win_limits;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use noop::{FailClosedSandbox, NoOpSandbox};

#[cfg(target_os = "linux")]
pub use linux::LinuxSandbox;
#[cfg(target_os = "macos")]
pub use macos::MacOsSandbox;
#[cfg(target_os = "windows")]
pub use windows::WindowsSandbox;

use maekon_core::config::{SandboxConfig, SandboxProfile};
use maekon_core::models::automation::AutomationAction;
use maekon_core::ports::sandbox::Sandbox;
use std::sync::Arc;

/// Return a log-safe string for an [`AutomationAction`].
///
/// `KeyType` carries free-form text that may contain passwords or other
/// sensitive content and MUST NOT appear in any log sink.  All other variants
/// expose only non-sensitive structural data (coordinates, key names).
pub(crate) fn redact_action(a: &AutomationAction) -> String {
    match a {
        AutomationAction::KeyType { text } => {
            format!("KeyType {{ text: [REDACTED len={}] }}", text.len())
        }
        AutomationAction::MouseMove { x, y } => format!("MouseMove {{ x: {x}, y: {y} }}"),
        AutomationAction::MouseClick { button, x, y } => {
            format!("MouseClick {{ button: {button:?}, x: {x}, y: {y} }}")
        }
        AutomationAction::KeyPress { key } => format!("KeyPress {{ key: {key:?} }}"),
        AutomationAction::KeyRelease { key } => format!("KeyRelease {{ key: {key:?} }}"),
        AutomationAction::Hotkey { keys } => format!("Hotkey {{ keys: {keys:?} }}"),
    }
}

/// Returns `true` when `config` uses the Permissive profile AND has no custom
/// resource limits. In that case it is safe to skip the subprocess sandbox
/// (there are no resource constraints to isolate).
///
/// Note that the action itself must still be performed (#4539): the consumer of
/// this decision is `SandboxActionDispatcher::dispatch`, which on that path runs
/// the action directly via the in-process InputDriver. The per-platform sandbox
/// adapters (`execute_sandboxed`) do not consult this helper and always route
/// through the worker regardless of profile — adding a no-op guard inside an
/// adapter would silently drop actions for direct callers (#5604).
pub(crate) fn is_permissive_noop(config: &SandboxConfig) -> bool {
    matches!(config.profile, SandboxProfile::Permissive)
        && config.max_memory_bytes == 0
        && config.max_cpu_time_ms == 0
}

pub fn create_platform_sandbox(config: &SandboxConfig) -> Arc<dyn Sandbox> {
    if !config.enabled {
        return Arc::new(NoOpSandbox);
    }

    create_native_sandbox()
}

fn create_native_sandbox() -> Arc<dyn Sandbox> {
    // Reached only when `SandboxConfig.enabled == true` (see
    // `create_platform_sandbox`). If no platform sandbox can actually enforce
    // isolation here we must fail CLOSED rather than silently substitute a
    // NoOpSandbox — the operator opted into sandboxing, so running uncontained
    // while reporting success would be a sandbox escape (review4 A2/A6/A7).
    #[cfg(target_os = "linux")]
    {
        let sandbox = LinuxSandbox::new();
        if sandbox.is_available() {
            return Arc::new(sandbox);
        }
        tracing::error!("Linux sandbox not available; failing closed (refusing automation)");
    }

    #[cfg(target_os = "macos")]
    {
        let sandbox = MacOsSandbox::new();
        if sandbox.is_available() {
            return Arc::new(sandbox);
        }
        tracing::error!("macOS sandbox not available; failing closed (refusing automation)");
    }

    #[cfg(target_os = "windows")]
    {
        let sandbox = WindowsSandbox::new();
        if sandbox.is_available() {
            return Arc::new(sandbox);
        }
        tracing::error!("Windows sandbox not available; failing closed (refusing automation)");
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        tracing::error!(
            "sandbox unsupported on this platform; failing closed (refusing automation)"
        );
    }

    Arc::new(FailClosedSandbox)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_config_returns_noop() {
        let config = SandboxConfig {
            enabled: false,
            ..Default::default()
        };
        let sandbox = create_platform_sandbox(&config);
        assert_eq!(sandbox.platform(), "noop");
    }

    #[test]
    fn enabled_config_never_silently_noops() {
        let config = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let sandbox = create_platform_sandbox(&config);
        // When enabled, the factory must return either a real enforcing platform
        // sandbox (is_available == true) or — when this build/platform cannot
        // enforce isolation — a FailClosedSandbox that refuses execution. It must
        // NEVER silently substitute the NoOp sandbox, which would run actions
        // fully uncontained while reporting success (review4 A2/A6/A7).
        assert_ne!(
            sandbox.platform(),
            "noop",
            "enabled sandbox must not fall back to NoOp (silent fail-open)"
        );
        assert!(sandbox.is_available() || sandbox.platform() == "fail-closed");
    }

    #[test]
    fn factory_returns_send_sync() {
        let config = SandboxConfig::default();
        let sandbox = create_platform_sandbox(&config);
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&sandbox);
    }
}
