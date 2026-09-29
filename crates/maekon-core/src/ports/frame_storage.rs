//! Port for persisting and managing captured frame images on disk.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use crate::ports::vision::{capture_denied, CaptureAuthority, CapturePermit};
use std::sync::Arc;

/// Port for persisting captured frame images to storage.
///
/// Implemented by `FrameFileStorage` in `maekon-storage`.
/// Consumers receive `Arc<dyn FrameStoragePort>` via DI.
///
/// Diagnostic methods (`frames_dir`, `buffer_pool_stats`, `disk_status`)
/// remain on the concrete type — they are infrastructure-level concerns
/// that do not belong in the port contract.
///
/// # Errors
/// - `CoreError::Storage` (wire: `storage.failed`) for SQLite
///   index/retention metadata operations (iter-47 mass fix pattern).
/// - `CoreError::AudioCapture` is NOT used — frame save uses
///   `CoreError::Io` (wire: `internal.io`) via `#[from]` for filesystem
///   write failures (ADR-019 §7).
/// - `save_frames_batch` returns per-frame Results; a single failure
///   does not abort the batch — callers inspect each item.
#[async_trait]
pub trait FrameStoragePort: Send + Sync {
    fn capture_authority(&self) -> Option<Arc<dyn CaptureAuthority>> {
        None
    }

    /// Check the original capture permit inside the storage barrier and again
    /// at the actual write. A caller-side precheck cannot authorize queued I/O.
    async fn save_frame_authorized(
        &self,
        timestamp: DateTime<Utc>,
        data: &[u8],
        permit: &CapturePermit,
    ) -> Result<PathBuf, CoreError> {
        let _ = (timestamp, data, permit);
        Err(capture_denied("authorized frame storage unavailable"))
    }

    /// Automation may consume only an image whose original in-process capture
    /// authority is still valid. History browsing retains its separate port.
    async fn load_latest_frame_authorized(
        &self,
        permit: &CapturePermit,
    ) -> Result<Option<(Vec<u8>, String)>, CoreError> {
        let _ = permit;
        Err(capture_denied("authorized latest frame unavailable"))
    }

    /// Save a single frame image. Returns the relative path of the saved file.
    async fn save_frame(&self, timestamp: DateTime<Utc>, data: &[u8])
        -> Result<PathBuf, CoreError>;

    /// Save multiple frames in a batch. Returns per-frame results.
    async fn save_frames_batch(
        &self,
        frames: Vec<(DateTime<Utc>, Vec<u8>)>,
    ) -> Vec<Result<PathBuf, CoreError>>;

    /// Load a single frame image by relative path.
    async fn load_frame(&self, relative_path: &Path) -> Result<Vec<u8>, CoreError>;

    /// Load the most recently captured frame image (read-only path used by the
    /// automation OCR element-finder). Returns the decoded frame bytes plus the
    /// image format string (e.g. `"webp"`), or `None` when no frame exists.
    ///
    /// A torn/corrupt newest frame is skipped in favour of the next-older good
    /// frame rather than surfacing an error, so a single bad write never blocks
    /// element-finding. Decryption (when the backing store is encrypted at rest)
    /// happens inside this call, so callers receive plaintext image bytes —
    /// this is why automation MUST share the SAME encrypted store instance as
    /// the capture writer instead of building a keyless one over the same dir.
    async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, CoreError>;

    /// Delete frames older than the configured retention period.
    /// Returns the number of deleted files.
    async fn enforce_retention(&self) -> Result<usize, CoreError>;

    /// Delete oldest frames to stay within storage size limits.
    /// Returns the number of deleted files.
    async fn enforce_storage_limit(&self) -> Result<usize, CoreError>;

    /// GDPR Art. 17 local data erasure: deletes all frame image files.
    ///
    /// Deletes every date directory under `<base>/frames/`.
    /// Returns the number of deleted files. Returns 0 when there is nothing
    /// to delete.
    ///
    /// # Errors
    /// - `CoreError::Storage` — returned when directory enumeration fails.
    ///   Failure to delete an individual date directory is best-effort (logged
    ///   and skipped, continuing), but the returned count only includes files
    ///   that were actually deleted.
    async fn delete_all_frames(&self) -> Result<usize, CoreError>;
}

#[cfg(test)]
mod capture_contract_tests {
    use super::*;
    use crate::error_codes::PermissionCode;
    use crate::ports::vision::{CaptureAccess, CaptureGuard};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Granted;

    impl CaptureGuard for Granted {
        fn check(&self, _: CaptureAccess) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct LegacyStorage {
        root: PathBuf,
        reads: AtomicUsize,
        writes: AtomicUsize,
    }

    #[async_trait]
    impl FrameStoragePort for LegacyStorage {
        async fn save_frame(&self, _: DateTime<Utc>, data: &[u8]) -> Result<PathBuf, CoreError> {
            let path = self.root.join("legacy-frame.bin");
            self.writes.fetch_add(1, Ordering::SeqCst);
            std::fs::write(&path, data)?;
            Ok(path)
        }

        async fn save_frames_batch(
            &self,
            frames: Vec<(DateTime<Utc>, Vec<u8>)>,
        ) -> Vec<Result<PathBuf, CoreError>> {
            let mut results = Vec::new();
            for (at, data) in frames {
                results.push(self.save_frame(at, &data).await);
            }
            results
        }

        async fn load_frame(&self, path: &Path) -> Result<Vec<u8>, CoreError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(std::fs::read(path)?)
        }

        async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, CoreError> {
            let path = self.root.join("legacy-frame.bin");
            if path.exists() {
                Ok(Some((self.load_frame(&path).await?, "webp".into())))
            } else {
                Ok(None)
            }
        }

        async fn enforce_retention(&self) -> Result<usize, CoreError> {
            Ok(0)
        }

        async fn enforce_storage_limit(&self) -> Result<usize, CoreError> {
            Ok(0)
        }

        async fn delete_all_frames(&self) -> Result<usize, CoreError> {
            Ok(0)
        }
    }

    fn assert_denied<T>(result: Result<T, CoreError>) {
        match result {
            Err(CoreError::PermissionDenied { code, .. }) => {
                assert_eq!(code, PermissionCode::PrivacyDenied);
            }
            Err(error) => panic!("expected privacy denial, got {error}"),
            Ok(_) => panic!("legacy storage claimed authorized success"),
        }
    }

    #[tokio::test]
    async fn legacy_storage_defaults_deny_before_reading_or_writing() {
        let dir = tempfile::tempdir().unwrap();
        let storage = LegacyStorage {
            root: dir.path().to_path_buf(),
            reads: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        };
        let data = b"synthetic legacy frame";
        let at = Utc::now();
        let path = storage.save_frame(at, data).await.unwrap();
        assert_eq!(storage.load_latest_frame().await.unwrap().unwrap().0, data);
        assert_eq!(storage.writes.swap(0, Ordering::SeqCst), 1);
        assert_eq!(storage.reads.swap(0, Ordering::SeqCst), 1);
        std::fs::remove_file(&path).unwrap();

        let permit = CapturePermit::new(Arc::new(Granted));
        assert!(storage.capture_authority().is_none());
        assert_denied(storage.save_frame_authorized(at, data, &permit).await);
        assert_denied(storage.load_latest_frame_authorized(&permit).await);
        assert_eq!(storage.writes.load(Ordering::SeqCst), 0);
        assert_eq!(storage.reads.load(Ordering::SeqCst), 0);
        assert!(!path.exists());
    }
}
