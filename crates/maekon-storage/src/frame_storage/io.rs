use super::disk::{DISK_SPACE_CRITICAL_MB, DISK_SPACE_WARN_MB};
use super::fs::FrameFileStorage;
use super::util::list_date_dirs;
use crate::error::StorageError;
use chrono::{DateTime, Utc};
use maekon_core::ports::vision::{CaptureAccess, CapturePermit};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, warn};

/// Maximum collision retries before giving up on a single frame write.
///
/// Each retry draws a fresh monotonic counter, so exhausting this bound means
/// the directory is genuinely saturated rather than racing — extremely unlikely
/// given an `AtomicU32` namespace.
const FRAME_WRITE_MAX_RETRIES: u32 = 16;

/// Write `data` to an already-created `file` at `file_path`, flushing on success.
///
/// On a `write_all`/`flush` failure the partially-written (torn) file is removed
/// best-effort before the error is returned, so no torn frame is left behind
/// under the just-claimed (highest) counter for `load_latest_frame` to trip over
/// (#6244). The removal error is intentionally ignored — the original write
/// failure is the meaningful one to surface.
pub(super) async fn write_all_or_cleanup(
    mut file: fs::File,
    file_path: &Path,
    data: &[u8],
) -> Result<(), StorageError> {
    if let Err(e) = file.write_all(data).await {
        let _ = fs::remove_file(file_path).await;
        return Err(StorageError::Internal(format!(
            "frame file save failure: {e}"
        )));
    }
    if let Err(e) = file.flush().await {
        let _ = fs::remove_file(file_path).await;
        return Err(StorageError::Internal(format!(
            "frame file save failure: {e}"
        )));
    }
    Ok(())
}

/// Create `dir` (and any missing parents) restricted to the owner.
///
/// #7074 (MS-001): frame directories hold screen captures (the most sensitive
/// data in the product), so they are created owner-only rather than inheriting
/// the umask (typically 0o755 = world-traversable):
/// - **Unix**: every created component gets mode `0o700` via `DirBuilder`, so
///   there is no world-traversable window.
/// - **Windows**: the tree is created, then an owner-only DACL is applied to the
///   leaf directory (best-effort defense-in-depth; a failure is logged and
///   directory creation still succeeds).
pub(super) async fn create_dir_owner_only(dir: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder.create(dir).await.map_err(|e| {
            StorageError::Internal(format!("Failed to create frame directory: {e}"))
        })?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir).await.map_err(|e| {
            StorageError::Internal(format!("Failed to create frame directory: {e}"))
        })?;
    }
    #[cfg(windows)]
    {
        if let Err(e) = crate::encryption::set_owner_only_directory_dacl(dir) {
            warn!(
                dir = %dir.display(),
                "frame directory: failed to set owner-only DACL: {e}"
            );
        }
    }
    Ok(())
}

/// Atomically write `data` to a uniquely-named frame file in `day_dir`.
///
/// The filename is `<time_str>-<counter:010>.webp`, where `counter` is drawn
/// from the shared monotonic `frame_counter`. The full (un-wrapped) counter
/// value is used so >1000 frames sharing a one-second timestamp never reuse a
/// suffix. The fixed 10-digit zero-padding keeps the lexicographic filename
/// ordering aligned with counter order (relied on by `load_latest_frame`).
///
/// As defense-in-depth against a counter that restarts at 0 on process restart,
/// the file is opened with `create_new(true)`; on `AlreadyExists` a fresh
/// counter is drawn and the write retried, so an existing frame is never
/// silently clobbered. Returns the chosen filename on success.
async fn write_frame_atomic(
    frame_counter: &AtomicU32,
    day_dir: &Path,
    time_str: &str,
    data: &[u8],
) -> Result<String, StorageError> {
    for _ in 0..FRAME_WRITE_MAX_RETRIES {
        let counter = frame_counter.fetch_add(1, Ordering::SeqCst);
        let filename = format!("{time_str}-{counter:010}.webp");
        let file_path = day_dir.join(&filename);

        // #7074 (MS-001): create frame files owner-only, mirroring the secret
        // stores — screen captures are the most sensitive data in the product, so
        // they must not inherit the umask default (typically 0o644 = world-
        // readable). On Unix mode 0o600 is applied atomically in the create_new
        // open; on Windows the owner-only DACL is applied below while the file is
        // still empty.
        let mut open_opts = fs::OpenOptions::new();
        open_opts.write(true).create_new(true);
        #[cfg(unix)]
        open_opts.mode(0o600);

        match open_opts.open(&file_path).await {
            Ok(file) => {
                // #7074 (MS-001): apply the owner-only DACL while the just-created
                // frame file is still empty (before any bytes are written). Frame
                // content is AES-256-GCM ciphertext in the production capture path,
                // so a DACL failure is logged and the write proceeds — the
                // permission is defense-in-depth on already-encrypted data, matching
                // the warn-and-continue treatment in file_secret_store.
                #[cfg(windows)]
                if let Err(e) = crate::encryption::set_owner_only_dacl(&file_path) {
                    warn!(
                        file = %file_path.display(),
                        "frame file: failed to set owner-only DACL: {e}"
                    );
                }
                // Torn-file cleanup on write/flush failure lives in the helper
                // (#6244) so the same guarantee is unit-testable in isolation.
                write_all_or_cleanup(file, &file_path, data).await?;
                return Ok(filename);
            }
            // Name already taken (e.g. counter reset to 0 after a restart) —
            // draw a fresh counter and retry instead of overwriting.
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(StorageError::Internal(format!(
                    "frame file save failure: {e}"
                )))
            }
        }
    }

    Err(StorageError::Internal(format!(
        "frame file save failure: could not find a free filename after {FRAME_WRITE_MAX_RETRIES} retries"
    )))
}

/// Keep the permit check inside the same blocking job that owns file creation
/// and writes. Revocation while queued must prevent even the first byte.
async fn write_frame_authorized(
    counter: Arc<AtomicU32>,
    day_dir: PathBuf,
    time_str: String,
    data: Vec<u8>,
    permit: CapturePermit,
) -> Result<String, StorageError> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let data = zeroize::Zeroizing::new(data);
        for _ in 0..FRAME_WRITE_MAX_RETRIES {
            permit.check(CaptureAccess::Retain)?;
            let filename = format!(
                "{time_str}-{:010}.webp",
                counter.fetch_add(1, Ordering::SeqCst)
            );
            let path = day_dir.join(&filename);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = match options.open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e.into()),
            };
            // This exact path is owned by this successful create_new call.
            // Never delete a colliding file or another producer's output.
            let result = (|| -> Result<(), StorageError> {
                #[cfg(windows)]
                if let Err(e) = crate::encryption::set_owner_only_dacl(&path) {
                    warn!("frame file: failed to set owner-only DACL: {e}");
                }
                permit.check(CaptureAccess::Retain)?;
                file.write_all(&data)?;
                file.flush()?;
                permit.check(CaptureAccess::Retain)?;
                Ok(())
            })();
            drop(file);
            if let Err(error) = result {
                if let Err(cleanup) = std::fs::remove_file(&path) {
                    warn!("rejected frame cleanup failed: {cleanup}");
                }
                return Err(error);
            }
            return Ok(filename);
        }
        Err(StorageError::Internal(
            "authorized frame filename retries exhausted".into(),
        ))
    })
    .await
    .map_err(|e| StorageError::Internal(format!("authorized frame write task failed: {e}")))?
}

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
        self.save_frame_with_permit(timestamp, webp_data, None)
            .await
    }

    pub async fn save_frame_authorized(
        &self,
        timestamp: DateTime<Utc>,
        webp_data: &[u8],
        permit: &CapturePermit,
    ) -> Result<PathBuf, StorageError> {
        self.save_frame_with_permit(timestamp, webp_data, Some(permit))
            .await
    }

    async fn save_frame_with_permit(
        &self,
        timestamp: DateTime<Utc>,
        webp_data: &[u8],
        permit: Option<&CapturePermit>,
    ) -> Result<PathBuf, StorageError> {
        // #4928: acquire the frame barrier (shared read) — serializes against the
        // write taken by delete_all_files. If a delete is in progress, wait here;
        // after the delete, the write is skipped because deletion_flag is set.
        let _barrier = self.frame_barrier.read().await;
        if let Some(permit) = permit {
            permit.check(CaptureAccess::Retain)?;
        }
        // #4928: if the erasure block signal (`deletion_flag || erasing`) is set,
        // skip the write as a no-op rather than writing a file (the return value
        // is an empty PathBuf). #4928 round-3 (FIX B): `erasing` blocks the
        // re-consent race inside the erase window (grant_consent cannot clear it).
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
        // #7074 (MS-001): create the day directory owner-only (Unix 0o700 / Windows
        // owner-only DACL) so the screen-capture tree is not world-traversable.
        create_dir_owner_only(&day_dir).await?;

        let time_str = timestamp.format("%H-%M-%S").to_string();

        let data_to_write = if let Some(ref key) = self.encryption_key {
            key.encrypt(webp_data)?
        } else {
            webp_data.to_vec()
        };

        let written_len = data_to_write.len() as u64;
        let filename = if let Some(permit) = permit {
            write_frame_authorized(
                self.frame_counter.clone(),
                day_dir,
                time_str,
                data_to_write,
                permit.clone(),
            )
            .await?
        } else {
            write_frame_atomic(&self.frame_counter, &day_dir, &time_str, &data_to_write).await?
        };

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
        // #4928: acquire the frame barrier (shared read) — serializes against delete_all_files.
        let _barrier = self.frame_barrier.read().await;
        // #4928: if the erasure block signal (`deletion_flag || erasing`) is set,
        // write no files and skip with empty paths (#4928 round-3 FIX B — `erasing`
        // blocks the re-consent race).
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
        if free_mb < DISK_SPACE_WARN_MB {
            warn!(
                free_mb,
                batch_size = frames.len(),
                "disk space low — frame batch save proceeding with caution"
            );
        }

        let mut handles = Vec::with_capacity(frames.len());

        for (timestamp, webp_data) in frames {
            let base_dir = self.base_dir.clone();
            let frame_counter = Arc::clone(&self.frame_counter);
            let enc_key = self.encryption_key.clone();

            handles.push(tokio::spawn(async move {
                let date_str = timestamp.format("%Y-%m-%d").to_string();
                let day_dir = base_dir.join("frames").join(&date_str);

                // #7074 (MS-001): owner-only day directory (see save_frame).
                create_dir_owner_only(&day_dir).await?;

                let time_str = timestamp.format("%H-%M-%S").to_string();

                let data_to_write = if let Some(ref key) = enc_key {
                    key.encrypt(&webp_data)?
                } else {
                    webp_data
                };

                let written_len = data_to_write.len() as u64;
                let filename =
                    write_frame_atomic(&frame_counter, &day_dir, &time_str, &data_to_write).await?;

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

        // #6281 (review4 maekon-web): jail the resolved path under base_dir so a
        // traversal in `relative_path` (e.g. a corrupted/tampered DB-stored path)
        // cannot read outside the frames root. `canonicalize()` resolves `..` and
        // symlinks and requires the path to exist, so a missing file maps to the
        // same NotFound as before; a path that escapes the root is rejected as
        // NotFound (fail-closed, no traversal-vs-missing information leak). Jails
        // EVERY caller of load_frame, including the get_frame_image fast path that
        // previously bypassed the handler's own path-jail.
        let canonical = match full_path.canonicalize() {
            Ok(c) => c,
            Err(_) => {
                return Err(StorageError::NotFound {
                    resource_type: "Frame".to_string(),
                    id: relative_path.display().to_string(),
                });
            }
        };
        let base_canonical = self
            .base_dir
            .canonicalize()
            .unwrap_or_else(|_| self.base_dir.clone());
        if !canonical.starts_with(&base_canonical) {
            return Err(StorageError::NotFound {
                resource_type: "Frame".to_string(),
                id: relative_path.display().to_string(),
            });
        }

        // Acquire the pooled buffer only AFTER the fallible read+decrypt so an
        // early `?`-return cannot drop a pooled `Vec` and permanently deplete
        // the pool. There must be no `?` between `acquire()` and `release()`.
        let raw = fs::read(&canonical)
            .await
            .map_err(|e| StorageError::Internal(format!("frame file read failure: {e}")))?;

        // Both branches yield `Zeroizing<Vec<u8>>` so the decrypted plaintext is
        // wiped on drop (#6242); the unencrypted branch is wrapped to unify the
        // types and is harmless (raw is already on-disk bytes). `&data` still
        // derefs to `&[u8]`.
        let data: zeroize::Zeroizing<Vec<u8>> = if let Some(ref key) = self.encryption_key {
            key.decrypt(&raw)?
        } else {
            zeroize::Zeroizing::new(raw)
        };

        let mut buffer = self.buffer_pool.acquire();
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

            // Try newest-first, descending to the next-lower counter on any
            // per-file load failure (#6244). A torn/corrupt newest frame (e.g.
            // a partial write that escaped cleanup, or an undecryptable file)
            // must not block returning the prior good frame — skip-and-try-next
            // rather than propagating the error.
            for latest in &files {
                let Some(filename) = latest.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                let relative_path = PathBuf::from("frames").join(&day).join(filename);
                match self.load_frame(&relative_path).await {
                    Ok(bytes) => {
                        let format = latest
                            .extension()
                            .and_then(|e| e.to_str())
                            .map(|s| s.to_lowercase())
                            .unwrap_or_else(|| "webp".to_string());
                        return Ok(Some((bytes, format)));
                    }
                    Err(e) => {
                        warn!(
                            file = %relative_path.display(),
                            "skipping unreadable frame, trying next-lower counter: {e}"
                        );
                        continue;
                    }
                }
            }
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

                // #6281 (review4 maekon-web): jail under base_dir — same path-jail
                // as load_frame, applied to this batch sibling so a traversal in a
                // stored path cannot read outside the frames root (missing/escaping
                // → NotFound, fail-closed).
                let canonical = match full_path.canonicalize() {
                    Ok(c) => c,
                    Err(_) => {
                        return Err(StorageError::NotFound {
                            resource_type: "Frame".to_string(),
                            id: path.display().to_string(),
                        });
                    }
                };
                let base_canonical = base_dir.canonicalize().unwrap_or_else(|_| base_dir.clone());
                if !canonical.starts_with(&base_canonical) {
                    return Err(StorageError::NotFound {
                        resource_type: "Frame".to_string(),
                        id: path.display().to_string(),
                    });
                }

                // Acquire the pooled buffer only AFTER the fallible read+decrypt so an
                // early `?`-return cannot drop a pooled `Vec` and permanently deplete
                // the pool. There must be no `?` between `acquire()` and `release()`.
                let raw = fs::read(&canonical)
                    .await
                    .map_err(|e| StorageError::Internal(format!("frame file read failure: {e}")))?;

                // Both branches yield `Zeroizing<Vec<u8>>` so the decrypted
                // plaintext is wiped on drop (#6242); `&data` derefs to `&[u8]`.
                let data: zeroize::Zeroizing<Vec<u8>> = if let Some(ref key) = enc_key {
                    key.decrypt(&raw)?
                } else {
                    zeroize::Zeroizing::new(raw)
                };

                let mut buffer = buffer_pool.acquire();
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

#[cfg(test)]
mod capture_boundary_tests {
    use super::*;
    use maekon_core::ports::vision::{capture_denied, CaptureGuard};
    use std::sync::atomic::AtomicBool;

    struct Revocable(Arc<AtomicBool>);
    impl CaptureGuard for Revocable {
        fn check(&self, _: CaptureAccess) -> Result<(), maekon_core::error::CoreError> {
            if self.0.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(capture_denied("synthetic revocation"))
            }
        }
    }

    struct QueuedGuard {
        allowed: Arc<AtomicBool>,
        directory: PathBuf,
        observed_created_file: Arc<AtomicBool>,
    }

    impl CaptureGuard for QueuedGuard {
        fn check(&self, _: CaptureAccess) -> Result<(), maekon_core::error::CoreError> {
            if self.allowed.load(Ordering::Acquire) {
                return Ok(());
            }
            if std::fs::read_dir(&self.directory).unwrap().next().is_some() {
                self.observed_created_file.store(true, Ordering::Release);
            }
            Err(capture_denied("synthetic revocation"))
        }
    }

    #[tokio::test]
    async fn authorized_port_roundtrip_preserves_encryption() {
        use crate::encryption::EncryptionKey;
        use maekon_core::ports::frame_storage::FrameStoragePort;

        let dir = tempfile::tempdir().unwrap();
        let key = Arc::new(EncryptionKey::from_bytes([0x42; 32]));
        let storage =
            FrameFileStorage::with_encryption(dir.path().to_path_buf(), 100, 7, Some(key))
                .await
                .unwrap();
        let permit = CapturePermit::new(Arc::new(Revocable(Arc::new(AtomicBool::new(true)))));
        let data = b"RIFF\x00\x00\x00\x00WEBPVP8 synthetic frame";
        let port: &dyn FrameStoragePort = &storage;
        let path = port
            .save_frame_authorized(Utc::now(), data, &permit)
            .await
            .unwrap();
        assert!(!path.as_os_str().is_empty());
        assert_eq!(port.load_frame(&path).await.unwrap(), data);
        assert_eq!(port.load_latest_frame().await.unwrap().unwrap().0, data);

        let wrong = FrameFileStorage::with_encryption(
            dir.path().to_path_buf(),
            100,
            7,
            Some(Arc::new(EncryptionKey::from_bytes([0x43; 32]))),
        )
        .await
        .unwrap();
        let error = wrong.load_frame(&path).await.unwrap_err();
        assert!(matches!(error, StorageError::Encryption(_)), "{error}");
    }

    #[tokio::test]
    async fn authorized_write_preserves_collisions_and_other_io_errors() {
        let dir = tempfile::tempdir().unwrap();
        let permit = CapturePermit::new(Arc::new(Revocable(Arc::new(AtomicBool::new(true)))));
        let existing = dir.path().join("collision-0000000000.webp");
        std::fs::write(&existing, b"previous producer").unwrap();
        let counter = Arc::new(AtomicU32::new(0));
        let name = write_frame_authorized(
            counter.clone(),
            dir.path().to_path_buf(),
            "collision".into(),
            b"new producer".to_vec(),
            permit.clone(),
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&existing).unwrap(), b"previous producer");
        assert_eq!(
            std::fs::read(dir.path().join(name)).unwrap(),
            b"new producer"
        );
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        let attempts = Arc::new(AtomicU32::new(0));
        let error = write_frame_authorized(
            attempts.clone(),
            dir.path().join("missing-parent"),
            "invalid".into(),
            b"synthetic".to_vec(),
            permit,
        )
        .await
        .unwrap_err();
        match error {
            StorageError::Io(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
            error => panic!("expected the original I/O error, got {error}"),
        }
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn queued_frame_write_checks_revocation_before_file_creation() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let allowed = Arc::new(AtomicBool::new(true));
            let observed_created_file = Arc::new(AtomicBool::new(false));
            let permit = CapturePermit::new(Arc::new(QueuedGuard {
                allowed: allowed.clone(),
                directory: dir.path().to_path_buf(),
                observed_created_file: observed_created_file.clone(),
            }));
            let counter = Arc::new(AtomicU32::new(0));
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            });
            started_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            let pending = write_frame_authorized(
                counter.clone(), dir.path().to_path_buf(), "queued".into(), b"synthetic-frame".to_vec(), permit.clone(),
            );
            tokio::pin!(pending);
            tokio::select! {
                biased;
                result = &mut pending => panic!("write escaped the occupied blocking queue: {result:?}"),
                () = tokio::task::yield_now() => {}
            }
            allowed.store(false, Ordering::Release);
            release_tx.send(()).unwrap();
            blocker.await.unwrap();
            let error = pending.await.unwrap_err();
            assert!(error.to_string().contains("synthetic revocation"), "{error}");
            assert!(!observed_created_file.load(Ordering::Acquire));
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
            allowed.store(true, Ordering::Release);
            let filename = write_frame_authorized(
                counter, dir.path().to_path_buf(), "positive".into(), b"synthetic-frame".to_vec(), permit,
            ).await.unwrap();
            assert_eq!(std::fs::read(dir.path().join(filename)).unwrap(), b"synthetic-frame");
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        });
    }
}
