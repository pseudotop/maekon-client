use std::sync::Arc;

#[cfg(feature = "server")]
use async_trait::async_trait;
use oneshim_core::config::OcrValidationConfig;
use oneshim_core::error::CoreError;
use oneshim_core::ports::ocr_provider::OcrProvider;
use oneshim_core::ports::ocr_provider::OcrResult;
#[cfg(feature = "server")]
use tracing::debug;

use super::types::ExternalOcrPrivacyGuard;

#[cfg_attr(not(feature = "server"), allow(dead_code))]
pub(super) struct GuardedOcrProvider {
    inner: Arc<dyn OcrProvider>,
    privacy_guard: ExternalOcrPrivacyGuard,
    allow_unredacted_external_ocr: bool,
    ocr_validation: OcrValidationConfig,
}

#[cfg_attr(not(feature = "server"), allow(dead_code))]
impl GuardedOcrProvider {
    pub(super) fn new(
        inner: Arc<dyn OcrProvider>,
        privacy_guard: ExternalOcrPrivacyGuard,
        allow_unredacted_external_ocr: bool,
        ocr_validation: OcrValidationConfig,
    ) -> Self {
        Self {
            inner,
            privacy_guard,
            allow_unredacted_external_ocr,
            ocr_validation,
        }
    }

    fn validate_ocr_results(&self, results: Vec<OcrResult>) -> Result<Vec<OcrResult>, CoreError> {
        if !self.ocr_validation.enabled || results.is_empty() {
            return Ok(results);
        }

        let total = results.len();
        let mut invalid = 0usize;
        let mut filtered = Vec::with_capacity(total);

        for mut result in results {
            let text = result.text.trim();
            let is_valid_geometry =
                result.x >= 0 && result.y >= 0 && result.width > 0 && result.height > 0;
            let is_valid_confidence =
                result.confidence.is_finite() && (0.0..=1.0).contains(&result.confidence);

            if text.is_empty()
                || !is_valid_geometry
                || !is_valid_confidence
                || result.confidence < self.ocr_validation.min_confidence
            {
                invalid += 1;
                continue;
            }

            result.text = text.to_string();
            filtered.push(result);
        }

        let invalid_ratio = invalid as f64 / total as f64;
        if invalid_ratio > self.ocr_validation.max_invalid_ratio {
            return Err(CoreError::OcrError { code: oneshim_core::error_codes::ProviderCode::OcrFailed, message: format!(
                "OCR calibration validation failure: invalid_ratio={invalid_ratio:.2}, max_invalid_ratio={:.2}",
                self.ocr_validation.max_invalid_ratio
            ) });
        }

        Ok(filtered)
    }
}

#[cfg(feature = "server")]
#[async_trait]
impl OcrProvider for GuardedOcrProvider {
    async fn extract_elements(
        &self,
        image: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        if !self.inner.is_external() {
            return self.inner.extract_elements(image, image_format).await;
        }

        let sanitized = self
            .privacy_guard
            .prepare_image_for_external(
                image,
                self.inner.provider_name(),
                self.allow_unredacted_external_ocr,
            )
            .await?;

        debug!(
            redacted_regions = sanitized.redacted_regions,
            allow_unredacted_external_ocr = self.allow_unredacted_external_ocr,
            "External OCR image sanitization completed"
        );

        let results = self
            .inner
            .extract_elements(&sanitized.image_data, image_format)
            .await?;
        self.validate_ocr_results(results)
    }

    fn provider_name(&self) -> &str {
        self.inner.provider_name()
    }

    fn is_external(&self) -> bool {
        self.inner.is_external()
    }
}
