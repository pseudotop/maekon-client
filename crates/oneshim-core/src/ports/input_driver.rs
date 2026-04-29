//! Synthetic input driver port — defines the contract for injecting
//! mouse, keyboard, and hotkey events into the OS for automation.
//! Implemented by platform-specific adapters in `oneshim-automation`.

use async_trait::async_trait;

use crate::error::CoreError;

/// Synthetic input driver port — injects mouse, keyboard, hotkey events.
///
/// # Errors
/// - `CoreError::Internal` (wire: `internal.generic`) — enigo library
///   failures and platform-specific input injection errors (macOS
///   CGEvent posting, Windows SendInput, Linux uinput). The OS
///   refused our injection request for reasons outside typical
///   error-categorization.
/// - `CoreError::PermissionDenied` (wire: `permission.permission_denied`)
///   is NOT emitted by this port directly — it flows from the upstream
///   accessibility adapter when macOS Accessibility or Input Monitoring
///   permission is missing. Callers check permission before invoking.
/// - `CoreError::ServiceUnavailable` (wire: `service.unavailable`) —
///   running on an unsupported platform (e.g., headless no-op driver).
/// - No distinct "bad key name" / "unknown button" wire code — those
///   are internal mapping failures and route through Internal.
#[async_trait]
pub trait InputDriver: Send + Sync {
    async fn mouse_move(&self, x: i32, y: i32) -> Result<(), CoreError>;

    async fn mouse_click(&self, button: &str, x: i32, y: i32) -> Result<(), CoreError>;

    async fn type_text(&self, text: &str) -> Result<(), CoreError>;

    async fn key_press(&self, key: &str) -> Result<(), CoreError>;

    async fn key_release(&self, key: &str) -> Result<(), CoreError>;

    async fn hotkey(&self, keys: &[String]) -> Result<(), CoreError>;

    fn platform(&self) -> &str;
}
