//! OS change-region source port — the screen regions the operating system
//! reports as changed, so callers can tell the screen changed without
//! capturing and comparing frames.

use std::sync::Arc;

use crate::error::CoreError;
use crate::models::dirty_region::DirtyRegions;

/// Regions of the authorized display that changed since the previous call.
///
/// An implementation listens only under a capture permit and keeps checking
/// it: once the permit is revoked it stops listening, and every later call
/// fails. Regions over windows of excluded apps never leave it, because when
/// something changed is itself information.
///
/// # Errors
/// - `CoreError::PermissionDenied` (wire: `permission.permission_denied`) —
///   capture is not authorized, the source has stopped, or the regions could
///   not be checked against excluded windows. Callers must treat this as
///   "not observed", never as "unchanged".
/// - `CoreError::Internal` (wire: `internal.generic`) — the OS source failed
///   and has stopped.
pub trait DirtyRegionSource: Send + Sync {
    fn take(&self) -> Result<DirtyRegions, CoreError>;

    fn name(&self) -> &str;
}

/// How a caller learns that the screen changed.
#[derive(Clone)]
pub enum ScreenChangeSource {
    /// The operating system reports changed regions.
    Regions(Arc<dyn DirtyRegionSource>),
    /// No OS source here (unsupported platform or session, or the source
    /// could not start): capture samples and compare them instead.
    CaptureComparison,
}

impl std::fmt::Debug for ScreenChangeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Regions(source) => f.debug_tuple("Regions").field(&source.name()).finish(),
            Self::CaptureComparison => f.write_str("CaptureComparison"),
        }
    }
}
