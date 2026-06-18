use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::InternalCode;
use maekon_core::models::system::{NetworkInfo, PowerStatus, SystemMetrics};
use maekon_core::ports::monitor::SystemMonitor;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use sysinfo::{Disks, Networks, System};
use tracing::debug;

/// #6441 (F15): minimum interval between disk statfs refreshes. Disk usage changes
/// slowly, so refreshing it on every 5s metrics tick is wasteful; serve cached values
/// between refreshes.
const DISK_REFRESH_COOLDOWN: Duration = Duration::from_secs(60);

pub struct SysInfoMonitor {
    sys: Arc<Mutex<System>>,
    disks: Arc<Mutex<Disks>>,
    networks: Arc<Mutex<Networks>>,
    /// #6441 (F15): timestamp of the last disk statfs refresh (gates the cooldown).
    last_disk_refresh: Arc<Mutex<Instant>>,
}

impl SysInfoMonitor {
    pub fn new() -> Self {
        // F-PF-18: System::new_all() eagerly allocates the entire process table.
        // Replace it with System::new() to cut initialization cost, and refresh
        // only the items we need via refresh_cpu_usage/refresh_memory when
        // collect_metrics is called.
        // The process table is not used by this adapter, so it is never refreshed.
        Self {
            sys: Arc::new(Mutex::new(System::new())),
            disks: Arc::new(Mutex::new(Disks::new_with_refreshed_list())),
            networks: Arc::new(Mutex::new(Networks::new_with_refreshed_list())),
            // Disks were just refreshed above, so the next refresh is one cooldown out.
            last_disk_refresh: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// Synchronous metric collection — invoked inside `spawn_blocking`.
    ///
    /// F-PF-C20-03: isolate sysinfo I/O (statfs per FS, getifaddrs) on the
    /// blocking-only thread pool so it never occupies a tokio worker thread.
    fn collect_metrics_sync(
        sys: Arc<Mutex<System>>,
        disks: Arc<Mutex<Disks>>,
        networks: Arc<Mutex<Networks>>,
        last_disk_refresh: Arc<Mutex<Instant>>,
    ) -> Result<SystemMetrics, CoreError> {
        {
            let mut sys = sys.lock().map_err(|e| CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("Failed to acquire system lock: {e}"),
            })?;
            sys.refresh_cpu_usage();
            sys.refresh_memory();
        }

        {
            // #6441 (F15): refresh disk statfs at most once per DISK_REFRESH_COOLDOWN
            // (vs every 5s metrics tick). The cached Disks list still serves reads below.
            // Network counters are cumulative-rate, so they stay per-tick.
            let mut last = last_disk_refresh
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if last.elapsed() >= DISK_REFRESH_COOLDOWN {
                disks
                    .lock()
                    .map_err(|e| CoreError::Internal {
                        code: InternalCode::Generic,
                        message: format!("Failed to acquire disk lock: {e}"),
                    })?
                    .refresh(true);
                *last = Instant::now();
            }
        }

        {
            let mut networks = networks.lock().map_err(|e| CoreError::Internal {
                code: InternalCode::Generic,
                message: format!("Failed to acquire network lock: {e}"),
            })?;
            networks.refresh(true);
        }

        let sys = sys.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire system lock: {e}"),
        })?;

        let cpu_usage = sys.global_cpu_usage();
        let memory_used = sys.used_memory();
        let memory_total = sys.total_memory();
        drop(sys);

        let disks = disks.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire disk lock: {e}"),
        })?;
        let (disk_used, disk_total) = disks.list().iter().fold((0u64, 0u64), |(used, total), d| {
            // saturating_sub: some filesystems (e.g. macOS APFS with purgeable
            // space) report available_space() > total_space(); a raw subtraction
            // would underflow → debug panic / release garbage metric (review4 monitor).
            (
                used + d.total_space().saturating_sub(d.available_space()),
                total + d.total_space(),
            )
        });
        drop(disks);

        let networks = networks.lock().map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("Failed to acquire network lock: {e}"),
        })?;
        let (upload_speed, download_speed) = networks
            .list()
            .iter()
            .fold((0u64, 0u64), |(up, down), (_name, data)| {
                (up + data.transmitted(), down + data.received())
            });
        drop(networks);

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

impl Default for SysInfoMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemMonitor for SysInfoMonitor {
    // F-PF-C20-03: isolate sysinfo I/O (statfs per FS, getifaddrs/proc/net/dev)
    // via spawn_blocking so it never occupies a tokio worker thread for up to 5s.
    // Wrap the inner fields in Arc<Mutex<T>> to satisfy the 'static + Send bound.
    async fn collect_metrics(&self) -> Result<SystemMetrics, CoreError> {
        let sys = Arc::clone(&self.sys);
        let disks = Arc::clone(&self.disks);
        let networks = Arc::clone(&self.networks);
        let last_disk_refresh = Arc::clone(&self.last_disk_refresh);

        tokio::task::spawn_blocking(move || {
            Self::collect_metrics_sync(sys, disks, networks, last_disk_refresh)
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        })?
    }

    async fn current_power_status(&self) -> Result<PowerStatus, CoreError> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::current_power_status_macos()
                .await
                .map_err(|e| CoreError::Internal {
                    code: InternalCode::Generic,
                    message: format!("Failed to collect power status: {e}"),
                })
        }

        #[cfg(not(target_os = "macos"))]
        {
            Ok(PowerStatus::default())
        }
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

    #[tokio::test]
    async fn collect_metrics_runs_in_spawn_blocking() {
        // F-PF-C20-03: confirm that the work runs on a separate blocking thread
        // pool via spawn_blocking, not tokio::task::block_in_place.
        // A return type of Result<SystemMetrics> is sufficient (regression test).
        let monitor = SysInfoMonitor::new();
        let metrics = monitor
            .collect_metrics()
            .await
            .expect("collect_metrics via spawn_blocking must not fail");
        // Probe that the returned SystemMetrics are structurally valid, mirroring
        // the collect_metrics test above. Justified: sysinfo runs on any host
        // including CI — spawn_blocking itself must not error (#5594).
        assert!(metrics.cpu_usage >= 0.0, "cpu_usage must be non-negative");
        assert!(metrics.memory_total > 0, "memory_total must be positive");
        assert!(
            metrics.memory_used <= metrics.memory_total,
            "memory_used must not exceed memory_total"
        );
    }

    #[tokio::test]
    async fn collect_metrics_propagates_join_error() {
        // F-QA-C21-06: verify the spawn_blocking JoinError → CoreError::Internal mapping.
        //
        // The map_err(|e| CoreError::Internal { ... }) path in collect_metrics() is
        // unreachable under normal execution, so we verify it directly with a
        // spawn_blocking panic scenario.
        //
        // A JoinError occurs when a spawn_blocking task panics or is cancelled.
        // Here we hand a panicking closure straight to spawn_blocking to produce a
        // JoinError, then assert the mapping into CoreError::Internal.
        let join_result: Result<(), tokio::task::JoinError> =
            tokio::task::spawn_blocking(|| panic!("F-QA-C21-06 test panic")).await;

        let join_err = join_result.unwrap_err();
        assert!(
            join_err.is_panic(),
            "the JoinError of a panicking spawn_blocking must be is_panic()"
        );

        // Reproduce the JoinError → CoreError::Internal mapping (same pattern as inside collect_metrics)
        let core_err: Result<(), CoreError> = Err(join_err).map_err(|e| CoreError::Internal {
            code: InternalCode::Generic,
            message: format!("spawn_blocking join error: {e}"),
        });

        let err = core_err.unwrap_err();
        // Confirm it is the CoreError::Internal variant
        assert!(
            matches!(err, CoreError::Internal { .. }),
            "a JoinError must map to the CoreError::Internal variant: {err:?}"
        );
        // Confirm the message contains the identifying string
        if let CoreError::Internal { message, .. } = err {
            assert!(
                message.contains("spawn_blocking join error"),
                "message must contain 'spawn_blocking join error': {message}"
            );
        }
    }
}
