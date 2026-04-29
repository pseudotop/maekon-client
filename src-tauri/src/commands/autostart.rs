//! Tauri IPC commands for autostart management.
//!
//! Source-of-truth: OS state is authoritative for `is_autostart_enabled`.
//! AppConfig.autostart stores ONLY onboarding state (prompt_state, counter).

use tauri::command;

use oneshim_core::config::AutostartPromptState;
use oneshim_core::error_codes::AutostartCode;

use crate::autostart::{self, AutostartCapabilities};
use crate::ipc_error::IpcError;
use crate::runtime_state::ConfigRuntimeState;

#[command]
pub async fn enable_autostart() -> Result<(), IpcError> {
    autostart::enable_autostart().map_err(|e| {
        IpcError::new(
            AutostartCode::EnableFailed.as_str(),
            format!("autostart enable failed: {e}"),
        )
    })
}

#[command]
pub async fn disable_autostart() -> Result<(), IpcError> {
    autostart::disable_autostart().map_err(|e| {
        IpcError::new(
            AutostartCode::DisableFailed.as_str(),
            format!("autostart disable failed: {e}"),
        )
    })
}

#[command]
pub async fn is_autostart_enabled() -> Result<bool, IpcError> {
    autostart::is_autostart_enabled().map_err(|e| {
        IpcError::new(
            AutostartCode::QueryFailed.as_str(),
            format!("autostart query failed: {e}"),
        )
    })
}

#[command]
pub async fn autostart_capabilities() -> Result<AutostartCapabilities, IpcError> {
    Ok(autostart::detect_capabilities())
}

#[command]
pub async fn mark_autostart_prompt_state(
    new_state: AutostartPromptState,
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<(), IpcError> {
    state
        .config_manager()
        .update_with(|c| {
            c.autostart.prompt_state = new_state;
            Ok(())
        })
        .map(|_| ())
        .map_err(IpcError::from)
}

/// Get autostart-only config (smaller payload than full AppConfig).
#[command]
pub async fn get_autostart_config(
    state: tauri::State<'_, ConfigRuntimeState>,
) -> Result<oneshim_core::config::AutostartConfig, IpcError> {
    Ok(state.config_manager().get().autostart)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn is_autostart_enabled_returns_bool() {
        let result = is_autostart_enabled().await;
        assert!(
            result.is_ok(),
            "is_autostart_enabled should not error: {:?}",
            result
        );
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[tokio::test]
    async fn autostart_capabilities_returns_supported_on_macos_windows() {
        let result = autostart_capabilities().await.unwrap();
        assert!(
            result.supported,
            "macOS/Windows must return supported=true (Linux is environment-dependent — see autostart::tests::linux_capability_tests)"
        );
    }

    /// Integration test: enable → is_enabled = true → disable → is_enabled = false.
    /// Modifies OS state (LaunchAgents/Registry/systemd unit) — run manually.
    /// Per Addendum A3: inline here instead of tests/ because src-tauri has no [lib].
    #[tokio::test]
    #[ignore = "modifies OS state — run manually"]
    async fn enable_then_disable_round_trip() {
        let initial = is_autostart_enabled().await.unwrap_or(false);

        enable_autostart().await.expect("enable failed");
        let after_enable = is_autostart_enabled().await.expect("query failed");
        assert!(
            after_enable,
            "is_autostart_enabled should return true after enable"
        );

        disable_autostart().await.expect("disable failed");
        let after_disable = is_autostart_enabled().await.expect("query failed");
        assert!(
            !after_disable,
            "is_autostart_enabled should return false after disable"
        );

        // Restore initial state
        if initial {
            let _ = enable_autostart().await;
        }
    }
}
