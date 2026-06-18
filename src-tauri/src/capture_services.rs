use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use maekon_core::config::AppConfig;
use maekon_core::consent::ConsentManager;
use maekon_core::ports::accessibility::AccessibilityExtractor;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::monitor::{ActivityMonitor, ProcessMonitor};
use maekon_core::ports::vision::FrameProcessor;
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
        let frame_storage = Arc::new(frame_storage_concrete);

        let process_monitor: Arc<dyn ProcessMonitor> =
            Arc::new(maekon_monitor::process::ProcessTracker::new());
        let activity_monitor: Arc<dyn ActivityMonitor> = Arc::new(
            maekon_monitor::activity::ActivityTracker::new(process_monitor.clone()),
        );

        let ocr_tessdata = std::env::var("MAEKON_TESSDATA").ok().map(PathBuf::from);
        let frame_processor: Arc<dyn FrameProcessor> =
            Arc::new(EdgeFrameProcessor::with_pii_level(
                config.vision.thumbnail_width,
                config.vision.thumbnail_height,
                ocr_tessdata,
                config.privacy.pii_filter_level,
            ));

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
