//! Frame capture, processing, and retention helpers.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tracing::{debug, error, info, warn};

use maekon_api_contracts::stream::{FrameUpdate, RealtimeEvent};
use maekon_core::models::frame::{ImagePayload, OcrRegion};
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::vision::{CapturePauseState, CaptureRequest, FrameProcessor};
use sha2::digest::zeroize::Zeroize;
use tokio::sync::broadcast;

use crate::scheduler::config::base64_decode;
use crate::scheduler::SchedulerStorage;

/// Capture a frame, process it (full/delta/thumbnail), save image data and
/// metadata.  Returns the OCR text extracted from the frame (if any) and
/// any OCR regions with bounding boxes for GUI element correlation.
///
/// Returns `(ocr_text_hint, ocr_regions, raw_rgba)` where `raw_rgba` contains
/// the frame's RGBA bytes + dimensions for ML classification.
pub(crate) type FrameCaptureResult = (Option<String>, Vec<OcrRegion>, Option<(Vec<u8>, u32, u32)>);

/// Existing own-field policy plus a veto from the shared runtime pause owner.
/// The epoch is not a native capture permit or an entered-storage cancellation.
pub(crate) struct FrameCapturePolicy<'a> {
    pub(crate) pause: &'a CapturePauseState,
    pub(crate) pii_filter_level: maekon_core::config::PiiFilterLevel,
    // OCR content requires its own consent even when screen capture is allowed.
    pub(crate) ocr_permitted: bool,
}

#[tracing::instrument(skip_all)]
pub(crate) async fn handle_frame_capture(
    capture_req: &CaptureRequest,
    processor: &Arc<dyn FrameProcessor>,
    frame_storage: &Option<Arc<dyn FrameStoragePort>>,
    sqlite: &Arc<dyn SchedulerStorage>,
    session_id: &str,
    policy: &FrameCapturePolicy<'_>,
    event_tx: &Option<broadcast::Sender<RealtimeEvent>>,
) -> FrameCaptureResult {
    let Ok(origin) = policy.pause.snapshot() else {
        return (None, Vec::new(), None);
    };
    let ocr_permitted = policy.ocr_permitted;
    let pii_filter_level = policy.pii_filter_level;
    let mut gated_capture_req = capture_req.clone();
    gated_capture_req.ocr_processing_permitted = ocr_permitted;

    match processor.capture_and_process(&gated_capture_req).await {
        Ok(mut frame) => {
            // The processor may have yielded across pause/resume. Retire the
            // old result before any new file, metadata, event or OCR fan-out.
            if let Err(_denied) = policy.pause.check(origin) {
                if let Some(raw) = frame.raw_rgba.as_mut() {
                    raw.zeroize();
                }
                for region in &mut frame.ocr_regions {
                    region.text.zeroize();
                }
                match frame.image_payload.as_mut() {
                    Some(ImagePayload::Full { data, ocr_text, .. }) => {
                        data.zeroize();
                        if let Some(text) = ocr_text {
                            text.zeroize();
                        }
                    }
                    Some(
                        ImagePayload::Delta { data, .. } | ImagePayload::Thumbnail { data, .. },
                    ) => {
                        data.zeroize();
                    }
                    None => {}
                }
                frame.metadata.app_name.zeroize();
                frame.metadata.window_title.zeroize();
                return (None, Vec::new(), None);
            }
            debug!("frame completed: {:?}", frame.metadata.trigger_type);

            // Own-field gate: when ocr_processing is not granted, empty the OCR
            // regions (coordinates + text). The frame image itself is captured
            // under screen_capture consent, but OCR-extracted text requires
            // separate consent, so when the gate is closed the entire text path
            // is blocked.
            let ocr_regions = if ocr_permitted {
                frame.ocr_regions.clone()
            } else {
                Vec::new()
            };
            let raw_rgba = if ocr_permitted {
                frame.raw_rgba.map(|rgba| {
                    let (w, h) = frame.metadata.resolution;
                    (rgba, w, h)
                })
            } else {
                None
            };

            let (file_path, ocr_text) = if let Some(ref payload) = frame.image_payload {
                let (data_str, ocr) = match payload {
                    // Own-field gate: when ocr_processing is not granted, discard
                    // any OCR text carried in the payload (None) — it leaks through
                    // neither the storage nor the return path.
                    ImagePayload::Full { data, ocr_text, .. } => (
                        data.as_str(),
                        if ocr_permitted {
                            ocr_text.clone()
                        } else {
                            None
                        },
                    ),
                    ImagePayload::Delta { data, .. } => (data.as_str(), None),
                    ImagePayload::Thumbnail { data, .. } => (data.as_str(), None),
                };

                let saved_path = if let Some(ref fs) = frame_storage {
                    match base64_decode(data_str) {
                        Ok(webp_bytes) => {
                            match fs.save_frame(frame.metadata.timestamp, &webp_bytes).await {
                                Ok(path) => Some(path.to_string_lossy().to_string()),
                                Err(e) => {
                                    warn!("frame file save failure: {e}");
                                    None
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Base64 decoding failure: {e}");
                            None
                        }
                    }
                } else {
                    None
                };

                (saved_path, ocr)
            } else {
                (None, None)
            };

            // D5 iter-3: sanitize OCR text before SQLite persist per PII contract.
            // Raw OCR output from external provider may contain user PII (email
            // addresses, phone numbers, card numbers) visible in the captured
            // screenshot. Sanitize at the write boundary before frames.ocr_text
            // persists.
            let sanitized_ocr = ocr_text.as_deref().map(|raw| {
                maekon_vision::privacy::sanitize_title_with_level(raw, pii_filter_level)
            });

            // #6133: the frame-metadata write is a synchronous `SchedulerStorage`
            // call that acquires the SQLite `parking_lot` write lock and runs the
            // INSERT inline. On the async monitor loop this blocks the reactor
            // thread for the duration of the write. Offload it to the blocking
            // pool (mirror the F-PF-19 / #6083 aggregation offload in system.rs):
            // clone the `Arc<dyn SchedulerStorage>` and move owned copies of the
            // needed data into the closure, then `.await` the JoinHandle so a
            // closure panic surfaces as a JoinError (logged + treated as a save
            // failure, which skips the FrameUpdate emission below).
            let frame_id = {
                let sqlite = sqlite.clone();
                // The original `frame.metadata` must remain available for the
                // FrameUpdate emission, so clone owned copies for the closure.
                let metadata = frame.metadata.clone();
                // `WindowBounds` is `Copy`, so this is a cheap value copy (not a clone).
                let window_bounds = capture_req.window_bounds;
                let handle = tokio::task::spawn_blocking(move || {
                    sqlite.save_frame_metadata_with_bounds(
                        &metadata,
                        file_path.as_deref(),
                        sanitized_ocr.as_deref(),
                        window_bounds.as_ref(),
                    )
                });
                match handle.await {
                    Ok(result) => result,
                    Err(join_err) => {
                        warn!("frame metadata save task panicked: {join_err}");
                        // Treat a join error as a save failure so the FrameUpdate
                        // is not emitted for a frame that was never persisted.
                        Err(maekon_core::error::CoreError::Storage {
                            code: maekon_core::error_codes::StorageCode::Failed,
                            message: format!("frame metadata save task panicked: {join_err}"),
                        })
                    }
                }
            };
            match frame_id {
                Ok(frame_id) => {
                    // Emit FrameUpdate after successful DB insert. Fields sourced from
                    // in-memory frame.metadata — no DB round-trip needed (spec §B).
                    if let Some(tx) = event_tx.as_ref() {
                        let update = FrameUpdate {
                            id: frame_id,
                            timestamp: frame.metadata.timestamp.to_rfc3339(),
                            app_name: frame.metadata.app_name.clone(),
                            window_title: frame.metadata.window_title.clone(),
                            importance: frame.metadata.importance,
                            trigger_type: frame.metadata.trigger_type.clone(),
                        };
                        if let Err(e) = tx.send(RealtimeEvent::Frame(update)) {
                            debug!("frame event channel send failed: {e}");
                        }
                    }
                }
                Err(e) => warn!("frame data save failure: {e}"),
            }

            if let Err(e) = sqlite.increment_session_counters(session_id, 0, 1, 0).await {
                debug!("increment_session_counters failed: {e}");
            }

            (ocr_text, ocr_regions, raw_rgba)
        }
        Err(e) => {
            warn!("frame failure: {e}");
            (None, Vec::new(), None)
        }
    }
}

/// Own-field gate (#4802): redact the window title based on whether
/// window_title_collection consent is given.
///
/// When `permitted` is true, returns the original title verbatim; when false,
/// returns an empty string. The caller (the monitor loop) passes
/// `consent.window_title_collection` (the Valid-only value of
/// effective_permissions()). Standardizing on an empty string ensures every
/// downstream consumer sees the redacted value. CRITICAL: this is
/// ConsentPermissions.window_title_collection, not a config toggle.
pub(crate) fn redact_window_title(title: String, permitted: bool) -> String {
    if permitted {
        title
    } else {
        String::new()
    }
}

/// Interval between automatic frame retention enforcement runs (100 seconds).
pub(crate) const FRAME_RETENTION_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(100);

/// Persistent frame-retention failures are a privacy-health incident, not a
/// background warning. Three failed scheduler cycles (~5 minutes) escalate to
/// an error-level signal; the counter resets only after both retention and the
/// storage-limit pass succeed (#9276).
const FRAME_RETENTION_FAILURE_THRESHOLD: u32 = 3;
static FRAME_RETENTION_CONSECUTIVE_FAILURES: AtomicU32 = AtomicU32::new(0);

/// Enforce frame retention and storage limits. Called periodically from the
/// monitor loop to prevent unbounded disk usage.
pub(crate) async fn enforce_frame_retention(frame_storage: &dyn FrameStoragePort) {
    let mut failed = false;
    if let Err(e) = frame_storage.enforce_retention().await {
        warn!("frame retention enforcement failed: {e}");
        failed = true;
    }
    if let Err(e) = frame_storage.enforce_storage_limit().await {
        warn!("frame storage limit enforcement failed: {e}");
        failed = true;
    }

    if failed {
        let consecutive_failures = FRAME_RETENTION_CONSECUTIVE_FAILURES
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        if consecutive_failures >= FRAME_RETENTION_FAILURE_THRESHOLD {
            error!(
                consecutive_failures,
                "frame retention privacy health degraded"
            );
        }
    } else {
        let recovered_after_failures =
            FRAME_RETENTION_CONSECUTIVE_FAILURES.swap(0, Ordering::Relaxed);
        if recovered_after_failures >= FRAME_RETENTION_FAILURE_THRESHOLD {
            info!(
                recovered_after_failures,
                "frame retention privacy health recovered"
            );
        }
    }
}

#[cfg(test)]
mod pause_result_tests {
    use super::*;
    use chrono::{Duration as ChronoDuration, Utc};
    use maekon_core::config::PiiFilterLevel;
    use maekon_core::error::CoreError;
    use maekon_core::models::frame::{BoundingBox, FrameMetadata, ProcessedFrame};
    use maekon_core::models::storage_records::FrameRecord;
    use maekon_core::ports::vision::CapturePauseState;
    use maekon_storage::frame_storage::FrameFileStorage;
    use maekon_storage::sqlite::SqliteStorage;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use tempfile::TempDir;
    use tokio::sync::Notify;

    const MARKER: [u8; 4] = [193, 47, 82, 255];
    const OCR_MARKER: &str = "processor-pause-marker";

    struct ControlledProcessor {
        calls: AtomicUsize,
        block: bool,
        started: Notify,
        resume: Notify,
    }

    impl ControlledProcessor {
        fn new(block: bool) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                block,
                started: Notify::new(),
                resume: Notify::new(),
            }
        }
    }

    #[async_trait::async_trait]
    impl FrameProcessor for ControlledProcessor {
        async fn capture_and_process(
            &self,
            request: &CaptureRequest,
        ) -> Result<ProcessedFrame, CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.block {
                self.started.notify_one();
                self.resume.notified().await;
            }
            Ok(ProcessedFrame {
                metadata: FrameMetadata {
                    timestamp: Utc::now(),
                    trigger_type: request.trigger_type.clone(),
                    app_name: request.app_name.clone(),
                    window_title: request.window_title.clone(),
                    resolution: (1, 1),
                    importance: 1.0,
                    monitor_id: None,
                    app_bundle_id: None,
                },
                // Synthetic storage bytes: no native capture or image decoding is claimed.
                image_payload: Some(ImagePayload::Full {
                    data: "wS9S/w==".into(),
                    format: "webp".into(),
                    ocr_text: Some(OCR_MARKER.into()),
                }),
                ocr_regions: vec![OcrRegion {
                    text: OCR_MARKER.into(),
                    bbox: BoundingBox {
                        x: 0,
                        y: 0,
                        width: 1,
                        height: 1,
                    },
                    confidence: 1.0,
                }],
                raw_rgba: Some(MARKER.to_vec()),
            })
        }
    }

    struct CaptureFixture {
        dir: TempDir,
        sqlite: Arc<SqliteStorage>,
        files: Option<Arc<dyn FrameStoragePort>>,
        pause: Arc<CapturePauseState>,
        events: Option<broadcast::Sender<RealtimeEvent>>,
        receiver: broadcast::Receiver<RealtimeEvent>,
    }

    impl CaptureFixture {
        async fn new() -> Self {
            let dir = TempDir::new().unwrap();
            let files: Arc<dyn FrameStoragePort> = Arc::new(
                FrameFileStorage::new(dir.path().to_path_buf(), 100, 7)
                    .await
                    .unwrap(),
            );
            let (tx, receiver) = broadcast::channel(4);
            Self {
                dir,
                sqlite: Arc::new(SqliteStorage::open_in_memory(30).unwrap()),
                files: Some(files),
                pause: Arc::new(CapturePauseState::new(Arc::new(AtomicBool::new(false)))),
                events: Some(tx),
                receiver,
            }
        }

        fn request() -> CaptureRequest {
            CaptureRequest {
                trigger_type: "active_window_change".into(),
                importance: 1.0,
                app_name: "Notes".into(),
                window_title: "Synthetic capture".into(),
                monitor_id: None,
                app_bundle_id: None,
                window_bounds: None,
                screen_scale_factor: None,
                ocr_processing_permitted: true,
            }
        }

        async fn capture(&self, pause_during_await: bool) -> FrameCaptureResult {
            let controlled = Arc::new(ControlledProcessor::new(true));
            let processor: Arc<dyn FrameProcessor> = controlled.clone();
            let sqlite: Arc<dyn SchedulerStorage> = self.sqlite.clone();
            let request = Self::request();
            let policy = FrameCapturePolicy {
                pause: &self.pause,
                pii_filter_level: PiiFilterLevel::Off,
                ocr_permitted: true,
            };
            let acquisition = handle_frame_capture(
                &request,
                &processor,
                &self.files,
                &sqlite,
                "synthetic-session",
                &policy,
                &self.events,
            );
            let transition = async {
                controlled.started.notified().await;
                assert_eq!(controlled.calls.load(Ordering::SeqCst), 1);
                if pause_during_await {
                    assert!(self.pause.toggle().unwrap());
                    assert!(!self.pause.toggle().unwrap());
                }
                controlled.resume.notify_one();
            };
            let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                tokio::join!(acquisition, transition)
            })
            .await
            .expect("controlled processor acquisition must complete");
            assert_eq!(controlled.calls.load(Ordering::SeqCst), 1);
            result
        }

        fn rows(&self) -> Vec<FrameRecord> {
            self.sqlite
                .get_frames(
                    Utc::now() - ChronoDuration::hours(1),
                    Utc::now() + ChronoDuration::hours(1),
                    10,
                )
                .unwrap()
        }

        fn frame_files(&self) -> Vec<PathBuf> {
            std::fs::read_dir(self.dir.path().join("frames"))
                .unwrap()
                .flat_map(|day| std::fs::read_dir(day.unwrap().path()).unwrap())
                .map(|file| file.unwrap().path())
                .collect()
        }

        fn assert_no_result(&mut self, result: FrameCaptureResult) {
            assert!(
                self.rows().is_empty(),
                "pause ABA capture result persisted stale metadata"
            );
            assert!(
                self.frame_files().is_empty(),
                "pause ABA capture result persisted stale pixels"
            );
            assert!(matches!(
                self.receiver.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
            assert!(
                result.0.is_none(),
                "pause ABA capture result returned stale OCR"
            );
            assert!(
                result.1.is_empty(),
                "pause ABA capture result returned stale regions"
            );
            assert!(
                result.2.is_none(),
                "pause ABA capture result returned stale RGBA"
            );
        }
    }

    #[tokio::test]
    async fn processor_pause_aba_rejects_result_before_storage_and_ocr_return() {
        let mut fixture = CaptureFixture::new().await;
        let origin = fixture.pause.snapshot().unwrap();
        let result = fixture.capture(true).await;
        assert_eq!(fixture.pause.snapshot().unwrap(), origin + 2);
        fixture.assert_no_result(result);
    }

    #[tokio::test]
    async fn processor_open_and_fresh_pause_epoch_keep_real_output() {
        let mut fixture = CaptureFixture::new().await;
        for expected in 1..=2 {
            let result = fixture.capture(false).await;
            assert_eq!(result.0.as_deref(), Some(OCR_MARKER));
            assert_eq!(result.1.len(), 1);
            assert_eq!(result.1[0].text, OCR_MARKER);
            assert_eq!(result.2.unwrap(), (MARKER.to_vec(), 1, 1));
            let rows = fixture.rows();
            assert_eq!(rows.len(), expected);
            assert!(rows
                .iter()
                .all(|row| row.ocr_text.as_deref() == Some(OCR_MARKER)));
            let files = fixture.frame_files();
            assert_eq!(files.len(), expected);
            assert!(files
                .iter()
                .all(|file| std::fs::read(file).unwrap() == MARKER));
            assert!(matches!(
                fixture.receiver.try_recv(),
                Ok(RealtimeEvent::Frame(_))
            ));
            assert!(matches!(
                fixture.receiver.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ));
            assert!(fixture.pause.toggle().unwrap());
            assert!(!fixture.pause.toggle().unwrap());
        }
    }

    #[tokio::test]
    async fn processor_paused_before_acquisition_starts_no_work() {
        let mut fixture = CaptureFixture::new().await;
        assert!(fixture.pause.toggle().unwrap());
        let controlled = Arc::new(ControlledProcessor::new(false));
        let processor: Arc<dyn FrameProcessor> = controlled.clone();
        let sqlite: Arc<dyn SchedulerStorage> = fixture.sqlite.clone();
        let result = handle_frame_capture(
            &CaptureFixture::request(),
            &processor,
            &fixture.files,
            &sqlite,
            "synthetic-session",
            &FrameCapturePolicy {
                pause: &fixture.pause,
                pii_filter_level: PiiFilterLevel::Off,
                ocr_permitted: true,
            },
            &fixture.events,
        )
        .await;
        assert_eq!(
            controlled.calls.load(Ordering::SeqCst),
            0,
            "paused capture invoked the processor"
        );
        fixture.assert_no_result(result);
    }
}
