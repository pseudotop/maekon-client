//! System and process monitoring ports — defines contracts for collecting
//! CPU/memory metrics, active window info, and user activity context.
//! Implemented by `SysInfoMonitor`, `ProcessTracker`, and `ActivityTracker`
//! in `maekon-monitor`.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::context::{ProcessInfo, UserContext, WindowInfo};
use crate::models::event::ProcessDetail;
use crate::models::system::{PowerStatus, SystemMetrics};

/// Collects CPU, memory, disk, and network metrics.
///
/// # Errors
/// Returns `CoreError::Internal` (wire: `internal.generic`) on mutex lock
/// poisoning in the sysinfo state; platform API calls themselves are
/// infallible in the sysinfo crate.
#[async_trait]
pub trait SystemMonitor: Send + Sync {
    async fn collect_metrics(&self) -> Result<SystemMetrics, CoreError>;

    async fn current_power_status(&self) -> Result<PowerStatus, CoreError> {
        Ok(PowerStatus::default())
    }
}

/// Active window detection and process enumeration.
///
/// # Errors
/// - `CoreError::PermissionDenied` (wire: `permission.permission_denied`) when
///   accessibility permission is missing (macOS) or AT-SPI2 is unavailable
///   (Linux). Platform check runs before any OS API call.
/// - `CoreError::Internal` (wire: `internal.generic`) on intra-process failure
///   (lock poisoning, tokio join error). Platform API errors (rare in practice)
///   also surface here.
#[async_trait]
pub trait ProcessMonitor: Send + Sync {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError>;

    async fn get_top_processes(&self, limit: usize) -> Result<Vec<ProcessInfo>, CoreError>;

    async fn get_detailed_processes(
        &self,
        foreground_pid: Option<u32>,
        top_n: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError>;
}

/// Collects composite user activity context (window, mouse, keyboard, idle).
///
/// # Errors
/// Returns `CoreError::Internal` (wire: `internal.generic`) on intra-process
/// failure (lock poisoning). Does not surface OS permission errors
/// separately — missing permissions degrade gracefully to partial context.
#[async_trait]
pub trait ActivityMonitor: Send + Sync {
    async fn collect_context(&self) -> Result<UserContext, CoreError>;

    /// Collect only the lightweight context the monitor hot-path needs — active
    /// window + mouse, WITHOUT the (expensive) top-process enumeration (#6441 F13).
    /// The 1 Hz monitor loop never reads [`UserContext::processes`], so walking the
    /// full process table every tick is wasted work. The default delegates to
    /// [`collect_context`](Self::collect_context) so non-hot-path impls need no change.
    async fn collect_active_context(&self) -> Result<UserContext, CoreError> {
        self.collect_context().await
    }
}
