//! Static system information provider port.

use crate::models::system::StaticSystemInfo;

/// Provides static system hardware/software information.
///
/// # Errors
/// **Infallible.** `system_info` returns `StaticSystemInfo` directly,
/// not `Result<_, _>`. Missing or unreadable OS metadata (e.g.,
/// hostname resolution failure, `/etc/os-release` read error) falls
/// back to sentinel values (empty strings, "unknown" labels) rather
/// than propagating an error — this port is collected once at startup
/// and its output feeds the bug-report payload, so silent degradation
/// is preferred over a diagnostic-blocking failure.
pub trait SystemInfoProvider: Send + Sync {
    /// Collect current system information.
    fn system_info(&self) -> StaticSystemInfo;
}
