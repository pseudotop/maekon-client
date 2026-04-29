//! Focus probe port — defines the contract for querying the currently
//! focused UI element and validating execution bindings before automation.
//! Implemented by platform-specific adapters in `oneshim-vision`.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::gui::{ExecutionBinding, FocusSnapshot, FocusValidation};

/// Query the currently focused UI element and validate execution bindings.
///
/// # Errors
/// - `CoreError::PermissionDenied` (wire: `permission.permission_denied`) —
///   accessibility permission missing (macOS TCC, Linux AT-SPI, Windows
///   UIAccess) at probe time.
/// - `CoreError::GuiInteraction` (wire: `gui.*` per variant) — AX tree
///   traversal error, UIA CacheRequest failure, AT-SPI D-Bus error,
///   element no longer valid (stale reference).
/// - `CoreError::ServiceUnavailable` (wire: `service.unavailable`) —
///   running on an unsupported platform or with a no-op adapter (e.g.,
///   headless test mode).
/// - Current focus absent (no foreground window, all windows minimized)
///   is `Ok(FocusSnapshot::empty())`, not Err.
#[async_trait]
pub trait FocusProbe: Send + Sync {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError>;

    async fn validate_execution_binding(
        &self,
        binding: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError>;
}
