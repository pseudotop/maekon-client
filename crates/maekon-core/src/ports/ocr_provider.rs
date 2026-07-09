//! OCR provider port — defines the contract for extracting text elements
//! and their bounding boxes from screen capture images.
//! Implemented by `RemoteOcrProvider` in `maekon-network` and
//! `OcrExtractor` (Tesseract) in `maekon-vision`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OcrResult {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub confidence: f64,
}

/// OCR provider port — extracts text elements and bounding boxes from
/// screen capture images.
///
/// # Errors
/// - `CoreError::OcrError` (wire: `provider.ocr_failed`) for provider-side
///   failures: malformed OCR response, empty output, library panic
///   (leptonica/tesseract). This covers both remote OCR endpoints and
///   local Tesseract invocation.
/// - HTTP-layer failures (remote OCR) follow the canonical semantic
///   status mapping: `CoreError::Auth` (401/403), `CoreError::RequestTimeout`
///   (408/504), `CoreError::RateLimit` (429), `CoreError::ServiceUnavailable`
///   (502/503). See `docs/guides/http-status-error-mapping.md`.
/// - `CoreError::PermissionDenied` (wire: `permission.permission_denied`) —
///   emitted upstream if screen-capture permission is missing; not
///   raised inside this port itself, but surfaces through callers that
///   feed it images captured without permission.
/// - `CoreError::Network` (wire: `network.generic`) for pre-response
///   transport failures (DNS, connection refused) against remote OCR
///   providers.
#[async_trait]
pub trait OcrProvider: Send + Sync {
    async fn extract_elements(
        &self,
        image: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError>;

    fn provider_name(&self) -> &str;

    fn is_external(&self) -> bool;

    /// Network endpoints that may carry OCR images or extracted text.
    ///
    /// In-process/local providers return an empty list. Providers that report
    /// `is_external() == false` while still using an endpoint must expose that
    /// URL so guard decorators can independently verify that every such
    /// endpoint is loopback before allowing an unguarded local fast path.
    fn egress_endpoint_urls(&self) -> Vec<&str> {
        Vec::new()
    }
}

/// Canonical `OcrProvider` test double (#7729 ctd-W2 G2).
///
/// Deterministic, local-only stand-in that returns fixed `results` regardless
/// of the image bytes, so pipeline tests don't depend on a real platform OCR
/// engine (unavailable on headless CI / Linux). Consolidates two byte-for-byte
/// identical hand-rolled `FakeOcrProvider` copies previously duplicated in
/// `maekon-vision` (`processor.rs`, `privacy_gateway.rs`). Deliberately does
/// NOT attempt to also cover the egress-boundary-security `Fake*OcrProvider`
/// variants in `src-tauri/src/provider_adapters/tests.rs` — those exercise
/// `is_external()` / `egress_endpoint_urls()` boundary semantics that are
/// meaningfully different per test case, not incidental duplication.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone)]
pub struct FakeOcrProvider {
    results: Vec<OcrResult>,
    provider_name: &'static str,
    is_external: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl FakeOcrProvider {
    /// Local (`is_external() == false`) fake returning `results` for every call.
    pub fn new(results: Vec<OcrResult>) -> Self {
        Self {
            results,
            provider_name: "fake-native-ocr",
            is_external: false,
        }
    }

    /// Override the default `provider_name()` (`"fake-native-ocr"`).
    pub fn with_provider_name(mut self, name: &'static str) -> Self {
        self.provider_name = name;
        self
    }

    /// Mark this fake as an external provider (`is_external() == true`).
    pub fn external(mut self) -> Self {
        self.is_external = true;
        self
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl OcrProvider for FakeOcrProvider {
    async fn extract_elements(
        &self,
        _image: &[u8],
        _image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        Ok(self.results.clone())
    }

    fn provider_name(&self) -> &str {
        self.provider_name
    }

    fn is_external(&self) -> bool {
        self.is_external
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_result_serde() {
        let result = OcrResult {
            text: "save".to_string(),
            x: 100,
            y: 200,
            width: 60,
            height: 25,
            confidence: 0.92,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: OcrResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.text, "save");
        assert_eq!(deser.x, 100);
        assert!((deser.confidence - 0.92).abs() < f64::EPSILON);
    }

    #[test]
    fn ocr_result_vec_serde() {
        let results = vec![
            OcrResult {
                text: "file".to_string(),
                x: 0,
                y: 0,
                width: 40,
                height: 20,
                confidence: 0.88,
            },
            OcrResult {
                text: "edit".to_string(),
                x: 50,
                y: 0,
                width: 40,
                height: 20,
                confidence: 0.91,
            },
        ];
        let json = serde_json::to_string(&results).unwrap();
        let deser: Vec<OcrResult> = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.len(), 2);
        assert_eq!(deser[1].text, "edit");
    }
}
