use crate::capture_services::SharedCaptureServices;
use maekon_core::consent::ConsentManager;
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_storage::encryption::EncryptionKey;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub(super) struct CaptureLaunchWiring {
    pub(super) capture_paused: Arc<AtomicBool>,
    pub(super) indicator_visible: Arc<AtomicBool>,
    pub(super) detection_active: Arc<AtomicBool>,
    pub(super) shared_capture_services: Option<Arc<SharedCaptureServices>>,
    pub(super) consent_manager: Arc<dyn ConsentManagerPort>,
}

pub(super) fn build_capture_wiring(
    runtime_handle: &tokio::runtime::Handle,
    data_dir_path: &Path,
    config: &maekon_core::config::AppConfig,
    encryption_key: Option<Arc<EncryptionKey>>,
    cua_safe_mode: bool,
) -> CaptureLaunchWiring {
    let (capture_paused_initial, indicator_visible_initial) =
        super::cua_safe_mode::initial_capture_flags(config.indicator.show_border, cua_safe_mode);
    let capture_paused = Arc::new(AtomicBool::new(capture_paused_initial));
    let indicator_visible = Arc::new(AtomicBool::new(indicator_visible_initial));
    let detection_active = Arc::new(AtomicBool::new(false));

    let shared_capture_services = match runtime_handle.block_on(SharedCaptureServices::build(
        data_dir_path,
        config,
        encryption_key,
    )) {
        Ok(services) => Some(Arc::new(services)),
        Err(error) => {
            tracing::warn!("shared capture services init failed: {error}");
            None
        }
    };
    let consent_manager = shared_capture_services
        .as_ref()
        .map(|services| services.consent_manager.clone())
        .unwrap_or_else(|| Arc::new(ConsentManager::new(data_dir_path.join("consent.json"))));

    CaptureLaunchWiring {
        capture_paused,
        indicator_visible,
        detection_active,
        shared_capture_services,
        consent_manager,
    }
}

/// 스토리지 준비 직후(스케줄러/에이전트/웹 어댑터 기동 전) erasure 배선을 마무리한다.
///
/// (1) #4928: ConsentManager 의 공유 `deletion_flag` 를 LIVE `SqliteStorage` 에 install 한다.
///     `set_deletion_flag(&self)` 는 ArcSwap 셀을 교체하므로 이미 Arc 로 공유된 storage
///     (및 `connection_arc()` 를 공유하는 모든 어댑터)에 즉시 적용된다. 프레임 스토리지에는
///     `capture_services::build` 에서 동일 Arc 가 install 되어 있어, 셋(consent ↔ SQLite ↔
///     frames)이 동일 flag 를 공유한다. 이 시점은 모든 쓰기 어댑터 기동 전이다.
/// (2) #4801 GDPR Art. 17: 이전 기동에서 완료되지 않은 로컬 삭제 마커가 있으면 재시도한다.
pub(super) fn install_erasure_wiring(
    handle: &tokio::runtime::Handle,
    sqlite_storage: &Arc<maekon_storage::sqlite::SqliteStorage>,
    consent_manager: &Arc<dyn ConsentManagerPort>,
    shared_capture_services: &Option<Arc<SharedCaptureServices>>,
) {
    sqlite_storage.set_deletion_flag(consent_manager.deletion_flag());
    // #4928 round-3 (FIX B): erase-window 차단 신호 `erasing` 도 동일 Arc 로 install 한다.
    // 프레임 스토리지에는 `capture_services::build` 에서 동일 Arc 가 install 되어, 셋
    // (consent ↔ SQLite ↔ frames)이 동일 `erasing` 을 공유한다(ptr-eq).
    sqlite_storage.set_erasing(consent_manager.erasing());
    let retry_frame_storage = shared_capture_services
        .as_ref()
        .map(|s| s.frame_storage.clone());
    handle.block_on(crate::commands::consent::retry_pending_local_erase(
        sqlite_storage.clone(),
        retry_frame_storage,
    ));
}

#[cfg(test)]
mod tests {
    use super::CaptureLaunchWiring;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn capture_launch_wiring_exposes_shared_flags() {
        let wiring = CaptureLaunchWiring {
            capture_paused: Arc::new(AtomicBool::new(true)),
            indicator_visible: Arc::new(AtomicBool::new(false)),
            detection_active: Arc::new(AtomicBool::new(false)),
            shared_capture_services: None,
            consent_manager: Arc::new(maekon_core::consent::ConsentManager::new(
                std::path::PathBuf::from("consent.json"),
            )),
        };

        assert!(wiring.capture_paused.load(Ordering::SeqCst));
        assert!(!wiring.indicator_visible.load(Ordering::SeqCst));
        assert!(!wiring.detection_active.load(Ordering::SeqCst));
        assert!(wiring.shared_capture_services.is_none());
    }
}
