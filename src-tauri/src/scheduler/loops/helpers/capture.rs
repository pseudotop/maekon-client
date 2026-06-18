//! Frame capture, processing, and retention helpers.

use std::sync::Arc;

use tracing::{debug, warn};

use maekon_api_contracts::stream::{FrameUpdate, RealtimeEvent};
use maekon_core::models::frame::{ImagePayload, OcrRegion};
use maekon_core::ports::frame_storage::FrameStoragePort;
use maekon_core::ports::vision::{CaptureRequest, FrameProcessor};
use tokio::sync::broadcast;

use crate::scheduler::config::{base64_decode, SchedulerStorage};

/// Capture a frame, process it (full/delta/thumbnail), save image data and
/// metadata.  Returns the OCR text extracted from the frame (if any) and
/// any OCR regions with bounding boxes for GUI element correlation.
///
/// Returns `(ocr_text_hint, ocr_regions, raw_rgba)` where `raw_rgba` contains
/// the frame's RGBA bytes + dimensions for ML classification.
pub(crate) type FrameCaptureResult = (Option<String>, Vec<OcrRegion>, Option<(Vec<u8>, u32, u32)>);

#[tracing::instrument(skip_all)]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_frame_capture(
    capture_req: &CaptureRequest,
    processor: &Arc<dyn FrameProcessor>,
    frame_storage: &Option<Arc<dyn FrameStoragePort>>,
    sqlite: &Arc<dyn SchedulerStorage>,
    session_id: &str,
    pii_filter_level: maekon_core::config::PiiFilterLevel,
    // Own-field gate: OCR text extraction/storage requires ocr_processing
    // consent. Even if only the composite gate (screen_capture) passes, when
    // ocr_processing is false the OCR text is discarded and not stored in
    // frames.ocr_text, and ocr_regions (bounding-box coordinates) plus the hint
    // return value are also emptied (ocr_permitted is the Valid-only value of
    // effective_permissions(), evaluated at the caller).
    ocr_permitted: bool,
    event_tx: &Option<broadcast::Sender<RealtimeEvent>>,
) -> FrameCaptureResult {
    match processor.capture_and_process(capture_req).await {
        Ok(frame) => {
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
            let raw_rgba = frame.raw_rgba.map(|rgba| {
                let (w, h) = frame.metadata.resolution;
                (rgba, w, h)
            });

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

/// Enforce frame retention and storage limits. Called periodically from the
/// monitor loop to prevent unbounded disk usage.
pub(crate) async fn enforce_frame_retention(frame_storage: &dyn FrameStoragePort) {
    if let Err(e) = frame_storage.enforce_retention().await {
        warn!("frame retention enforcement failed: {e}");
    }
    if let Err(e) = frame_storage.enforce_storage_limit().await {
        warn!("frame storage limit enforcement failed: {e}");
    }
}
