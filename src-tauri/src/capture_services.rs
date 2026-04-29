use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use oneshim_core::config::AppConfig;
use oneshim_core::consent::ConsentManager;
use oneshim_core::ports::accessibility::AccessibilityExtractor;
use oneshim_core::ports::frame_storage::FrameStoragePort;
use oneshim_core::ports::monitor::{ActivityMonitor, ProcessMonitor};
use oneshim_core::ports::vision::FrameProcessor;
use oneshim_storage::encryption::EncryptionKey;
use oneshim_storage::frame_storage::FrameFileStorage;
use oneshim_vision::processor::EdgeFrameProcessor;

pub(crate) struct SharedCaptureServices {
    pub(crate) frame_storage: Arc<dyn FrameStoragePort>,
    pub(crate) process_monitor: Arc<dyn ProcessMonitor>,
    pub(crate) activity_monitor: Arc<dyn ActivityMonitor>,
    pub(crate) frame_processor: Arc<dyn FrameProcessor>,
    pub(crate) accessibility_extractor: Option<Arc<dyn AccessibilityExtractor>>,
    pub(crate) consent_manager: Arc<ConsentManager>,
}

impl SharedCaptureServices {
    pub(crate) async fn build(
        data_dir: &Path,
        config: &AppConfig,
        encryption_key: Option<Arc<EncryptionKey>>,
    ) -> Result<Self> {
        let frame_storage = Arc::new(
            FrameFileStorage::with_encryption(
                data_dir.to_path_buf(),
                config.storage.max_storage_mb,
                config.storage.retention_days,
                encryption_key,
            )
            .await?,
        );

        let process_monitor: Arc<dyn ProcessMonitor> =
            Arc::new(oneshim_monitor::process::ProcessTracker::new());
        let activity_monitor: Arc<dyn ActivityMonitor> = Arc::new(
            oneshim_monitor::activity::ActivityTracker::new(process_monitor.clone()),
        );

        let ocr_tessdata = std::env::var("ONESHIM_TESSDATA").ok().map(PathBuf::from);
        let frame_processor: Arc<dyn FrameProcessor> = Arc::new(EdgeFrameProcessor::new(
            config.vision.thumbnail_width,
            config.vision.thumbnail_height,
            ocr_tessdata,
        ));

        Ok(Self {
            frame_storage,
            process_monitor,
            activity_monitor,
            frame_processor,
            accessibility_extractor: oneshim_vision::accessibility::create_extractor(),
            consent_manager: Arc::new(ConsentManager::new(data_dir.join("consent.json"))),
        })
    }
}
