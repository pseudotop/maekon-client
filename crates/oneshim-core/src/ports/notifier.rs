//! Desktop notification port — defines the contract for displaying
//! OS-native notifications (suggestions, alerts, errors).
//! Implemented by `TauriNotifier` (production, uses `tauri_plugin_notification`)
//! and `LogOnlyNotifier` (fallback, log-only) in `src-tauri/src/agent_runtime_support.rs`.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::suggestion::Suggestion;

/// # Errors
/// Notification libraries (notify-rust on macOS/Linux, Windows toast API)
/// can fail for OS-specific reasons: notification center unavailable,
/// user blocked notifications, focus-mode suppressed. Adapters emit
/// `CoreError::Internal` (wire: `internal.generic`) for these — the
/// notifier is intentionally best-effort; callers should not branch on
/// the specific error type, only log-and-continue.
#[async_trait]
pub trait DesktopNotifier: Send + Sync {
    async fn show_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError>;

    async fn show_notification(&self, title: &str, body: &str) -> Result<(), CoreError>;

    async fn show_error(&self, message: &str) -> Result<(), CoreError>;
}
