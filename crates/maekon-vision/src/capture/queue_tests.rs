use super::*;
use crate::processor::EdgeFrameProcessor;
use maekon_core::ports::vision::{capture_denied, CaptureGuard, FrameProcessor};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

const DISPLAY: u32 = 17;
const WAIT: Duration = Duration::from_secs(10);

struct Guard {
    current_scene: AtomicBool,
    acquire: AtomicBool,
    retain: AtomicBool,
    completion_fault: AtomicBool,
    retain_checks: AtomicUsize,
}

impl Guard {
    fn allowed() -> (Arc<Self>, CapturePermit) {
        let guard = Arc::new(Self {
            current_scene: AtomicBool::new(true),
            acquire: AtomicBool::new(true),
            retain: AtomicBool::new(true),
            completion_fault: AtomicBool::new(false),
            retain_checks: AtomicUsize::new(0),
        });
        let permit = CapturePermit::new(guard.clone());
        (guard, permit)
    }
}

impl CaptureGuard for Guard {
    fn display_id(&self) -> Option<u32> {
        Some(DISPLAY)
    }

    fn check(&self, access: CaptureAccess) -> Result<(), CoreError> {
        let allowed = match access {
            CaptureAccess::CurrentScene => self.current_scene.load(Ordering::SeqCst),
            CaptureAccess::Acquire { display_id } => {
                display_id == DISPLAY && self.acquire.load(Ordering::SeqCst)
            }
            CaptureAccess::Retain => {
                let prior = self.retain_checks.fetch_add(1, Ordering::SeqCst);
                self.retain.load(Ordering::SeqCst)
                    && !(prior == 1 && self.completion_fault.load(Ordering::SeqCst))
            }
            _ => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(capture_denied("test authority revoked"))
        }
    }
}

enum Behavior {
    Pixels,
    SourceError,
    Panic,
}

struct Source {
    frame: DynamicImage,
    pixels: AtomicUsize,
    bounds: Mutex<Option<(i32, i32, u32, u32)>>,
    foreground_leaves: Option<Arc<Guard>>,
    entered: Option<mpsc::Sender<()>>,
    release: Option<Mutex<mpsc::Receiver<()>>>,
    behavior: Behavior,
}

impl Source {
    fn new(width: u32, height: u32) -> Self {
        Self {
            frame: DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                width,
                height,
                image::Rgba([128, 64, 32, 255]),
            )),
            pixels: AtomicUsize::new(0),
            bounds: Mutex::new(None),
            foreground_leaves: None,
            entered: None,
            release: None,
            behavior: Behavior::Pixels,
        }
    }
}

impl CaptureSource for Source {
    fn capture(
        &self,
        bounds: Option<&WindowBounds>,
        permit: &CapturePermit,
    ) -> Result<(DynamicImage, Option<usize>), CoreError> {
        *self.bounds.lock().unwrap() = bounds.map(|b| (b.x, b.y, b.width, b.height));
        match self.behavior {
            Behavior::SourceError => {
                return Err(CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "source failure".into(),
                });
            }
            Behavior::Panic => panic!("synthetic capture worker panic"),
            Behavior::Pixels => {}
        }
        let frame = capture_pixels_authorized(DISPLAY, permit, &mut || {
            self.pixels.fetch_add(1, Ordering::SeqCst);
            Ok(self.frame.clone())
        })?;
        if let Some(guard) = &self.foreground_leaves {
            guard.current_scene.store(false, Ordering::SeqCst);
        }
        if let Some(entered) = &self.entered {
            entered.send(()).unwrap();
            // Keep the worker pending until the caller stops polling the async task.
            self.release
                .as_ref()
                .unwrap()
                .lock()
                .unwrap()
                .recv_timeout(WAIT)
                .unwrap();
        }
        Ok((frame, Some(0)))
    }
}

fn processor(source: Arc<Source>, width: u32, height: u32) -> Arc<dyn FrameProcessor> {
    Arc::new(EdgeFrameProcessor::new(width, height, None).with_capture_source(source))
}

fn assert_denied<T>(result: Result<T, CoreError>) {
    assert!(matches!(
        result,
        Err(CoreError::PermissionDenied {
            code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
            ..
        })
    ));
}

fn assert_pixels(bytes: &[u8], expected: (u32, u32)) {
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::WebP)
        .unwrap()
        .to_rgba8();
    assert_eq!((decoded.width(), decoded.height()), expected);
    for pixel in decoded.pixels() {
        for (channel, expected) in pixel.0[..3].iter().zip([128i16, 64, 32]) {
            assert!((i16::from(*channel) - expected).abs() <= 8);
        }
        assert_eq!(pixel.0[3], 255);
    }
}

fn current_thread_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

fn start_queued_thumbnail(
    runtime: &tokio::runtime::Runtime,
    processor: Arc<dyn FrameProcessor>,
    permit: CapturePermit,
) -> tokio::task::JoinHandle<Result<Vec<u8>, CoreError>> {
    let (started, observed) = tokio::sync::oneshot::channel();
    let pending = runtime.spawn(async move {
        started.send(()).unwrap();
        processor.thumbnail_authorized(None, &permit).await
    });
    // One current-thread poll queues the worker and yields at its join await.
    runtime.block_on(observed).unwrap();
    pending
}

fn finish_worker_without_polling_delivery(runtime: &tokio::runtime::Runtime) {
    let (sent, received) = mpsc::channel();
    // The source has already entered the sole blocking thread. This sentinel
    // runs after that worker returns while the async scheduler remains stopped.
    let sentinel = runtime.spawn_blocking(move || sent.send(()).unwrap());
    received.recv_timeout(WAIT).unwrap();
    drop(sentinel);
}

#[tokio::test]
async fn port_returns_real_webp_and_forwards_bounds() {
    let source = Arc::new(Source::new(8, 4));
    let (_, permit) = Guard::allowed();
    let bounds = WindowBounds {
        x: -80,
        y: 40,
        width: 320,
        height: 200,
    };
    let bytes = processor(source.clone(), 4, 4)
        .thumbnail_authorized(Some(&bounds), &permit)
        .await
        .unwrap();
    assert_pixels(&bytes, (4, 2));
    assert_eq!(source.pixels.load(Ordering::SeqCst), 1);
    assert_eq!(*source.bounds.lock().unwrap(), Some((-80, 40, 320, 200)));
}

#[tokio::test]
async fn retained_frame_survives_foreground_change_without_upscaling() {
    let (guard, permit) = Guard::allowed();
    let mut source = Source::new(8, 4);
    source.foreground_leaves = Some(guard);
    let bytes = processor(Arc::new(source), 16, 16)
        .thumbnail_authorized(None, &permit)
        .await
        .unwrap();
    assert_pixels(&bytes, (8, 4));
}

#[tokio::test]
async fn invalid_current_scene_never_acquires_pixels() {
    let source = Arc::new(Source::new(8, 4));
    let (guard, permit) = Guard::allowed();
    guard.current_scene.store(false, Ordering::SeqCst);
    assert_denied(
        processor(source.clone(), 4, 4)
            .thumbnail_authorized(None, &permit)
            .await,
    );
    assert_eq!(source.pixels.load(Ordering::SeqCst), 0);
}

#[test]
fn queued_old_grant_cannot_use_a_fresh_grant() {
    let runtime = current_thread_runtime();
    let (release, wait) = mpsc::channel();
    let (ready, running) = mpsc::channel();
    let blocker = runtime.spawn_blocking(move || {
        ready.send(()).unwrap();
        wait.recv_timeout(WAIT).unwrap();
    });
    running.recv_timeout(WAIT).unwrap();
    let source = Arc::new(Source::new(8, 4));
    let processor = processor(source.clone(), 4, 4);
    let (old_guard, old_permit) = Guard::allowed();
    let pending = start_queued_thumbnail(&runtime, processor.clone(), old_permit.clone());
    old_guard.acquire.store(false, Ordering::SeqCst);
    old_guard.retain.store(false, Ordering::SeqCst);
    let (_, fresh_permit) = Guard::allowed();
    release.send(()).unwrap();
    assert_denied(runtime.block_on(pending).unwrap());
    runtime.block_on(blocker).unwrap();
    assert_eq!(source.pixels.load(Ordering::SeqCst), 0);
    let bytes = runtime
        .block_on(processor.thumbnail_authorized(None, &fresh_permit))
        .unwrap();
    assert_pixels(&bytes, (4, 2));
    assert_eq!(source.pixels.load(Ordering::SeqCst), 1);
    assert!(matches!(
        old_permit.check(CaptureAccess::Retain),
        Err(CoreError::PermissionDenied {
            code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
            ..
        })
    ));
}

#[tokio::test]
async fn retention_denial_precedes_processing_errors() {
    let (guard, permit) = Guard::allowed();
    guard.retain.store(false, Ordering::SeqCst);
    assert_denied(
        processor(Arc::new(Source::new(0, 4)), 0, 4)
            .thumbnail_authorized(None, &permit)
            .await,
    );
}

#[tokio::test]
async fn zero_source_or_target_dimensions_preserve_typed_errors() {
    for (sw, sh, tw, th, message) in [
        (0, 4, 4, 4, "Source image size is zero"),
        (8, 0, 4, 4, "Source image size is zero"),
        (8, 4, 0, 4, "Target image size is zero"),
        (8, 4, 4, 0, "Target image size is zero"),
    ] {
        let (_, permit) = Guard::allowed();
        let result = processor(Arc::new(Source::new(sw, sh)), tw, th)
            .thumbnail_authorized(None, &permit)
            .await;
        assert!(
            matches!(result, Err(CoreError::Internal { code: maekon_core::error_codes::InternalCode::Generic, message: actual }) if actual == message)
        );
    }
}

#[tokio::test]
async fn acquisition_error_survives_retention_denial() {
    let (guard, permit) = Guard::allowed();
    guard.retain.store(false, Ordering::SeqCst);
    let mut source = Source::new(8, 4);
    source.behavior = Behavior::SourceError;
    let result = processor(Arc::new(source), 4, 4)
        .thumbnail_authorized(None, &permit)
        .await;
    assert!(
        matches!(result, Err(CoreError::Internal { code: maekon_core::error_codes::InternalCode::Generic, message }) if message == "source failure")
    );
}

#[tokio::test]
async fn worker_panic_preserves_join_error() {
    let (guard, permit) = Guard::allowed();
    guard.retain.store(false, Ordering::SeqCst);
    let mut source = Source::new(8, 4);
    source.behavior = Behavior::Panic;
    let result = processor(Arc::new(source), 4, 4)
        .thumbnail_authorized(None, &permit)
        .await;
    assert!(
        matches!(result, Err(CoreError::Internal { code: maekon_core::error_codes::InternalCode::Generic, message }) if message.contains("thumbnail task panicked") && message.contains("synthetic capture worker panic"))
    );
}

#[test]
fn original_permit_is_rechecked_after_worker_before_delivery() {
    let runtime = current_thread_runtime();
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut source = Source::new(8, 4);
    source.entered = Some(entered);
    source.release = Some(Mutex::new(released));
    let source = Arc::new(source);
    let (guard, permit) = Guard::allowed();
    let pending = start_queued_thumbnail(&runtime, processor(source.clone(), 4, 4), permit);
    observed.recv_timeout(WAIT).unwrap();
    release.send(()).unwrap();
    finish_worker_without_polling_delivery(&runtime);
    guard.retain.store(false, Ordering::SeqCst);
    assert_denied(runtime.block_on(pending).unwrap());
    assert_eq!(source.pixels.load(Ordering::SeqCst), 1);
}

#[test]
fn completed_worker_error_is_preserved_after_transient_guard_recovers() {
    let runtime = current_thread_runtime();
    let (entered, observed) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let mut source = Source::new(8, 4);
    source.entered = Some(entered);
    source.release = Some(Mutex::new(released));
    let (guard, permit) = Guard::allowed();
    // A synthetic transient denial at processing completion must stay an error;
    // this models a failed check, not a new consent grant for an old frame.
    guard.completion_fault.store(true, Ordering::SeqCst);
    let pending = start_queued_thumbnail(&runtime, processor(Arc::new(source), 4, 4), permit);
    observed.recv_timeout(WAIT).unwrap();
    release.send(()).unwrap();
    finish_worker_without_polling_delivery(&runtime);
    guard.completion_fault.store(false, Ordering::SeqCst);
    assert_denied(runtime.block_on(pending).unwrap());
}

#[test]
fn full_frame_guarded_delivery_rechecks_and_preserves_worker_denial() {
    for transient_worker_denial in [false, true] {
        let runtime = current_thread_runtime();
        let (entered, observed) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let mut source = Source::new(8, 4);
        source.entered = Some(entered);
        source.release = Some(Mutex::new(released));
        let source = Arc::new(source);
        let (guard, permit) = Guard::allowed();
        guard
            .completion_fault
            .store(transient_worker_denial, Ordering::SeqCst);
        let (queued, ready) = tokio::sync::oneshot::channel();
        let queued_source = source.clone();
        let pending = runtime.spawn(async move {
            queued.send(()).unwrap();
            acquire_frame(queued_source, None, permit).await
        });
        runtime.block_on(ready).unwrap();
        observed.recv_timeout(WAIT).unwrap();
        release.send(()).unwrap();
        finish_worker_without_polling_delivery(&runtime);
        // Exercise both late revocation after worker success and a transient
        // worker denial that must not turn into success on async delivery.
        guard
            .retain
            .store(transient_worker_denial, Ordering::SeqCst);
        guard.completion_fault.store(false, Ordering::SeqCst);
        assert_denied(runtime.block_on(pending).unwrap());
        assert_eq!(source.pixels.load(Ordering::SeqCst), 1);
    }
}
