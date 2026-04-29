use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::context::{ProcessInfo, WindowInfo};
use oneshim_core::models::event::ProcessDetail;
use oneshim_core::ports::monitor::ProcessMonitor;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::System;
use tracing::debug;

/// Minimum interval between full process-list refreshes.
const REFRESH_COOLDOWN: Duration = Duration::from_secs(2);

pub struct ProcessTracker {
    sys: Mutex<System>,
    /// Timestamp of the last `refresh_processes` call.
    last_refresh: Mutex<Instant>,
}

impl ProcessTracker {
    pub fn new() -> Self {
        Self {
            sys: Mutex::new(System::new_all()),
            last_refresh: Mutex::new(Instant::now()),
        }
    }

    /// Refresh the process list only if the cooldown has elapsed.
    fn refresh_if_stale(&self, sys: &mut System) {
        let mut last = self.last_refresh.lock().unwrap_or_else(|e| e.into_inner());
        if last.elapsed() >= REFRESH_COOLDOWN {
            sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            *last = Instant::now();
        }
    }

    /// Test-only accessor for the cached `last_refresh` timestamp.
    #[cfg(test)]
    pub(crate) fn _last_refresh_instant(&self) -> Instant {
        *self.last_refresh.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Test-only mutator to drive cooldown expiration without wall-clock sleep.
    #[cfg(test)]
    pub(crate) fn _set_last_refresh(&self, t: Instant) {
        *self.last_refresh.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }
}

impl Default for ProcessTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ProcessMonitor for ProcessTracker {
    async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::get_active_window_macos()
                .await
                .map_err(Into::into)
        }
        #[cfg(target_os = "windows")]
        {
            crate::windows::get_active_window_windows().map_err(Into::into)
        }
        #[cfg(target_os = "linux")]
        {
            crate::linux::get_active_window_linux()
                .await
                .map_err(Into::into)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            Ok(None)
        }
    }

    // P2 PR-A: sys lock held across refresh + process enumeration. sysinfo's
    // process iterator borrows from the Sys instance, so the lock must live
    // until collection is complete. Tightening would require a clone that
    // defeats sysinfo's purpose.
    #[allow(clippy::significant_drop_tightening)]
    async fn get_top_processes(&self, limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
        let mut sys = self.sys.lock().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to acquire system lock: {e}"),
        })?;
        self.refresh_if_stale(&mut sys);

        let mut processes: Vec<ProcessInfo> = sys
            .processes()
            .values()
            .map(|p| ProcessInfo {
                pid: p.pid().as_u32(),
                name: p.name().to_string_lossy().to_string(),
                cpu_usage: p.cpu_usage(),
                memory_bytes: p.memory(),
            })
            .collect();

        processes.sort_by(|a, b| {
            b.cpu_usage
                .partial_cmp(&a.cpu_usage)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        processes.truncate(limit);

        debug!("top {}items collect", processes.len());
        Ok(processes)
    }

    // P2 PR-A: same sysinfo borrow constraint as `get_top_processes`.
    #[allow(clippy::significant_drop_tightening)]
    async fn get_detailed_processes(
        &self,
        foreground_pid: Option<u32>,
        top_n: usize,
    ) -> Result<Vec<ProcessDetail>, CoreError> {
        let mut sys = self.sys.lock().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Failed to acquire system lock: {e}"),
        })?;
        self.refresh_if_stale(&mut sys);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut all_details: Vec<ProcessDetail> = sys
            .processes()
            .values()
            .map(|p| {
                let pid = p.pid().as_u32();
                let start_time = p.start_time();
                let running_secs = if start_time > 0 && now > start_time {
                    now - start_time
                } else {
                    0
                };

                let exe_path = p.exe().map(|path| {
                    let path_str = path.to_string_lossy().to_string();
                    if path_str.contains("/Users/") {
                        path_str
                            .split("/Users/")
                            .last()
                            .and_then(|s| s.split('/').nth(1))
                            .map(|rest| format!("~/{}", rest))
                            .unwrap_or_else(|| "~/...".to_string())
                    } else if path_str.contains("\\Users\\") {
                        path_str
                            .split("\\Users\\")
                            .last()
                            .and_then(|s| s.split('\\').nth(1))
                            .map(|rest| format!("~\\{}", rest))
                            .unwrap_or_else(|| "~\\...".to_string())
                    } else {
                        path_str
                    }
                });

                ProcessDetail {
                    name: p.name().to_string_lossy().to_string(),
                    pid,
                    cpu_percent: p.cpu_usage(),
                    memory_mb: p.memory() as f64 / (1024.0 * 1024.0),
                    window_count: 0, // filled by platform-specific window APIs
                    is_foreground: foreground_pid == Some(pid),
                    running_secs,
                    executable_path: exe_path,
                }
            })
            .collect();

        all_details.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut result: Vec<ProcessDetail> = Vec::with_capacity(top_n + 1);
        let mut seen_pids: HashSet<u32> = HashSet::new();

        if let Some(fg_pid) = foreground_pid {
            if let Some(fg_detail) = all_details.iter().find(|d| d.pid == fg_pid) {
                result.push(fg_detail.clone());
                seen_pids.insert(fg_pid);
            }
        }

        for detail in all_details {
            if result.len() > top_n {
                break;
            }
            if !seen_pids.contains(&detail.pid) {
                seen_pids.insert(detail.pid);
                result.push(detail);
            }
        }

        debug!(
            "detailed process list collected: count={}, foreground={:?}",
            result.len(),
            foreground_pid
        );
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_top_processes() {
        let tracker = ProcessTracker::new();
        let procs = tracker.get_top_processes(5).await.unwrap();
        assert!(procs.len() <= 5);
        assert!(!procs.is_empty());
    }

    #[tokio::test]
    async fn refresh_if_stale_skips_within_cooldown() {
        let tracker = ProcessTracker::new();
        let before = tracker._last_refresh_instant();
        let _ = tracker.get_top_processes(5).await.unwrap();
        let after = tracker._last_refresh_instant();
        // Second call within the 2s cooldown — last_refresh should not advance.
        assert_eq!(before, after, "refresh_if_stale advanced within cooldown");
    }

    #[tokio::test]
    async fn refresh_if_stale_refreshes_after_cooldown() {
        let tracker = ProcessTracker::new();
        let pushed_back = Instant::now()
            .checked_sub(Duration::from_secs(3))
            .expect("test runner: Instant::now() must be >= 3s after process start");
        tracker._set_last_refresh(pushed_back);
        let before = tracker._last_refresh_instant();
        let _ = tracker.get_top_processes(5).await.unwrap();
        let after = tracker._last_refresh_instant();
        assert!(
            after > before,
            "refresh_if_stale did not advance after cooldown"
        );
    }
}
