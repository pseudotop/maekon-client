use async_trait::async_trait;
use chrono::Utc;
use image::DynamicImage;
use oneshim_core::error::CoreError;
use oneshim_core::models::frame::{FrameMetadata, ImagePayload, ProcessedFrame};
use oneshim_core::ports::vision::{CaptureRequest, FrameProcessor};
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
    #[cfg(feature = "ocr")]
    ocr_extractor: Option<crate::ocr::OcrExtractor>,
}

impl EdgeFrameProcessor {
    #[allow(unused_variables)]
    pub fn new(thumbnail_width: u32, thumbnail_height: u32, ocr_tessdata: Option<PathBuf>) -> Self {
        Self {
            capture: ScreenCapture::new(),
            prev_frame: Mutex::new(None),
            thumbnail_width,
            thumbnail_height,
            #[cfg(feature = "ocr")]
            ocr_extractor: ocr_tessdata
                .map(|p| crate::ocr::OcrExtractor::new(Some(p)))
                .or_else(|| Some(crate::ocr::OcrExtractor::new(None))),
        }
    }
}

#[cfg(not(feature = "ocr"))]
fn extract_ocr_text(_frame: &DynamicImage, _processor: &EdgeFrameProcessor) -> Option<String> {
    None
}

#[cfg(feature = "ocr")]
fn extract_ocr_text(frame: &DynamicImage, processor: &EdgeFrameProcessor) -> Option<String> {
    let extractor = processor.ocr_extractor.as_ref()?;
    match extractor.extract(frame) {
        Ok(text) if !text.is_empty() => {
            let sanitized = privacy::sanitize_title(&text);
            Some(sanitized)
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("OCR extraction failed: {e}");
            None
        }
    }
}

/// Extract OCR regions with bounding boxes from the frame.
/// Returns empty Vec when OCR feature is disabled.
#[cfg(not(feature = "ocr"))]
fn extract_ocr_regions(
    _frame: &DynamicImage,
    _processor: &EdgeFrameProcessor,
) -> Vec<oneshim_core::models::frame::OcrRegion> {
    Vec::new()
}

/// Extract OCR regions with bounding boxes from the frame.
#[cfg(feature = "ocr")]
fn extract_ocr_regions(
    frame: &DynamicImage,
    processor: &EdgeFrameProcessor,
) -> Vec<oneshim_core::models::frame::OcrRegion> {
    let Some(extractor) = processor.ocr_extractor.as_ref() else {
        return Vec::new();
    };
    match extractor.extract_regions(frame) {
        Ok(regions) => {
            debug!("OCR extracted {} regions", regions.len());
            regions
        }
        Err(e) => {
            tracing::warn!("OCR region extraction failure: {e}");
            Vec::new()
        }
    }
}

#[async_trait]
impl FrameProcessor for EdgeFrameProcessor {
    async fn capture_and_process(
        &self,
        capture_request: &CaptureRequest,
    ) -> Result<ProcessedFrame, CoreError> {
        let sanitized_title = privacy::sanitize_title(&capture_request.window_title);
        let importance = capture_request.importance;

        let current_frame = Arc::new(
            self.capture
                .capture_for_window(capture_request.window_bounds.as_ref())?,
        );
        let (w, h) = (current_frame.width(), current_frame.height());

        let metadata = FrameMetadata {
            timestamp: Utc::now(),
            trigger_type: capture_request.trigger_type.clone(),
            app_name: capture_request.app_name.clone(),
            window_title: sanitized_title,
            resolution: (w, h),
            importance,
        };

        let mut ocr_regions = Vec::new();
        let mut raw_rgba: Option<Vec<u8>> = None;

        let image_payload = if importance >= 0.8 {
            debug!("frame (in progress {:.1})", importance);
            // Offload heavy High-quality encoding to blocking thread
            let frame_ref = Arc::clone(&current_frame);
            let encoded = tokio::task::spawn_blocking(move || {
                encoder::encode_webp_base64(&frame_ref, WebPQuality::High)
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("encode task panicked: {e}"),
            })??;
            let ocr_text = extract_ocr_text(&current_frame, self);
            ocr_regions = extract_ocr_regions(&current_frame, self);
            // Preserve raw RGBA for ML classifier (before current_frame is moved)
            if !ocr_regions.is_empty() {
                raw_rgba = Some(current_frame.to_rgba8().into_vec());
            }
            Some(ImagePayload::Full {
                data: encoded,
                format: "webp".to_string(),
                ocr_text,
            })
        } else if importance >= 0.5 {
            debug!("(in progress {:.1})", importance);
            // Compute delta while holding the lock, then drop before .await
            let delta_result = {
                let prev = self.prev_frame.lock().map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("prev_frame lock poisoned: {e}"),
                })?;
                match prev.as_ref() {
                    Some(prev) => delta::compute_delta(prev, &current_frame),
                    None => None, // marker: no prev frame
                }
            }; // MutexGuard dropped here

            let has_prev = {
                let prev = self.prev_frame.lock().map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("prev_frame lock poisoned: {e}"),
                })?;
                prev.is_some()
            };

            if has_prev {
                if let Some(delta_region) = delta_result {
                    let frame_ref = Arc::clone(&current_frame);
                    let encoded = tokio::task::spawn_blocking(move || {
                        encoder::encode_webp_base64(&frame_ref, WebPQuality::Medium)
                    })
                    .await
                    .map_err(|e| CoreError::Internal {
                        code: oneshim_core::error_codes::InternalCode::Generic,
                        message: format!("encode task panicked: {e}"),
                    })??;
                    Some(ImagePayload::Delta {
                        data: encoded,
                        region: delta_region.region,
                        changed_ratio: delta_region.changed_ratio,
                    })
                } else {
                    None // no meaningful change
                }
            } else {
                let frame_ref = Arc::clone(&current_frame);
                let encoded = tokio::task::spawn_blocking(move || {
                    encoder::encode_webp_base64(&frame_ref, WebPQuality::Medium)
                })
                .await
                .map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
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
                let thumb = thumbnail::fast_resize(&frame_ref, tw, th)?;
                encoder::encode_webp_base64(&thumb, WebPQuality::Low)
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("encode task panicked: {e}"),
            })??;
            Some(ImagePayload::Thumbnail {
                data: encoded,
                width: self.thumbnail_width,
                height: self.thumbnail_height,
            })
        } else {
            debug!("(in progress {:.1})", importance);
            None
        };

        *self.prev_frame.lock().map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
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
            let thumb = thumbnail::fast_resize(&frame, tw, th)?;
            let encoded = encoder::encode_webp_base64(&thumb, WebPQuality::Low)?;
            use base64::Engine;
            base64::engine::general_purpose::STANDARD
                .decode(&encoded)
                .map_err(|e| CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: format!("base64 decode failed: {e}"),
                })
        })
        .await
        .map_err(|e| CoreError::Internal {
            code: oneshim_core::error_codes::InternalCode::Generic,
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
}
