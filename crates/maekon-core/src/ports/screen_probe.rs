//! Screen change probe port — samples the screen as per-tile signatures so
//! callers can tell whether the screen changed and when it stopped changing.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::screen_activity::ScreenSample;

/// Take signature-only samples of the current screen.
///
/// Implementations authorize every sample against the live capture policy
/// (consent, pause, excluded apps) and return signatures only: pixels never
/// leave the adapter.
///
/// # Errors
/// - `CoreError::PermissionDenied` (wire: `permission.permission_denied`) —
///   capture is not authorized right now, including when no capture authority
///   is wired. Callers must treat this as "not observed", never as "unchanged".
/// - `CoreError::Internal` (wire: `internal.generic`) — capture or signature
///   computation failed.
#[async_trait]
pub trait ScreenChangeProbe: Send + Sync {
    async fn sample(&self) -> Result<ScreenSample, CoreError>;

    fn name(&self) -> &str;
}
