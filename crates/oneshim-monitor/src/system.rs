use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::system::{NetworkInfo, SystemMetrics};
use oneshim_core::ports::monitor::SystemMonitor;
use std::sync::Mutex;
use sysinfo::{Disks, Networks, System};
use tracing::debug;

pub struct SysInfoMonitor {
    sys: Mutex<System>,
    disks: Mutex<Disks>,
    networks: Mutex<Networks>,
}

impl SysInfoMonitor {
    pub fn new() -> Self {
        Self {
            sys: Mutex::new(System::new_all()),
            disks: Mutex::new(Disks::new_with_refreshed_list()),
            networks: Mutex::new(Networks::new_with_refreshed_list()),
        }
    }
}

impl Default for SysInfoMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemMonitor for SysInfoMonitor {
    // P2 PR-A: 3 separate sysinfo locks (sys, disks, networks) are held
    // across quick metric extraction. Each is a fast read, and sysinfo's
    // iterators borrow from the owned Sys instances, requiring the lock to
    // outlive iteration. Tightening would require substantial refactoring
    // of sysinfo usage.
    #[allow(clippy::significant_drop_tightening)]
    async fn collect_metrics(&self) -> Result<SystemMetrics, CoreError> {
        {
            let mut sys = self.sys.lock().map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to acquire system lock: {e}"),
            })?;
            sys.refresh_cpu_usage();
            sys.refresh_memory();
        }

        {
            let mut disks = self.disks.lock().map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to acquire disk lock: {e}"),
            })?;
            disks.refresh(true);
        }

        {
            let mut networks = self.networks.lock().map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Failed to acquire network lock: {e}"),
            })?;
            networks.refresh(true);
        }

        let sys = self.sys.lock().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to acquire system lock: {e}"),
        })?;

        let cpu_usage = sys.global_cpu_usage();
        let memory_used = sys.used_memory();
        let memory_total = sys.total_memory();

        let disks = self.disks.lock().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to acquire disk lock: {e}"),
        })?;
        let (disk_used, disk_total) = disks.list().iter().fold((0u64, 0u64), |(used, total), d| {
            (
                used + d.total_space() - d.available_space(),
                total + d.total_space(),
            )
        });

        let networks = self.networks.lock().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to acquire network lock: {e}"),
        })?;
        let (upload_speed, download_speed) = networks
            .list()
            .iter()
            .fold((0u64, 0u64), |(up, down), (_name, data)| {
                (up + data.transmitted(), down + data.received())
            });

        let network = Some(NetworkInfo {
            upload_speed,
            download_speed,
            is_connected: download_speed > 0 || upload_speed > 0,
        });

        let metrics = SystemMetrics {
            timestamp: chrono::Utc::now(),
            cpu_usage,
            memory_used,
            memory_total,
            disk_used,
            disk_total,
            network,
            typing_wpm: 0.0,
        };

        debug!(
            "system metrics: CPU {:.1}%, memory {}/{}MB",
            metrics.cpu_usage,
            metrics.memory_used / 1_048_576,
            metrics.memory_total / 1_048_576
        );

        Ok(metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn collect_metrics() {
        let monitor = SysInfoMonitor::new();
        let metrics = monitor.collect_metrics().await.unwrap();

        assert!(metrics.cpu_usage >= 0.0);
        assert!(metrics.memory_total > 0);
        assert!(metrics.memory_used <= metrics.memory_total);
    }
}
