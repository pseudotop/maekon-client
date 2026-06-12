//! Static system information adapter using sysinfo.

use std::sync::Mutex;

use maekon_core::models::system::StaticSystemInfo;
use maekon_core::ports::system_info_provider::SystemInfoProvider;
use sysinfo::System;

/// Adapter providing static system hardware/software info.
pub struct SysInfoProvider {
    sys: Mutex<System>,
}

impl SysInfoProvider {
    pub fn new() -> Self {
        // F-PF-18: System::new_all() 대신 System::new() 사용.
        // system_info() 호출 시 refresh_memory 로 필요한 항목만 갱신한다.
        // CPU/프로세스 테이블은 이 어댑터에서 불필요하므로 사전 할당하지 않는다.
        Self {
            sys: Mutex::new(System::new()),
        }
    }
}

impl Default for SysInfoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemInfoProvider for SysInfoProvider {
    /// 현재 시스템 정보를 동기적으로 수집한다.
    ///
    /// # Concurrency
    /// Sync method. Async callers MUST wrap in `tokio::task::spawn_blocking` —
    /// internally holds a `std::sync::Mutex` and calls `sysinfo::refresh_memory()`.
    /// See F-RR-C23-01 (cycle 23 W1 B2 #3668) for prior incident.
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
}
