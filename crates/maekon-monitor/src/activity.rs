use async_trait::async_trait;
use chrono::Utc;
use maekon_core::error::CoreError;
use maekon_core::models::context::UserContext;
use maekon_core::ports::monitor::{ActivityMonitor, ProcessMonitor};
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
        // #6441 (F16): collect the two independent sources concurrently so a
        // tick waits on the slower, not the sum.
        let (active_window, processes) = tokio::join!(
            self.process_monitor.get_active_window(),
            self.process_monitor.get_top_processes(10),
        );

        let context = UserContext {
            timestamp: Utc::now(),
            active_window: active_window?,
            processes: processes?,
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

    async fn collect_active_context(&self) -> Result<UserContext, CoreError> {
        // #6441 (F13): the 1 Hz monitor loop needs only the active window; it
        // never reads `processes`. Skip the top-process enumeration entirely
        // (the full table walk + clone + sort that `collect_top_sync`
        // performs on every call).
        //
        // #7652 (HIGH-1): this used to ALSO collect the OS cursor position
        // into `UserContext.mouse_position` every tick (a real per-OS
        // syscall/subprocess -- `CGEventGetLocation` on macOS, `GetCursorPos`
        // on Windows, an `xdotool getmouselocation` FORK on Linux). No
        // consumer ever read that field (verified: `ctx.mouse_position` had
        // zero call sites outside this file's own tests), so it was pure
        // per-tick waste -- worst on Linux, where it forked a subprocess once
        // a second forever. `mouse_hook` now supplies real, continuously
        // event-driven mouse activity (clicks/scroll/move/position) directly
        // into `InputActivityCollector`, which is a strictly better signal
        // than a 1 Hz poll would have been, so the poll was removed rather
        // than wired to a consumer (routing it would have double-counted
        // move distance against the event-driven observer).
        let active_window = self.process_monitor.get_active_window().await?;

        Ok(UserContext {
            timestamp: Utc::now(),
            active_window,
            processes: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::context::{ProcessInfo, WindowInfo};
    use maekon_core::models::event::ProcessDetail;

    struct MockProcessMonitor;

    #[async_trait]
    impl ProcessMonitor for MockProcessMonitor {
        async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
            Ok(Some(WindowInfo {
                title: "test.rs".to_string(),
                app_name: "Code".to_string(),
                app_bundle_id: None,
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

    #[tokio::test]
    async fn collect_active_context_skips_processes() {
        let tracker = ActivityTracker::new(Arc::new(MockProcessMonitor));
        let ctx = tracker.collect_active_context().await.unwrap();
        // #6441 (F13): active window is collected, but processes are NOT
        // enumerated (the monitor hot-path never reads them).
        assert!(ctx.active_window.is_some());
        assert_eq!(ctx.active_window.unwrap().app_name, "Code");
        assert!(ctx.processes.is_empty());
    }
}
