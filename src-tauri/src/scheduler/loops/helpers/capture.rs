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
    // Own-field gate: OCR 텍스트 추출/저장은 ocr_processing 동의가 있어야 한다.
    // 복합 게이트(screen_capture)만 통과해도 ocr_processing 이 false 이면 OCR 텍스트는
    // 폐기되어 frames.ocr_text 에 저장되지 않고, ocr_regions(바운딩 박스 좌표)와
    // 힌트 반환값도 비워진다 (effective_permissions() 가 호출자에서 평가된 Valid-only 값).
    ocr_permitted: bool,
    event_tx: &Option<broadcast::Sender<RealtimeEvent>>,
) -> FrameCaptureResult {
    match processor.capture_and_process(capture_req).await {
        Ok(frame) => {
            debug!("frame completed: {:?}", frame.metadata.trigger_type);

            // Own-field gate: ocr_processing 미부여 시 OCR 영역(좌표 + 텍스트)을 비운다.
            // 프레임 이미지 자체는 screen_capture 동의로 캡처되지만, OCR로 추출한
            // 텍스트는 별도 동의가 필요하므로 게이트가 닫히면 텍스트 경로 전체를 차단한다.
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
                    // Own-field gate: ocr_processing 미부여 시 페이로드에 OCR 텍스트가
                    // 실려 있어도 폐기한다 (None) — 저장/반환 어느 경로로도 새지 않는다.
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
            match sqlite.save_frame_metadata_with_bounds(
                &frame.metadata,
                file_path.as_deref(),
                sanitized_ocr.as_deref(),
                capture_req.window_bounds.as_ref(),
            ) {
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

/// Own-field gate (#4802): 윈도우 제목을 window_title_collection 동의 여부로 redact 한다.
///
/// `permitted` 가 true 이면 원본 제목을 그대로 반환하고, false 이면 빈 문자열을 반환한다.
/// 호출자(monitor 루프)는 `consent.window_title_collection`(effective_permissions() 의
/// Valid-only 값)을 넘긴다. 빈 문자열로 통일해 모든 다운스트림 소비자가 redact 된 값을
/// 보게 한다. CRITICAL: ConsentPermissions 의 window_title_collection 이지 config 토글이 아니다.
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
