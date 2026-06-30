use async_trait::async_trait;
use chrono::Utc;
use image::DynamicImage;
use maekon_core::config::PiiFilterLevel;
use maekon_core::error::CoreError;
use maekon_core::models::frame::{FrameMetadata, ImagePayload, ProcessedFrame};
use maekon_core::ports::vision::{CaptureRequest, FrameProcessor};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tracing::debug;

use crate::capture::ScreenCapture;
use crate::delta;
use crate::encoder::{self, WebPQuality};
use crate::privacy;
use crate::thumbnail;

pub struct EdgeFrameProcessor {
    capture: ScreenCapture,
    prev_frame: Mutex<Option<Arc<DynamicImage>>>,
    thumbnail_width: u32,
    thumbnail_height: u32,
    /// PII filter level applied to OCR output at the source, including the
    /// per-word `ProcessedFrame.ocr_regions` text (#6088).
    pii_level: PiiFilterLevel,
    /// Long-lived OCR extractor shared across frames. Wrapped in `Arc` so it can
    /// be cloned cheaply into the per-frame `spawn_blocking` closure while
    /// preserving the cached `LepTess` instance (`OcrExtractor` is `Send + Sync`
    /// via `Mutex<Option<LepTess>>`). Reusing the same extractor avoids
    /// re-initializing Tesseract (~10-50ms) on every high-importance frame
    /// (#6132).
    #[cfg(feature = "ocr")]
    ocr_extractor: Option<Arc<crate::ocr::OcrExtractor>>,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureMetadataInput<'a> {
    pub trigger_type: &'a str,
    pub app_name: &'a str,
    pub window_title: &'a str,
    pub resolution: (u32, u32),
    pub importance: f32,
    pub monitor_id: Option<usize>,
    pub app_bundle_id: Option<&'a str>,
    /// Configured PII filter level applied to the window title (review4 V8). The
    /// title was previously masked at a hardcoded Standard, silently downgrading
    /// Strict (leaving IP / API-key / passport in the persisted + SSE'd title).
    pub pii_level: PiiFilterLevel,
}

/// Mask the raw text of every OCR region with the configured PII level (#6088).
///
/// `OcrExtractor` produces per-word regions whose `text` is the raw recognized
/// string. Sanitizing here — at the single source where regions are populated —
/// guarantees that no downstream consumer (e.g. the `analyze_current_scene`
/// Tauri command, which surfaces region text directly and as GUI labels) can
/// leak unredacted PII, mirroring the masking already applied to the full OCR
/// text.
fn sanitize_ocr_regions(
    regions: Vec<maekon_core::models::frame::OcrRegion>,
    pii_level: PiiFilterLevel,
) -> Vec<maekon_core::models::frame::OcrRegion> {
    regions
        .into_iter()
        .map(|mut region| {
            region.text = privacy::sanitize_title_with_level(&region.text, pii_level);
            region
        })
        .collect()
}

pub fn build_frame_metadata(input: CaptureMetadataInput<'_>) -> FrameMetadata {
    FrameMetadata {
        timestamp: Utc::now(),
        trigger_type: input.trigger_type.to_string(),
        app_name: input.app_name.to_string(),
        window_title: privacy::sanitize_title_with_level(input.window_title, input.pii_level),
        resolution: input.resolution,
        importance: input.importance,
        monitor_id: input.monitor_id,
        app_bundle_id: input.app_bundle_id.map(ToString::to_string),
    }
}

impl EdgeFrameProcessor {
    #[allow(unused_variables)]
    pub fn new(thumbnail_width: u32, thumbnail_height: u32, ocr_tessdata: Option<PathBuf>) -> Self {
        Self::with_pii_level(
            thumbnail_width,
            thumbnail_height,
            ocr_tessdata,
            PiiFilterLevel::Standard,
        )
    }

    /// Construct a processor with an explicit PII filter level so OCR text
    /// (full text + per-region `ocr_regions`) is sanitized at the source with
    /// the configured policy rather than the `Standard` default (#6088).
    #[allow(unused_variables)]
    pub fn with_pii_level(
        thumbnail_width: u32,
        thumbnail_height: u32,
        ocr_tessdata: Option<PathBuf>,
        pii_level: PiiFilterLevel,
    ) -> Self {
        Self {
            capture: ScreenCapture::new(),
            prev_frame: Mutex::new(None),
            thumbnail_width,
            thumbnail_height,
            pii_level,
            #[cfg(feature = "ocr")]
            ocr_extractor: Some(Arc::new(
                ocr_tessdata
                    .map(|p| crate::ocr::OcrExtractor::new(Some(p)))
                    .unwrap_or_else(|| crate::ocr::OcrExtractor::new(None)),
            )),
        }
    }
}

#[async_trait]
impl FrameProcessor for EdgeFrameProcessor {
    async fn capture_and_process(
        &self,
        capture_request: &CaptureRequest,
    ) -> Result<ProcessedFrame, CoreError> {
        let importance = capture_request.importance;

        // Screen capture is a blocking OS syscall (xcap). Offload to the
        // blocking thread pool so the Tokio scheduler is not stalled.
        let capture = self.capture.clone();
        let window_bounds = capture_request.window_bounds;
        let (captured_frame, selected_monitor_id) = tokio::task::spawn_blocking(move || {
            capture.capture_for_window_with_monitor(window_bounds.as_ref())
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("capture task panicked: {e}"),
        })?
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("screen capture failed: {e}"),
        })?;
        let current_frame = Arc::new(captured_frame);
        let (w, h) = (current_frame.width(), current_frame.height());

        let metadata = build_frame_metadata(CaptureMetadataInput {
            trigger_type: &capture_request.trigger_type,
            app_name: &capture_request.app_name,
            window_title: &capture_request.window_title,
            resolution: (w, h),
            importance,
            monitor_id: capture_request.monitor_id.or(selected_monitor_id),
            app_bundle_id: capture_request.app_bundle_id.as_deref(),
            pii_level: self.pii_level,
        });

        let mut ocr_regions = Vec::new();
        let mut raw_rgba: Option<Vec<u8>> = None;

        let image_payload = if importance >= 0.8 {
            debug!("frame (in progress {:.1})", importance);
            // Offload CPU-heavy encoding and synchronous OCR to blocking thread.
            let frame_ref = Arc::clone(&current_frame);
            // Reuse the long-lived, cached OCR extractor instead of building a
            // fresh `OcrExtractor` per frame, which re-initialized Tesseract and
            // defeated the `LepTess` cache (#6132). Cloning the `Arc` is cheap and
            // shares the cached instance with the blocking closure. When
            // ocr_processing consent is absent, pass `None` so the blocking
            // closure does no OCR work at all.
            #[cfg(feature = "ocr")]
            let ocr_extractor = if capture_request.ocr_processing_permitted {
                self.ocr_extractor.clone()
            } else {
                None
            };
            let scale_factor = capture_request.screen_scale_factor;
            // #6315: pii_level is needed for the per-region sanitize that now runs
            // INSIDE the blocking closure (not only the cfg(ocr) title sanitize), so
            // capture it unconditionally.
            let pii_level = self.pii_level;
            let (encoded, ocr_text_val, ocr_regions_val, raw_rgba_val) =
                tokio::task::spawn_blocking(move || {
                    let enc = encoder::encode_webp_base64(&frame_ref, WebPQuality::High)?;

                    #[cfg(feature = "ocr")]
                    let (text_val, raw_regions) = match ocr_extractor.as_ref() {
                        Some(extractor) => {
                            let text = match extractor.extract(&frame_ref) {
                                Ok(t) if !t.is_empty() => {
                                    Some(crate::privacy::sanitize_title_with_level(&t, pii_level))
                                }
                                _ => None,
                            };
                            let regions = match extractor.extract_regions(&frame_ref) {
                                Ok(r) => r,
                                Err(e) => {
                                    tracing::warn!("OCR region extraction failure: {e}");
                                    Vec::new()
                                }
                            };
                            (text, regions)
                        }
                        None => (None, Vec::new()),
                    };
                    #[cfg(not(feature = "ocr"))]
                    let (text_val, raw_regions): (
                        Option<String>,
                        Vec<maekon_core::models::frame::OcrRegion>,
                    ) = (None, Vec::new());

                    // #6315: scale + PII-sanitize the regions and build the ML
                    // raw-RGBA copy (a full W*H*4 convert) INSIDE the blocking
                    // closure, so none of this per-pixel/masking CPU runs on the
                    // async reactor (mirrors the V9 Delta-branch fix, #6308). The
                    // per-region mask (#6088) keeps downstream consumers from seeing
                    // unredacted region text.
                    let scaled = crate::ocr_geometry::scale_ocr_regions_to_logical(
                        &raw_regions,
                        scale_factor,
                    );
                    let sanitized = sanitize_ocr_regions(scaled, pii_level);
                    let raw_rgba_opt = if sanitized.is_empty() {
                        None
                    } else {
                        Some(frame_ref.to_rgba8().into_vec())
                    };

                    Ok::<_, crate::error::VisionError>((enc, text_val, sanitized, raw_rgba_opt))
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("encode task panicked: {e}"),
                })??;
            let ocr_text = ocr_text_val;
            ocr_regions = ocr_regions_val;
            raw_rgba = raw_rgba_val;
            Some(ImagePayload::Full {
                data: encoded,
                format: "webp".to_string(),
                ocr_text,
            })
        } else if importance >= 0.5 {
            debug!("(in progress {:.1})", importance);
            // Snapshot the previous frame under a SHORT lock (cheap Arc clone), then
            // run the O(W*H) delta scan AND the encode together off the reactor in a
            // single spawn_blocking. The scan must not run on the async worker nor
            // hold prev_frame across the CPU burst (review4 V9). This also collapses
            // the previously-redundant second lock acquisition.
            let prev_snapshot = self
                .prev_frame
                .lock()
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("prev_frame lock poisoned: {e}"),
                })?
                .clone();

            if prev_snapshot.is_some() {
                let frame_ref = Arc::clone(&current_frame);
                let delta_encoded = tokio::task::spawn_blocking(move || {
                    let region = match prev_snapshot.as_ref() {
                        Some(prev) => delta::compute_delta(prev, &frame_ref),
                        None => None,
                    };
                    match region {
                        Some(region) => {
                            let encoded =
                                encoder::encode_webp_base64(&frame_ref, WebPQuality::Medium)?;
                            Ok::<_, crate::error::VisionError>(Some((encoded, region)))
                        }
                        None => Ok(None),
                    }
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("delta task panicked: {e}"),
                })??;
                delta_encoded.map(|(encoded, delta_region)| ImagePayload::Delta {
                    data: encoded,
                    region: delta_region.region,
                    changed_ratio: delta_region.changed_ratio,
                })
            } else {
                let frame_ref = Arc::clone(&current_frame);
                let encoded = tokio::task::spawn_blocking(move || {
                    encoder::encode_webp_base64(&frame_ref, WebPQuality::Medium)
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("encode task panicked: {e}"),
                })??;
                Some(ImagePayload::Full {
                    data: encoded,
                    format: "webp".to_string(),
                    ocr_text: None,
                })
            }
        } else if importance >= 0.3 {
            debug!("(in progress {:.1})", importance);
            let tw = self.thumbnail_width;
            let th = self.thumbnail_height;
            let frame_ref = Arc::clone(&current_frame);
            let encoded = tokio::task::spawn_blocking(move || {
                let thumb = thumbnail::resize_to_fit(&frame_ref, tw, th)?;
                let (width, height) = (thumb.width(), thumb.height());
                let data = encoder::encode_webp_base64(&thumb, WebPQuality::Low)?;
                Ok::<_, crate::error::VisionError>((data, width, height))
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("encode task panicked: {e}"),
            })??;
            Some(ImagePayload::Thumbnail {
                data: encoded.0,
                width: encoded.1,
                height: encoded.2,
            })
        } else {
            debug!("(in progress {:.1})", importance);
            None
        };

        *self.prev_frame.lock().map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("prev_frame lock poisoned: {e}"),
        })? = Some(current_frame);

        Ok(ProcessedFrame {
            metadata,
            image_payload,
            ocr_regions,
            raw_rgba,
        })
    }

    async fn capture_thumbnail(&self) -> Result<Vec<u8>, CoreError> {
        let capture = self.capture.clone();
        let tw = self.thumbnail_width;
        let th = self.thumbnail_height;
        tokio::task::spawn_blocking(move || {
            let frame = capture.capture_primary()?;
            let thumb = thumbnail::resize_to_fit(&frame, tw, th)?;
            let encoded = encoder::encode_webp_base64(&thumb, WebPQuality::Low)?;
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(&encoded)
                .map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("base64 decode failed: {e}"),
                })
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: maekon_core::error_codes::InternalCode::Generic,
            message: format!("thumbnail task panicked: {e}"),
        })?
    }
}

#[cfg(test)]
mod tests {
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

    // #6132: the high-importance OCR branch must reuse the long-lived cached
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
}
