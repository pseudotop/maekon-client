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
        // #4928: ConsentManager 를 먼저 생성해 공유 erasure 차단 flag 를 확보한 뒤,
        // FrameFileStorage 가 Arc 로 감싸지기 *전에* install 한다 (set_deletion_flag
        // 는 &mut self). 동일 Arc 를 SqliteStorage 에도 install 하는 것은 상위
        // composition root(app_runtime_launch)가 담당한다.
        let consent_manager = Arc::new(ConsentManager::new(data_dir.join("consent.json")));

        let mut frame_storage_concrete = FrameFileStorage::with_encryption(
            data_dir.to_path_buf(),
            config.storage.max_storage_mb,
            config.storage.retention_days,
            encryption_key,
        )
        .await?;
        // 공유 flag install (ptr-eq 로 ConsentManager / SQLite 와 연결됨).
        frame_storage_concrete.set_deletion_flag(consent_manager.deletion_flag());
        // #4928 round-3 (FIX B): 동일하게 `erasing` 신호도 install 한다(grant_consent 가
        // clear 할 수 없는 erase-window 차단 신호 — SQLite 와 동일 Arc).
        frame_storage_concrete.set_erasing(consent_manager.erasing());
        let frame_storage = Arc::new(frame_storage_concrete);

        let process_monitor: Arc<dyn ProcessMonitor> =
            Arc::new(maekon_monitor::process::ProcessTracker::new());
        let activity_monitor: Arc<dyn ActivityMonitor> = Arc::new(
            maekon_monitor::activity::ActivityTracker::new(process_monitor.clone()),
        );

        let ocr_tessdata = std::env::var("MAEKON_TESSDATA").ok().map(PathBuf::from);
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
            accessibility_extractor: maekon_vision::accessibility::create_extractor(),
            // #4928: 위에서 frame_storage 에 flag 를 install 한 것과 동일 인스턴스.
            consent_manager,
        })
    }
}
