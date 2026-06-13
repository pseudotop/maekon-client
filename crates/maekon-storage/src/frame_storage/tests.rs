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

    // ── #4928: deletion_flag 프레임 배리어 ──────────────────────────────

    /// deletion_flag 가 set 이면 save_frame 이 파일을 쓰지 않고 빈 경로를 반환한다.
    #[tokio::test]
    async fn save_frame_skips_when_deletion_flag_set() {
        let (mut storage, _temp) = create_test_storage().await;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        storage.set_deletion_flag(flag.clone());

        // flag clear: 정상 저장.
        let p1 = storage
            .save_frame(Utc::now(), b"before-revoke")
            .await
            .unwrap();
        assert!(
            !p1.as_os_str().is_empty(),
            "flag clear 시 경로가 비어선 안 된다"
        );

        // flag set: 스킵 (빈 경로, 파일 미생성).
        flag.store(true, std::sync::atomic::Ordering::Release);
        let p2 = storage
            .save_frame(Utc::now(), b"after-revoke")
            .await
            .unwrap();
        assert!(
            p2.as_os_str().is_empty(),
            "deletion_flag set 시 save_frame 은 빈 경로를 반환해야 한다 (#4928 스킵)"
        );

        // 디스크에는 revoke 전 1개 프레임만 존재해야 한다.
        let dirs = list_date_dirs(&storage.frames_dir()).await.unwrap();
        let mut total_files = 0usize;
        for d in dirs {
            total_files +=
                super::super::util::count_files_in_dir(&storage.frames_dir().join(&d)).await;
        }
        assert_eq!(
            total_files, 1,
            "revoke 이후 프레임 쓰기는 스킵되어 파일이 1개(revoke 전)만 남아야 한다"
        );
    }

    /// deletion_flag 가 set 이면 save_frames_batch 가 모두 스킵(빈 경로)한다.
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
                "deletion_flag set 시 batch 의 모든 항목이 빈 경로(스킵)여야 한다"
            );
        }
        // 디스크에 어떤 프레임 파일도 없어야 한다.
        assert!(
            list_date_dirs(&storage.frames_dir())
                .await
                .unwrap()
                .is_empty(),
            "flag set 시 batch 는 디렉터리/파일을 만들지 않아야 한다"
        );
    }

    /// set_deletion_flag install 후 deletion_flag() 가 동일 Arc(ptr-eq)를 반환한다.
    #[tokio::test]
    async fn set_deletion_flag_is_ptr_eq() {
        let (mut storage, _temp) = create_test_storage().await;
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        storage.set_deletion_flag(flag.clone());
        let observed = storage.deletion_flag();
        assert!(
            std::sync::Arc::ptr_eq(&observed, &flag),
            "install 된 flag 와 deletion_flag() 반환값은 동일 Arc 여야 한다"
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
}
