use super::disk::{DISK_SPACE_CRITICAL_MB, DISK_SPACE_WARN_MB};
use super::fs::FrameFileStorage;
use super::util::list_date_dirs;
use crate::error::StorageError;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::sync::{atomic::Ordering, Arc};
use tokio::fs;
use tracing::{debug, error, warn};

impl FrameFileStorage {
    /// Save a frame image to disk.
    ///
    /// Returns `StorageError` if free disk space is below the critical threshold (50 MB).
    /// Logs a warning if free space is below the warn threshold (100 MB).
    pub async fn save_frame(
        &self,
        timestamp: DateTime<Utc>,
        webp_data: &[u8],
    ) -> Result<PathBuf, StorageError> {
        // #4928: 프레임 배리어(공유 read) 획득 — delete_all_files 의 write 와 직렬화.
        // 삭제가 진행 중이면 여기서 대기하고, 삭제 후엔 deletion_flag set 으로 스킵된다.
        let _barrier = self.frame_barrier.read().await;
        // #4928: erasure 차단 신호(`deletion_flag || erasing`)가 set 이면 파일을 쓰지
        // 않고 no-op 으로 스킵한다(반환 경로는 빈 PathBuf). #4928 round-3(FIX B):
        // `erasing` 은 erase 윈도우 안의 재동의 race 를 차단한다(grant_consent 가 clear 불가).
        if self.deletion_flag.load(Ordering::Acquire) || self.erasing.load(Ordering::Acquire) {
            debug!("frame save skipped — deletion_flag/erasing set (consent revoked, #4928)");
            return Ok(PathBuf::new());
        }
        let free_mb = self.disk_cache.get_free_mb(&self.base_dir);
        if free_mb < DISK_SPACE_CRITICAL_MB {
            error!(free_mb, "disk space critical — skipping frame save");
            return Err(StorageError::Internal("disk space critical".into()));
        }
        if free_mb < DISK_SPACE_WARN_MB {
            warn!(
                free_mb,
                "disk space low — frame save proceeding with caution"
            );
        }

        let date_str = timestamp.format("%Y-%m-%d").to_string();
        let day_dir = self.base_dir.join("frames").join(&date_str);
        fs::create_dir_all(&day_dir)
            .await
            .map_err(|e| StorageError::Internal(format!("Failed to create dated folder: {e}")))?;

        let counter = self.frame_counter.fetch_add(1, Ordering::SeqCst) % 1000;
        let time_str = timestamp.format("%H-%M-%S").to_string();
        let filename = format!("{time_str}-{counter:03}.webp");
        let file_path = day_dir.join(&filename);

        let data_to_write = if let Some(ref key) = self.encryption_key {
            key.encrypt(webp_data)?
        } else {
            webp_data.to_vec()
        };

        let written_len = data_to_write.len() as u64;
        fs::write(&file_path, &data_to_write)
            .await
            .map_err(|e| StorageError::Internal(format!("frame file save failure: {e}")))?;

        self.cached_size_bytes
            .fetch_add(written_len, Ordering::Relaxed);

        let relative_path = PathBuf::from("frames").join(&date_str).join(&filename);

        debug!(
            "frame save: {} ({}bytes raw, {}bytes on disk)",
            relative_path.display(),
            webp_data.len(),
            written_len
        );

        Ok(relative_path)
    }

    pub async fn save_frames_batch(
        &self,
        frames: Vec<(DateTime<Utc>, Vec<u8>)>,
    ) -> Vec<Result<PathBuf, StorageError>> {
        // #4928: 프레임 배리어(공유 read) 획득 — delete_all_files 와 직렬화.
        let _barrier = self.frame_barrier.read().await;
        // #4928: erasure 차단 신호(`deletion_flag || erasing`)가 set 이면 어떤 파일도
        // 쓰지 않고 빈 경로로 스킵한다(#4928 round-3 FIX B — erasing 은 재동의 race 차단).
        if self.deletion_flag.load(Ordering::Acquire) || self.erasing.load(Ordering::Acquire) {
            debug!(
                batch_size = frames.len(),
                "frame batch save skipped — deletion_flag/erasing set (consent revoked, #4928)"
            );
            return frames.iter().map(|_| Ok(PathBuf::new())).collect();
        }
        let free_mb = self.disk_cache.get_free_mb(&self.base_dir);
        if free_mb < DISK_SPACE_CRITICAL_MB {
            error!(
                free_mb,
                batch_size = frames.len(),
                "disk space critical — skipping batch save"
            );
            return frames
                .iter()
                .map(|_| Err(StorageError::Internal("disk space critical".into())))
                .collect();
        }

        let mut handles = Vec::with_capacity(frames.len());

        for (timestamp, webp_data) in frames {
            let base_dir = self.base_dir.clone();
            let counter = self.frame_counter.fetch_add(1, Ordering::SeqCst) % 1000;
            let enc_key = self.encryption_key.clone();

            handles.push(tokio::spawn(async move {
                let date_str = timestamp.format("%Y-%m-%d").to_string();
                let day_dir = base_dir.join("frames").join(&date_str);

                fs::create_dir_all(&day_dir).await.map_err(|e| {
                    StorageError::Internal(format!("Failed to create dated folder: {e}"))
                })?;

                let time_str = timestamp.format("%H-%M-%S").to_string();
                let filename = format!("{time_str}-{counter:03}.webp");
                let file_path = day_dir.join(&filename);

                let data_to_write = if let Some(ref key) = enc_key {
                    key.encrypt(&webp_data)?
                } else {
                    webp_data
                };

                let written_len = data_to_write.len() as u64;
                fs::write(&file_path, &data_to_write)
                    .await
                    .map_err(|e| StorageError::Internal(format!("frame file save failure: {e}")))?;

                let relative_path = PathBuf::from("frames").join(&date_str).join(&filename);

                Ok((relative_path, written_len))
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        let mut total_written: u64 = 0;
        for handle in handles {
            match handle.await {
                Ok(Ok((path, size))) => {
                    total_written += size;
                    results.push(Ok(path));
                }
                Ok(Err(e)) => results.push(Err(e)),
                Err(e) => results.push(Err(StorageError::Internal(format!("Task failed: {e}")))),
            }
        }

        if total_written > 0 {
            self.cached_size_bytes
                .fetch_add(total_written, Ordering::Relaxed);
        }

        results
    }

    /// Load a frame from disk, decrypting if encryption is enabled.
    pub async fn load_frame(&self, relative_path: &Path) -> Result<Vec<u8>, StorageError> {
        let full_path = self.base_dir.join(relative_path);

        if !full_path.exists() {
            return Err(StorageError::NotFound {
                resource_type: "Frame".to_string(),
                id: relative_path.display().to_string(),
            });
        }

        let mut buffer = self.buffer_pool.acquire();

        let raw = fs::read(&full_path)
            .await
            .map_err(|e| StorageError::Internal(format!("frame file read failure: {e}")))?;

        let data = if let Some(ref key) = self.encryption_key {
            key.decrypt(&raw)?
        } else {
            raw
        };

        buffer.extend_from_slice(&data);
        let result = buffer.clone();

        self.buffer_pool.release(buffer);

        Ok(result)
    }

    pub async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, StorageError> {
        let frames_dir = self.base_dir.join("frames");
        if !frames_dir.exists() {
            return Ok(None);
        }

        let mut day_dirs = list_date_dirs(&frames_dir).await?;
        day_dirs.sort_by(|a, b| b.cmp(a));

        for day in day_dirs {
            let day_path = frames_dir.join(&day);
            if !day_path.exists() {
                continue;
            }

            let mut files = Vec::new();
            let mut entries = fs::read_dir(&day_path)
                .await
                .map_err(|e| StorageError::Internal(format!("frame folder read failure: {e}")))?;

            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| StorageError::Internal(format!("Failed to read frame entry: {e}")))?
            {
                let path = entry.path();
                if path.is_file() {
                    files.push(path);
                }
            }

            if files.is_empty() {
                continue;
            }

            files.sort_by(|a, b| {
                let a_name = a.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                let b_name = b.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                b_name.cmp(a_name)
            });

            let latest = &files[0];
            let Some(filename) = latest.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let relative_path = PathBuf::from("frames").join(&day).join(filename);
            let bytes = self.load_frame(&relative_path).await?;
            let format = latest
                .extension()
                .and_then(|e| e.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_else(|| "webp".to_string());
            return Ok(Some((bytes, format)));
        }

        Ok(None)
    }

    pub async fn load_frames_batch(
        &self,
        paths: Vec<PathBuf>,
    ) -> Vec<Result<Vec<u8>, StorageError>> {
        let mut handles = Vec::with_capacity(paths.len());

        for path in paths {
            let base_dir = self.base_dir.clone();
            let buffer_pool = Arc::clone(&self.buffer_pool);
            let enc_key = self.encryption_key.clone();

            handles.push(tokio::spawn(async move {
                let full_path = base_dir.join(&path);

                if !full_path.exists() {
                    return Err(StorageError::NotFound {
                        resource_type: "Frame".to_string(),
                        id: path.display().to_string(),
                    });
                }

                let mut buffer = buffer_pool.acquire();

                let raw = fs::read(&full_path)
                    .await
                    .map_err(|e| StorageError::Internal(format!("frame file read failure: {e}")))?;

                let data = if let Some(ref key) = enc_key {
                    key.decrypt(&raw)?
                } else {
                    raw
                };

                buffer.extend_from_slice(&data);
                let result = buffer.clone();

                buffer_pool.release(buffer);

                Ok(result)
            }));
        }

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(StorageError::Internal(format!("Task failed: {e}")))),
            }
        }

        results
    }
}
