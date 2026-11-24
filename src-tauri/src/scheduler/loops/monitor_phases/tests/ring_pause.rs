use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

#[tokio::test]
async fn pause_aba_thumbnail_never_reaches_pre_event_flush() {
    let started = Arc::new(Notify::new());
    let resume = Arc::new(Notify::new());
    let processor = MarkerProcessor {
        started: Some(started.clone()),
        resume: Some(resume.clone()),
        ..Default::default()
    };
    let pause = CapturePauseState::new(Arc::new(AtomicBool::new(false)));
    let accepted_epoch = pause.snapshot().expect("tick starts open");
    let ring = CaptureRingBuffer::new(3, 2, 0.5);
    ring.push(marker_frame("before-pause", 231));
    let mut cadence = RingThumbnailCadence::default();
    let monitor = StubActivityMonitor {
        active_app: Some("Notes".into()),
        fail: false,
    };
    let visible = || vec!["Notes".to_string()];
    let context = RingThumbnailContext {
        pause: &pause,
        accepted_epoch: Some(accepted_epoch),
        ..ring_context(&monitor, None, &visible)
    };
    let capture = capture_ring_thumbnail_if_due(
        &mut cadence,
        &processor,
        &ring,
        &context,
        Duration::from_secs(5),
    );
    let transition = async {
        started.notified().await;
        assert!(pause.toggle().expect("pause during thumbnail acquisition"));
        assert!(!pause.toggle().expect("resume before acquisition completes"));
        resume.notify_one();
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(capture, transition);
    })
    .await
    .expect("controlled thumbnail acquisition must finish");

    assert_eq!(processor.captures.load(Ordering::Relaxed), 1);
    {
        let bounds = processor.bounds.lock().unwrap();
        assert_eq!(bounds.len(), 1);
        assert!(bounds[0].is_none());
    }
    assert_eq!(
        pause.snapshot().expect("pause returned to open"),
        accepted_epoch
            .checked_add(2)
            .expect("two pause transitions")
    );
    assert!(
        matches!(
            pause.check(accepted_epoch).unwrap_err(),
            CoreError::PermissionDenied {
                code: maekon_core::error_codes::PermissionCode::PrivacyDenied,
                message,
            } if message == "capture denied: capture pause epoch changed"
        ),
        "tick epoch expired"
    );
    let flushed = ring
        .check_and_flush(1.0, marker_frame("subsequent-trigger", 9))
        .expect("significant event flushes the real ring");
    assert!(
        flushed.pre_event_frames.is_empty(),
        "pause ABA ring marker reached pre-event flush"
    );

    let fresh_processor = MarkerProcessor::default();
    let fresh_context = RingThumbnailContext {
        accepted_epoch: Some(pause.snapshot().expect("later tick accepts new epoch")),
        ..context
    };
    cadence.last_capture_at = None;
    capture_ring_thumbnail_if_due(
        &mut cadence,
        &fresh_processor,
        &ring,
        &fresh_context,
        Duration::from_secs(5),
    )
    .await;
    let recovered = ring
        .check_and_flush(1.0, marker_frame("fresh-trigger", 9))
        .unwrap();
    assert_eq!(fresh_processor.captures.load(Ordering::Relaxed), 1);
    assert_eq!(recovered.pre_event_frames.len(), 1);
    assert_eq!(
        recovered.pre_event_frames[0].thumbnail_data,
        [193, 47, 82, 255]
    );
}

struct AwaitingMonitor {
    calls: std::sync::atomic::AtomicUsize,
    block_at: usize,
    started: Arc<Notify>,
    resume: Arc<Notify>,
}

impl AwaitingMonitor {
    fn new(block_at: usize) -> Self {
        Self {
            calls: std::sync::atomic::AtomicUsize::new(0),
            block_at,
            started: Arc::new(Notify::new()),
            resume: Arc::new(Notify::new()),
        }
    }
}

#[async_trait]
impl ActivityMonitor for AwaitingMonitor {
    async fn collect_context(&self) -> Result<UserContext, CoreError> {
        if self.calls.fetch_add(1, Ordering::Relaxed) + 1 == self.block_at {
            self.started.notify_one();
            self.resume.notified().await;
        }
        StubActivityMonitor {
            active_app: Some("Notes".into()),
            fail: false,
        }
        .collect_context()
        .await
    }
}

#[tokio::test]
async fn not_due_thumbnail_still_invalidates_old_epoch_without_querying_apps() {
    for (toggles, has_origin) in [(0, true), (1, true), (2, true), (0, false)] {
        let pause = CapturePauseState::new(Arc::new(AtomicBool::new(false)));
        let decision = CaptureCacheEpoch::default().observe(true, &pause);
        let ring = CaptureRingBuffer::new(3, 2, 0.5);
        ring.push(marker_frame("old-marker", 231));
        let mut cadence = RingThumbnailCadence::default();
        assert!(cadence.should_capture(Instant::now(), Duration::from_secs(60)));
        let processor = MarkerProcessor::default();
        let monitor = AwaitingMonitor::new(usize::MAX);
        let visible = || vec!["Notes".to_string()];
        let context = RingThumbnailContext {
            pause: &pause,
            accepted_epoch: decision.pause_epoch.filter(|_| has_origin),
            ..ring_context(&monitor, None, &visible)
        };
        for _ in 0..toggles {
            pause.toggle().unwrap();
        }
        capture_ring_thumbnail_if_due(
            &mut cadence,
            &processor,
            &ring,
            &context,
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(processor.captures.load(Ordering::Relaxed), 0);
        assert_eq!(monitor.calls.load(Ordering::Relaxed), 0);
        let flushed = ring
            .check_and_flush(1.0, marker_frame("trigger", 9))
            .unwrap();
        assert_eq!(
            flushed.pre_event_frames.len(),
            usize::from(toggles == 0 && has_origin)
        );
    }
}

async fn exercise_app_observation_pause(block_at: usize) {
    let pause = CapturePauseState::new(Arc::new(AtomicBool::new(false)));
    let accepted_epoch = CaptureCacheEpoch::default()
        .observe(true, &pause)
        .pause_epoch;
    let monitor = AwaitingMonitor::new(block_at);
    let processor = MarkerProcessor::default();
    let ring = CaptureRingBuffer::new(3, 2, 0.5);
    ring.push(marker_frame("before-app-await", 231));
    let mut cadence = RingThumbnailCadence::default();
    let visible = || vec!["Notes".to_string()];
    let context = RingThumbnailContext {
        pause: &pause,
        accepted_epoch,
        ..ring_context(&monitor, None, &visible)
    };
    let capture_and_gate = async {
        capture_ring_thumbnail_if_due(
            &mut cadence,
            &processor,
            &ring,
            &context,
            Duration::from_secs(5),
        )
        .await;
        if block_at == 3 {
            // The same production gate runs after the helper, before ring flush.
            Some(context.denied(&ring).await)
        } else {
            None
        }
    };
    let transition = async {
        monitor.started.notified().await;
        assert!(pause.toggle().unwrap());
        assert!(!pause.toggle().unwrap());
        monitor.resume.notify_one();
    };
    let (post_helper_denied, ()) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(capture_and_gate, transition)
    })
    .await
    .expect("app observation barrier must complete");
    assert_eq!(monitor.calls.load(Ordering::Relaxed), block_at);
    assert_eq!(
        processor.captures.load(Ordering::Relaxed),
        usize::from(block_at > 1)
    );
    let flushed = ring
        .check_and_flush(1.0, marker_frame("trigger", 9))
        .unwrap();
    assert!(
        flushed.pre_event_frames.is_empty(),
        "post-app pause marker reached pre-event flush"
    );
    if let Some(denied) = post_helper_denied {
        assert!(denied, "production pre-flush gate must veto this tick");
    }
}

#[tokio::test]
async fn pause_during_initial_app_observation_prevents_thumbnail_acquisition() {
    exercise_app_observation_pause(1).await;
}

#[tokio::test]
async fn pause_during_final_app_observation_never_reaches_flush() {
    exercise_app_observation_pause(2).await;
}

#[tokio::test]
async fn pause_during_production_pre_flush_observation_retires_ring() {
    exercise_app_observation_pause(3).await;
}

struct FailingThumbnail(MarkerProcessor);

#[async_trait]
impl FrameProcessor for FailingThumbnail {
    async fn capture_and_process(
        &self,
        _: &maekon_core::ports::vision::CaptureRequest,
    ) -> Result<maekon_core::models::frame::ProcessedFrame, CoreError> {
        panic!("ring fixture must use the actual thumbnail route")
    }

    async fn capture_thumbnail(&self, bounds: Option<&WindowBounds>) -> Result<Vec<u8>, CoreError> {
        self.0.capture_thumbnail(bounds).await?;
        Err(CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: "controlled acquisition error".into(),
        })
    }
}

#[tokio::test]
async fn acquisition_error_after_pause_aba_retires_old_ring() {
    for revoke in [false, true] {
        let started = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let processor = FailingThumbnail(MarkerProcessor {
            started: Some(started.clone()),
            resume: Some(resume.clone()),
            ..Default::default()
        });
        let pause = CapturePauseState::new(Arc::new(AtomicBool::new(false)));
        let monitor = StubActivityMonitor {
            active_app: Some("Notes".into()),
            fail: false,
        };
        let visible = || vec!["Notes".to_string()];
        let context = RingThumbnailContext {
            pause: &pause,
            accepted_epoch: pause.snapshot().ok(),
            ..ring_context(&monitor, None, &visible)
        };
        let ring = CaptureRingBuffer::new(3, 2, 0.5);
        ring.push(marker_frame("before-error", 231));
        let mut cadence = RingThumbnailCadence::default();
        let capture = capture_ring_thumbnail_if_due(
            &mut cadence,
            &processor,
            &ring,
            &context,
            Duration::from_secs(5),
        );
        let transition = async {
            started.notified().await;
            if revoke {
                assert!(pause.toggle().unwrap());
                assert!(!pause.toggle().unwrap());
            }
            resume.notify_one();
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(capture, transition);
        })
        .await
        .expect("error acquisition barrier must complete");
        assert_eq!(processor.0.captures.load(Ordering::Relaxed), 1);
        let flushed = ring
            .check_and_flush(1.0, marker_frame("trigger", 9))
            .unwrap();
        assert_eq!(flushed.pre_event_frames.len(), usize::from(!revoke));
    }
}
