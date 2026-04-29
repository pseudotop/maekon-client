pub mod ipc;
mod noop;

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

use oneshim_core::config::SandboxConfig;
use oneshim_core::ports::sandbox::Sandbox;
use std::sync::Arc;

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
