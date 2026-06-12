use async_trait::async_trait;

use maekon_core::error::CoreError;
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};

pub struct LocalOcrProvider;

impl LocalOcrProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for LocalOcrProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl OcrProvider for LocalOcrProvider {
    async fn extract_elements(
        &self,
        image: &[u8],
        _image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        #[cfg(feature = "ocr")]
        {
            use crate::ocr::OcrExtractor;

            let img = image::load_from_memory(image).map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Image decoding failed: {e}"),
            })?;

            let extractor = OcrExtractor::new(None);
            let word_boxes = extractor
                .extract_words_with_boxes(&img)
                .await
                .map_err(|e| CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("OCR extraction failed: {e}"),
                })?;

            let results: Vec<OcrResult> = word_boxes
                .into_iter()
                .map(|wb| OcrResult {
                    text: wb.text,
                    x: wb.x,
                    y: wb.y,
                    width: wb.w.max(0) as u32,
                    height: wb.h.max(0) as u32,
                    confidence: 0.0, // Tesseract word-level confidence requires extra API
                })
                .collect();

            Ok(results)
        }

        #[cfg(not(feature = "ocr"))]
        {
            let _ = image;
            Ok(vec![])
        }
    }

    fn provider_name(&self) -> &str {
        "local-tesseract"
    }

    fn is_external(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_ocr_provider_name() {
        let provider = LocalOcrProvider::new();
        assert_eq!(provider.provider_name(), "local-tesseract");
        assert!(!provider.is_external());
    }

    #[tokio::test]
    async fn local_ocr_provider_invalid_image() {
        let provider = LocalOcrProvider::new();
        let result = provider.extract_elements(b"fake-image", "png").await;
        #[cfg(not(feature = "ocr"))]
        {
            // Without the "ocr" feature the provider is a no-op stub: contract is Ok(empty vec).
            let elements =
                result.expect("stub LocalOcrProvider must return Ok even for invalid image bytes");
            assert!(
                elements.is_empty(),
                "stub LocalOcrProvider must return an empty element list (got {})",
                elements.len()
            );
        }
        #[cfg(feature = "ocr")]
        {
            let err = result.unwrap_err();
            assert!(
                matches!(err, CoreError::OcrError { .. }),
                "invalid image bytes must produce OcrError, got: {err:?}"
            );
        }
    }
}
