use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use maekon_core::config::AppConfig;
use maekon_core::consent::ConsentManager;
use maekon_core::error::CoreError;
use maekon_core::ports::accessibility::AccessibilityExtractor;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::monitor::{ActivityMonitor, ProcessMonitor};
use maekon_core::ports::vision::{
    capture_denied, CaptureAccess, CaptureAuthority, CapturePermit, FrameProcessor,
};
use maekon_storage::encryption::EncryptionKey;
use maekon_storage::frame_storage::FrameFileStorage;
use maekon_vision::processor::EdgeFrameProcessor;

pub(crate) struct SharedCaptureServices {
    pub(crate) frame_storage: Arc<dyn FrameStoragePort>,
    pub(crate) process_monitor: Arc<dyn ProcessMonitor>,
    pub(crate) activity_monitor: Arc<dyn ActivityMonitor>,
    pub(crate) frame_processor: Arc<dyn FrameProcessor>,
    pub(crate) accessibility_extractor: Option<Arc<dyn AccessibilityExtractor>>,
    pub(crate) consent_manager: Arc<dyn ConsentManagerPort>,
}

impl SharedCaptureServices {
    pub(crate) async fn build(
        data_dir: &Path,
        config: &AppConfig,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Result<Self> {
        // #4928: create the ConsentManager first to obtain the shared
        // erasure-blocking flag, then install it *before* FrameFileStorage is
        // wrapped in an Arc (set_deletion_flag takes &mut self). Installing the
        // same Arc into SqliteStorage is handled by the upper composition root
        // (app_runtime_launch).
        let consent_manager = Arc::new(ConsentManager::new(data_dir.join("consent.json")));

        let mut frame_storage_concrete = FrameFileStorage::with_encryption(
            data_dir.to_path_buf(),
            config.storage.max_storage_mb,
            config.storage.retention_days,
            encryption_key,
        )
        .await?;
        // Install the shared flag (linked to ConsentManager / SQLite via ptr-eq).
        frame_storage_concrete.set_deletion_flag(consent_manager.deletion_flag());
        // #4928 round-3 (FIX B): likewise install the `erasing` signal (an
        // erase-window blocking signal that grant_consent cannot clear — same
        // Arc as SQLite).
        frame_storage_concrete.set_erasing(consent_manager.erasing());
        // Retain original permits only for authorized writes. Runtime capture
        // authority is installed by the coordinated consumer activation stage.
        let frame_storage = bind_frame_storage(Arc::new(frame_storage_concrete), None);

        let process_monitor: Arc<dyn ProcessMonitor> =
            Arc::new(maekon_monitor::process::ProcessTracker::new());
        let activity_monitor: Arc<dyn ActivityMonitor> = Arc::new(
            maekon_monitor::activity::ActivityTracker::new(process_monitor.clone()),
        );

        let ocr_tessdata = std::env::var("MAEKON_TESSDATA").ok().map(PathBuf::from);
        // #8054: thread the configured OCR recognition languages (default
        // Korean-first) into the processor so macOS Vision / leptess recognize
        // Korean screen text instead of silently defaulting to English-only.
        let frame_processor: Arc<dyn FrameProcessor> = Arc::new(
            EdgeFrameProcessor::with_pii_level(
                config.vision.thumbnail_width,
                config.vision.thumbnail_height,
                ocr_tessdata,
                config.privacy.pii_filter_level,
            )
            .with_ocr_languages(config.vision.ocr_languages.clone()),
        );

        Ok(Self {
            frame_storage,
            process_monitor,
            activity_monitor,
            frame_processor,
            accessibility_extractor: maekon_vision::accessibility::create_extractor(),
            // #4928: same instance whose flag was installed into frame_storage above.
            consent_manager,
        })
    }
}

/// Only successful writes with their ORIGINAL in-process permit become an OCR
/// source. Restart deliberately has no authorized latest image until a fresh
/// capture; history reads retain their separate existing re-auth contract.
struct AuthorizedFrameStorage {
    inner: Arc<dyn FrameStoragePort>,
    authority: Option<Arc<dyn CaptureAuthority>>,
    latest: Mutex<Option<(chrono::DateTime<chrono::Utc>, PathBuf, CapturePermit)>>,
}

pub(crate) fn bind_frame_storage(
    inner: Arc<dyn FrameStoragePort>,
    authority: Option<Arc<dyn CaptureAuthority>>,
) -> Arc<dyn FrameStoragePort> {
    Arc::new(AuthorizedFrameStorage {
        inner,
        authority,
        latest: Mutex::new(None),
    })
}

#[async_trait::async_trait]
impl FrameStoragePort for AuthorizedFrameStorage {
    fn capture_authority(&self) -> Option<Arc<dyn CaptureAuthority>> {
        self.authority.clone()
    }

    async fn save_frame_authorized(
        &self,
        at: chrono::DateTime<chrono::Utc>,
        data: &[u8],
        permit: &CapturePermit,
    ) -> Result<PathBuf, CoreError> {
        let path = self.inner.save_frame_authorized(at, data, permit).await?;
        permit.check(CaptureAccess::Retain)?;
        if path.as_os_str().is_empty() {
            return Err(capture_denied("frame write skipped during erasure"));
        }
        let mut latest = self
            .latest
            .lock()
            .map_err(|_| capture_denied("frame authority poisoned"))?;
        if latest
            .as_ref()
            .is_none_or(|(previous, _, _)| at >= *previous)
        {
            *latest = Some((at, path.clone(), permit.clone()));
        }
        drop(latest);
        Ok(path)
    }

    async fn load_latest_frame_authorized(
        &self,
        consumer: &CapturePermit,
    ) -> Result<Option<(Vec<u8>, String)>, CoreError> {
        consumer.check(CaptureAccess::CurrentScene)?;
        consumer.check(CaptureAccess::Ocr)?;
        let latest = self
            .latest
            .lock()
            .map_err(|_| capture_denied("frame authority poisoned"))?
            .clone();
        let Some((_, path, origin)) = latest else {
            return Ok(None);
        };
        origin.check(CaptureAccess::CurrentScene)?;
        origin.check(CaptureAccess::Ocr)?;
        let bytes = self.inner.load_frame(&path).await?;
        origin.check(CaptureAccess::CurrentScene)?;
        origin.check(CaptureAccess::Ocr)?;
        consumer.check(CaptureAccess::CurrentScene)?;
        consumer.check(CaptureAccess::Ocr)?;
        Ok(Some((bytes, "webp".into())))
    }

    async fn save_frame(
        &self,
        at: chrono::DateTime<chrono::Utc>,
        data: &[u8],
    ) -> Result<PathBuf, CoreError> {
        self.inner.save_frame(at, data).await
    }
    async fn save_frames_batch(
        &self,
        frames: Vec<(chrono::DateTime<chrono::Utc>, Vec<u8>)>,
    ) -> Vec<Result<PathBuf, CoreError>> {
        self.inner.save_frames_batch(frames).await
    }
    async fn load_frame(&self, path: &Path) -> Result<Vec<u8>, CoreError> {
        self.inner.load_frame(path).await
    }
    async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, CoreError> {
        self.inner.load_latest_frame().await
    }
    async fn enforce_retention(&self) -> Result<usize, CoreError> {
        self.inner.enforce_retention().await
    }
    async fn enforce_storage_limit(&self) -> Result<usize, CoreError> {
        self.inner.enforce_storage_limit().await
    }
    async fn delete_all_frames(&self) -> Result<usize, CoreError> {
        *self
            .latest
            .lock()
            .map_err(|_| capture_denied("frame authority poisoned"))? = None;
        self.inner.delete_all_frames().await
    }
}

#[cfg(test)]
mod origin_storage_tests {
    use super::*;
    use maekon_core::models::context::WindowInfo;
    use maekon_core::ports::vision::{CaptureGuard, CapturePauseState};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::sync::Notify;

    struct PauseAuthority(Arc<CapturePauseState>);

    impl PauseAuthority {
        fn running() -> Self {
            Self(Arc::new(CapturePauseState::new(Arc::new(AtomicBool::new(
                false,
            )))))
        }
    }

    struct PauseGuard {
        state: Arc<CapturePauseState>,
        generation: u64,
    }

    impl CaptureGuard for PauseGuard {
        fn check(&self, _: CaptureAccess) -> Result<(), CoreError> {
            self.state.check(self.generation)
        }
    }

    impl CaptureAuthority for PauseAuthority {
        fn authorize(&self, _: Option<&WindowInfo>) -> Result<CapturePermit, CoreError> {
            Ok(CapturePermit::new(Arc::new(PauseGuard {
                state: self.0.clone(),
                generation: self.0.snapshot()?,
            })))
        }

        fn toggle_pause(&self) -> Result<bool, CoreError> {
            self.0.toggle()
        }
    }

    async fn real_storage(path: &Path) -> Result<Arc<dyn FrameStoragePort>> {
        Ok(Arc::new(
            FrameFileStorage::new(path.to_path_buf(), 100, 30).await?,
        ))
    }

    fn assert_bytes(value: Option<(Vec<u8>, String)>, expected: &[u8]) {
        let Some((bytes, format)) = value else {
            panic!("expected the authorized synthetic frame");
        };
        assert_eq!(bytes, expected);
        assert_eq!(format, "webp");
    }

    fn assert_capture_denied(error: CoreError, reason: &str) {
        let CoreError::PermissionDenied { code, message } = error else {
            panic!("expected capture privacy denial, got {error:?}");
        };
        assert_eq!(
            code,
            maekon_core::error_codes::PermissionCode::PrivacyDenied
        );
        assert_eq!(message, format!("capture denied: {reason}"));
    }

    #[tokio::test]
    async fn legacy_history_does_not_create_authorized_origin() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let inner = real_storage(dir.path()).await?;
        let storage = bind_frame_storage(inner.clone(), None);
        assert!(storage.capture_authority().is_none());
        let at = chrono::Utc::now();
        let marker = b"synthetic legacy marker";
        let path = storage.save_frame(at, marker).await?;
        assert_eq!(storage.load_frame(&path).await?, marker);
        assert_bytes(storage.load_latest_frame().await?, marker);
        let authority = PauseAuthority::running();
        let consumer = authority.authorize(None)?;
        assert!(storage
            .load_latest_frame_authorized(&consumer)
            .await?
            .is_none());

        storage
            .save_frame_authorized(at + chrono::Duration::seconds(1), marker, &consumer)
            .await?;
        assert_bytes(
            storage.load_latest_frame_authorized(&consumer).await?,
            marker,
        );
        let restarted = bind_frame_storage(inner, None);
        assert!(restarted
            .load_latest_frame_authorized(&consumer)
            .await?
            .is_none());
        assert_bytes(restarted.load_latest_frame().await?, marker);
        Ok(())
    }

    #[tokio::test]
    async fn pause_aba_rejects_old_origin_and_fresh_write_recovers() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let authority = Arc::new(PauseAuthority::running());
        let storage = bind_frame_storage(real_storage(dir.path()).await?, Some(authority.clone()));
        let old = authority.authorize(None)?;
        let at = chrono::Utc::now();
        let original = b"synthetic original capture";
        let path = storage.save_frame_authorized(at, original, &old).await?;
        assert_bytes(storage.load_latest_frame_authorized(&old).await?, original);
        let Some(installed) = storage.capture_authority() else {
            panic!("the explicitly supplied authority must be retained");
        };
        assert!(installed.toggle_pause()?);
        assert!(!installed.toggle_pause()?);
        // No guard or observer call ran between the two real epoch transitions.
        let fresh = authority.authorize(None)?;
        assert_capture_denied(
            storage
                .load_latest_frame_authorized(&fresh)
                .await
                .unwrap_err(),
            "capture pause epoch changed",
        );
        assert_capture_denied(
            storage
                .save_frame_authorized(at + chrono::Duration::seconds(1), b"denied", &old)
                .await
                .unwrap_err(),
            "capture pause epoch changed",
        );
        assert_eq!(storage.load_frame(&path).await?, original);
        let recovered = b"synthetic fresh capture";
        storage
            .save_frame_authorized(at + chrono::Duration::seconds(2), recovered, &fresh)
            .await?;
        assert_bytes(
            storage.load_latest_frame_authorized(&fresh).await?,
            recovered,
        );
        Ok(())
    }

    #[tokio::test]
    async fn older_completion_and_erasure_do_not_replace_latest_origin() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let storage = bind_frame_storage(real_storage(dir.path()).await?, None);
        let authority = PauseAuthority::running();
        let permit = authority.authorize(None)?;
        let at = chrono::Utc::now();
        storage
            .save_frame_authorized(at + chrono::Duration::seconds(2), b"newer", &permit)
            .await?;
        storage.save_frame_authorized(at, b"older", &permit).await?;
        assert_bytes(
            storage.load_latest_frame_authorized(&permit).await?,
            b"newer",
        );
        assert!(storage.delete_all_frames().await? > 0);
        assert!(storage
            .load_latest_frame_authorized(&permit)
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn skipped_erasure_write_cannot_publish_latest_origin() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let deletion = Arc::new(AtomicBool::new(true));
        let mut inner = FrameFileStorage::new(dir.path().to_path_buf(), 100, 30).await?;
        inner.set_deletion_flag(deletion);
        let storage = bind_frame_storage(Arc::new(inner), None);
        let authority = PauseAuthority::running();
        let permit = authority.authorize(None)?;
        assert_capture_denied(
            storage
                .save_frame_authorized(chrono::Utc::now(), b"not published", &permit)
                .await
                .unwrap_err(),
            "frame write skipped during erasure",
        );
        assert!(storage
            .load_latest_frame_authorized(&permit)
            .await?
            .is_none());
        Ok(())
    }

    /// Delay a real file read after the bytes have been obtained, so only the
    /// wrapper's post-await checks can prevent their return to the consumer.
    struct DelayedRead {
        inner: Arc<dyn FrameStoragePort>,
        entered: Notify,
        release: Notify,
    }

    #[async_trait::async_trait]
    impl FrameStoragePort for DelayedRead {
        async fn save_frame_authorized(
            &self,
            at: chrono::DateTime<chrono::Utc>,
            data: &[u8],
            permit: &CapturePermit,
        ) -> Result<PathBuf, CoreError> {
            self.inner.save_frame_authorized(at, data, permit).await
        }

        async fn save_frame(
            &self,
            at: chrono::DateTime<chrono::Utc>,
            data: &[u8],
        ) -> Result<PathBuf, CoreError> {
            self.inner.save_frame(at, data).await
        }

        async fn save_frames_batch(
            &self,
            frames: Vec<(chrono::DateTime<chrono::Utc>, Vec<u8>)>,
        ) -> Vec<Result<PathBuf, CoreError>> {
            self.inner.save_frames_batch(frames).await
        }

        async fn load_frame(&self, path: &Path) -> Result<Vec<u8>, CoreError> {
            let bytes = self.inner.load_frame(path).await?;
            self.entered.notify_one();
            self.release.notified().await;
            Ok(bytes)
        }

        async fn load_latest_frame(&self) -> Result<Option<(Vec<u8>, String)>, CoreError> {
            self.inner.load_latest_frame().await
        }

        async fn enforce_retention(&self) -> Result<usize, CoreError> {
            self.inner.enforce_retention().await
        }

        async fn enforce_storage_limit(&self) -> Result<usize, CoreError> {
            self.inner.enforce_storage_limit().await
        }

        async fn delete_all_frames(&self) -> Result<usize, CoreError> {
            self.inner.delete_all_frames().await
        }
    }

    #[tokio::test]
    async fn original_and_consumer_revocation_during_read_each_reject_bytes() -> Result<()> {
        // Independent synthetic epochs isolate each post-read check. Native
        // scene and consent ownership are deliberately outside this fixture.
        for revoke_original in [true, false] {
            let dir = tempfile::tempdir()?;
            let delayed = Arc::new(DelayedRead {
                inner: real_storage(dir.path()).await?,
                entered: Notify::new(),
                release: Notify::new(),
            });
            let storage = bind_frame_storage(delayed.clone(), None);
            let original_authority = PauseAuthority::running();
            let consumer_authority = PauseAuthority::running();
            let original = original_authority.authorize(None)?;
            let consumer = consumer_authority.authorize(None)?;
            storage
                .save_frame_authorized(chrono::Utc::now(), b"read race marker", &original)
                .await?;
            let revoke = async {
                delayed.entered.notified().await;
                let authority = if revoke_original {
                    &original_authority
                } else {
                    &consumer_authority
                };
                authority.toggle_pause()?;
                authority.toggle_pause()?;
                delayed.release.notify_one();
                Ok::<(), CoreError>(())
            };
            let (result, revocation) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(storage.load_latest_frame_authorized(&consumer), revoke)
            })
            .await?;
            revocation?;
            assert_capture_denied(result.unwrap_err(), "capture pause epoch changed");

            let fresh_origin = original_authority.authorize(None)?;
            let fresh_consumer = consumer_authority.authorize(None)?;
            storage
                .save_frame_authorized(
                    chrono::Utc::now() + chrono::Duration::seconds(1),
                    b"fresh read race marker",
                    &fresh_origin,
                )
                .await?;
            delayed.release.notify_one();
            assert_bytes(
                storage
                    .load_latest_frame_authorized(&fresh_consumer)
                    .await?,
                b"fresh read race marker",
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn legacy_batch_preserves_each_result_path_and_real_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let storage = bind_frame_storage(real_storage(dir.path()).await?, None);
        let now = chrono::Utc::now();
        let markers = [
            b"batch first marker".to_vec(),
            b"batch second marker".to_vec(),
        ];
        let results = storage
            .save_frames_batch(vec![
                (now, markers[0].clone()),
                (now + chrono::Duration::seconds(1), markers[1].clone()),
            ])
            .await;
        assert_eq!(results.len(), 2);
        let mut paths = Vec::new();
        for (result, marker) in results.into_iter().zip(markers) {
            let path = result?;
            assert!(path.is_relative() && path.starts_with("frames"));
            let disk_path = dir.path().join(&path);
            assert!(disk_path.is_file());
            assert_eq!(tokio::fs::read(&disk_path).await?, marker);
            assert_eq!(storage.load_frame(&path).await?, marker);
            paths.push(path);
        }
        assert_ne!(paths[0], paths[1]);
        let authority = PauseAuthority::running();
        let consumer = authority.authorize(None)?;
        assert!(storage
            .load_latest_frame_authorized(&consumer)
            .await?
            .is_none());
        Ok(())
    }

    #[tokio::test]
    async fn retention_deletes_two_expired_files_and_preserves_fresh_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let inner = FrameFileStorage::new(dir.path().to_path_buf(), 100, 7).await?;
        let storage = bind_frame_storage(Arc::new(inner), None);
        let now = chrono::Utc::now();
        let old = now - chrono::Duration::days(10);
        let first = storage.save_frame(old, b"expired first").await?;
        let second = storage
            .save_frame(old + chrono::Duration::seconds(1), b"expired second")
            .await?;
        let fresh = storage.save_frame(now, b"fresh retained marker").await?;
        for relative in [&first, &second, &fresh] {
            assert!(relative.is_relative() && relative.starts_with("frames"));
        }
        let first_on_disk = dir.path().join(&first);
        let second_on_disk = dir.path().join(&second);
        let fresh_on_disk = dir.path().join(&fresh);
        assert!(first_on_disk.is_file() && second_on_disk.is_file() && fresh_on_disk.is_file());
        assert_eq!(storage.enforce_retention().await?, 2);
        assert!(!first_on_disk.exists());
        assert!(!second_on_disk.exists());
        assert!(fresh_on_disk.is_file());
        assert_eq!(
            tokio::fs::read(&fresh_on_disk).await?,
            b"fresh retained marker"
        );
        assert_eq!(storage.load_frame(&fresh).await?, b"fresh retained marker");
        assert_eq!(storage.enforce_retention().await?, 0);
        Ok(())
    }

    #[tokio::test]
    async fn storage_limit_deletes_two_oldest_files_and_preserves_exact_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let inner = FrameFileStorage::new(dir.path().to_path_buf(), 1, 30).await?;
        let storage = bind_frame_storage(Arc::new(inner), None);
        let now = chrono::Utc::now();
        const FRAME_BYTES: usize = 600 * 1024;
        let mut paths = Vec::new();
        for (days_old, marker) in [(3, 11), (2, 22), (1, 33)] {
            paths.push(
                storage
                    .save_frame(
                        now - chrono::Duration::days(days_old),
                        &vec![marker; FRAME_BYTES],
                    )
                    .await?,
            );
        }
        let mut disk_paths = Vec::new();
        for path in &paths {
            assert!(path.is_relative() && path.starts_with("frames"));
            let disk_path = dir.path().join(path);
            assert_eq!(
                tokio::fs::metadata(&disk_path).await?.len(),
                FRAME_BYTES as u64
            );
            disk_paths.push(disk_path);
        }
        // Retention's byte accounting removes oldest date directories until
        // <= 1 MiB: 1,843,200 - 2 * 614,400 = 614,400 bytes, so exactly two files.
        assert_eq!(storage.enforce_storage_limit().await?, 2);
        assert!(!disk_paths[0].exists());
        assert!(!disk_paths[1].exists());
        assert_eq!(
            tokio::fs::metadata(&disk_paths[2]).await?.len(),
            FRAME_BYTES as u64
        );
        let expected = vec![33; FRAME_BYTES];
        assert_eq!(tokio::fs::read(&disk_paths[2]).await?, expected);
        assert_eq!(storage.load_frame(&paths[2]).await?, expected);
        assert_eq!(storage.enforce_storage_limit().await?, 0);
        Ok(())
    }

    struct OcrGuard(Arc<AtomicBool>);

    impl CaptureGuard for OcrGuard {
        fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
            if matches!(access, CaptureAccess::Ocr) && !self.0.load(Ordering::Acquire) {
                return Err(capture_denied("synthetic OCR access revoked"));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn original_and_consumer_ocr_denial_each_gate_real_reads() -> Result<()> {
        for revoke_original in [true, false] {
            let dir = tempfile::tempdir()?;
            let delayed = Arc::new(DelayedRead {
                inner: real_storage(dir.path()).await?,
                entered: Notify::new(),
                release: Notify::new(),
            });
            let storage = bind_frame_storage(delayed.clone(), None);
            let original_allowed = Arc::new(AtomicBool::new(true));
            let consumer_allowed = Arc::new(AtomicBool::new(true));
            let original = CapturePermit::new(Arc::new(OcrGuard(original_allowed.clone())));
            let consumer = CapturePermit::new(Arc::new(OcrGuard(consumer_allowed.clone())));
            storage
                .save_frame_authorized(chrono::Utc::now(), b"OCR gated bytes", &original)
                .await?;
            let selected = if revoke_original {
                &original_allowed
            } else {
                &consumer_allowed
            };

            selected.store(false, Ordering::Release);
            // Current-scene access still succeeds: only OCR access is revoked.
            original.check(CaptureAccess::CurrentScene)?;
            consumer.check(CaptureAccess::CurrentScene)?;
            let denied_before_read = tokio::time::timeout(
                Duration::from_secs(5),
                storage.load_latest_frame_authorized(&consumer),
            )
            .await?;
            assert_capture_denied(
                denied_before_read.unwrap_err(),
                "synthetic OCR access revoked",
            );

            selected.store(true, Ordering::Release);
            let revoke_after_read = async {
                delayed.entered.notified().await;
                selected.store(false, Ordering::Release);
                delayed.release.notify_one();
            };
            let (denied_after_read, ()) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(
                    storage.load_latest_frame_authorized(&consumer),
                    revoke_after_read
                )
            })
            .await?;
            assert_capture_denied(
                denied_after_read.unwrap_err(),
                "synthetic OCR access revoked",
            );
            original.check(CaptureAccess::CurrentScene)?;
            consumer.check(CaptureAccess::CurrentScene)?;

            selected.store(true, Ordering::Release);
            delayed.release.notify_one();
            assert_bytes(
                storage.load_latest_frame_authorized(&consumer).await?,
                b"OCR gated bytes",
            );
        }
        Ok(())
    }
}
