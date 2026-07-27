use crate::error::VisionError;
use maekon_core::models::frame::{BoundingBox, OcrRegion};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, warn};

static TESSDATA_PATH: OnceLock<Option<String>> = OnceLock::new();

/// Load a raw RGBA frame into a `LepTess` instance (review4 V3).
///
/// leptess 0.14's `set_image_from_mem` takes a SINGLE encoded-image buffer (it
/// calls leptonica `pixReadMem` internally), not the 5-arg raw-strided-RGBA shape
/// the previous code passed. Encode the frame to an in-memory PNG first, then hand
/// the encoded bytes to Tesseract.
fn set_leptess_image(
    lt: &mut leptess::LepTess,
    rgba_raw: &[u8],
    w: u32,
    h: u32,
) -> Result<(), VisionError> {
    use image::codecs::png::PngEncoder;
    use image::ImageEncoder;

    let mut png: Vec<u8> = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(rgba_raw, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| VisionError::Ocr(format!("OCR image encode failed: {e}")))?;
    lt.set_image_from_mem(&png).map_err(|_| {
        VisionError::Ocr("OCR image setup failed: Image memory setup failed".to_string())
    })
}

/// Default Tesseract language when none is configured — preserves the historical
/// English-only behavior. Korean requires the `kor` traineddata to be present in
/// the tessdata directory, so the caller opts in explicitly via
/// [`OcrExtractor::new_with_lang`] rather than defaulting to it and failing
/// `LepTess::new` when the data file is missing (#8054).
pub const DEFAULT_TESSERACT_LANG: &str = "eng";

/// Map BCP-47 recognition languages (as used by the macOS/native OCR config,
/// e.g. `ko-KR`, `en-US`) to a Tesseract `+`-joined language string
/// (e.g. `kor+eng`). Unknown tags are skipped; an empty result falls back to
/// [`DEFAULT_TESSERACT_LANG`] so a misconfigured list never yields an
/// un-initializable extractor (#8054).
#[must_use]
pub fn tesseract_lang_from_bcp47(languages: &[String]) -> String {
    let mut codes: Vec<&str> = Vec::new();
    for lang in languages {
        let primary = lang
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let code = match primary.as_str() {
            "ko" => "kor",
            "en" => "eng",
            "ja" => "jpn",
            "zh" => "chi_sim",
            "de" => "deu",
            "fr" => "fra",
            "es" => "spa",
            _ => continue,
        };
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    if codes.is_empty() {
        DEFAULT_TESSERACT_LANG.to_string()
    } else {
        codes.join("+")
    }
}

pub struct OcrExtractor {
    tessdata_path: Option<PathBuf>,
    max_chars: usize,
    /// Tesseract language string (e.g. `eng`, `kor+eng`) passed to
    /// `LepTess::new`. Defaults to [`DEFAULT_TESSERACT_LANG`].
    lang: String,
    /// Cached LepTess instance for reuse across extract calls.
    /// Avoids re-initialization overhead (~10-50ms per call).
    cached_leptess: Mutex<Option<leptess::LepTess>>,
}

impl OcrExtractor {
    pub fn new(tessdata_path: Option<PathBuf>) -> Self {
        Self::new_with_lang(tessdata_path, DEFAULT_TESSERACT_LANG)
    }

    /// Construct an extractor for an explicit Tesseract language string
    /// (e.g. `kor+eng`). The corresponding `<lang>.traineddata` files must be
    /// present in the tessdata directory or `LepTess::new` will fail at first
    /// use (#8054).
    pub fn new_with_lang(tessdata_path: Option<PathBuf>, lang: &str) -> Self {
        let path_str = tessdata_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());
        if let Err(e) = TESSDATA_PATH.set(path_str) {
            debug!("set failed: {e:?}");
        }

        Self {
            tessdata_path,
            max_chars: 0,
            lang: lang.to_string(),
            cached_leptess: Mutex::new(None),
        }
    }

    pub fn with_max_chars(mut self, max_chars: usize) -> Self {
        self.max_chars = max_chars;
        self
    }

    /// The Tesseract language string this extractor initializes with.
    #[must_use]
    pub fn lang(&self) -> &str {
        &self.lang
    }

    /// Extract text from an image using Tesseract OCR.
    ///
    /// # Blocking
    ///
    /// This method calls Tesseract synchronously and **will block the calling
    /// thread**. When using async runtimes (Tokio), wrap this call in
    /// [`tokio::task::spawn_blocking`] to avoid starving the executor:
    ///
    /// ```rust,ignore
    /// let text = tokio::task::spawn_blocking(move || extractor.extract(&image)).await??;
    /// ```
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
            leptess::LepTess::new(tessdata.as_deref(), &self.lang)
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
            set_leptess_image(&mut lt, rgba.as_raw(), w, h)?;

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

        set_leptess_image(lt, rgba.as_raw(), w, h)?;

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
        let lang = self.lang.clone();
        let raw_data = rgba.into_raw();

        let result = tokio::task::spawn_blocking(move || {
            let tessdata_ref = tessdata.as_deref();

            let mut lt = leptess::LepTess::new(tessdata_ref, &lang)
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))?;

            set_leptess_image(&mut lt, &raw_data, w, h)?;

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

        let lang = self.lang.clone();
        let raw_data = rgba.into_raw();

        tokio::task::spawn_blocking(move || {
            let tessdata_ref = tessdata.as_deref();

            let mut lt = leptess::LepTess::new(tessdata_ref, &lang)
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))?;

            set_leptess_image(&mut lt, &raw_data, w, h)?;

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

            let box_count = boxes.get_n();
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
                let Some(b) = boxes.get_box(i) else { continue };
                // get_box returns the plumbing Box; wrap it in the leptess Box to
                // get the struct-returning get_geometry() -> BoxGeometry. (review4 V3)
                let geom = leptess::leptonica::Box { raw: b }.get_geometry();
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

    let box_count = boxes.get_n();
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
        let Some(b) = boxes.get_box(i) else { continue };
        // get_box returns the plumbing Box; wrap it in the leptess Box to get the
        // struct-returning get_geometry() -> BoxGeometry. (review4 V3)
        let geom = leptess::leptonica::Box { raw: b }.get_geometry();
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
    ///
    /// # Blocking
    ///
    /// This method calls Tesseract synchronously and **will block the calling
    /// thread**. When using async runtimes (Tokio), wrap this call in
    /// [`tokio::task::spawn_blocking`] to avoid starving the executor:
    ///
    /// ```rust,ignore
    /// let regions = tokio::task::spawn_blocking(move || extractor.extract_regions(&image)).await??;
    /// ```
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
            leptess::LepTess::new(tessdata.as_deref(), &self.lang)
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
            set_leptess_image(&mut lt, rgba.as_raw(), w, h)?;
            return build_regions_from_leptess(&mut lt);
        };

        set_leptess_image(lt, rgba.as_raw(), w, h)?;

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
        let lang = self.lang.clone();
        let raw_data = rgba.into_raw();

        tokio::task::spawn_blocking(move || {
            let tessdata_ref = tessdata.as_deref();

            let mut lt = leptess::LepTess::new(tessdata_ref, &lang)
                .map_err(|e| VisionError::Ocr(format!("OCR initialize failure: {e}")))?;

            set_leptess_image(&mut lt, &raw_data, w, h)?;

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
        let err = extractor.extract(&img).unwrap_err();
        assert!(
            matches!(err, VisionError::Ocr(_)),
            "empty image extract must produce Ocr error, got: {err:?}"
        );
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
    fn default_lang_is_english() {
        // #8054: the historical English-only default must be preserved so a
        // build without kor.traineddata never fails LepTess::new.
        let extractor = OcrExtractor::new(None);
        assert_eq!(extractor.lang(), "eng");
    }

    #[test]
    fn new_with_lang_overrides_language() {
        let extractor = OcrExtractor::new_with_lang(None, "kor+eng");
        assert_eq!(extractor.lang(), "kor+eng");
    }

    #[test]
    fn bcp47_maps_to_tesseract_codes() {
        // #8054: the config exposes BCP-47 tags (ko-KR/en-US, matching the
        // macOS Vision path); leptess needs the mapped Tesseract codes.
        assert_eq!(
            tesseract_lang_from_bcp47(&["ko-KR".to_string(), "en-US".to_string()]),
            "kor+eng"
        );
        assert_eq!(tesseract_lang_from_bcp47(&["en_US".to_string()]), "eng");
        // Unknown tags are skipped; an all-unknown list falls back to English
        // so a misconfiguration never yields an un-initializable extractor.
        assert_eq!(tesseract_lang_from_bcp47(&["zz-ZZ".to_string()]), "eng");
        assert_eq!(tesseract_lang_from_bcp47(&[]), "eng");
        // Duplicate primaries collapse (ko-KR + ko-KP → single kor).
        assert_eq!(
            tesseract_lang_from_bcp47(&["ko-KR".to_string(), "ko-KP".to_string()]),
            "kor"
        );
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
        let err = extractor.extract_async(&img).await.unwrap_err();
        assert!(
            matches!(err, VisionError::Ocr(_)),
            "empty image extract_async must produce Ocr error, got: {err:?}"
        );
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
        let err = extractor.extract_words_with_boxes(&img).await.unwrap_err();
        assert!(
            matches!(err, VisionError::Ocr(_)),
            "empty image extract_words_with_boxes must produce Ocr error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn roi_extraction_invalid_ratio() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));

        let err = extractor.extract_roi_async(&img, 0.5).await.unwrap_err();
        assert!(
            matches!(err, VisionError::Ocr(_)),
            "zero-size image ROI extraction must produce Ocr error, got: {err:?}"
        );
    }

    #[test]
    fn extract_regions_empty_image_returns_error() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));
        let err = extractor.extract_regions(&img).unwrap_err();
        assert!(
            matches!(err, VisionError::Ocr(_)),
            "empty image extract_regions must produce Ocr error, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn extract_regions_async_empty_image_returns_error() {
        let extractor = OcrExtractor::new(None);
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(0, 0));
        let err = extractor.extract_regions_async(&img).await.unwrap_err();
        assert!(
            matches!(err, VisionError::Ocr(_)),
            "empty image extract_regions_async must produce Ocr error, got: {err:?}"
        );
    }

    #[test]
    fn ocr_region_has_valid_bbox_fields() {
        use maekon_core::models::frame::{BoundingBox, OcrRegion};
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
