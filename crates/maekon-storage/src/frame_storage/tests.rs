#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use super::super::disk::DiskSpaceCache;
    use super::super::util::list_date_dirs;
    use super::super::{BufferPoolStats, FrameFileStorage};
    use crate::encryption::EncryptionKey;
    use crate::error::StorageError;
    use chrono::Utc;
    use std::path::Path;
    use std::sync::Arc;
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
        // Contract is expressed with '/' separators; normalize the OS separator
        // first so this asserts the same logical path on Windows ('\\').
        let path_str = path.to_string_lossy().replace('\\', "/");
        assert!(path_str.contains("frames/"));
        assert!(path_str.ends_with(".webp"));

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

    /// Regression: >1000 frames sharing a one-second timestamp must each get a
    /// distinct filename. The previous `% 1000` counter wrap reused suffixes
    /// after 1000 frames, silently overwriting earlier frames in the same second.
    #[tokio::test]
    async fn more_than_1000_frames_same_second_do_not_collide() {
        let (storage, _temp) = create_test_storage().await;

        let timestamp = Utc::now();
        const N: usize = 1050;

        let mut paths = std::collections::HashSet::new();
        for i in 0..N {
            let data = format!("frame-{i}").into_bytes();
            let path = storage.save_frame(timestamp, &data).await.unwrap();
            assert!(
                paths.insert(path.clone()),
                "frame {i} reused filename {} -- collision after >1000 frames",
                path.display()
            );
        }
        assert_eq!(paths.len(), N, "all {N} frames must have unique paths");

        // Every written frame must still be loadable by its returned path,
        // proving the read-back/lookup path is consistent with the new names.
        for path in &paths {
            let loaded = storage.load_frame(path).await.unwrap();
            assert!(loaded.starts_with(b"frame-"));
        }
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
        for (i, result) in results.into_iter().enumerate() {
            let path =
                result.unwrap_or_else(|e| panic!("save_frames_batch frame {i} must succeed: {e}"));
            // Contract: each saved frame returns a relative path under frames/<date>/<name>.webp.
            // Normalize the OS separator so Windows ('\\') asserts the same logical path.
            let path_str = path.to_string_lossy().replace('\\', "/");
            assert!(
                path_str.starts_with("frames/"),
                "frame {i}: path must start with 'frames/', got '{path_str}'"
            );
            assert!(
                path_str.ends_with(".webp"),
                "frame {i}: path must end with '.webp', got '{path_str}'"
            );
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

        assert!(
            matches!(
                storage
                    .load_frame(Path::new("frames/2099-01-01/00-00-00-000.webp"))
                    .await
                    .unwrap_err(),
                StorageError::NotFound { .. }
            ),
            "loading a non-existent frame must yield StorageError::NotFound"
        );
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

    /// Regression (retention-datedir): `enforce_retention` must classify date
    /// directories with the same shared `list_date_dirs` recognizer that the
    /// storage-limit and GDPR delete-all paths use. The previous inline check
    /// only tested `name.len() == 10`, so it never confirmed the entry was a
    /// directory and skipped the `YYYY-MM-DD` hyphen-at-index-4 shape check.
    /// That weaker check could match a 10-char file or a 10-char non-date
    /// directory at the frames root, classifying retention candidates
    /// inconsistently with the other two paths.
    #[tokio::test]
    async fn enforce_retention_uses_shared_date_dir_recognizer() {
        // retention_days = 7 (from create_test_storage).
        let (storage, _temp) = create_test_storage().await;
        let frames_dir = storage.frames_dir();
        tokio::fs::create_dir_all(&frames_dir).await.unwrap();

        // (1) Genuinely old date directory -- must be deleted (older than cutoff).
        let old_dir = frames_dir.join("2000-01-01");
        tokio::fs::create_dir_all(&old_dir).await.unwrap();
        tokio::fs::write(old_dir.join("10-00-00-000.webp"), b"old")
            .await
            .unwrap();

        // (2) Recent date directory -- newer than the 7-day cutoff, must survive.
        let recent_name = Utc::now().format("%Y-%m-%d").to_string();
        let recent_dir = frames_dir.join(&recent_name);
        tokio::fs::create_dir_all(&recent_dir).await.unwrap();
        tokio::fs::write(recent_dir.join("10-00-00-000.webp"), b"recent")
            .await
            .unwrap();

        // (3) A 10-char *file* whose name sorts before the cutoff. The old inline
        //     check (no is_dir guard) would have treated this as a deletable date
        //     dir; the shared recognizer ignores non-directories.
        let lookalike_file = frames_dir.join("1999-12-31"); // 10 chars, < cutoff
        tokio::fs::write(&lookalike_file, b"not-a-dir")
            .await
            .unwrap();

        // (4) A 10-char *directory* without the YYYY-MM-DD hyphen at index 4. The
        //     old check (no hyphen-at-4 guard) would have matched on length alone.
        let bogus_dir = frames_dir.join("abcd123456"); // 10 chars, no '-' at idx 4
        tokio::fs::create_dir_all(&bogus_dir).await.unwrap();
        tokio::fs::write(bogus_dir.join("payload.bin"), b"keep")
            .await
            .unwrap();

        let deleted = storage.enforce_retention().await.unwrap();

        // Only the single file in the genuinely-old date directory is deleted.
        assert_eq!(
            deleted, 1,
            "only the old date directory's frame should be counted/deleted (retention-datedir)"
        );

        // The recognizer-recognized directories that survive must exactly match
        // what list_date_dirs reports -- i.e. the recent date dir, and nothing else
        // that the helper considers a date dir.
        let mut remaining = list_date_dirs(&frames_dir).await.unwrap();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![recent_name.clone()],
            "list_date_dirs must see only the recent date dir after retention (retention-datedir)"
        );

        // The old date directory is gone.
        assert!(
            !old_dir.exists(),
            "the old date directory must be removed by retention"
        );
        // The non-date lookalike file is untouched (never a retention candidate).
        assert!(
            lookalike_file.exists(),
            "a 10-char file must not be treated as a deletable date dir (retention-datedir)"
        );
        // The 10-char non-date directory is untouched (fails the hyphen-at-4 shape).
        assert!(
            bogus_dir.join("payload.bin").exists(),
            "a 10-char non-date directory must not be treated as a date dir (retention-datedir)"
        );
    }

    #[tokio::test]
    async fn reconcile_cache_size_counts_nested_frame_files() {
        let (storage, temp) = create_test_storage().await;
        let day_dir = temp.path().join("frames").join("2026-05-26");
        tokio::fs::create_dir_all(&day_dir).await.unwrap();

        tokio::fs::write(day_dir.join("10-00-00-000.webp"), vec![0u8; 7])
            .await
            .unwrap();
        tokio::fs::write(day_dir.join("10-00-01-001.webp"), vec![1u8; 11])
            .await
            .unwrap();
        tokio::fs::write(temp.path().join("root-noise.bin"), vec![2u8; 19])
            .await
            .unwrap();

        let total = storage.reconcile_cache_size().unwrap();

        assert_eq!(total, 18);
        assert_eq!(storage.total_size_mb().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn buffer_pool_stats() {
        let (storage, _temp) = create_test_storage().await;

        let stats: BufferPoolStats = storage.buffer_pool_stats();
        assert_eq!(stats.pool_capacity, super::super::buffer::BUFFER_POOL_SIZE);
        assert_eq!(stats.buffer_size, super::super::buffer::DEFAULT_BUFFER_SIZE);
    }

    #[test]
    fn buffer_pool_acquire_release() {
        use super::super::buffer::BufferPool;
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

        let now = Utc::now();
        let yesterday = now - chrono::Duration::days(1);
        storage.save_frame(now, b"frame-today-1").await.unwrap();
        storage.save_frame(now, b"frame-today-2").await.unwrap();
        storage
            .save_frame(yesterday, b"frame-yesterday")
            .await
            .unwrap();

        let frames_dir = storage.frames_dir();
        let dirs_before = list_date_dirs(&frames_dir).await.unwrap();
        assert!(!dirs_before.is_empty());

        let deleted = storage.delete_all_files().await.unwrap();
        assert_eq!(deleted, 3);

        let remaining = list_date_dirs(&frames_dir).await.unwrap();
        assert!(remaining.is_empty());
    }

    // ---- #4928: deletion_flag frame barrier --------------------------------

    /// When deletion_flag is set, save_frame writes no file and returns an empty path.
    #[tokio::test]
    async fn save_frame_skips_when_deletion_flag_set() {
        let (mut storage, _temp) = create_test_storage().await;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        storage.set_deletion_flag(flag.clone());

        // flag clear: normal save.
        let p1 = storage
            .save_frame(Utc::now(), b"before-revoke")
            .await
            .unwrap();
        assert!(
            !p1.as_os_str().is_empty(),
            "path must not be empty when the flag is clear"
        );

        // flag set: skip (empty path, no file created).
        flag.store(true, std::sync::atomic::Ordering::Release);
        let p2 = storage
            .save_frame(Utc::now(), b"after-revoke")
            .await
            .unwrap();
        assert!(
            p2.as_os_str().is_empty(),
            "save_frame must return an empty path when deletion_flag is set (#4928 skip)"
        );

        // Only the single pre-revoke frame must exist on disk.
        let dirs = list_date_dirs(&storage.frames_dir()).await.unwrap();
        let mut total_files = 0usize;
        for d in dirs {
            total_files +=
                super::super::util::count_files_in_dir(&storage.frames_dir().join(&d)).await;
        }
        assert_eq!(
            total_files, 1,
            "frame writes after revoke must be skipped, leaving only the 1 pre-revoke file"
        );
    }

    /// When deletion_flag is set, save_frames_batch skips every entry (empty paths).
    #[tokio::test]
    async fn save_frames_batch_skips_when_deletion_flag_set() {
        let (mut storage, _temp) = create_test_storage().await;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        storage.set_deletion_flag(flag.clone());

        let now = Utc::now();
        let frames: Vec<_> = (0..4)
            .map(|i| (now, format!("f{i}").into_bytes()))
            .collect();
        let results = storage.save_frames_batch(frames).await;
        assert_eq!(results.len(), 4);
        for r in results {
            let p = r.unwrap();
            assert!(
                p.as_os_str().is_empty(),
                "every batch entry must be an empty path (skipped) when deletion_flag is set"
            );
        }
        // No frame file may exist on disk.
        assert!(
            list_date_dirs(&storage.frames_dir())
                .await
                .unwrap()
                .is_empty(),
            "with the flag set, the batch must not create any directory or file"
        );
    }

    /// After set_deletion_flag installs a flag, deletion_flag() returns the same Arc (ptr-eq).
    #[tokio::test]
    async fn set_deletion_flag_is_ptr_eq() {
        let (mut storage, _temp) = create_test_storage().await;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        storage.set_deletion_flag(flag.clone());
        let observed = storage.deletion_flag();
        assert!(
            std::sync::Arc::ptr_eq(&observed, &flag),
            "the installed flag and the deletion_flag() return value must be the same Arc"
        );
    }

    #[test]
    fn disk_space_cache_returns_max_for_nonexistent_path() {
        let cache = DiskSpaceCache::new();
        let free = cache.get_free_mb(Path::new("/nonexistent/path/that/does/not/exist"));
        assert_eq!(free, u64::MAX);
    }

    #[test]
    fn disk_space_cache_returns_real_value_for_temp_dir() {
        let cache = DiskSpaceCache::new();
        let free = cache.get_free_mb(&std::env::temp_dir());
        assert!(free < u64::MAX);
        assert!(free > 0);
    }

    #[test]
    fn disk_space_cache_caches_within_interval() {
        let cache = DiskSpaceCache::new();
        let path = std::env::temp_dir();
        let first = cache.get_free_mb(&path);
        let second = cache.get_free_mb(&path);
        assert_eq!(first, second);
    }

    // ---- Encrypted frame storage tests -------------------------------

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

        let full_path = temp.path().join(&rel_path);
        let raw_on_disk = tokio::fs::read(&full_path).await.unwrap();

        assert_ne!(raw_on_disk, test_data);
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

        let storage2 =
            FrameFileStorage::with_encryption(temp_dir.path().to_path_buf(), 100, 7, Some(key2))
                .await
                .unwrap();

        assert!(
            matches!(
                storage2.load_frame(&rel_path).await.unwrap_err(),
                StorageError::Encryption(_)
            ),
            "decrypting a frame with the wrong key must yield StorageError::Encryption"
        );
    }

    /// Regression: a load that fails on the read+decrypt step must not deplete
    /// the buffer pool. The decrypt failure is the fallible step that used to
    /// sit between `acquire()` and `release()`; if the pooled buffer is dropped
    /// on the early `?`-return the pool shrinks permanently (bounded perf
    /// regression). The pool occupancy must be unchanged after the error.
    #[tokio::test]
    async fn load_error_path_does_not_deplete_buffer_pool() {
        let temp_dir = TempDir::new().unwrap();
        let key1 = Arc::new(EncryptionKey::from_bytes([0x42; 32]));
        let key2 = Arc::new(EncryptionKey::from_bytes([0x43; 32]));

        let storage1 =
            FrameFileStorage::with_encryption(temp_dir.path().to_path_buf(), 100, 7, Some(key1))
                .await
                .unwrap();
        let rel_path = storage1
            .save_frame(Utc::now(), b"secret frame data")
            .await
            .unwrap();

        // storage2 holds the wrong key, so every decrypt below fails.
        let storage2 =
            FrameFileStorage::with_encryption(temp_dir.path().to_path_buf(), 100, 7, Some(key2))
                .await
                .unwrap();

        let initial = storage2.buffer_pool.len();
        assert_eq!(
            initial,
            super::super::buffer::BUFFER_POOL_SIZE,
            "fresh pool should be full"
        );

        // Far more failing loads than the pool capacity -- a leak would drain it.
        // Each load fails on the AES-GCM decrypt step (wrong key), so the error
        // must be StorageError::Encryption -- proving the pool guarantee is tested
        // against the read+decrypt failure path, not some unrelated error.
        for _ in 0..(super::super::buffer::BUFFER_POOL_SIZE * 4) {
            assert!(
                matches!(
                    storage2.load_frame(&rel_path).await.unwrap_err(),
                    StorageError::Encryption(_)
                ),
                "wrong-key load must fail on decrypt with StorageError::Encryption"
            );
        }
        assert_eq!(
            storage2.buffer_pool.len(),
            initial,
            "single-frame load error path must not deplete the buffer pool"
        );

        // Same guarantee for the parallel batch path (shared Arc<BufferPool>).
        let paths: Vec<_> = (0..(super::super::buffer::BUFFER_POOL_SIZE * 4))
            .map(|_| rel_path.clone())
            .collect();
        let results = storage2.load_frames_batch(paths).await;
        // Same decrypt-failure contract as the single-frame path: every batch entry
        // must surface StorageError::Encryption (wrong key), confirming the parallel
        // path also exercises the read+decrypt failure that could deplete the pool.
        assert!(
            results
                .iter()
                .all(|r| matches!(r, Err(StorageError::Encryption(_)))),
            "every wrong-key batch load must fail on decrypt with StorageError::Encryption"
        );
        assert_eq!(
            storage2.buffer_pool.len(),
            initial,
            "batch load error path must not deplete the buffer pool"
        );
    }

    // ---- #6244: torn-frame resilience --------------------------------------

    /// Regression (#6244, part 1): a write failure must not leave a torn frame
    /// file behind under the just-claimed (highest) counter. The file is created
    /// read-only so `write_all` fails deterministically on every platform; the
    /// cleanup must then remove the file before the error is propagated.
    #[tokio::test]
    async fn write_failure_removes_torn_file() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("12-00-00-0000000000.webp");

        // Create the target read-only so the subsequent write_all fails. We keep
        // .write(true) so the open itself succeeds (mirroring write_frame_atomic's
        // create_new open), but the read-only mode makes the OS reject writes.
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o444);
        }
        // Materialize the file, then reopen read-only so writes are rejected.
        opts.open(&file_path).unwrap();
        let ro_file = tokio::fs::OpenOptions::new()
            .read(true)
            .open(&file_path)
            .await
            .unwrap();

        assert!(file_path.exists(), "precondition: torn file must exist");

        let result =
            super::super::io::write_all_or_cleanup(ro_file, &file_path, b"frame payload").await;

        // The write_all failure is wrapped as StorageError::Internal with a
        // "frame file save failure" message -- assert the variant AND the message
        // so a future refactor cannot quietly turn this into an unrelated error.
        let err = result.expect_err("writing to a read-only handle must fail (#6244)");
        assert!(
            matches!(&err, StorageError::Internal(msg) if msg.contains("frame file save failure")),
            "read-only write must surface StorageError::Internal(\"frame file save failure: ...\"), got {err:?}"
        );
        assert!(
            !file_path.exists(),
            "torn file must be removed on write failure so it cannot become the highest-counter frame (#6244)"
        );
    }

    /// Regression (#6244, part 2): a corrupt/torn newest frame must not block
    /// `load_latest_frame` -- it skips to the next-lower counter and returns the
    /// prior good frame. Uses encrypted storage so an undecryptable newest file
    /// makes `load_frame` fail (the unencrypted path reads raw bytes and would
    /// not surface corruption).
    #[tokio::test]
    async fn load_latest_frame_skips_corrupt_newest_returns_prior() {
        let (storage, _temp) = create_encrypted_test_storage().await;

        // Prior good frame (lower counter).
        let ts = Utc::now();
        let good_path = storage.save_frame(ts, b"prior-good-frame").await.unwrap();
        let day_dir = storage.frames_dir().join(ts.format("%Y-%m-%d").to_string());

        // Inject a corrupt newest frame: a lexicographically-higher filename
        // (higher counter suffix) than the good one, containing non-decryptable
        // bytes. load_latest_frame sorts descending, so this is tried first.
        let good_name = good_path.file_name().unwrap().to_str().unwrap();
        let time_prefix = &good_name[..good_name.rfind('-').unwrap()];
        let corrupt_name = format!("{time_prefix}-9999999999.webp");
        tokio::fs::write(day_dir.join(&corrupt_name), b"not-encrypted-garbage")
            .await
            .unwrap();

        let latest = storage
            .load_latest_frame()
            .await
            .expect("load_latest_frame must not propagate a corrupt-newest error")
            .expect("the prior good frame must still be returned");
        assert_eq!(
            latest.0, b"prior-good-frame",
            "a corrupt newest frame must be skipped and the prior good frame returned (#6244)"
        );
        assert_eq!(latest.1, "webp");
    }

    /// Regression (#6244, part 2): if every frame in a day is unreadable, the
    /// loader returns Ok(None) rather than erroring, so the caller can fall back
    /// gracefully (e.g. to an older day) instead of failing hard.
    #[tokio::test]
    async fn load_latest_frame_returns_none_when_all_corrupt() {
        let (storage, _temp) = create_encrypted_test_storage().await;

        // Two undecryptable files in the same day, no good frame anywhere.
        let day_dir = storage.frames_dir().join("2026-05-26");
        tokio::fs::create_dir_all(&day_dir).await.unwrap();
        tokio::fs::write(day_dir.join("10-00-00-0000000000.webp"), b"garbage-a")
            .await
            .unwrap();
        tokio::fs::write(day_dir.join("10-00-01-0000000001.webp"), b"garbage-b")
            .await
            .unwrap();

        let latest = storage
            .load_latest_frame()
            .await
            .expect("all-corrupt day must yield Ok(None), not Err (#6244)");
        assert!(
            latest.is_none(),
            "with no readable frame, load_latest_frame must return None"
        );
    }

    // ---- #6245: storage-limit eviction must not over-delete ----------------

    /// Regression (#6245): `enforce_storage_limit` must track its eviction budget
    /// in bytes, not truncated MB. With per-directory sizes that all truncate to
    /// the same whole MB (1.9 MB -> 1 MB), the old MB counter shrank far slower
    /// than the real on-disk size and evicted every directory, blowing past the
    /// limit. The byte-accurate loop evicts only the minimum oldest directories
    /// needed to get under budget and leaves the rest intact.
    #[tokio::test]
    async fn enforce_storage_limit_stops_at_limit_no_over_delete() {
        let temp_dir = TempDir::new().unwrap();
        // 5 MB budget. 10 oldest-first directories of ~1.9 MB each (19 MB total).
        let storage = FrameFileStorage::new(temp_dir.path().to_path_buf(), 5, 7)
            .await
            .unwrap();

        let frames_dir = storage.frames_dir();
        // 1.9 MB: truncates to 1 MB, so the buggy MB accounting under-counts every
        // directory by ~0.9 MB and never reaches the stop condition until empty.
        const DIR_BYTES: usize = 1_992_294;
        let payload = vec![0u8; DIR_BYTES];
        for day in 1..=10u32 {
            let dir_name = format!("2026-05-{day:02}");
            let dir_path = frames_dir.join(&dir_name);
            tokio::fs::create_dir_all(&dir_path).await.unwrap();
            tokio::fs::write(dir_path.join("12-00-00-0000000000.webp"), &payload)
                .await
                .unwrap();
        }

        // cached_size_initialized starts false, so enforce_storage_limit walks the
        // tree and uses the exact byte total -- no cache priming needed.
        storage.enforce_storage_limit().await.unwrap();

        let remaining = list_date_dirs(&frames_dir).await.unwrap();
        // Byte-accurate: 19,922,944 bytes, limit 5 MB = 5,242,880 bytes. Removing
        // 8 directories leaves 3,984,592 bytes (<= limit); 7 would leave 5,976,882
        // (> limit). So exactly 8 are evicted and 2 remain. The buggy MB loop would
        // have deleted all 10 (remaining == 0).
        assert_eq!(
            remaining.len(),
            2,
            "eviction must stop once under the byte limit, not over-delete (#6245); remaining={remaining:?}"
        );

        // The survivors must be the newest directories (oldest-first eviction).
        let mut sorted = remaining.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            vec!["2026-05-09".to_string(), "2026-05-10".to_string()],
            "the two newest directories must survive (#6245)"
        );

        // And the surviving on-disk size is genuinely within budget.
        let remaining_bytes = super::super::util::calculate_dir_size(&frames_dir)
            .await
            .unwrap();
        assert!(
            remaining_bytes <= 5 * 1024 * 1024,
            "remaining {remaining_bytes} bytes must be within the 5 MB limit (#6245)"
        );
    }

    /// #7074 (MS-001): a written frame file must be owner-only (mode 0o600), not
    /// the world-readable umask default (0o644). Regression — before the fix the
    /// `create_new` open carried no `.mode(0o600)`, so screen captures inherited
    /// world-readable permissions. (The Windows owner-only DACL is exercised on CI
    /// via the `#[cfg(windows)]` path in `write_frame_atomic`.)
    #[cfg(unix)]
    #[tokio::test]
    async fn frame_file_is_owner_only_0o600() {
        use std::os::unix::fs::PermissionsExt as _;

        let (storage, temp) = create_test_storage().await;
        let rel = storage
            .save_frame(Utc::now(), b"RIFF\x00\x00\x00\x00WEBPVP8 frame")
            .await
            .unwrap();
        let full = temp.path().join(&rel);
        let mode = std::fs::metadata(&full).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "frame file must be created owner-only (0o600), got 0o{mode:o}"
        );
    }

    /// #7074 (MS-001): the frames root and each day directory must be owner-only
    /// (mode 0o700), not the world-traversable umask default (0o755). Regression —
    /// before the fix the directories were created via `create_dir_all` and
    /// inherited the umask.
    #[cfg(unix)]
    #[tokio::test]
    async fn frame_directories_are_owner_only_0o700() {
        use std::os::unix::fs::PermissionsExt as _;

        let (storage, temp) = create_test_storage().await;
        let rel = storage
            .save_frame(Utc::now(), b"RIFF\x00\x00\x00\x00WEBPVP8 frame")
            .await
            .unwrap();

        // frames/ root (created by FrameFileStorage::new).
        let frames_dir = temp.path().join("frames");
        let frames_mode = std::fs::metadata(&frames_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            frames_mode, 0o700,
            "frames root must be owner-only (0o700), got 0o{frames_mode:o}"
        );

        // The day directory (created on the save path) — derive it from the
        // returned relative path: frames/<YYYY-MM-DD>/<file>.
        let day_dir = temp.path().join(rel.parent().unwrap());
        let day_mode = std::fs::metadata(&day_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            day_mode, 0o700,
            "frame day directory must be owner-only (0o700), got 0o{day_mode:o}"
        );
    }
}
