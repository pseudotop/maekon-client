//! Tauri-side adapter for RuntimeLogProvider port.

use std::path::PathBuf;

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::models::bug_report::RuntimeLogSnapshot;
use oneshim_core::ports::runtime_log_provider::RuntimeLogProvider;

use super::log_helpers;

const MAX_LINE_LIMIT: usize = 500;

pub struct TauriRuntimeLogProvider {
    log_dir: PathBuf,
}

impl TauriRuntimeLogProvider {
    pub fn new(log_dir: PathBuf) -> Self {
        Self { log_dir }
    }
}

#[async_trait]
impl RuntimeLogProvider for TauriRuntimeLogProvider {
    async fn snapshot(&self, line_limit: usize) -> Result<RuntimeLogSnapshot, CoreError> {
        let line_limit = line_limit.clamp(10, MAX_LINE_LIMIT);
        let log_dir = self.log_dir.clone();

        tokio::task::spawn_blocking(move || {
            let latest_log =
                log_helpers::newest_log_file(&log_dir).map_err(|msg| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: msg,
                })?;

            let (log_file, line_count, recent_text) = if let Some(path) = latest_log {
                let (count, text) =
                    log_helpers::tail_log_file(&path, line_limit).map_err(|msg| {
                        CoreError::Internal {
                            code: oneshim_core::error_codes::InternalCode::Generic,
                            message: msg,
                        }
                    })?;
                (Some(path.display().to_string()), count, text)
            } else {
                (None, 0, String::new())
            };

            Ok(RuntimeLogSnapshot {
                log_dir: log_dir.display().to_string(),
                log_file,
                line_count,
                recent_text,
            })
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
            message: format!("Log snapshot task failed: {e}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn snapshot_returns_empty_for_missing_dir() {
        let provider = TauriRuntimeLogProvider::new(PathBuf::from("/nonexistent/logs"));
        let snap = provider.snapshot(50).await.unwrap();
        assert!(snap.log_file.is_none());
        assert_eq!(snap.line_count, 0);
        assert!(snap.recent_text.is_empty());
    }

    #[tokio::test]
    async fn snapshot_reads_newest_file() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("app.log"), "line1\nline2\nline3\n").unwrap();
        let provider = TauriRuntimeLogProvider::new(dir.path().to_path_buf());
        let snap = provider.snapshot(100).await.unwrap();
        assert_eq!(snap.line_count, 3);
        assert!(snap.recent_text.contains("line1"));
        assert!(snap.log_file.is_some());
    }

    #[tokio::test]
    async fn snapshot_clamps_line_limit() {
        let dir = TempDir::new().unwrap();
        let lines: String = (0..600).map(|i| format!("line {i}\n")).collect();
        fs::write(dir.path().join("big.log"), &lines).unwrap();
        let provider = TauriRuntimeLogProvider::new(dir.path().to_path_buf());
        let snap = provider.snapshot(1000).await.unwrap();
        assert!(snap.line_count <= 500);
    }
}
