use super::buffer::{BufferPool, BUFFER_POOL_SIZE, DEFAULT_BUFFER_SIZE};
use super::disk::DiskSpaceCache;
use crate::encryption::EncryptionKey;
use crate::error::StorageError;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

/// File-system frame storage.
///
/// Frames are written to `<base_dir>/frames/<YYYY-MM-DD>/` as WebP files.
/// When `encryption_key` is provided, each file is encrypted at rest with
/// AES-256-GCM (12-byte nonce prepended, 16-byte auth tag appended).
pub struct FrameFileStorage {
    pub(super) base_dir: PathBuf,
    pub(super) max_storage_mb: u64,
    pub(super) retention_days: u32,
    /// Monotonic frame sequence counter, shared (`Arc`) so the spawned batch
    /// tasks can re-fetch a fresh value when retrying past a filename collision.
    pub(super) frame_counter: Arc<AtomicU32>,
    pub(super) buffer_pool: Arc<BufferPool>,
    pub(super) disk_cache: DiskSpaceCache,
    pub(super) encryption_key: Option<Arc<EncryptionKey>>,
    /// Approximate total size of all frame files, updated on save/delete.
    /// Avoids O(n) directory stat on every `total_size_mb()` call.
    /// Initialized lazily on the first call to `total_size_mb()`.
    pub(super) cached_size_bytes: AtomicU64,
    /// Whether `cached_size_bytes` has been initialized from a directory walk.
    pub(super) cached_size_initialized: std::sync::atomic::AtomicBool,
    /// #4928: consent-revoke erasure block signal (the same shared `Arc` as SQLite).
    ///
    /// When set, `save_frame`/`save_frames_batch` skip the write rather than
    /// writing a file (no-op `Ok`). The composition root installs
    /// `ConsentManager::deletion_flag()` here. Non-erase constructors
    /// (tests/offline) default to `false`.
    pub(super) deletion_flag: Arc<AtomicBool>,
    /// #4928 round-3 (FIX B): grant_consent-during-erase TOCTOU block signal (the same `Arc` as SQLite).
    ///
    /// `deletion_flag` can be flipped back to `false` by `grant_consent` on
    /// re-consent, but `erasing` is set/cleared via RAII only by
    /// `erase_all_local_data`. The frame-write skip predicate is
    /// `deletion_flag || erasing`, so even if a re-consent slips into the erase
    /// window, frame writes stay skipped until the erase completes. The
    /// composition root installs the shared `Arc`.
    pub(super) erasing: Arc<AtomicBool>,
    /// #4928: frame-write ↔ full-delete serialization barrier (all-async → tokio `RwLock`).
    ///
    /// The write path takes `read().await` (shared) and `delete_all_files` takes
    /// `write().await` (exclusive), so a write waits while a delete is in
    /// progress and, once the delete finishes, is skipped because `deletion_flag`
    /// is set. A frame-local instance is sufficient (independent of the SQLite mutex).
    pub(super) frame_barrier: Arc<RwLock<()>>,
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
        // #7074 (MS-001): create the frames root owner-only (Unix 0o700 / Windows
        // owner-only DACL) so the screen-capture tree is not world-traversable.
        super::io::create_dir_owner_only(&frames_dir).await?;

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
            frame_counter: Arc::new(AtomicU32::new(0)),
            buffer_pool: Arc::new(BufferPool::new(BUFFER_POOL_SIZE, DEFAULT_BUFFER_SIZE)),
            disk_cache: DiskSpaceCache::new(),
            encryption_key,
            cached_size_bytes: AtomicU64::new(0),
            cached_size_initialized: std::sync::atomic::AtomicBool::new(false),
            // #4928: defaults to false (writes allowed). The composition root installs the shared flag.
            deletion_flag: Arc::new(AtomicBool::new(false)),
            // #4928 round-3: defaults to false. The composition root installs the shared `erasing`.
            erasing: Arc::new(AtomicBool::new(false)),
            frame_barrier: Arc::new(RwLock::new(())),
        })
    }

    pub fn frames_dir(&self) -> PathBuf {
        self.base_dir.join("frames")
    }

    /// #4928: install the shared `deletion_flag` (composition-root wiring seam).
    ///
    /// Accepts the same `Arc<AtomicBool>` as `SqliteStorage`
    /// (= `ConsentManager::deletion_flag()`) so that, after a revoke, frame
    /// writes are skipped at the funnel.
    pub fn set_deletion_flag(&mut self, flag: Arc<AtomicBool>) {
        self.deletion_flag = flag;
    }

    /// #4928: return the currently installed `deletion_flag` (for ptr-eq sharing checks/tests).
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.deletion_flag.clone()
    }

    /// #4928 round-3 (FIX B): install the shared `erasing` signal (composition-root wiring seam).
    ///
    /// Accepts the same `Arc<AtomicBool>` as `SqliteStorage`
    /// (= `ConsentManager::erasing()`) so that, even if a re-consent slips into
    /// the erase window, frame writes stay skipped until the erase completes.
    pub fn set_erasing(&mut self, erasing: Arc<AtomicBool>) {
        self.erasing = erasing;
    }

    /// #4928 round-3: return the currently installed `erasing` signal (for ptr-eq checks/tests).
    pub fn erasing(&self) -> Arc<AtomicBool> {
        self.erasing.clone()
    }

    pub fn buffer_pool_stats(&self) -> super::BufferPoolStats {
        super::BufferPoolStats {
            pool_capacity: BUFFER_POOL_SIZE,
            buffer_size: DEFAULT_BUFFER_SIZE,
        }
    }

    /// Query current disk health status for scheduler event emission.
    pub fn disk_status(&self) -> super::disk::DiskStatus {
        let free_mb = self.disk_cache.get_free_mb(&self.base_dir);
        super::disk::DiskStatus {
            free_mb,
            healthy: free_mb >= super::disk::DISK_SPACE_CRITICAL_MB,
        }
    }
}
