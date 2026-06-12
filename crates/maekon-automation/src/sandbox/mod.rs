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

pub use noop::NoOpSandbox;

#[cfg(target_os = "linux")]
pub use linux::LinuxSandbox;
#[cfg(target_os = "macos")]
pub use macos::MacOsSandbox;
#[cfg(target_os = "windows")]
pub use windows::WindowsSandbox;

use maekon_core::config::{SandboxConfig, SandboxProfile};
use maekon_core::ports::sandbox::Sandbox;
use std::sync::Arc;

/// `config`가 Permissive 프로파일이면서 커스텀 리소스 제한이 없을 때 `true`를 반환한다.
/// 이 경우 서브프로세스 샌드박스를 건너뛰어도 안전하다 (격리할 자원 제약이 없음).
///
/// 단, 동작 자체를 누락해서는 안 된다(#4539): 이 판정의 소비자는
/// `SandboxActionDispatcher::dispatch`이며, 그 경로에서 액션을 인-프로세스
/// InputDriver로 직접 실행한다. 플랫폼별 sandbox 어댑터(`execute_sandboxed`)는
/// 이 헬퍼를 참조하지 않고 프로파일과 무관하게 항상 worker를 경유한다 —
/// 어댑터 안에 no-op 가드를 추가하면 직접 호출자에서 액션이 조용히 드롭된다(#5604).
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
    #[cfg(target_os = "linux")]
    {
        let sandbox = LinuxSandbox::new();
        if sandbox.is_available() {
            return Arc::new(sandbox);
        }
        tracing::warn!("Linux sandbox not available; using NoOp sandbox");
    }

    #[cfg(target_os = "macos")]
    {
        let sandbox = MacOsSandbox::new();
        if sandbox.is_available() {
            return Arc::new(sandbox);
        }
        tracing::warn!("macOS sandbox not available; using NoOp sandbox");
    }

    #[cfg(target_os = "windows")]
    {
        let sandbox = WindowsSandbox::new();
        if sandbox.is_available() {
            return Arc::new(sandbox);
        }
        tracing::warn!("Windows sandbox not available; using NoOp sandbox");
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        tracing::warn!("sandbox unsupported on this platform; using NoOp sandbox");
    }

    Arc::new(NoOpSandbox)
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
    fn enabled_config_returns_platform_sandbox() {
        let config = SandboxConfig {
            enabled: true,
            ..Default::default()
        };
        let sandbox = create_platform_sandbox(&config);
        assert!(sandbox.is_available());
    }

    #[test]
    fn factory_returns_send_sync() {
        let config = SandboxConfig::default();
        let sandbox = create_platform_sandbox(&config);
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&sandbox);
    }
}
