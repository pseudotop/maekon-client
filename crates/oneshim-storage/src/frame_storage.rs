use crate::encryption::EncryptionKey;
use crate::error::StorageError;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use crossbeam::queue::ArrayQueue;
use oneshim_core::error::CoreError;
use oneshim_core::ports::frame_storage::FrameStoragePort;
use parking_lot::Mutex as ParkingMutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::fs;
use tracing::{debug, error, info, warn};

const BUFFER_POOL_SIZE: usize = 16;

const DEFAULT_BUFFER_SIZE: usize = 256 * 1024;

const PARALLEL_DELETE_LIMIT: usize = 8;

const DISK_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const DISK_SPACE_WARN_MB: u64 = 100;
const DISK_SPACE_CRITICAL_MB: u64 = 50;

struct DiskSpaceCache {
    last_check: ParkingMutex<Option<Instant>>,
    cached_free_mb: AtomicU64,
}

impl DiskSpaceCache {
    fn new() -> Self {
        Self {
            last_check: ParkingMutex::new(None),
            cached_free_mb: AtomicU64::new(u64::MAX),
        }
    }

    fn get_free_mb(&self, path: &Path) -> u64 {
        let mut last = self.last_check.lock();
        let now = Instant::now();
        if last.is_some_and(|t| now.duration_since(t) < DISK_CHECK_INTERVAL) {
            return self.cached_free_mb.load(AtomicOrdering::Relaxed);
        }
        let free = query_disk_free_mb(path);
        self.cached_free_mb.store(free, AtomicOrdering::Relaxed);
        *last = Some(now);
        free
    }
}

#[allow(clippy::unnecessary_cast)] // statvfs field types vary by platform
fn query_disk_free_mb(path: &Path) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return u64::MAX;
        };
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } == 0 {
            (stat.f_bavail as u64 * stat.f_frsize) / (1024 * 1024)
        } else {
            u64::MAX
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free_bytes: u64 = 0;
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut free_bytes as *mut u64 as *mut _,
            )
        };
        if ok != 0 {
            free_bytes / (1024 * 1024)
        } else {
            u64::MAX
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        u64::MAX
    }
}

pub struct DiskStatus {
    pub free_mb: u64,
    pub healthy: bool,
}

struct BufferPool {
    pool: ArrayQueue<Vec<u8>>,
}

impl BufferPool {
    fn new(capacity: usize, buffer_size: usize) -> Self {
        let pool = ArrayQueue::new(capacity);
        for _ in 0..capacity {
            if let Err(e) = pool.push(Vec::with_capacity(buffer_size)) {
                debug!("push failed: {e:?}");
            }
        }
        Self { pool }
    }

    fn acquire(&self) -> Vec<u8> {
        self.pool
            .pop()
            .unwrap_or_else(|| Vec::with_capacity(DEFAULT_BUFFER_SIZE))
    }

    fn release(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        if let Err(e) = self.pool.push(buffer) {
            debug!("push failed: {e:?}");
        }
    }
}

pub struct FrameFileStorage {
    base_dir: PathBuf,
    max_storage_mb: u64,
    retention_days: u32,
    frame_counter: AtomicU32,
    buffer_pool: Arc<BufferPool>,
    disk_cache: DiskSpaceCache,
    encryption_key: Option<Arc<EncryptionKey>>,
    /// Approximate total size of all frame files, updated on save/delete.
    /// Avoids O(n) directory stat on every `total_size_mb()` call.
    /// Initialized lazily on the first call to `total_size_mb()`.
    cached_size_bytes: AtomicU64,
    /// Whether `cached_size_bytes` has been initialized from a directory walk.
    cached_size_initialized: std::sync::atomic::AtomicBool,
}

impl FrameFileStorage {
    /// Create a new frame file storage.
    ///
    /// When `encryption_key` is `Some`, frame files are encrypted at rest using
    /// AES-256-GCM before writing and decrypted after reading.
    pub async fn new(
        base_dir: PathBuf,
        max_storage_mb: u64,
        retention_days: u32,
    ) -> Result<Self, StorageError> {
        Self::with_encryption(base_dir, max_storage_mb, retention_days, None).await
    }

    /// Create a new frame file storage with optional encryption.
    pub async fn with_encryption(
        base_dir: PathBuf,
        max_storage_mb: u64,
        retention_days: u32,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Result<Self, StorageError> {
        let frames_dir = base_dir.join("frames");
        fs::create_dir_all(&frames_dir).await.map_err(|e| {
            StorageError::Internal(format!("Failed to create frame directory: {e}"))
        })?;

        let encrypted_label = if encryption_key.is_some() {
            "encrypted"
        } else {
            "plaintext"
        };
        info!(
            "frame storage initialized: {} (max={}MB, retention={} days, buffer_pool={}, {})",
            frames_dir.display(),
            max_storage_mb,
            retention_days,
            BUFFER_POOL_SIZE,
            encrypted_label
        );

        Ok(Self {
            base_dir,
            max_storage_mb,
            retention_days,
            frame_counter: AtomicU32::new(0),
            buffer_pool: Arc::new(BufferPool::new(BUFFER_POOL_SIZE, DEFAULT_BUFFER_SIZE)),
            disk_cache: DiskSpaceCache::new(),
            encryption_key,
            cached_size_bytes: AtomicU64::new(0),
            cached_size_initialized: std::sync::atomic::AtomicBool::new(false),
        })
    }

    /// Save a frame image to disk.
    ///
    /// Returns `CoreError::Storage` if free disk space is below the critical threshold (50 MB).
    /// Logs a warning if free space is below the warn threshold (100 MB).
    pub async fn save_frame(
        &self,
        timestamp: DateTime<Utc>,
        webp_data: &[u8],
    ) -> Result<PathBuf, StorageError> {
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

        // Update cached size tracker
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

    /// # Arguments
    ///
    /// # Returns
    pub async fn save_frames_batch(
        &self,
        frames: Vec<(DateTime<Utc>, Vec<u8>)>,
    ) -> Vec<Result<PathBuf, StorageError>> {
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

        // Update cached size tracker in a single atomic add
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

    /// # Returns
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
        // Lazy-init: walk the directory once on the first call
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
        let frames_dir = self.base_dir.join("frames");
        if !frames_dir.exists() {
            return Ok(0);
        }

        let dirs = list_date_dirs(&frames_dir).await?;
        if dirs.is_empty() {
            return Ok(0);
        }

        let mut deleted = 0usize;
        for chunk in dirs.chunks(PARALLEL_DELETE_LIMIT) {
            let mut handles = Vec::with_capacity(chunk.len());
            for dir_name in chunk {
                let dir_path = frames_dir.join(dir_name);
                handles.push(tokio::spawn(async move {
                    let count = count_files_in_dir(&dir_path).await;
                    match fs::remove_dir_all(&dir_path).await {
                        Ok(()) => Some(count),
                        Err(e) => {
                            warn!(
                                "GDPR frame file delete warning: {} — {}",
                                dir_path.display(),
                                e
                            );
                            None
                        }
                    }
                }));
            }
            for handle in handles {
                if let Ok(Some(count)) = handle.await {
                    deleted += count;
                }
            }
        }

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
    /// Walks all files directly under `<base_dir>/` and sums their sizes, then
    /// atomically stores the result.  Call this after an unexpected crash or
    /// whenever the in-memory counter may have drifted (e.g. after a partial
    /// delete failure).
    pub fn reconcile_cache_size(&self) -> std::io::Result<u64> {
        let mut total: u64 = 0;
        if self.base_dir.exists() {
            for entry in std::fs::read_dir(&self.base_dir)? {
                let entry = entry?;
                if entry.path().is_file() {
                    total += entry.metadata()?.len();
                }
            }
        }
        self.cached_size_bytes.store(total, Ordering::Relaxed);
        Ok(total)
    }

    pub fn frames_dir(&self) -> PathBuf {
        self.base_dir.join("frames")
    }

    pub fn buffer_pool_stats(&self) -> BufferPoolStats {
        BufferPoolStats {
            pool_capacity: BUFFER_POOL_SIZE,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }

    /// Query current disk health status for scheduler event emission.
    pub fn disk_status(&self) -> DiskStatus {
        let free_mb = self.disk_cache.get_free_mb(&self.base_dir);
        DiskStatus {
            free_mb,
            healthy: free_mb >= DISK_SPACE_CRITICAL_MB,
        }
    }
}

#[async_trait]
impl FrameStoragePort for FrameFileStorage {
    async fn save_frame(
        &self,
        timestamp: DateTime<Utc>,
        data: &[u8],
    ) -> Result<PathBuf, CoreError> {
        self.save_frame(timestamp, data).await.map_err(Into::into)
    }

    async fn save_frames_batch(
        &self,
        frames: Vec<(DateTime<Utc>, Vec<u8>)>,
    ) -> Vec<Result<PathBuf, CoreError>> {
        self.save_frames_batch(frames)
            .await
            .into_iter()
            .map(|r| r.map_err(Into::into))
            .collect()
    }

    async fn load_frame(&self, relative_path: &Path) -> Result<Vec<u8>, CoreError> {
        self.load_frame(relative_path).await.map_err(Into::into)
    }

    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.enforce_retention().await.map_err(Into::into)
    }

    async fn enforce_storage_limit(&self) -> Result<usize, CoreError> {
        self.enforce_storage_limit().await.map_err(Into::into)
    }
}

#[derive(Debug, Clone)]
pub struct BufferPoolStats {
    pub pool_capacity: usize,
    pub buffer_size: usize,
}

async fn count_files_in_dir(path: &Path) -> usize {
    let mut count = 0;
    if let Ok(mut entries) = fs::read_dir(path).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.path().is_file() {
                count += 1;
            }
        }
    }
    count
}

async fn calculate_dir_size(path: &Path) -> Result<u64, StorageError> {
    let mut total = 0u64;

    let mut entries = fs::read_dir(path)
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read directory: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read entry: {e}")))?
    {
        let path = entry.path();
        let metadata = fs::metadata(&path)
            .await
            .map_err(|e| StorageError::Internal(format!("Failed to read metadata: {e}")))?;

        if metadata.is_file() {
            total += metadata.len();
        } else if metadata.is_dir() {
            total += Box::pin(calculate_dir_size(&path)).await?;
        }
    }

    Ok(total)
}

async fn list_date_dirs(frames_dir: &Path) -> Result<Vec<String>, StorageError> {
    let mut dirs = Vec::with_capacity(365);

    if !frames_dir.exists() {
        return Ok(dirs);
    }

    let mut entries = fs::read_dir(frames_dir)
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read frames directory: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| StorageError::Internal(format!("Failed to read entry: {e}")))?
    {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.len() == 10 && name.chars().nth(4) == Some('-') {
                    dirs.push(name.to_string());
                }
            }
        }
    }

    Ok(dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_storage() -> (FrameFileStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let storage = FrameFileStorage::new(temp_dir.path().to_path_buf(), 100, 7)
            .await
            .unwrap();
        (storage, temp_dir)
    }

    #[tokio::test]
    async fn save_and_load_frame() {
        let (storage, _temp) = create_test_storage().await;

        let test_data = b"RIFF\x00\x00\x00\x00WEBPVP8 test data";
        let timestamp = Utc::now();

        let path = storage.save_frame(timestamp, test_data).await.unwrap();
        assert!(path.to_string_lossy().contains("frames/"));
        assert!(path.to_string_lossy().ends_with(".webp"));

        let loaded = storage.load_frame(&path).await.unwrap();
        assert_eq!(loaded, test_data);
    }

    #[tokio::test]
    async fn load_latest_frame_returns_most_recent_file() {
        let (storage, _temp) = create_test_storage().await;

        let t1 = Utc::now() - chrono::Duration::seconds(1);
        let t2 = Utc::now();

        storage.save_frame(t1, b"older-frame").await.unwrap();
        storage.save_frame(t2, b"newer-frame").await.unwrap();

        let latest = storage.load_latest_frame().await.unwrap().unwrap();
        assert_eq!(latest.0, b"newer-frame");
        assert_eq!(latest.1, "webp");
    }

    #[tokio::test]
    async fn save_multiple_same_second() {
        let (storage, _temp) = create_test_storage().await;

        let timestamp = Utc::now();
        let data = b"test";

        let path1 = storage.save_frame(timestamp, data).await.unwrap();
        let path2 = storage.save_frame(timestamp, data).await.unwrap();
        let path3 = storage.save_frame(timestamp, data).await.unwrap();

        assert_ne!(path1, path2);
        assert_ne!(path2, path3);
    }

    #[tokio::test]
    async fn save_frames_batch_parallel() {
        let (storage, _temp) = create_test_storage().await;

        let now = Utc::now();
        let frames: Vec<_> = (0..10)
            .map(|i| (now, format!("frame data {i}").into_bytes()))
            .collect();

        let results = storage.save_frames_batch(frames).await;

        assert_eq!(results.len(), 10);
        for result in results {
            assert!(result.is_ok());
        }
    }

    #[tokio::test]
    async fn load_frames_batch_parallel() {
        let (storage, _temp) = create_test_storage().await;

        let now = Utc::now();
        let frames: Vec<_> = (0..5)
            .map(|i| (now, format!("batch frame {i}").into_bytes()))
            .collect();

        let save_results = storage.save_frames_batch(frames.clone()).await;
        let paths: Vec<_> = save_results.into_iter().filter_map(|r| r.ok()).collect();

        let load_results = storage.load_frames_batch(paths).await;

        assert_eq!(load_results.len(), 5);
        for (i, result) in load_results.into_iter().enumerate() {
            let data = result.unwrap();
            assert_eq!(data, format!("batch frame {i}").into_bytes());
        }
    }

    #[tokio::test]
    async fn load_nonexistent_frame() {
        let (storage, _temp) = create_test_storage().await;

        let result = storage
            .load_frame(Path::new("frames/2099-01-01/00-00-00-000.webp"))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn total_size_empty() {
        let (storage, _temp) = create_test_storage().await;

        let size = storage.total_size_mb().await.unwrap();
        assert_eq!(size, 0);
    }

    #[tokio::test]
    async fn total_size_with_files() {
        let (storage, _temp) = create_test_storage().await;

        let data = vec![0u8; 100 * 1024];
        for _ in 0..10 {
            storage.save_frame(Utc::now(), &data).await.unwrap();
        }

        let size = storage.total_size_mb().await.unwrap();
        assert!(size <= 2);
    }

    #[tokio::test]
    async fn retention_empty() {
        let (storage, _temp) = create_test_storage().await;

        let deleted = storage.enforce_retention().await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn frames_dir_path() {
        let (storage, temp) = create_test_storage().await;

        assert_eq!(storage.frames_dir(), temp.path().join("frames"));
    }

    #[tokio::test]
    async fn buffer_pool_stats() {
        let (storage, _temp) = create_test_storage().await;

        let stats = storage.buffer_pool_stats();
        assert_eq!(stats.pool_capacity, BUFFER_POOL_SIZE);
        assert_eq!(stats.buffer_size, DEFAULT_BUFFER_SIZE);
    }

    #[test]
    fn buffer_pool_acquire_release() {
        let pool = BufferPool::new(4, 1024);

        let buf1 = pool.acquire();
        let buf2 = pool.acquire();
        assert!(buf1.capacity() >= 1024);
        assert!(buf2.capacity() >= 1024);

        pool.release(buf1);
        pool.release(buf2);

        let buf3 = pool.acquire();
        assert!(buf3.capacity() >= 1024);
    }

    #[tokio::test]
    async fn delete_all_files_empty_storage() {
        let (storage, _temp) = create_test_storage().await;

        let deleted = storage.delete_all_files().await.unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn delete_all_files_removes_frames() {
        let (storage, _temp) = create_test_storage().await;

        // Save several frames across multiple dates
        let now = Utc::now();
        let yesterday = now - chrono::Duration::days(1);
        storage.save_frame(now, b"frame-today-1").await.unwrap();
        storage.save_frame(now, b"frame-today-2").await.unwrap();
        storage
            .save_frame(yesterday, b"frame-yesterday")
            .await
            .unwrap();

        // Verify frames directory has date-dirs before deletion
        let frames_dir = storage.frames_dir();
        let dirs_before = list_date_dirs(&frames_dir).await.unwrap();
        assert!(!dirs_before.is_empty());

        let deleted = storage.delete_all_files().await.unwrap();
        assert_eq!(deleted, 3);

        // Verify frames directory is now empty (no date dirs left)
        let remaining = list_date_dirs(&frames_dir).await.unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn disk_space_cache_returns_max_for_nonexistent_path() {
        let cache = DiskSpaceCache::new();
        let free = cache.get_free_mb(Path::new("/nonexistent/path/that/does/not/exist"));
        // statvfs fails on non-existent path → returns u64::MAX
        assert_eq!(free, u64::MAX);
    }

    #[test]
    fn disk_space_cache_returns_real_value_for_temp_dir() {
        let cache = DiskSpaceCache::new();
        let free = cache.get_free_mb(&std::env::temp_dir());
        // Should return actual disk space, not u64::MAX
        assert!(free < u64::MAX);
        assert!(free > 0);
    }

    #[test]
    fn disk_space_cache_caches_within_interval() {
        let cache = DiskSpaceCache::new();
        let path = std::env::temp_dir();
        let first = cache.get_free_mb(&path);
        let second = cache.get_free_mb(&path);
        // Both should return the same cached value
        assert_eq!(first, second);
    }

    // ── Encrypted frame storage tests ──────────────────────────────

    async fn create_encrypted_test_storage() -> (FrameFileStorage, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let key = Arc::new(EncryptionKey::from_bytes([0x42; 32]));
        let storage =
            FrameFileStorage::with_encryption(temp_dir.path().to_path_buf(), 100, 7, Some(key))
                .await
                .unwrap();
        (storage, temp_dir)
    }

    #[tokio::test]
    async fn encrypted_save_and_load_frame() {
        let (storage, _temp) = create_encrypted_test_storage().await;

        let test_data = b"RIFF\x00\x00\x00\x00WEBPVP8 test data";
        let timestamp = Utc::now();

        let path = storage.save_frame(timestamp, test_data).await.unwrap();
        let loaded = storage.load_frame(&path).await.unwrap();
        assert_eq!(loaded, test_data);
    }

    #[tokio::test]
    async fn encrypted_data_differs_from_plaintext() {
        let (storage, temp) = create_encrypted_test_storage().await;

        let test_data = b"sensitive frame content here";
        let timestamp = Utc::now();

        let rel_path = storage.save_frame(timestamp, test_data).await.unwrap();

        // Read the raw bytes from disk (without decryption)
        let full_path = temp.path().join(&rel_path);
        let raw_on_disk = tokio::fs::read(&full_path).await.unwrap();

        // On-disk data must NOT equal the plaintext (it's encrypted)
        assert_ne!(raw_on_disk, test_data);
        // On-disk data should be larger: 12 nonce + data + 16 auth tag
        assert!(raw_on_disk.len() > test_data.len());
    }

    #[tokio::test]
    async fn encrypted_batch_save_and_load() {
        let (storage, _temp) = create_encrypted_test_storage().await;

        let now = Utc::now();
        let frames: Vec<_> = (0..5)
            .map(|i| (now, format!("encrypted batch frame {i}").into_bytes()))
            .collect();

        let save_results = storage.save_frames_batch(frames.clone()).await;
        let paths: Vec<_> = save_results.into_iter().filter_map(|r| r.ok()).collect();
        assert_eq!(paths.len(), 5);

        let load_results = storage.load_frames_batch(paths).await;
        for (i, result) in load_results.into_iter().enumerate() {
            let data = result.unwrap();
            assert_eq!(data, format!("encrypted batch frame {i}").into_bytes());
        }
    }

    #[tokio::test]
    async fn encrypted_load_latest_frame() {
        let (storage, _temp) = create_encrypted_test_storage().await;

        let t1 = Utc::now() - chrono::Duration::seconds(1);
        let t2 = Utc::now();

        storage.save_frame(t1, b"older-encrypted").await.unwrap();
        storage.save_frame(t2, b"newer-encrypted").await.unwrap();

        let latest = storage.load_latest_frame().await.unwrap().unwrap();
        assert_eq!(latest.0, b"newer-encrypted");
    }

    #[tokio::test]
    async fn wrong_key_cannot_decrypt_frame() {
        let temp_dir = TempDir::new().unwrap();
        let key1 = Arc::new(EncryptionKey::from_bytes([0x42; 32]));
        let key2 = Arc::new(EncryptionKey::from_bytes([0x43; 32]));

        let storage1 =
            FrameFileStorage::with_encryption(temp_dir.path().to_path_buf(), 100, 7, Some(key1))
                .await
                .unwrap();

        let test_data = b"secret frame data";
        let rel_path = storage1.save_frame(Utc::now(), test_data).await.unwrap();

        // Create a second storage with a different key
        let storage2 =
            FrameFileStorage::with_encryption(temp_dir.path().to_path_buf(), 100, 7, Some(key2))
                .await
                .unwrap();

        // Decryption with wrong key should fail
        let result = storage2.load_frame(&rel_path).await;
        assert!(result.is_err());
    }
}
