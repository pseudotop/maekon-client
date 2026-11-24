use super::*;
use base64::Engine;
use image::{DynamicImage, RgbaImage};

fn make_test_image(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(
        w,
        h,
        image::Rgba([100, 150, 200, 255]),
    ))
}

#[test]
fn processor_creation() {
    let proc = EdgeFrameProcessor::new(480, 270, None);
    assert_eq!(proc.thumbnail_width, 480);
    assert_eq!(proc.thumbnail_height, 270);
    assert!(proc.prev_frame.lock().unwrap().is_none());
}

// #7602 (audit ①): the missing-OCR-capability warning must fire ONLY when
// OCR was actually requested but produced no active source (the platform
// capability gap — e.g. Linux) — never when OCR was simply not requested
// (consent withheld, which also yields `ocr_active == false` but is an
// expected, silent case). This truth table has no prior equivalent: before
// #7602 the pipeline never distinguished these two "empty OCR" causes at
// all, so this predicate (and therefore this test) did not exist.
#[test]
fn ocr_unavailable_warning_fires_only_when_requested_but_inactive() {
    assert!(
        should_warn_ocr_platform_unavailable(true, false),
        "OCR requested but no source ran (platform gap) must warn"
    );
    assert!(
        !should_warn_ocr_platform_unavailable(true, true),
        "OCR requested and a source ran must not warn"
    );
    assert!(
        !should_warn_ocr_platform_unavailable(false, false),
        "OCR not requested (consent withheld) must stay silent even though ocr_active is also false"
    );
    assert!(
        !should_warn_ocr_platform_unavailable(false, true),
        "OCR not requested must stay silent regardless of ocr_active"
    );
}

// Smoke test: the `Once`-guarded logging side effect itself must not
// panic when invoked (repeatedly, from concurrent-looking call sites)
// even though its actual firing is exercised indirectly via the
// predicate above and the pipeline integration tests below.
#[test]
fn ocr_unavailable_warning_once_does_not_panic_on_repeated_calls() {
    warn_ocr_platform_unavailable_once();
    warn_ocr_platform_unavailable_once();
}

#[test]
fn full_frame_encoding_high_importance() {
    let img = make_test_image(640, 480);
    let encoded = encoder::encode_webp_base64(&img, WebPQuality::High).unwrap();
    assert!(!encoded.is_empty());
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .unwrap();
    assert!(!decoded.is_empty());
}

#[test]
fn delta_encoding_medium_importance() {
    let img1 = make_test_image(100, 100);
    let img2 = DynamicImage::ImageRgba8(RgbaImage::from_pixel(
        100,
        100,
        image::Rgba([200, 50, 50, 255]),
    ));
    let result = delta::compute_delta(&img1, &img2);
    assert!(result.is_some());
    let dr = result.unwrap();
    assert!(dr.changed_ratio > 0.0);
}

#[test]
fn thumbnail_generation_low_importance() {
    let img = make_test_image(1920, 1080);
    let thumb = thumbnail::fast_resize(&img, 480, 270).unwrap();
    assert_eq!(thumb.width(), 480);
    assert_eq!(thumb.height(), 270);
}

#[test]
fn privacy_sanitization_in_pipeline() {
    let title = "Login - admin@company.com - Firefox";
    let sanitized = privacy::sanitize_title(title);
    assert!(sanitized.contains("[EMAIL]"));
    assert!(!sanitized.contains("admin@company.com"));
}

fn region(text: &str) -> maekon_core::models::frame::OcrRegion {
    maekon_core::models::frame::OcrRegion {
        text: text.to_string(),
        bbox: maekon_core::models::frame::BoundingBox {
            x: 0,
            y: 0,
            width: 100,
            height: 20,
        },
        confidence: 0.9,
    }
}

// #6088: per-region OCR text must be masked at the source so that
// analyze_current_scene cannot expose raw PII via region text or GUI labels.
#[test]
fn ocr_regions_text_is_sanitized_at_source() {
    let regions = vec![
        region("contact admin@company.com now"),
        region("plain label"),
    ];
    let masked = sanitize_ocr_regions(regions, PiiFilterLevel::Standard);

    assert!(masked[0].text.contains("[EMAIL]"));
    assert!(!masked[0].text.contains("admin@company.com"));
    // Geometry and non-PII text are preserved unchanged.
    assert_eq!(masked[0].bbox.width, 100);
    assert_eq!(masked[1].text, "plain label");
}

#[test]
fn ocr_regions_respect_configured_pii_level() {
    // Off must leave region text untouched (explicit operator opt-out).
    let off = sanitize_ocr_regions(vec![region("admin@company.com")], PiiFilterLevel::Off);
    assert_eq!(off[0].text, "admin@company.com");
}

#[test]
fn with_pii_level_stores_configured_level() {
    let proc = EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Strict);
    assert_eq!(proc.pii_level, PiiFilterLevel::Strict);
    // The default constructor keeps the Standard policy.
    let default_proc = EdgeFrameProcessor::new(480, 270, None);
    assert_eq!(default_proc.pii_level, PiiFilterLevel::Standard);
}

// #6132: the legacy high-importance OCR branch must reuse the long-lived cached
// extractor (the same `Arc<OcrExtractor>`) rather than constructing a fresh
// one per frame, which would defeat the cached `LepTess` reuse. Cloning the
// shared `Arc` is what the branch does; verify it yields the same instance.
#[cfg(feature = "ocr")]
#[test]
fn ocr_extractor_is_shared_and_reused() {
    let proc = EdgeFrameProcessor::new(480, 270, None);
    let shared = proc
        .ocr_extractor
        .as_ref()
        .expect("processor builds a default OCR extractor");
    // The per-frame branch clones this Arc; cloning must alias the same
    // cached extractor instance, not allocate a new one.
    let cloned = proc
        .ocr_extractor
        .clone()
        .expect("clone keeps the extractor");
    assert!(
        Arc::ptr_eq(shared, &cloned),
        "cloned extractor must point at the same cached instance"
    );
    assert_eq!(
        Arc::strong_count(shared),
        2,
        "exactly the original field reference and our clone should exist"
    );
}

// #7477: shipped (non-leptess) builds run with `--features grpc`, which does
// not enable `ocr`. Before the fix the `#[cfg(not(feature = "ocr"))]` branch
// hardcoded empty OCR, so `capture_and_process` returned `ocr_regions: []`,
// `ocr_text: None`, `raw_rgba: None` on every frame — starving GUI element
// correlation and the ML crop classifier. These tests exercise the wired
// native OCR provider through the real high-importance pipeline branch.
#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
mod native_ocr_pipeline {
    use super::*;
    use maekon_core::ports::ocr_provider::{FakeOcrProvider, OcrResult};

    fn text_frame() -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(
            200,
            80,
            image::Rgba([255, 255, 255, 255]),
        ))
    }

    fn high_importance_request() -> CaptureRequest {
        CaptureRequest {
            trigger_type: "ErrorDetected".to_string(),
            importance: 0.9, // high → the full-frame + OCR branch
            app_name: "Terminal".to_string(),
            window_title: "session".to_string(),
            monitor_id: None,
            app_bundle_id: None,
            window_bounds: None,
            screen_scale_factor: None,
            ocr_processing_permitted: true,
        }
    }

    // Fails before the fix: with no OCR source wired into the processor, the
    // non-leptess pipeline returns unconditionally-empty OCR. Passes after:
    // the wired provider feeds ocr_regions + ocr_text + raw_rgba, with PII
    // masked at the source exactly like the leptess path.
    #[tokio::test]
    async fn shipped_pipeline_yields_masked_ocr_from_native_provider() {
        let provider = Arc::new(FakeOcrProvider::new(vec![OcrResult {
            text: "contact admin@company.com".to_string(),
            x: 10,
            y: 20,
            width: 180,
            height: 24,
            confidence: 0.95,
        }]));
        let processor =
            EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Standard)
                .with_ocr_provider(Some(provider));

        let frame = processor
            .process_captured_frame(text_frame(), &high_importance_request(), None)
            .await
            .expect("processing a high-importance frame must succeed");

        // The bug was empty regions in every shipped build; now populated.
        assert_eq!(
            frame.ocr_regions.len(),
            1,
            "the wired native OCR provider must feed ocr_regions in a non-leptess build"
        );

        // PII is masked at the source (not weakened relative to leptess).
        let region_text = &frame.ocr_regions[0].text;
        assert!(
            region_text.contains("[EMAIL]"),
            "region text must be PII-masked, got {region_text:?}"
        );
        assert!(!region_text.contains("admin@company.com"));

        // The full-frame payload carries a non-empty, masked OCR text hint.
        match frame
            .image_payload
            .expect("importance >= 0.8 must yield an image payload")
        {
            ImagePayload::Full { ocr_text, .. } => {
                let text = ocr_text.expect("native OCR must produce a page text hint");
                assert!(
                    text.contains("[EMAIL]"),
                    "hint must be masked, got {text:?}"
                );
                assert!(!text.contains("admin@company.com"));
            }
            other => panic!("expected ImagePayload::Full, got {other:?}"),
        }

        // raw_rgba is populated for ML crop data, decoupled from region count.
        let rgba = frame
            .raw_rgba
            .expect("raw_rgba must be populated when OCR is active");
        assert_eq!(rgba.len(), 200 * 80 * 4);
    }

    // Without a provider wired (e.g. Linux, where create_native_ocr() is
    // None), the pipeline stays empty — proving the provider wiring is the
    // load-bearing change, not an unrelated code path.
    #[tokio::test]
    async fn shipped_pipeline_without_provider_is_empty() {
        let processor =
            EdgeFrameProcessor::with_pii_level(480, 270, None, PiiFilterLevel::Standard)
                .with_ocr_provider(None);

        let frame = processor
            .process_captured_frame(text_frame(), &high_importance_request(), None)
            .await
            .expect("processing must succeed even without an OCR provider");

        assert!(
            frame.ocr_regions.is_empty(),
            "no provider wired → no OCR regions"
        );
        assert!(
            frame.raw_rgba.is_none(),
            "no OCR attempt → no raw_rgba retained"
        );
    }
}

use maekon_core::config::AppConfig;
use maekon_core::error_codes::PermissionCode;
use maekon_core::models::context::{WindowBounds, WindowInfo};
use maekon_core::ports::vision::{CaptureAccess, CaptureGuard};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

struct EpochCaptureGuard {
    bound_epoch: u64,
    live_epoch: Arc<AtomicU64>,
    snapshot: Arc<AppConfig>,
    live_config: Arc<Mutex<Arc<AppConfig>>>,
    ocr_allowed: Arc<AtomicBool>,
}

impl CaptureGuard for EpochCaptureGuard {
    fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
        if self.live_epoch.load(Ordering::SeqCst) != self.bound_epoch {
            return Err(capture_denied("synthetic original grant revoked"));
        }
        if !Arc::ptr_eq(&self.snapshot, &self.live_config.lock().unwrap()) {
            return Err(capture_denied("synthetic original config replaced"));
        }
        match access {
            CaptureAccess::Ocr if self.ocr_allowed.load(Ordering::SeqCst) => Ok(()),
            CaptureAccess::Acquire { display_id } if display_id != 37 => {
                Err(capture_denied("synthetic display mismatch"))
            }
            CaptureAccess::Ocr | CaptureAccess::FullText => {
                Err(capture_denied("synthetic text consent withheld"))
            }
            _ => Ok(()),
        }
    }

    fn display_id(&self) -> Option<u32> {
        Some(37)
    }

    fn config_snapshot(&self) -> Option<Arc<AppConfig>> {
        Some(self.snapshot.clone())
    }
}

struct RecordingCaptureAuthority {
    permit: Mutex<CapturePermit>,
    requests: Mutex<Vec<Option<WindowInfo>>>,
    refuses: bool,
}

impl CaptureAuthority for RecordingCaptureAuthority {
    fn authorize(&self, expected: Option<&WindowInfo>) -> Result<CapturePermit, CoreError> {
        self.requests.lock().unwrap().push(expected.cloned());
        if self.refuses {
            return Err(CoreError::PermissionDenied {
                code: PermissionCode::PermissionDenied,
                message: "synthetic platform permission unavailable".into(),
            });
        }
        Ok(self.permit.lock().unwrap().clone())
    }
}

struct AuthorityPixelSource {
    calls: Arc<AtomicUsize>,
}

impl CaptureSource for AuthorityPixelSource {
    fn capture(
        &self,
        _bounds: Option<&WindowBounds>,
        permit: &CapturePermit,
    ) -> Result<(DynamicImage, Option<usize>), CoreError> {
        let pixels = crate::capture::capture_pixels_authorized(37, permit, &mut || {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(make_test_image(16, 8))
        })?;
        Ok((pixels, Some(0)))
    }
}

struct ProcessorAuthorityFixture {
    authority: Arc<RecordingCaptureAuthority>,
    live_epoch: Arc<AtomicU64>,
    snapshot: Arc<AppConfig>,
    pixel_calls: Arc<AtomicUsize>,
    live_config: Arc<Mutex<Arc<AppConfig>>>,
    ocr_allowed: Arc<AtomicBool>,
}

impl ProcessorAuthorityFixture {
    fn new(refuses: bool) -> Self {
        let live_epoch = Arc::new(AtomicU64::new(1));
        let mut config = AppConfig::default_config();
        config.privacy.pii_filter_level = PiiFilterLevel::Strict;
        config.vision.thumbnail_width = 8;
        config.vision.thumbnail_height = 4;
        let snapshot = Arc::new(config);
        let live_config = Arc::new(Mutex::new(snapshot.clone()));
        let ocr_allowed = Arc::new(AtomicBool::new(false));
        let permit = CapturePermit::new(Arc::new(EpochCaptureGuard {
            bound_epoch: 1,
            live_epoch: live_epoch.clone(),
            snapshot: snapshot.clone(),
            live_config: live_config.clone(),
            ocr_allowed: ocr_allowed.clone(),
        }));
        Self {
            authority: Arc::new(RecordingCaptureAuthority {
                permit: Mutex::new(permit),
                requests: Mutex::new(Vec::new()),
                refuses,
            }),
            live_epoch,
            snapshot,
            pixel_calls: Arc::new(AtomicUsize::new(0)),
            live_config,
            ocr_allowed,
        }
    }

    fn processor(&self) -> EdgeFrameProcessor {
        EdgeFrameProcessor::new(8, 4, None).with_capture_source(Arc::new(AuthorityPixelSource {
            calls: self.pixel_calls.clone(),
        }))
    }

    fn replace_grant(&self, epoch: u64) {
        self.live_epoch.store(epoch, Ordering::SeqCst);
        *self.authority.permit.lock().unwrap() = CapturePermit::new(Arc::new(EpochCaptureGuard {
            bound_epoch: epoch,
            live_epoch: self.live_epoch.clone(),
            snapshot: self.snapshot.clone(),
            live_config: self.live_config.clone(),
            ocr_allowed: self.ocr_allowed.clone(),
        }));
    }
}

fn authority_window() -> WindowInfo {
    WindowInfo {
        title: "synthetic foreground".into(),
        app_name: "synthetic app".into(),
        app_bundle_id: Some("test.capture.origin".into()),
        pid: 314,
        bounds: Some(WindowBounds {
            x: -160,
            y: 20,
            width: 640,
            height: 480,
        }),
    }
}

fn guarded_request(importance: f32) -> CaptureRequest {
    CaptureRequest {
        trigger_type: "synthetic".into(),
        importance,
        app_name: "synthetic".into(),
        window_title: "synthetic".into(),
        monitor_id: Some(999),
        app_bundle_id: None,
        window_bounds: None,
        screen_scale_factor: None,
        ocr_processing_permitted: false,
    }
}

#[tokio::test]
async fn full_frame_guarded_missing_policy_never_acquires() {
    struct NoSnapshot;
    impl CaptureGuard for NoSnapshot {
        fn check(&self, _: CaptureAccess) -> Result<(), CoreError> {
            Ok(())
        }
    }
    let f = ProcessorAuthorityFixture::new(false);
    let permit = CapturePermit::new(Arc::new(NoSnapshot));
    let error = f
        .processor()
        .capture_authorized(&guarded_request(0.6), &permit)
        .await
        .unwrap_err();
    assert_capture_privacy_error(error, "capture denied: capture policy snapshot unavailable");
    assert_eq!(f.pixel_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn full_frame_guarded_isolates_legacy_delta_and_thumbnail_caches() {
    let f = ProcessorAuthorityFixture::new(false);
    let mut p = f.processor();
    p.thumbnail_width = 2;
    p.thumbnail_height = 1;
    let baseline = Arc::new(make_test_image(16, 8));
    *p.prev_frame.lock().unwrap() = Some(baseline.clone());
    let permit = f.authority.authorize(None).unwrap();
    for importance in [0.9, 0.6, 0.4] {
        let out = p
            .capture_authorized(&guarded_request(importance), &permit)
            .await
            .unwrap();
        assert_eq!(out.metadata.monitor_id, Some(0));
        let (data, size) = match out.image_payload.unwrap() {
            ImagePayload::Full { data, .. } => (data, (16, 8)),
            ImagePayload::Thumbnail {
                data,
                width,
                height,
            } => (data, (width, height)),
            other => panic!("guarded capture reused a delta: {other:?}"),
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(data)
            .unwrap();
        let decoded = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), size);
        assert_eq!(size, if importance < 0.5 { (8, 4) } else { (16, 8) });
        for pixel in decoded.pixels() {
            for (value, expected) in pixel.0[..3].iter().zip([100i16, 150, 200]) {
                assert!((i16::from(*value) - expected).abs() <= 8);
            }
        }
        assert!(Arc::ptr_eq(
            p.prev_frame.lock().unwrap().as_ref().unwrap(),
            &baseline
        ));
    }
}

#[tokio::test]
async fn full_frame_guarded_regrant_and_config_aba_never_revive_origin() {
    let f = ProcessorAuthorityFixture::new(false);
    let p = f.processor();
    let original = f.authority.authorize(None).unwrap();
    p.capture_authorized(&guarded_request(0.6), &original)
        .await
        .unwrap();
    f.replace_grant(2);
    assert_capture_privacy_error(
        p.capture_authorized(&guarded_request(0.6), &original)
            .await
            .unwrap_err(),
        "capture denied: synthetic original grant revoked",
    );
    assert_eq!(f.pixel_calls.load(Ordering::SeqCst), 1);
    let fresh = f.authority.authorize(None).unwrap();
    p.capture_authorized(&guarded_request(0.6), &fresh)
        .await
        .unwrap();
    // Equal settings are still a distinct published policy identity.
    *f.live_config.lock().unwrap() = Arc::new((*f.snapshot).clone());
    assert_capture_privacy_error(
        p.capture_authorized(&guarded_request(0.6), &fresh)
            .await
            .unwrap_err(),
        "capture denied: synthetic original config replaced",
    );
    assert_eq!(f.pixel_calls.load(Ordering::SeqCst), 2);
    assert!(p.prev_frame.lock().unwrap().is_none());
}

#[test]
fn full_frame_guarded_queued_old_grant_never_acquires() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let f = ProcessorAuthorityFixture::new(false);
    let p = Arc::new(f.processor());
    let permit = f.authority.authorize(None).unwrap();
    let (release, held) = std::sync::mpsc::channel();
    let (started, observed) = std::sync::mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        started.send(()).unwrap();
        held.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
    });
    observed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let (queued, received) = tokio::sync::oneshot::channel();
    let processor = p.clone();
    let task = runtime.spawn(async move {
        queued.send(()).unwrap();
        processor
            .capture_authorized(&guarded_request(0.6), &permit)
            .await
    });
    runtime.block_on(received).unwrap();
    f.replace_grant(2);
    release.send(()).unwrap();
    runtime.block_on(blocker).unwrap();
    assert_capture_privacy_error(
        runtime.block_on(task).unwrap().unwrap_err(),
        "capture denied: synthetic original grant revoked",
    );
    assert_eq!(f.pixel_calls.load(Ordering::SeqCst), 0);
    assert!(p.prev_frame.lock().unwrap().is_none());
    let fresh = f.authority.authorize(None).unwrap();
    runtime
        .block_on(p.capture_authorized(&guarded_request(0.6), &fresh))
        .unwrap();
    assert_eq!(f.pixel_calls.load(Ordering::SeqCst), 1);
}

#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
struct PendingOcr {
    calls: AtomicUsize,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    wait: bool,
}

#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
#[async_trait]
impl maekon_core::ports::ocr_provider::OcrProvider for PendingOcr {
    async fn extract_elements(
        &self,
        image: &[u8],
        format: &str,
    ) -> Result<Vec<maekon_core::ports::ocr_provider::OcrResult>, CoreError> {
        use maekon_core::ports::ocr_provider::{FakeOcrProvider, OcrResult};
        assert_eq!(image::load_from_memory(image).unwrap().width(), 16);
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.wait {
            self.release.notified().await;
        }
        FakeOcrProvider::new(vec![OcrResult {
            text: "fixture@example.test".into(),
            x: 0,
            y: 0,
            width: 8,
            height: 4,
            confidence: 0.99,
        }])
        .extract_elements(image, format)
        .await
    }
    fn provider_name(&self) -> &str {
        "pending-synthetic-ocr"
    }
    fn is_external(&self) -> bool {
        false
    }
}

#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
fn pending_ocr(wait: bool) -> Arc<PendingOcr> {
    Arc::new(PendingOcr {
        calls: AtomicUsize::new(0),
        entered: Default::default(),
        release: Default::default(),
        wait,
    })
}

#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
#[tokio::test]
async fn full_frame_guarded_ocr_uses_origin_policy_and_own_consent() {
    let f = ProcessorAuthorityFixture::new(false);
    let provider = pending_ocr(false);
    let mut p = f.processor().with_ocr_provider(Some(provider.clone()));
    p.pii_level = PiiFilterLevel::Off;
    let permit = f.authority.authorize(None).unwrap();
    let mut request = guarded_request(0.9);
    request.ocr_processing_permitted = true;
    assert_capture_privacy_error(
        p.capture_authorized(&request, &permit).await.unwrap_err(),
        "capture denied: synthetic text consent withheld",
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    f.ocr_allowed.store(true, Ordering::SeqCst);
    let out = p.capture_authorized(&request, &permit).await.unwrap();
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert_eq!(out.ocr_regions.len(), 1);
    assert!(!out.ocr_regions[0].text.contains("fixture@example.test"));
    assert!(out.raw_rgba.is_some());
    assert!(p.prev_frame.lock().unwrap().is_none());
}

#[cfg(all(feature = "native-vision", not(feature = "ocr")))]
#[tokio::test]
async fn full_frame_guarded_discards_pending_ocr_after_origin_changes() {
    for change in ["regrant", "config", "ocr"] {
        let f = ProcessorAuthorityFixture::new(false);
        f.ocr_allowed.store(true, Ordering::SeqCst);
        let provider = pending_ocr(true);
        let p = Arc::new(f.processor().with_ocr_provider(Some(provider.clone())));
        let permit = f.authority.authorize(None).unwrap();
        let processor = p.clone();
        let task = tokio::spawn(async move {
            let mut request = guarded_request(0.9);
            request.ocr_processing_permitted = true;
            processor.capture_authorized(&request, &permit).await
        });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            provider.entered.notified(),
        )
        .await
        .unwrap();
        match change {
            "regrant" => f.replace_grant(2),
            "config" => *f.live_config.lock().unwrap() = Arc::new((*f.snapshot).clone()),
            _ => f.ocr_allowed.store(false, Ordering::SeqCst),
        }
        provider.release.notify_one();
        let expected = match change {
            "regrant" => "capture denied: synthetic original grant revoked",
            "config" => "capture denied: synthetic original config replaced",
            _ => "capture denied: synthetic text consent withheld",
        };
        assert_capture_privacy_error(task.await.unwrap().unwrap_err(), expected);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
        assert!(p.prev_frame.lock().unwrap().is_none());
    }
}

fn assert_capture_privacy_error(error: CoreError, expected_message: &str) {
    match error {
        CoreError::PermissionDenied { code, message } => {
            assert_eq!(code, PermissionCode::PrivacyDenied);
            assert_eq!(message, expected_message);
        }
        other => panic!("expected capture privacy denial, got {other:?}"),
    }
}

#[test]
fn begin_capture_without_authority_denies_before_pixel_source() {
    let fixture = ProcessorAuthorityFixture::new(false);
    let processor = fixture.processor();
    for expected in [None, Some(&authority_window())] {
        assert_capture_privacy_error(
            processor.begin_capture(expected).unwrap_err(),
            "capture denied: capture authority unavailable",
        );
    }
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 0);
    assert!(fixture.authority.requests.lock().unwrap().is_empty());
}

#[test]
fn begin_capture_builder_is_inert_until_authorization() {
    let fixture = ProcessorAuthorityFixture::new(false);
    let processor = fixture
        .processor()
        .with_capture_authority(fixture.authority.clone());
    assert!(fixture.authority.requests.lock().unwrap().is_empty());
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 0);
    processor.begin_capture(None).unwrap();
    assert_eq!(fixture.authority.requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn begin_capture_forwards_complete_expected_window_and_original_permit() {
    let fixture = ProcessorAuthorityFixture::new(false);
    let processor = fixture
        .processor()
        .with_capture_authority(fixture.authority.clone());
    let consumer: &dyn FrameProcessor = &processor;
    let window = authority_window();
    for expected in [None, Some(&window)] {
        let permit = consumer.begin_capture(expected).unwrap();
        assert_eq!(permit.display_id(), Some(37));
        let snapshot = permit.config_snapshot().unwrap();
        assert!(Arc::ptr_eq(&snapshot, &fixture.snapshot));
        assert_eq!(snapshot.privacy.pii_filter_level, PiiFilterLevel::Strict);
        permit
            .check(CaptureAccess::Acquire { display_id: 37 })
            .unwrap();
        for access in [CaptureAccess::Ocr, CaptureAccess::FullText] {
            assert_capture_privacy_error(
                permit.check(access).unwrap_err(),
                "capture denied: synthetic text consent withheld",
            );
        }
    }
    assert_eq!(
        serde_json::to_value(&*fixture.authority.requests.lock().unwrap()).unwrap(),
        serde_json::to_value(vec![None, Some(window)]).unwrap(),
    );
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn begin_capture_preserves_authority_error() {
    let fixture = ProcessorAuthorityFixture::new(true);
    let processor = fixture
        .processor()
        .with_capture_authority(fixture.authority.clone());
    let window = authority_window();
    match processor.begin_capture(Some(&window)).unwrap_err() {
        CoreError::PermissionDenied { code, message } => {
            assert_eq!(code, PermissionCode::PermissionDenied);
            assert_eq!(message, "synthetic platform permission unavailable");
        }
        other => panic!("expected original authority error, got {other:?}"),
    }
    assert_eq!(fixture.authority.requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn begin_capture_replacement_selects_latest_authority() {
    let original = ProcessorAuthorityFixture::new(true);
    let replacement = ProcessorAuthorityFixture::new(false);
    let processor = original
        .processor()
        .with_capture_authority(original.authority.clone())
        .with_capture_authority(replacement.authority.clone());
    let permit = processor.begin_capture(None).unwrap();
    assert!(original.authority.requests.lock().unwrap().is_empty());
    assert_eq!(replacement.authority.requests.lock().unwrap().len(), 1);
    assert!(Arc::ptr_eq(
        &permit.config_snapshot().unwrap(),
        &replacement.snapshot,
    ));
    assert_eq!(original.pixel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(replacement.pixel_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn begin_capture_original_permit_survives_thumbnail_and_never_revives() {
    let fixture = ProcessorAuthorityFixture::new(false);
    let processor = fixture
        .processor()
        .with_capture_authority(fixture.authority.clone());
    let permit = processor.begin_capture(Some(&authority_window())).unwrap();
    let bytes = processor.thumbnail_authorized(None, &permit).await.unwrap();
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::WebP).unwrap();
    assert_eq!((image.width(), image.height()), (8, 4));
    for (actual, expected) in image
        .to_rgba8()
        .get_pixel(0, 0)
        .0
        .into_iter()
        .zip([100u8, 150, 200, 255])
    {
        assert!(
            actual.abs_diff(expected) <= 10,
            "encoded thumbnail must retain the producer pixels"
        );
    }
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 1);

    fixture.replace_grant(2);
    assert_capture_privacy_error(
        processor
            .thumbnail_authorized(None, &permit)
            .await
            .unwrap_err(),
        "capture denied: synthetic original grant revoked",
    );
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 1);
    let fresh = processor.begin_capture(None).unwrap();
    let fresh_bytes = processor.thumbnail_authorized(None, &fresh).await.unwrap();
    let fresh_image =
        image::load_from_memory_with_format(&fresh_bytes, image::ImageFormat::WebP).unwrap();
    assert_eq!((fresh_image.width(), fresh_image.height()), (8, 4));
    assert_eq!(fixture.pixel_calls.load(Ordering::SeqCst), 2);
    assert_capture_privacy_error(
        permit.check(CaptureAccess::Retain).unwrap_err(),
        "capture denied: synthetic original grant revoked",
    );
}
