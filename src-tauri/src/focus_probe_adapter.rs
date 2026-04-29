use async_trait::async_trait;
use sha2::{Digest, Sha256};
use std::sync::Arc;

use oneshim_core::error::CoreError;
use oneshim_core::models::context::WindowInfo;
use oneshim_core::models::gui::{ExecutionBinding, FocusSnapshot, FocusValidation};
use oneshim_core::ports::focus_probe::FocusProbe;
use oneshim_core::ports::monitor::ProcessMonitor;
use tracing::debug;

pub struct ProcessMonitorFocusProbe {
    process_monitor: Arc<dyn ProcessMonitor>,
}

impl ProcessMonitorFocusProbe {
    pub fn new(process_monitor: Arc<dyn ProcessMonitor>) -> Self {
        Self { process_monitor }
    }
}

#[async_trait]
impl FocusProbe for ProcessMonitorFocusProbe {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError> {
        let window = self
            .process_monitor
            .get_active_window()
            .await?
            .ok_or_else(|| CoreError::ServiceUnavailable {
                code: oneshim_core::error_codes::ServiceCode::Unavailable,
                message: "No active window available".to_string(),
            })?;

        Ok(window_to_focus_snapshot(window))
    }

    async fn validate_execution_binding(
        &self,
        binding: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError> {
        let window = self.process_monitor.get_active_window().await?;

        let Some(window) = window else {
            return Ok(FocusValidation {
                valid: false,
                reason: Some("No active window available".to_string()),
                current_focus: None,
            });
        };

        let snapshot = window_to_focus_snapshot(window);
        let focus_match = snapshot.focus_hash == binding.focus_hash;
        let app_match = binding
            .app_name
            .as_deref()
            .map(|value| value == snapshot.app_name)
            .unwrap_or(true);
        let pid_match = binding
            .pid
            .map(|value| value == snapshot.pid)
            .unwrap_or(true);
        let valid = focus_match && app_match && pid_match;

        let reason = if valid {
            None
        } else {
            Some(format!(
                "focus_match={} app_match={} pid_match={}",
                focus_match, app_match, pid_match
            ))
        };

        Ok(FocusValidation {
            valid,
            reason,
            current_focus: Some(snapshot),
        })
    }
}

fn window_to_focus_snapshot(window: WindowInfo) -> FocusSnapshot {
    let focus_hash = hash_window_info(&window);
    FocusSnapshot {
        app_name: window.app_name,
        window_title: window.title,
        pid: window.pid,
        bounds: window.bounds,
        captured_at: chrono::Utc::now(),
        focus_hash,
    }
}

fn hash_window_info(window: &WindowInfo) -> String {
    let mut hasher = Sha256::new();
    hasher.update(window.app_name.as_bytes());
    hasher.update(b"|");
    hasher.update(window.title.as_bytes());
    hasher.update(b"|");
    hasher.update(window.pid.to_le_bytes());

    if let Some(bounds) = window.bounds {
        hasher.update(bounds.x.to_le_bytes());
        hasher.update(bounds.y.to_le_bytes());
        hasher.update(bounds.width.to_le_bytes());
        hasher.update(bounds.height.to_le_bytes());
    }

    let digest = hasher.finalize();
    encode_hex(digest.as_slice())
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        if let Err(e) = write!(&mut out, "{byte:02x}") {
            debug!("operation failed: {e}");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use oneshim_core::models::context::WindowBounds;
    use oneshim_core::ports::monitor::ProcessMonitor;
    use std::sync::Mutex;

    // ── MockProcessMonitor ──────────────────────────────────────────────

    struct MockProcessMonitor {
        window: Mutex<Option<WindowInfo>>,
    }

    impl MockProcessMonitor {
        fn with_window(window: WindowInfo) -> Self {
            Self {
                window: Mutex::new(Some(window)),
            }
        }

        fn no_window() -> Self {
            Self {
                window: Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl ProcessMonitor for MockProcessMonitor {
        async fn get_active_window(&self) -> Result<Option<WindowInfo>, CoreError> {
            Ok(self.window.lock().unwrap().clone())
        }

        async fn get_top_processes(
            &self,
            _limit: usize,
        ) -> Result<Vec<oneshim_core::models::context::ProcessInfo>, CoreError> {
            Ok(vec![])
        }

        async fn get_detailed_processes(
            &self,
            _foreground_pid: Option<u32>,
            _top_n: usize,
        ) -> Result<Vec<oneshim_core::models::event::ProcessDetail>, CoreError> {
            Ok(vec![])
        }
    }

    fn test_window() -> WindowInfo {
        WindowInfo {
            title: "editor".to_string(),
            app_name: "Code".to_string(),
            pid: 101,
            bounds: Some(WindowBounds {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            }),
        }
    }

    // ── hash determinism tests ──────────────────────────────────────────

    #[test]
    fn hash_is_deterministic_for_same_input() {
        let window = test_window();
        let h1 = hash_window_info(&window);
        let h2 = hash_window_info(&window);
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_changes_when_window_title_changes() {
        let base = test_window();
        let mut changed = base.clone();
        changed.title = "terminal".to_string();
        assert_ne!(hash_window_info(&base), hash_window_info(&changed));
    }

    #[test]
    fn hash_changes_when_app_name_changes() {
        let base = test_window();
        let mut changed = base.clone();
        changed.app_name = "Firefox".to_string();
        assert_ne!(hash_window_info(&base), hash_window_info(&changed));
    }

    #[test]
    fn hash_changes_when_pid_changes() {
        let base = test_window();
        let mut changed = base.clone();
        changed.pid = 999;
        assert_ne!(hash_window_info(&base), hash_window_info(&changed));
    }

    #[test]
    fn hash_changes_when_bounds_change() {
        let base = test_window();
        let mut changed = base.clone();
        changed.bounds = Some(WindowBounds {
            x: 50,
            y: 50,
            width: 200,
            height: 200,
        });
        assert_ne!(hash_window_info(&base), hash_window_info(&changed));
    }

    #[test]
    fn hash_differs_with_and_without_bounds() {
        let base = test_window();
        let mut no_bounds = base.clone();
        no_bounds.bounds = None;
        assert_ne!(hash_window_info(&base), hash_window_info(&no_bounds));
    }

    // ── window_to_focus_snapshot tests ───────────────────────────────────

    #[test]
    fn snapshot_preserves_window_fields() {
        let window = test_window();
        let snapshot = window_to_focus_snapshot(window.clone());
        assert_eq!(snapshot.app_name, window.app_name);
        assert_eq!(snapshot.window_title, window.title);
        assert_eq!(snapshot.pid, window.pid);
        assert!(snapshot.bounds.is_some());
        assert!(!snapshot.focus_hash.is_empty());
    }

    // ── FocusProbe::current_focus tests ─────────────────────────────────

    #[tokio::test]
    async fn current_focus_returns_snapshot_when_window_active() {
        let probe =
            ProcessMonitorFocusProbe::new(Arc::new(MockProcessMonitor::with_window(test_window())));
        let snapshot = probe.current_focus().await.unwrap();
        assert_eq!(snapshot.app_name, "Code");
        assert_eq!(snapshot.pid, 101);
    }

    #[tokio::test]
    async fn current_focus_errors_when_no_active_window() {
        let probe = ProcessMonitorFocusProbe::new(Arc::new(MockProcessMonitor::no_window()));
        let err = probe.current_focus().await.unwrap_err();
        assert!(matches!(err, CoreError::ServiceUnavailable { .. }));
    }

    // ── validate_execution_binding tests ────────────────────────────────

    #[tokio::test]
    async fn validation_succeeds_when_focus_matches() {
        let window = test_window();
        let hash = hash_window_info(&window);
        let probe = ProcessMonitorFocusProbe::new(Arc::new(MockProcessMonitor::with_window(
            window.clone(),
        )));

        let binding = ExecutionBinding {
            focus_hash: hash,
            app_name: Some("Code".to_string()),
            pid: Some(101),
        };
        let result = probe.validate_execution_binding(&binding).await.unwrap();
        assert!(result.valid);
        assert!(result.reason.is_none());
    }

    #[tokio::test]
    async fn validation_fails_when_hash_differs() {
        let probe =
            ProcessMonitorFocusProbe::new(Arc::new(MockProcessMonitor::with_window(test_window())));

        let binding = ExecutionBinding {
            focus_hash: "wrong-hash".to_string(),
            app_name: Some("Code".to_string()),
            pid: Some(101),
        };
        let result = probe.validate_execution_binding(&binding).await.unwrap();
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("focus_match=false"));
    }

    #[tokio::test]
    async fn validation_fails_when_app_name_differs() {
        let window = test_window();
        let hash = hash_window_info(&window);
        let probe =
            ProcessMonitorFocusProbe::new(Arc::new(MockProcessMonitor::with_window(window)));

        let binding = ExecutionBinding {
            focus_hash: hash,
            app_name: Some("Firefox".to_string()),
            pid: Some(101),
        };
        let result = probe.validate_execution_binding(&binding).await.unwrap();
        assert!(!result.valid);
        assert!(result.reason.unwrap().contains("app_match=false"));
    }

    #[tokio::test]
    async fn validation_fails_when_no_active_window() {
        let probe = ProcessMonitorFocusProbe::new(Arc::new(MockProcessMonitor::no_window()));

        let binding = ExecutionBinding {
            focus_hash: "any".to_string(),
            app_name: None,
            pid: None,
        };
        let result = probe.validate_execution_binding(&binding).await.unwrap();
        assert!(!result.valid);
    }

    #[tokio::test]
    async fn validation_ignores_none_app_name_and_pid() {
        let window = test_window();
        let hash = hash_window_info(&window);
        let probe =
            ProcessMonitorFocusProbe::new(Arc::new(MockProcessMonitor::with_window(window)));

        let binding = ExecutionBinding {
            focus_hash: hash,
            app_name: None, // should be ignored
            pid: None,      // should be ignored
        };
        let result = probe.validate_execution_binding(&binding).await.unwrap();
        assert!(result.valid);
    }
}
