//! Screen capture and frame processing ports — defines contracts for
//! deciding when to capture (`CaptureTrigger`) and how to process frames
//! (`FrameProcessor`: full, delta, thumbnail, or metadata-only).
//! Implemented by `SmartCaptureTrigger` and `EdgeFrameProcessor` in `maekon-vision`.

use async_trait::async_trait;
use std::sync::Arc;

use crate::error::CoreError;
use crate::models::context::{WindowBounds, WindowInfo};
use crate::models::event::ContextEvent;
use crate::models::frame::ProcessedFrame;

pub trait CaptureTrigger: Send + Sync {
    fn should_capture(&self, event: &ContextEvent) -> Option<CaptureRequest>;
}

#[derive(Debug, Clone)]
pub struct CaptureRequest {
    pub trigger_type: String,
    pub importance: f32,
    pub app_name: String,
    pub window_title: String,
    pub monitor_id: Option<usize>,
    pub app_bundle_id: Option<String>,
    /// Active window bounds for multi-monitor capture targeting.
    /// When set, the frame processor captures the monitor containing
    /// the window instead of always using the primary monitor.
    pub window_bounds: Option<WindowBounds>,
    /// Physical-to-logical scale for the capture source.
    ///
    /// `None` or `Some(1.0)` keeps OCR boxes in source pixels. HiDPI callers can
    /// inject `Some(2.0)` so OCR boxes align with logical window coordinates.
    pub screen_scale_factor: Option<f64>,
    /// Whether OCR/text-region processing is allowed for this capture.
    ///
    /// Screen capture consent permits image capture, but OCR processing requires
    /// its own consent gate. Callers must pass the current effective
    /// `ocr_processing` decision before invoking a `FrameProcessor`.
    pub ocr_processing_permitted: bool,
}

/// A capture's authority must survive awaits and remain revocable. Retaining an
/// older valid frame does not require its window to remain foreground.
#[derive(Debug, Clone, Copy)]
pub enum CaptureAccess {
    Retain,
    Acquire { display_id: u32 },
    CurrentScene,
    Ocr,
    FullText,
}

pub trait CaptureGuard: Send + Sync {
    fn check(&self, access: CaptureAccess) -> Result<(), CoreError>;
    fn display_id(&self) -> Option<u32> {
        None
    }
    fn config_snapshot(&self) -> Option<Arc<crate::config::AppConfig>> {
        None
    }
}

/// Non-serializable authority bound to the original policy and consent epoch.
/// A fresh grant must never authorize a frame from an earlier grant (#12018).
#[derive(Clone)]
pub struct CapturePermit(Arc<dyn CaptureGuard>);

impl std::fmt::Debug for CapturePermit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CapturePermit").finish_non_exhaustive()
    }
}

impl CapturePermit {
    pub fn new(guard: Arc<dyn CaptureGuard>) -> Self {
        Self(guard)
    }

    pub fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
        self.0.check(access)
    }

    pub fn display_id(&self) -> Option<u32> {
        self.0.display_id()
    }

    pub fn config_snapshot(&self) -> Option<Arc<crate::config::AppConfig>> {
        self.0.config_snapshot()
    }
}

pub trait CaptureAuthority: Send + Sync {
    /// Bind the observed native foreground identity before work is queued.
    /// An absent expectation still requires a successful live observation.
    fn authorize(&self, expected: Option<&WindowInfo>) -> Result<CapturePermit, CoreError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureObservationError {
    Unsupported,
    ForegroundUnavailable,
    EnumerationFailed,
    OwnerUnavailable,
    TitleUnavailable,
    GeometryUnavailable,
}

/// Window/display bounds in one platform coordinate space. The native adapter
/// resolves the actual xcap display ID; consumers never mix logical and pixel
/// coordinates to determine which background windows share the capture.
#[derive(Debug, Clone)]
pub struct CaptureWindow {
    pub native_id: u64,
    pub pid: u32,
    pub app_name: String,
    pub title: String,
    pub bounds: WindowBounds,
}

#[derive(Debug, Clone)]
pub struct CaptureScene {
    pub foreground: CaptureWindow,
    pub display_id: u32,
    pub display_bounds: WindowBounds,
    pub windows: Vec<CaptureWindow>,
}

pub trait CaptureObserver: Send + Sync {
    fn observe(&self, display_id: Option<u32>) -> Result<CaptureScene, CaptureObservationError>;
}

pub fn capture_denied(reason: &str) -> CoreError {
    CoreError::PermissionDenied {
        code: crate::error_codes::PermissionCode::PrivacyDenied,
        message: format!("capture denied: {reason}"),
    }
}

/// Captures and processes screen frames based on importance level.
///
/// # Errors
/// Methods return `CoreError::PermissionDenied` (wire:
/// `permission.permission_denied`) when screen-capture permission is
/// missing — emitted by the accessibility adapter before the
/// FrameProcessor is called. `CoreError::Internal` (wire:
/// `internal.generic`) on intra-process failures such as mutex
/// poisoning or image-buffer allocation errors.
/// `CoreError::OcrError` (wire: `provider.ocr_failed`) on OCR extraction
/// failure.
#[async_trait]
pub trait FrameProcessor: Send + Sync {
    fn begin_capture(&self, expected: Option<&WindowInfo>) -> Result<CapturePermit, CoreError> {
        let _ = expected;
        Err(capture_denied("capture authority unavailable"))
    }

    /// App capture paths retain this same permit through ring/storage/results.
    /// Adapters must explicitly implement the guarded acquisition boundary.
    async fn capture_authorized(
        &self,
        request: &CaptureRequest,
        permit: &CapturePermit,
    ) -> Result<ProcessedFrame, CoreError> {
        let _ = (request, permit);
        Err(capture_denied("authorized frame capture unavailable"))
    }

    async fn thumbnail_authorized(
        &self,
        window_bounds: Option<&WindowBounds>,
        permit: &CapturePermit,
    ) -> Result<Vec<u8>, CoreError> {
        let _ = (window_bounds, permit);
        Err(capture_denied("authorized thumbnail capture unavailable"))
    }

    async fn capture_and_process(
        &self,
        capture_request: &CaptureRequest,
    ) -> Result<ProcessedFrame, CoreError>;

    /// Capture a lightweight thumbnail for ring buffer use.
    ///
    /// `window_bounds`, when set, targets the monitor containing the active
    /// window so multi-monitor dashcam pre/post-event frames capture the
    /// correct display instead of always the primary one (#8054 P2-3). `None`
    /// falls back to the primary monitor. Returns raw WebP bytes at low
    /// quality. Default returns unsupported error.
    async fn capture_thumbnail(
        &self,
        window_bounds: Option<&WindowBounds>,
    ) -> Result<Vec<u8>, CoreError> {
        let _ = window_bounds;
        Err(CoreError::Internal {
            code: crate::error_codes::InternalCode::Generic,
            message: "thumbnail capture not supported".to_string(),
        })
    }
}

#[cfg(test)]
mod capture_contract_tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::error_codes::PermissionCode;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    fn assert_denied<T>(result: Result<T, CoreError>) {
        match result {
            Err(CoreError::PermissionDenied { code, .. }) => {
                assert_eq!(code, PermissionCode::PrivacyDenied);
            }
            Err(error) => panic!("expected privacy denial, got {error}"),
            Ok(_) => panic!("an unauthorized capture contract returned success"),
        }
    }

    struct OriginGuard {
        allowed: Arc<AtomicBool>,
        config: Arc<AppConfig>,
    }

    impl CaptureGuard for OriginGuard {
        fn check(&self, _: CaptureAccess) -> Result<(), CoreError> {
            if self.allowed.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(capture_denied("synthetic grant revoked"))
            }
        }

        fn display_id(&self) -> Option<u32> {
            Some(37)
        }

        fn config_snapshot(&self) -> Option<Arc<AppConfig>> {
            Some(self.config.clone())
        }
    }

    struct MetadataUnavailable;

    impl CaptureGuard for MetadataUnavailable {
        fn check(&self, _: CaptureAccess) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[test]
    fn capture_permit_preserves_origin_and_revocation_across_clones() {
        let allowed = Arc::new(AtomicBool::new(true));
        let config = Arc::new(AppConfig::default_config());
        let permit = CapturePermit::new(Arc::new(OriginGuard {
            allowed: allowed.clone(),
            config: config.clone(),
        }));
        let cloned = permit.clone();
        for current in [&permit, &cloned] {
            assert_eq!(current.display_id(), Some(37));
            assert!(Arc::ptr_eq(&current.config_snapshot().unwrap(), &config));
            assert!(format!("{current:?}").contains("CapturePermit"));
            current
                .check(CaptureAccess::Acquire { display_id: 37 })
                .unwrap();
        }

        allowed.store(false, Ordering::Release);
        for access in [
            CaptureAccess::Retain,
            CaptureAccess::Acquire { display_id: 37 },
            CaptureAccess::CurrentScene,
            CaptureAccess::Ocr,
            CaptureAccess::FullText,
        ] {
            assert_denied(permit.check(access));
            assert_denied(cloned.check(access));
        }
    }

    #[test]
    fn a_guard_without_metadata_does_not_invent_a_display_or_policy() {
        let permit = CapturePermit::new(Arc::new(MetadataUnavailable));
        assert!(permit.display_id().is_none());
        assert!(permit.config_snapshot().is_none());
    }

    struct LegacyProcessor(AtomicUsize);

    #[async_trait]
    impl FrameProcessor for LegacyProcessor {
        async fn capture_and_process(
            &self,
            _: &CaptureRequest,
        ) -> Result<ProcessedFrame, CoreError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(capture_denied("legacy fixture entered"))
        }
    }

    #[tokio::test]
    async fn legacy_processors_reject_authorized_calls_without_delegation() {
        let processor = LegacyProcessor(AtomicUsize::new(0));
        let request = CaptureRequest {
            trigger_type: "contract-fixture".into(),
            importance: 1.0,
            app_name: "synthetic".into(),
            window_title: "synthetic".into(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
            ocr_processing_permitted: false,
        };
        assert_denied(processor.capture_and_process(&request).await);
        assert_eq!(processor.0.swap(0, Ordering::SeqCst), 1);

        let permit = CapturePermit::new(Arc::new(MetadataUnavailable));
        assert_denied(processor.begin_capture(None));
        assert_denied(processor.capture_authorized(&request, &permit).await);
        assert_denied(processor.thumbnail_authorized(None, &permit).await);
        assert_eq!(processor.0.load(Ordering::SeqCst), 0);
    }
}
