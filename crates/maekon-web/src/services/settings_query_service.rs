use maekon_api_contracts::settings::{AppSettings, StorageStats};

use crate::error::ApiError;
use crate::services::settings_assembler::config_to_settings;
use crate::services::web_contexts::SettingsWebContext;

#[derive(Clone)]
pub struct SettingsQueryService {
    ctx: SettingsWebContext,
}

impl SettingsQueryService {
    pub fn new(ctx: SettingsWebContext) -> Self {
        Self { ctx }
    }

    pub fn get_settings(&self) -> AppSettings {
        if let Some(ref config_manager) = self.ctx.config_manager {
            let config = config_manager.get();
            config_to_settings(&config, self.ctx.default_secret_backend_kind)
        } else {
            AppSettings::default()
        }
    }

    /// Returns storage stats.
    ///
    /// ADR-026 PR-4: `get_storage_stats_summary` is now async and routes the
    /// SQLite aggregation through the storage `with_conn_read` funnel, so it is
    /// awaited directly. The recursive `std::fs::read_dir` over the frames
    /// directory is still genuinely-blocking I/O, so it keeps its own
    /// `spawn_blocking` (runtime checklist §1).
    pub async fn get_storage_stats(&self) -> Result<StorageStats, ApiError> {
        let storage = self.ctx.storage.clone();
        let frames_dir = self.ctx.frames_dir.clone();

        let stats = storage
            .get_storage_stats_summary()
            .await
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        let frames_size_bytes = match frames_dir {
            Some(dir) => tokio::task::spawn_blocking(move || calculate_dir_size(&dir))
                .await
                .map_err(|e| ApiError::Internal(format!("spawn_blocking join error: {e}")))?,
            None => 0,
        };

        let db_size_bytes = stats.page_count * stats.page_size;

        Ok(StorageStats {
            db_size_bytes,
            frames_size_bytes,
            total_size_bytes: db_size_bytes + frames_size_bytes,
            frame_count: stats.frame_count,
            event_count: stats.event_count,
            metric_count: stats.metric_count,
            oldest_data_date: stats.oldest_data_date,
            newest_data_date: stats.newest_data_date,
        })
    }
}

fn calculate_dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;

    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Ok(metadata) = std::fs::metadata(&path) {
                    total += metadata.len();
                }
            } else if path.is_dir() {
                total += calculate_dir_size(&path);
            }
        }
    }

    total
}
