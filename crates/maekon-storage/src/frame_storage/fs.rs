use super::buffer::{BufferPool, BUFFER_POOL_SIZE, DEFAULT_BUFFER_SIZE};
use super::disk::DiskSpaceCache;
use crate::encryption::EncryptionKey;
use crate::error::StorageError;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::sync::Arc;
use tokio::fs;
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
    pub(super) frame_counter: AtomicU32,
    pub(super) buffer_pool: Arc<BufferPool>,
    pub(super) disk_cache: DiskSpaceCache,
    pub(super) encryption_key: Option<Arc<EncryptionKey>>,
    /// Approximate total size of all frame files, updated on save/delete.
    /// Avoids O(n) directory stat on every `total_size_mb()` call.
    /// Initialized lazily on the first call to `total_size_mb()`.
    pub(super) cached_size_bytes: AtomicU64,
    /// Whether `cached_size_bytes` has been initialized from a directory walk.
    pub(super) cached_size_initialized: std::sync::atomic::AtomicBool,
    /// #4928: consent-revoke erasure 차단 신호 (SQLite 와 동일한 공유 `Arc`).
    ///
    /// set 이면 `save_frame`/`save_frames_batch` 가 파일을 쓰지 않고 스킵한다
    /// (no-op `Ok`). composition root 에서 `ConsentManager::deletion_flag()` 를
    /// install 한다. non-erase 생성자(테스트/오프라인)는 기본 `false`.
    pub(super) deletion_flag: Arc<AtomicBool>,
    /// #4928 round-3 (FIX B): grant_consent-during-erase TOCTOU 차단 신호 (SQLite 와 동일 Arc).
    ///
    /// `deletion_flag` 는 재동의 시 `grant_consent` 가 `false` 로 되돌릴 수 있으나,
    /// `erasing` 은 `erase_all_local_data` 만 RAII 로 set/clear 한다. 프레임 쓰기 skip
    /// 술어는 `deletion_flag || erasing` 이며, erase 윈도우 안에 재동의가 끼어들어도
    /// erase 가 끝날 때까지 프레임 쓰기가 스킵된다. composition root 가 공유 Arc 를 install.
    pub(super) erasing: Arc<AtomicBool>,
    /// #4928: 프레임 쓰기 ↔ 전체삭제 직렬화 배리어 (all-async → tokio `RwLock`).
    ///
    /// 쓰기 경로는 `read().await`(공유), `delete_all_files` 는 `write().await`(배타)
    /// 를 취해, 삭제가 진행 중이면 쓰기가 대기하고 삭제가 끝나면 deletion_flag set
    /// 으로 인해 스킵된다. frame-local 인스턴스로 충분하다(SQLite mutex 와 별개).
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
            // #4928: 기본 false(쓰기 허용). composition root 가 공유 flag 를 install.
            deletion_flag: Arc::new(AtomicBool::new(false)),
            // #4928 round-3: 기본 false. composition root 가 공유 `erasing` 을 install.
            erasing: Arc::new(AtomicBool::new(false)),
            frame_barrier: Arc::new(RwLock::new(())),
        })
    }

    pub fn frames_dir(&self) -> PathBuf {
        self.base_dir.join("frames")
    }

    /// #4928: 공유 `deletion_flag` 를 install 한다(composition-root 배선용 seam).
    ///
    /// `SqliteStorage` 와 동일한 `Arc<AtomicBool>` (= `ConsentManager::deletion_flag()`)
    /// 를 받아, revoke 이후 프레임 쓰기가 funnel 에서 스킵되도록 한다.
    pub fn set_deletion_flag(&mut self, flag: Arc<AtomicBool>) {
        self.deletion_flag = flag;
    }

    /// #4928: 현재 install 된 `deletion_flag` 를 반환한다(ptr-eq 공유 검증/테스트용).
    pub fn deletion_flag(&self) -> Arc<AtomicBool> {
        self.deletion_flag.clone()
    }

    /// #4928 round-3 (FIX B): 공유 `erasing` 신호를 install 한다(composition-root 배선용 seam).
    ///
    /// `SqliteStorage` 와 동일한 `Arc<AtomicBool>` (= `ConsentManager::erasing()`)를 받아,
    /// erase 윈도우 안에 재동의가 끼어들어도 erase 가 끝날 때까지 프레임 쓰기가 스킵되게 한다.
    pub fn set_erasing(&mut self, erasing: Arc<AtomicBool>) {
        self.erasing = erasing;
    }

    /// #4928 round-3: 현재 install 된 `erasing` 신호를 반환한다(ptr-eq 검증/테스트용).
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
