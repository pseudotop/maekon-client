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
            Some(data_dir) => tokio::task::spawn_blocking(move || calculate_frames_size(&data_dir))
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

/// `CoreState::frames_dir` is the frame storage base directory, not the
/// `<base>/frames` leaf. Stored frame paths intentionally include the `frames/`
/// prefix, so deletion resolves them against the base directory. Storage stats,
/// however, must exclude sibling database, log, and configuration files.
fn calculate_frames_size(data_dir: &std::path::Path) -> u64 {
    calculate_dir_size(&data_dir.join("frames"))
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

#[cfg(test)]
mod tests {
    use super::calculate_frames_size;

    #[test]
    fn frame_size_excludes_unrelated_files_under_data_root() {
        let data_dir = tempfile::tempdir().expect("temporary data directory");
        let frames_dir = data_dir.path().join("frames").join("2026-07-13");
        std::fs::create_dir_all(&frames_dir).expect("frames directory");
        std::fs::write(frames_dir.join("first.webp"), [1_u8; 3]).expect("first frame");
        std::fs::write(frames_dir.join("second.webp"), [2_u8; 5]).expect("second frame");
        std::fs::write(data_dir.path().join("maekon.db"), [0_u8; 13]).expect("database file");
        std::fs::create_dir_all(data_dir.path().join("logs")).expect("logs directory");
        std::fs::write(data_dir.path().join("logs/runtime.log"), [0_u8; 21]).expect("runtime log");

        assert_eq!(calculate_frames_size(data_dir.path()), 8);
    }

    #[test]
    fn frame_size_is_zero_when_frames_subtree_is_missing() {
        let data_dir = tempfile::tempdir().expect("temporary data directory");
        std::fs::write(data_dir.path().join("maekon.db"), [0_u8; 13]).expect("database file");

        assert_eq!(calculate_frames_size(data_dir.path()), 0);
    }
}
