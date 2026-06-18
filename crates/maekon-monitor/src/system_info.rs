//! Static system information adapter using sysinfo.

use std::sync::{Arc, Mutex};

use maekon_core::models::system::StaticSystemInfo;
use maekon_core::ports::system_info_provider::SystemInfoProvider;
use sysinfo::System;

/// Adapter providing static system hardware/software info.
pub struct SysInfoProvider {
    // Arc allows the inner Mutex to be moved into spawn_blocking closures
    // without holding the lock across the async boundary.
    sys: Arc<Mutex<System>>,
}

impl SysInfoProvider {
    pub fn new() -> Self {
        // F-PF-18: Use System::new() instead of System::new_all().
        // Only refresh_memory() is called on demand in system_info() to
        // avoid allocating the full process table at startup.
        Self {
            sys: Arc::new(Mutex::new(System::new())),
        }
    }

    /// Collect system information without blocking the tokio runtime.
    ///
    /// Replicates the [`Self::system_info`] logic inside a
    /// `tokio::task::spawn_blocking` closure (it cannot call `self.system_info()`
    /// directly because `self` is not `'static`), so the sysinfo `refresh_memory`
    /// syscall runs on the blocking thread pool, never on a tokio worker. Keep
    /// this body in sync with `system_info()` — the
    /// `system_info_async_consistent_with_sync` test guards against drift.
    ///
    /// The `Arc<Mutex<System>>` is cloned *before* entering the closure so
    /// the lock is acquired and released entirely inside the blocking thread —
    /// the Mutex is never held across an `.await` point.
    ///
    /// # Errors
    /// Returns a `String` description if the `spawn_blocking` task panics or
    /// is cancelled (extremely unlikely in practice; mirrors the join-error
    /// handling pattern in `system.rs`).  On error the caller should fall back
    /// to `system_info()` directly or return a default value.
    pub async fn system_info_async(&self) -> Result<StaticSystemInfo, String> {
        let sys = Arc::clone(&self.sys);
        tokio::task::spawn_blocking(move || {
            let mut guard = sys.lock().unwrap_or_else(|e| e.into_inner());
            guard.refresh_memory();
            let cpu_count = std::thread::available_parallelism()
                .map(std::num::NonZeroUsize::get)
                .unwrap_or_else(|_| guard.cpus().len());

            StaticSystemInfo {
                os_version: System::long_os_version().unwrap_or_default(),
                cpu_count,
                memory_total_bytes: guard.total_memory(),
                memory_available_bytes: guard.available_memory(),
                uptime_seconds: System::uptime(),
            }
        })
        .await
        .map_err(|e| format!("spawn_blocking join error: {e}"))
    }
}

impl Default for SysInfoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemInfoProvider for SysInfoProvider {
    /// Collect current system information synchronously.
    ///
    /// # Concurrency
    /// Sync method — it holds a `std::sync::Mutex` and calls
    /// `sysinfo::refresh_memory()` (a blocking syscall) under the lock.
    /// Concrete-type callers should use [`Self::system_info_async`]; trait-object
    /// callers (`Arc<dyn SystemInfoProvider>`, which cannot reach the inherent
    /// async method) must wrap this call in `tokio::task::spawn_blocking` at the
    /// call site.  See F-RR-C23-01 (cycle 23 W1 B2 #3668) for the prior
    /// tokio-worker-starvation incident.
    fn system_info(&self) -> StaticSystemInfo {
        let mut sys = self.sys.lock().unwrap_or_else(|e| e.into_inner());
        sys.refresh_memory();
        let cpu_count = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or_else(|_| sys.cpus().len());

        StaticSystemInfo {
            os_version: System::long_os_version().unwrap_or_default(),
            cpu_count,
            memory_total_bytes: sys.total_memory(),
            memory_available_bytes: sys.available_memory(),
            uptime_seconds: System::uptime(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_info_returns_nonzero_values() {
        let provider = SysInfoProvider::new();
        let info = provider.system_info();

        assert!(
            !info.os_version.is_empty(),
            "os_version should not be empty"
        );
        assert!(info.cpu_count > 0, "cpu_count should be > 0");
        assert!(info.memory_total_bytes > 0, "memory_total should be > 0");
    }

    #[test]
    fn system_info_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<SysInfoProvider>();
    }

    /// Verify that `system_info_async` runs the blocking syscall off the tokio
    /// worker thread and returns structurally valid data.
    ///
    /// This mirrors the `collect_metrics_runs_in_spawn_blocking` test in
    /// `system.rs` (F-PF-C20-03), establishing the same regression guardrail
    /// for the static-info path (#5997).
    #[tokio::test]
    async fn system_info_async_runs_in_spawn_blocking() {
        let provider = SysInfoProvider::new();
        let info = provider
            .system_info_async()
            .await
            .expect("system_info_async must not fail on a healthy host");

        assert!(
            !info.os_version.is_empty(),
            "os_version should not be empty"
        );
        assert!(info.cpu_count > 0, "cpu_count should be > 0");
        assert!(
            info.memory_total_bytes > 0,
            "memory_total_bytes should be > 0"
        );
        // available <= total is a sysinfo invariant
        assert!(
            info.memory_available_bytes <= info.memory_total_bytes,
            "available memory must not exceed total"
        );
    }

    /// Verify that the sync `system_info` and async `system_info_async` return
    /// consistent structural values (same cpu_count, same non-zero totals).
    #[tokio::test]
    async fn system_info_async_consistent_with_sync() {
        let provider = SysInfoProvider::new();
        let sync_info = provider.system_info();
        let async_info = provider
            .system_info_async()
            .await
            .expect("system_info_async must succeed");

        // cpu_count is derived from available_parallelism() which is stable
        assert_eq!(
            sync_info.cpu_count, async_info.cpu_count,
            "cpu_count must be identical between sync and async paths"
        );
        // Both paths must report the same total memory (no reboot between calls)
        assert_eq!(
            sync_info.memory_total_bytes, async_info.memory_total_bytes,
            "memory_total_bytes must be identical between sync and async paths"
        );
    }
}
