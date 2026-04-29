//! Port trait for OS accessibility API integration.
//!
//! Separate from `ElementFinder` which is used for click-target lookup in
//! the automation pipeline. `AccessibilityExtractor` passively extracts the
//! currently focused element on each scheduler tick for context enrichment.

use async_trait::async_trait;

use crate::config::PiiFilterLevel;
use crate::error::CoreError;
use crate::models::focused_element::{AccessibilityElement, FocusedElementInfo};

/// Extract focused UI element information from the OS accessibility API.
///
/// Implementations MUST:
/// - Return `Ok(None)` when no element is focused or permission is denied
/// - Never panic on OS permission revocation at runtime
/// - Apply PII-level gating according to the provided level
/// - Use `Zeroizing<String>` for raw text before PII filtering (in adapter)
///
/// # Errors
/// - `CoreError::PermissionDenied` (wire: `permission.permission_denied`)
///   when OS accessibility permission is missing at a point where the
///   adapter must fail-closed (e.g., `extract_window_elements` on macOS
///   AX tree traversal). The single-element `extract_focused_element`
///   degrades to `Ok(None)` instead per the MUST above.
/// - `CoreError::Internal` (wire: `internal.generic`) for tokio
///   spawn_blocking JoinError when native AX FFI is run on a blocking
///   thread (macOS/Linux/Windows native adapters).
#[async_trait]
pub trait AccessibilityExtractor: Send + Sync {
    /// Extract the currently focused UI element, filtered by PII level.
    ///
    /// `has_full_text_consent` gates the `Off` PII level. When `pii_level` is
    /// `Off` but consent is missing, implementations MUST silently fall back
    /// to `Standard`.
    async fn extract_focused_element(
        &self,
        pii_level: PiiFilterLevel,
        has_full_text_consent: bool,
    ) -> Result<Option<FocusedElementInfo>, CoreError>;

    /// Extract the accessibility tree for the focused window up to `max_depth`.
    ///
    /// Returns a flat list of elements from the window's accessibility subtree.
    /// The default implementation falls back to the single focused element,
    /// converted to an `AccessibilityElement`.
    ///
    /// Implementations SHOULD:
    /// - Respect `max_depth` to limit tree traversal (0 = focused element only)
    /// - Cap total elements at `max_elements` (default 300)
    /// - Apply the same PII gating as `extract_focused_element()`
    /// - Return `CoreError::PermissionDenied` when OS permission is missing
    async fn extract_window_elements(
        &self,
        _max_depth: u32,
        _max_elements: usize,
        pii_level: PiiFilterLevel,
        has_full_text_consent: bool,
    ) -> Result<Vec<AccessibilityElement>, CoreError> {
        // Default: delegate to extract_focused_element for backward compatibility
        let focused = self
            .extract_focused_element(pii_level, has_full_text_consent)
            .await?;
        Ok(focused
            .into_iter()
            .map(|f| AccessibilityElement {
                role: f.role,
                label: f.label.unwrap_or_default(),
                bounds: f.position,
            })
            .collect())
    }

    /// Check if OS-level accessibility permission is currently granted.
    fn has_permission(&self) -> bool;

    /// Human-readable name for logging/diagnostics.
    fn name(&self) -> &str;

    /// Request OS-level accessibility permission (may show a system dialog).
    /// Default implementation is a no-op.
    fn request_permission(&self) -> bool {
        self.has_permission()
    }
}
