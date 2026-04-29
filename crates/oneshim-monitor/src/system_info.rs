//! Static system information adapter using sysinfo.

use std::sync::Mutex;

use oneshim_core::models::system::StaticSystemInfo;
use oneshim_core::ports::system_info_provider::SystemInfoProvider;
use sysinfo::System;

/// Adapter providing static system hardware/software info.
pub struct SysInfoProvider {
    sys: Mutex<System>,
}

impl SysInfoProvider {
    pub fn new() -> Self {
        Self {
            sys: Mutex::new(System::new_all()),
        }
    }
}

impl Default for SysInfoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemInfoProvider for SysInfoProvider {
    fn system_info(&self) -> StaticSystemInfo {
        let mut sys = self.sys.lock().unwrap_or_else(|e| e.into_inner());
        sys.refresh_memory();

        StaticSystemInfo {
            os_version: System::long_os_version().unwrap_or_default(),
            cpu_count: sys.cpus().len(),
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
