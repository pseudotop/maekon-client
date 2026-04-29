use async_trait::async_trait;
use chrono::Utc;
use oneshim_core::error::CoreError;
use oneshim_core::models::context::{MousePosition, UserContext};
use oneshim_core::ports::monitor::{ActivityMonitor, ProcessMonitor};
use std::sync::Arc;
use tracing::debug;

pub struct ActivityTracker {
    process_monitor: Arc<dyn ProcessMonitor>,
}

impl ActivityTracker {
    pub fn new(process_monitor: Arc<dyn ProcessMonitor>) -> Self {
        Self { process_monitor }
    }
}

#[async_trait]
impl ActivityMonitor for ActivityTracker {
    async fn collect_context(&self) -> Result<UserContext, CoreError> {
        let active_window = self.process_monitor.get_active_window().await?;
        let processes = self.process_monitor.get_top_processes(10).await?;

        let context = UserContext {
            timestamp: Utc::now(),
            active_window,
            processes,
            mouse_position: get_mouse_position().await,
        };

        debug!(
            "context collected: app={}, process_count={}",
            context
                .active_window
                .as_ref()
                .map_or("none", |w| &w.app_name),
            context.processes.len()
        );

        Ok(context)
    }
}

/// - macOS: Core Graphics API
/// - Windows: Win32 GetCursorPos
/// - Linux: xdotool getmouselocation (X11/XWayland)
#[allow(clippy::unused_async)] // async required on linux (xdotool spawns process)
async fn get_mouse_position() -> Option<MousePosition> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::get_mouse_position_macos()
    }

    #[cfg(target_os = "windows")]
    {
        crate::windows::get_mouse_position_windows()
    }

    #[cfg(target_os = "linux")]
    {
        crate::linux::get_mouse_position_linux().await
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::context::{ProcessInfo, WindowInfo};
    use oneshim_core::models::event::ProcessDetail;

    struct MockProcessMonitor;

    #[async_trait]
    impl ProcessMonitor for MockProcessMonitor {
        async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
            Ok(Some(WindowInfo {
                title: "test.rs".to_string(),
                app_name: "Code".to_string(),
                pid: 1234,
                bounds: None,
            }))
        }

        async fn get_top_processes(&self, _limit: usize) -> Result<Vec<ProcessInfo>, CoreError> {
            Ok(vec![ProcessInfo {
                pid: 1234,
                name: "code".to_string(),
                cpu_usage: 5.0,
                memory_bytes: 100_000_000,
            }])
        }

        async fn get_detailed_processes(
            &self,
            _foreground_pid: Option<u32>,
            _top_n: usize,
        ) -> Result<Vec<ProcessDetail>, CoreError> {
            Ok(vec![ProcessDetail {
                name: "code".to_string(),
                pid: 1234,
                cpu_percent: 5.0,
                memory_mb: 100.0,
                window_count: 1,
                is_foreground: true,
                running_secs: 3600,
                executable_path: Some("/usr/bin/code".to_string()),
            }])
        }
    }

    #[tokio::test]
    async fn collect_context() {
        let tracker = ActivityTracker::new(Arc::new(MockProcessMonitor));
        let ctx = tracker.collect_context().await.unwrap();
        assert!(ctx.active_window.is_some());
        assert_eq!(ctx.active_window.unwrap().app_name, "Code");
        assert_eq!(ctx.processes.len(), 1);
    }
}
