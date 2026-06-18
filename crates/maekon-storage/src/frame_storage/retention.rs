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

        // Classify date directories with the shared `list_date_dirs` recognizer so
        // all three retention paths (retention, storage-limit, GDPR delete-all) treat
        // the same set of entries as date dirs. The previous inline `len() == 10`
        // check was weaker than the helper: it never confirmed the entry was a
        // directory and skipped the `YYYY-MM-DD` hyphen-at-index-4 shape check, so a
        // 10-char file (or 10-char non-date dir) could be misclassified here while
        // being ignored by the other two paths.
        let dirs_to_delete: Vec<_> = list_date_dirs(&frames_dir)
            .await?
            .into_iter()
            .filter(|dir_name| dir_name.as_str() < cutoff_date.as_str())
            .map(|dir_name| frames_dir.join(dir_name))
            .collect();

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
        // Track the eviction budget in BYTES, not truncated MB (#6245). Comparing
        // `total_bytes / 1024 / 1024` against the limit and subtracting a per-dir
        // `dir_size_bytes / 1024 / 1024` truncates each directory's contribution
        // downward (e.g. a 1.9 MB dir counts as 1 MB), so the running counter shrinks
        // slower than the real on-disk size and the loop over-deletes past the limit.
        // Comparing exact bytes against `max_storage_mb * 1024 * 1024` evicts the
        // minimal number of oldest directories needed to get under budget.
        let limit_bytes = self
            .max_storage_mb
            .saturating_mul(1024)
            .saturating_mul(1024);
        let mut current_bytes = total_bytes;

        if current_bytes <= limit_bytes {
            return Ok(0);
        }

        let mut deleted_count = 0;
        let mut total_deleted_bytes: u64 = 0;

        let mut dirs = list_date_dirs(&frames_dir).await?;
        dirs.sort(); // YYYY-MM-DD ascending (oldest first)
        for dir_name in dirs {
            if current_bytes <= limit_bytes {
                break;
            }

            let dir_path = frames_dir.join(&dir_name);
            let dir_size_bytes = calculate_dir_size(&dir_path).await.unwrap_or(0);
            let count = count_files_in_dir(&dir_path).await;
            deleted_count += count;

            if let Err(e) = fs::remove_dir_all(&dir_path).await {
                warn!("frame folder delete failure: {e}");
            } else {
                current_bytes = current_bytes.saturating_sub(dir_size_bytes);
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
        // #4928: acquire the frame barrier (exclusive write). Deletion only begins
        // after all in-flight save_frame (read) holders have finished, and no new
        // save can slip in while the deletion is running. (deletion_flag is already
        // set just before erase, so any write that enters after the barrier is
        // released is skipped at the funnel -- no leftover frames remain.)
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
