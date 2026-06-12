use async_trait::async_trait;
use chrono::Utc;
use image::DynamicImage;
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
    #[cfg(feature = "ocr")]
    ocr_extractor: Option<crate::ocr::OcrExtractor>,
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
}

pub fn build_frame_metadata(input: CaptureMetadataInput<'_>) -> FrameMetadata {
    FrameMetadata {
        timestamp: Utc::now(),
        trigger_type: input.trigger_type.to_string(),
        app_name: input.app_name.to_string(),
        window_title: privacy::sanitize_title(input.window_title),
        resolution: input.resolution,
        importance: input.importance,
        monitor_id: input.monitor_id,
        app_bundle_id: input.app_bundle_id.map(ToString::to_string),
    }
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
        });

        let mut ocr_regions = Vec::new();
        let mut raw_rgba: Option<Vec<u8>> = None;

        let image_payload = if importance >= 0.8 {
            debug!("frame (in progress {:.1})", importance);
            // Offload CPU-heavy encoding and synchronous OCR to blocking thread.
            let frame_ref = Arc::clone(&current_frame);
            #[cfg(feature = "ocr")]
            let ocr_extractor_path = self
                .ocr_extractor
                .as_ref()
                .and_then(|e| e.tessdata_path().cloned());
            #[cfg(not(feature = "ocr"))]
            let _ocr_extractor_path: Option<std::path::PathBuf> = None;
            let scale_factor = capture_request.screen_scale_factor;
            let (encoded, ocr_text_val, raw_regions) = tokio::task::spawn_blocking(move || {
                let enc = encoder::encode_webp_base64(&frame_ref, WebPQuality::High)?;

                #[cfg(feature = "ocr")]
                let (text_val, regions) = {
                    let extractor = crate::ocr::OcrExtractor::new(ocr_extractor_path);
                    let text = match extractor.extract(&frame_ref) {
                        Ok(t) if !t.is_empty() => Some(crate::privacy::sanitize_title(&t)),
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
                };
                #[cfg(not(feature = "ocr"))]
                let (text_val, regions): (
                    Option<String>,
                    Vec<maekon_core::models::frame::OcrRegion>,
                ) = (None, Vec::new());

                Ok::<_, crate::error::VisionError>((enc, text_val, regions))
            })
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
                message: format!("encode task panicked: {e}"),
            })??;
            let ocr_text = ocr_text_val;
            ocr_regions =
                crate::ocr_geometry::scale_ocr_regions_to_logical(&raw_regions, scale_factor);
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
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: format!("prev_frame lock poisoned: {e}"),
                })?;
                match prev.as_ref() {
                    Some(prev) => delta::compute_delta(prev, &current_frame),
                    None => None, // marker: no prev frame
                }
            }; // MutexGuard dropped here

            let has_prev = {
                let prev = self.prev_frame.lock().map_err(|e| CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
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
                        code: maekon_core::error_codes::InternalCode::Generic,
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
}
