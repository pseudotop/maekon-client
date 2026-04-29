use crate::error::VisionError;
use oneshim_core::models::frame::{BoundingBox, OcrRegion};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, warn};

static TESSDATA_PATH: OnceLock<Option<String>> = OnceLock::new();

pub struct OcrExtractor {
    tessdata_path: Option<PathBuf>,
    max_chars: usize,
    /// Cached LepTess instance for reuse across extract calls.
    /// Avoids re-initialization overhead (~10-50ms per call).
    cached_leptess: Mutex<Option<leptess::LepTess>>,
}

impl OcrExtractor {
    pub fn new(tessdata_path: Option<PathBuf>) -> Self {
        let path_str = tessdata_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        if let Err(e) = TESSDATA_PATH.set(path_str) {
            debug!("set failed: {e}");
        }

        Self {
            tessdata_path,
            max_chars: 0,
            cached_leptess: Mutex::new(None),
        }
    }

    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    pub fn extract(&self, image: &image::DynamicImage) -> Result<String, VisionError> {
        let rgba = image.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());

        if w == 0 || h == 0 {
            return Err(VisionError::Ocr(
                "Empty image: width or height is 0".to_string(),
            ));
        }

        // Try to reuse cached LepTess instance; fall back to creating new one
        let mut cached_guard = self.cached_leptess.lock().ok();

        let create_new = || -> Result<leptess::LepTess, VisionError> {
            let tessdata = self
                .tessdata_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            leptess::LepTess::new(tessdata.as_deref(), "eng")
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))
        };

        let lt = if let Some(ref mut guard) = cached_guard {
            if guard.is_none() {
                **guard = Some(create_new()?);
            }
            guard.as_mut().expect("LepTess instance initialized above")
        } else {
            // Lock failed — create a temporary instance
            let mut lt = create_new()?;
            lt.set_image_from_mem(rgba.as_raw(), w as i32, h as i32, 4, (w * 4) as i32)
                .map_err(|_| {
                    VisionError::Ocr(
                        "OCR image setup failed: Image memory setup failed".to_string(),
                    )
                })?;

            let text = lt
                .get_utf8_text()
                .map_err(|e| VisionError::Ocr(format!("OCR text extraction failed: {e}")))?;

            let result = text.trim().to_string();
            return if self.max_chars > 0 && result.len() > self.max_chars {
                Ok(result.chars().take(self.max_chars).collect())
            } else {
                Ok(result)
            };
        };

        lt.set_image_from_mem(rgba.as_raw(), w as i32, h as i32, 4, (w * 4) as i32)
            .map_err(|_| {
                VisionError::Ocr("OCR image setup failed: Image memory setup failed".to_string())
            })?;

        let text = lt
            .get_utf8_text()
            .map_err(|e| VisionError::Ocr(format!("OCR text extraction failed: {e}")))?;

        let result = text.trim().to_string();

        if self.max_chars > 0 && result.len() > self.max_chars {
            Ok(result.chars().take(self.max_chars).collect())
        } else {
            Ok(result)
        }
    }

    pub async fn extract_async(&self, image: &image::DynamicImage) -> Result<String, VisionError> {
        let rgba = image.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());

        if w == 0 || h == 0 {
            return Err(VisionError::Ocr(
                "Empty image: width or height is 0".to_string(),
            ));
        }

        let tessdata = self
            .tessdata_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());

        let max_chars = self.max_chars;
        let raw_data = rgba.into_raw();

        let result = tokio::task::spawn_blocking(move || {
            let tessdata_ref = tessdata.as_deref();

            let mut lt = leptess::LepTess::new(tessdata_ref, "eng")
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))?;

            lt.set_image_from_mem(&raw_data, w as i32, h as i32, 4, (w * 4) as i32)
                .map_err(|_| {
                    VisionError::Ocr(
                        "OCR image setup failed: Image memory setup failed".to_string(),
                    )
                })?;

            let text = lt
                .get_utf8_text()
                .map_err(|e| VisionError::Ocr(format!("OCR text extraction failed: {e}")))?;

            let result = text.trim().to_string();

            if max_chars > 0 && result.len() > max_chars {
                Ok(result.chars().take(max_chars).collect())
            } else {
                Ok(result)
            }
        })
        .await
        .map_err(|e| VisionError::Ocr(format!("OCR async task failed: Task join failed: {e}")))?;

        result
    }

    /// Extract text from a region-of-interest (center crop) of the image.
    /// Public API for consumers that need sub-region OCR; currently exercised by tests.
    #[allow(dead_code)]
    pub async fn extract_roi_async(
        &self,
        image: &image::DynamicImage,
        roi_ratio: f32,
    ) -> Result<String, VisionError> {
        use image::GenericImageView;

        let (w, h) = image.dimensions();

        if w == 0 || h == 0 {
            return Err(VisionError::Ocr(
                "Empty image: width or height is 0".to_string(),
            ));
        }

        let roi_ratio = roi_ratio.clamp(0.1, 1.0);
        let roi_w = ((w as f32) * roi_ratio) as u32;
        let roi_h = ((h as f32) * roi_ratio) as u32;
        let roi_x = (w - roi_w) / 2;
        let roi_y = (h - roi_h) / 2;

        debug!(
            "OCR ROI: source {}x{} -> ROI {}x{} at ({}, {})",
            w, h, roi_w, roi_h, roi_x, roi_y
        );

        let roi_image = image.crop_imm(roi_x, roi_y, roi_w, roi_h);

        self.extract_async(&roi_image).await
    }

    pub fn tessdata_path(&self) -> Option<&PathBuf> {
        self.tessdata_path.as_ref()
    }
}

#[derive(Debug, Clone)]
pub struct OcrWordBox {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl OcrExtractor {
    pub async fn extract_words_with_boxes(
        &self,
        image: &image::DynamicImage,
    ) -> Result<Vec<OcrWordBox>, VisionError> {
        let rgba = image.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());

        if w == 0 || h == 0 {
            return Err(VisionError::Ocr(
                "Empty image: width or height is 0".to_string(),
            ));
        }

        let tessdata = self
            .tessdata_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());

        let raw_data = rgba.into_raw();

        tokio::task::spawn_blocking(move || {
            let tessdata_ref = tessdata.as_deref();

            let mut lt = leptess::LepTess::new(tessdata_ref, "eng")
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))?;

            lt.set_image_from_mem(&raw_data, w as i32, h as i32, 4, (w * 4) as i32)
                .map_err(|_| {
                    VisionError::Ocr(
                        "OCR image setup failed: Image memory setup failed".to_string(),
                    )
                })?;

            let boxes = lt
                .get_component_boxes(leptess::capi::TessPageIteratorLevel_RIL_WORD, true)
                .ok_or_else(|| {
                    VisionError::Ocr(
                        "OCR text extraction failed: Failed to extract word boxes".to_string(),
                    )
                })?;

            let full_text = lt
                .get_utf8_text()
                .map_err(|e| VisionError::Ocr(format!("OCR text extraction failed: {e}")))?;
            let words: Vec<&str> = full_text.split_whitespace().collect();

            let box_count = boxes.len();
            if words.len() != box_count {
                warn!(
                    "OCR word/box count mismatch: {} words vs {} boxes — truncating to shorter",
                    words.len(),
                    box_count
                );
            }
            let count = words.len().min(box_count);

            let mut result = Vec::new();
            for i in 0..count {
                let Some(b) = boxes.get(i) else { continue };
                let geom = b.get_geometry();
                let word_text = words[i].to_string();
                if !word_text.is_empty() {
                    result.push(OcrWordBox {
                        text: word_text,
                        x: geom.x,
                        y: geom.y,
                        w: geom.w,
                        h: geom.h,
                    });
                }
            }

            Ok(result)
        })
        .await
        .map_err(|e| VisionError::Ocr(format!("OCR async task failed: Task join failed: {e}")))?
    }
}

/// Build OCR regions from a prepared LepTess instance.
///
/// Extracts word-level component boxes, reads full text, and pairs each word
/// with its bounding box. When word and box counts diverge (an unreliable
/// assumption in Tesseract), truncates to the shorter list and logs a warning.
/// Confidence is derived from `mean_text_conf()`.
fn build_regions_from_leptess(lt: &mut leptess::LepTess) -> Result<Vec<OcrRegion>, VisionError> {
    let boxes = lt
        .get_component_boxes(leptess::capi::TessPageIteratorLevel_RIL_WORD, true)
        .ok_or_else(|| {
            VisionError::Ocr("OCR text extraction failed: Failed to extract word boxes".to_string())
        })?;

    let full_text = lt
        .get_utf8_text()
        .map_err(|e| VisionError::Ocr(format!("OCR text extraction failed: {e}")))?;
    let words: Vec<&str> = full_text.split_whitespace().collect();

    let box_count = boxes.len();
    if words.len() != box_count {
        warn!(
            "OCR word/box count mismatch: {} words vs {} boxes — truncating to shorter",
            words.len(),
            box_count
        );
    }
    let count = words.len().min(box_count);
    let mean_conf = (lt.mean_text_conf() as f32 / 100.0).clamp(0.0, 1.0);

    let mut regions = Vec::with_capacity(count);
    for i in 0..count {
        let Some(b) = boxes.get(i) else { continue };
        let geom = b.get_geometry();
        let word_text = words[i].to_string();
        if !word_text.is_empty() {
            regions.push(OcrRegion {
                text: word_text,
                bbox: BoundingBox {
                    x: geom.x.max(0) as u32,
                    y: geom.y.max(0) as u32,
                    width: geom.w.max(0) as u32,
                    height: geom.h.max(0) as u32,
                },
                confidence: mean_conf,
            });
        }
    }

    Ok(regions)
}

impl OcrExtractor {
    /// Extract text with bounding box positions as `OcrRegion` values.
    ///
    /// Uses Tesseract word-level component images to obtain spatial coordinates.
    /// Each word that matches a component box is returned as a separate region.
    /// Reuses cached LepTess instance when available.
    pub fn extract_regions(
        &self,
        image: &image::DynamicImage,
    ) -> Result<Vec<OcrRegion>, VisionError> {
        let rgba = image.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());

        if w == 0 || h == 0 {
            return Err(VisionError::Ocr(
                "Empty image: width or height is 0".to_string(),
            ));
        }

        let create_new = || -> Result<leptess::LepTess, VisionError> {
            let tessdata = self
                .tessdata_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string());
            leptess::LepTess::new(tessdata.as_deref(), "eng")
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))
        };

        let mut cached_guard = self.cached_leptess.lock().ok();

        let lt = if let Some(ref mut guard) = cached_guard {
            if guard.is_none() {
                **guard = Some(create_new()?);
            }
            guard.as_mut().expect("LepTess instance initialized above")
        } else {
            // Lock failed — create a temporary instance
            let mut lt = create_new()?;
            lt.set_image_from_mem(rgba.as_raw(), w as i32, h as i32, 4, (w * 4) as i32)
                .map_err(|_| {
                    VisionError::Ocr(
                        "OCR image setup failed: Image memory setup failed".to_string(),
                    )
                })?;
            return build_regions_from_leptess(&mut lt);
        };

        lt.set_image_from_mem(rgba.as_raw(), w as i32, h as i32, 4, (w * 4) as i32)
            .map_err(|_| {
                VisionError::Ocr("OCR image setup failed: Image memory setup failed".to_string())
            })?;

        build_regions_from_leptess(lt)
    }

    /// Async version of `extract_regions`.
    pub async fn extract_regions_async(
        &self,
        image: &image::DynamicImage,
    ) -> Result<Vec<OcrRegion>, VisionError> {
        let rgba = image.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());

        if w == 0 || h == 0 {
            return Err(VisionError::Ocr(
                "Empty image: width or height is 0".to_string(),
            ));
        }

        let tessdata = self
            .tessdata_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        let raw_data = rgba.into_raw();

        tokio::task::spawn_blocking(move || {
            let tessdata_ref = tessdata.as_deref();

            let mut lt = leptess::LepTess::new(tessdata_ref, "eng")
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))?;

            lt.set_image_from_mem(&raw_data, w as i32, h as i32, 4, (w * 4) as i32)
                .map_err(|_| {
                    VisionError::Ocr(
                        "OCR image setup failed: Image memory setup failed".to_string(),
                    )
                })?;

            build_regions_from_leptess(&mut lt)
        })
        .await
        .map_err(|e| VisionError::Ocr(format!("OCR async task failed: Task join failed: {e}")))?
    }
}

pub fn extract_text(image: &image::DynamicImage) -> Result<String, VisionError> {
    let extractor = OcrExtractor::new(None);
    extractor.extract(image)
}

pub async fn extract_text_async(image: &image::DynamicImage) -> Result<String, VisionError> {
    let extractor = OcrExtractor::new(None);
    extractor.extract_async(image).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_image_returns_error() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));
        let result = extractor.extract(&img);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VisionError::Ocr(_)));
    }

    #[test]
    fn error_display_messages() {
        let e1 = VisionError::Ocr("initialize".to_string());
        assert!(e1.to_string().contains("OCR error"));

        let e2 = VisionError::Ocr("image setup".to_string());
        assert!(e2.to_string().contains("OCR error"));

        let e3 = VisionError::Ocr("extraction".to_string());
        assert!(e3.to_string().contains("OCR error"));

        let e4 = VisionError::Ocr("Empty image".to_string());
        assert!(e4.to_string().contains("OCR error"));

        let e5 = VisionError::Ocr("async".to_string());
        assert!(e5.to_string().contains("OCR error"));
    }

    #[test]
    fn extractor_creation() {
        let extractor = OcrExtractor::new(None);
        assert!(extractor.tessdata_path().is_none());

        let path = PathBuf::from("/usr/share/tessdata");
        let extractor = OcrExtractor::new(Some(path.clone()));
        assert_eq!(extractor.tessdata_path(), Some(&path));
    }

    #[test]
    fn max_chars_builder() {
        let extractor = OcrExtractor::new(None).with_max_chars(100);
        assert_eq!(extractor.max_chars, 100);
    }

    #[tokio::test]
    async fn empty_image_async_returns_error() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));
        let result = extractor.extract_async(&img).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VisionError::Ocr(_)));
    }

    #[test]
    fn ocr_word_box_creation() {
        let wb = OcrWordBox {
            text: "hello".to_string(),
            x: 10,
            y: 20,
            w: 50,
            h: 15,
        };
        assert_eq!(wb.text, "hello");
        assert_eq!(wb.x, 10);
        assert_eq!(wb.y, 20);
        assert_eq!(wb.w, 50);
        assert_eq!(wb.h, 15);
    }

    #[tokio::test]
    async fn extract_words_with_boxes_empty_image() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));
        let result = extractor.extract_words_with_boxes(&img).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VisionError::Ocr(_)));
    }

    #[tokio::test]
    async fn roi_extraction_invalid_ratio() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));

        let result = extractor.extract_roi_async(&img, 0.5).await;
        assert!(result.is_err());
    }

    #[test]
    fn extract_regions_empty_image_returns_error() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));
        let result = extractor.extract_regions(&img);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VisionError::Ocr(_)));
    }

    #[tokio::test]
    async fn extract_regions_async_empty_image_returns_error() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));
        let result = extractor.extract_regions_async(&img).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), VisionError::Ocr(_)));
    }

    #[test]
    fn ocr_region_has_valid_bbox_fields() {
        use oneshim_core::models::frame::{BoundingBox, OcrRegion};
        let region = OcrRegion {
            text: "test".to_string(),
            bbox: BoundingBox {
                x: 10,
                y: 20,
                width: 50,
                height: 15,
            },
            confidence: 0.5,
        };
        assert_eq!(region.text, "test");
        assert!(region.bbox.contains_point(30, 25));
        assert!(!region.bbox.contains_point(100, 100));
    }
}
