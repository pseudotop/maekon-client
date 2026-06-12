use super::fs::FrameFileStorage;
use super::util::{
    calculate_dir_size, count_files_in_dir, delete_date_dirs_chunked, list_date_dirs,
};
use crate::error::StorageError;
use chrono::Utc;
use std::sync::atomic::Ordering;
use tokio::fs;
use tracing::{info, warn};

const PARALLEL_DELETE_LIMIT: usize = 8;

impl FrameFileStorage {
    pub async fn enforce_retention(&self) -> Result<usize, StorageError> {
        let frames_dir = self.base_dir.join("frames");

        if !frames_dir.exists() {
            return Ok(0);
        }

        let cutoff_date = (Utc::now() - chrono::Duration::days(self.retention_days as i64))
            .format("%Y-%m-%d")
            .to_string();

        let mut entries = fs::read_dir(&frames_dir)
            .await
            .map_err(|e| StorageError::Internal(format!("Failed to read frames directory: {e}")))?;

        let mut dirs_to_delete = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| StorageError::Internal(format!("Failed to read directory entry: {e}")))?
        {
            let path = entry.path();

            if let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) {
                if dir_name.len() == 10 && dir_name < cutoff_date.as_str() {
                    dirs_to_delete.push(path);
                }
            }
        }

        if dirs_to_delete.is_empty() {
            return Ok(0);
        }

        let mut deleted_count = 0;
        for chunk in dirs_to_delete.chunks(PARALLEL_DELETE_LIMIT) {
            let mut handles = Vec::with_capacity(chunk.len());

            for path in chunk {
                let path = path.clone();
                handles.push(tokio::spawn(async move {
                    let count = count_files_in_dir(&path).await;
                    match fs::remove_dir_all(&path).await {
                        Ok(()) => Some(count),
                        Err(e) => {
                            warn!("frame folder delete failure: {e}");
                            None
                        }
                    }
                }));
            }

            for handle in handles {
                if let Ok(Some(count)) = handle.await {
                    deleted_count += count;
                }
            }
        }

        if deleted_count > 0 {
            info!(
                "frame retention policy: deleted {deleted_count} files (>{} days)",
                self.retention_days
            );
        }

        // Re-scan to correct any drift from external deletions or TOCTOU races
        let frames_dir = self.base_dir.join("frames");
        let actual_size = if frames_dir.exists() {
            calculate_dir_size(&frames_dir).await.unwrap_or(0)
        } else {
            0
        };
        self.cached_size_bytes.store(actual_size, Ordering::Relaxed);

        Ok(deleted_count)
    }

    pub async fn total_size_mb(&self) -> Result<u64, StorageError> {
        if !self.cached_size_initialized.load(Ordering::Acquire) {
            let frames_dir = self.base_dir.join("frames");
            let size_bytes = if frames_dir.exists() {
                calculate_dir_size(&frames_dir).await?
            } else {
                0
            };
            self.cached_size_bytes.store(size_bytes, Ordering::Relaxed);
            self.cached_size_initialized.store(true, Ordering::Release);
        }

        Ok(self.cached_size_bytes.load(Ordering::Relaxed) / 1024 / 1024)
    }

    pub async fn enforce_storage_limit(&self) -> Result<usize, StorageError> {
        let frames_dir = self.base_dir.join("frames");

        if !frames_dir.exists() {
            return Ok(0);
        }

        // Use cached size when available, otherwise compute once
        let total_bytes = if self.cached_size_initialized.load(Ordering::Acquire) {
            self.cached_size_bytes.load(Ordering::Relaxed)
        } else {
            let size = calculate_dir_size(&frames_dir).await?;
            self.cached_size_bytes.store(size, Ordering::Relaxed);
            self.cached_size_initialized.store(true, Ordering::Release);
            size
        };
        let mut current_mb = total_bytes / 1024 / 1024;

        if current_mb <= self.max_storage_mb {
            return Ok(0);
        }

        let mut deleted_count = 0;
        let mut total_deleted_bytes: u64 = 0;

        let mut dirs = list_date_dirs(&frames_dir).await?;
        dirs.sort(); // YYYY-MM-DD ascending (oldest first)
        for dir_name in dirs {
            if current_mb <= self.max_storage_mb {
                break;
            }

            let dir_path = frames_dir.join(&dir_name);
            let dir_size_bytes = calculate_dir_size(&dir_path).await.unwrap_or(0);
            let count = count_files_in_dir(&dir_path).await;
            deleted_count += count;

            if let Err(e) = fs::remove_dir_all(&dir_path).await {
                warn!("frame folder delete failure: {e}");
            } else {
                let dir_size_mb = dir_size_bytes / 1024 / 1024;
                current_mb = current_mb.saturating_sub(dir_size_mb);
                total_deleted_bytes += dir_size_bytes;
                info!("frame folder delete: {} ({count} files)", dir_name);
            }
        }

        // Subtract deleted bytes from the cached size tracker using an atomic
        // fetch_update so that a concurrent save() cannot interleave between
        // the load() and the fetch_sub(), which would have caused the counter
        // to underflow (TOCTOU).
        if total_deleted_bytes > 0 {
            let _ = self.cached_size_bytes.fetch_update(
                Ordering::Relaxed,
                Ordering::Relaxed,
                |current| Some(current.saturating_sub(total_deleted_bytes)),
            );
        }

        Ok(deleted_count)
    }

    /// Delete all frame files for GDPR compliance.
    ///
    /// Removes every date-directory under `<base>/frames/`. This is best-effort:
    /// individual directory removal failures are logged as warnings but do not
    /// abort the overall operation, and the returned count reflects only the
    /// directories that were successfully removed.
    pub async fn delete_all_files(&self) -> Result<usize, StorageError> {
        // #4928: 프레임 배리어(배타 write) 획득 — 진행 중인 save_frame(read) 들이
        // 모두 끝난 뒤에야 삭제가 시작되고, 삭제 도중 새 save 가 끼어들 수 없다.
        // (erase 직전 deletion_flag 가 이미 set 되었으므로, 배리어 해제 후 진입하는
        // 쓰기는 funnel 에서 스킵된다 — 잔존 프레임이 남지 않는다.)
        let _barrier = self.frame_barrier.write().await;
        let frames_dir = self.base_dir.join("frames");
        if !frames_dir.exists() {
            return Ok(0);
        }

        let dirs = list_date_dirs(&frames_dir).await?;
        if dirs.is_empty() {
            return Ok(0);
        }

        let deleted = delete_date_dirs_chunked(&frames_dir, &dirs, PARALLEL_DELETE_LIMIT).await;

        if deleted > 0 {
            // Reset cached size to zero since all frames were deleted
            self.cached_size_bytes.store(0, Ordering::Relaxed);
            info!(
                "GDPR: deleted {deleted} frame files across {} directories",
                dirs.len()
            );
        }

        Ok(deleted)
    }

    /// Recalculates `cached_size_bytes` from the actual filesystem state.
    ///
    /// Walks all frame files under `<base_dir>/frames/` and sums their sizes, then
    /// atomically stores the result.  Call this after an unexpected crash or
    /// whenever the in-memory counter may have drifted (e.g. after a partial
    /// delete failure).
    pub fn reconcile_cache_size(&self) -> std::io::Result<u64> {
        fn dir_size(path: &std::path::Path) -> std::io::Result<u64> {
            let mut total: u64 = 0;
            if path.exists() {
                for entry in std::fs::read_dir(path)? {
                    let entry = entry?;
                    let metadata = entry.metadata()?;
                    if metadata.is_file() {
                        total += metadata.len();
                    } else if metadata.is_dir() {
                        total += dir_size(&entry.path())?;
                    }
                }
            }
            Ok(total)
        }

        let frames_dir = self.frames_dir();
        let total = dir_size(&frames_dir)?;
        self.cached_size_bytes.store(total, Ordering::Relaxed);
        Ok(total)
    }
}
