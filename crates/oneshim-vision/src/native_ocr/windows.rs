//! Windows native OCR via WinRT `Windows.Media.Ocr.OcrEngine`.
//!
//! Available since Windows 10 1507. GPU-accelerated, zero external
//! dependencies — uses the OS-shipped language packs from the user profile.
//!
//! WinRT async operations (`IAsyncOperation`) are resolved synchronously
//! via `.GetResults()` inside a `spawn_blocking` context. The `windows` crate
//! auto-initializes COM MTA on first WinRT factory activation.

use async_trait::async_trait;
use oneshim_core::error::CoreError;
use oneshim_core::ports::ocr_provider::{OcrProvider, OcrResult};

/// Windows WinRT native OCR provider.
pub(crate) struct WindowsNativeOcr;

impl WindowsNativeOcr {
    /// Synchronous OCR via WinRT. Called from `spawn_blocking`.
    fn recognize_text_blocking(image_data: &[u8]) -> Result<Vec<OcrResult>, CoreError> {
        // 1. Create OcrEngine from user profile languages
        let engine =
            windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
                CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("OcrEngine creation failed: {e}"),
                }
            })?;

        // 2. Decode image bytes to SoftwareBitmap
        //    Write bytes into InMemoryRandomAccessStream, then decode via BitmapDecoder
        let stream = windows::Storage::Streams::InMemoryRandomAccessStream::new().map_err(|e| {
            CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Stream creation failed: {e}"),
            }
        })?;
        {
            let writer =
                windows::Storage::Streams::DataWriter::CreateDataWriter(&stream).map_err(|e| {
                    CoreError::OcrError {
                        code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                        message: format!("DataWriter failed: {e}"),
                    }
                })?;
            writer
                .WriteBytes(image_data)
                .map_err(|e| CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("WriteBytes failed: {e}"),
                })?;
            writer
                .StoreAsync()
                .map_err(|e| CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("StoreAsync failed: {e}"),
                })?
                .GetResults()
                .map_err(|e| CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("StoreAsync get failed: {e}"),
                })?;
            writer
                .FlushAsync()
                .map_err(|e| CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("FlushAsync failed: {e}"),
                })?
                .GetResults()
                .map_err(|e| CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("FlushAsync get failed: {e}"),
                })?;
            writer.DetachStream().map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("DetachStream failed: {e}"),
            })?;
        }

        // Reset stream position to start
        stream.Seek(0).map_err(|e| CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Seek failed: {e}"),
        })?;

        let decoder = windows::Graphics::Imaging::BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("BitmapDecoder failed: {e}"),
            })?
            .GetResults()
            .map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("BitmapDecoder get failed: {e}"),
            })?;

        let bitmap = decoder
            .GetSoftwareBitmapAsync()
            .map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("GetSoftwareBitmap failed: {e}"),
            })?
            .GetResults()
            .map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("GetSoftwareBitmap get failed: {e}"),
            })?;

        // 3. Run OCR
        let ocr_result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("RecognizeAsync failed: {e}"),
            })?
            .GetResults()
            .map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("RecognizeAsync get failed: {e}"),
            })?;

        // 4. Extract results — iterate lines → words with bounding rectangles
        let mut results = Vec::new();
        let lines = ocr_result.Lines().map_err(|e| CoreError::OcrError {
            code: oneshim_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Lines failed: {e}"),
        })?;
        for line in &lines {
            let words = line.Words().map_err(|e| CoreError::OcrError {
                code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Words failed: {e}"),
            })?;
            for word in &words {
                let text = word
                    .Text()
                    .map_err(|e| CoreError::OcrError {
                        code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                        message: format!("Text failed: {e}"),
                    })?
                    .to_string_lossy();
                let rect = word.BoundingRect().map_err(|e| CoreError::OcrError {
                    code: oneshim_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("BoundingRect failed: {e}"),
                })?;
                results.push(OcrResult {
                    text,
                    x: rect.X.round() as i32,
                    y: rect.Y.round() as i32,
                    width: rect.Width.round() as u32,
                    height: rect.Height.round() as u32,
                    confidence: 1.0, // WinRT OCR does not expose per-word confidence
                });
            }
        }

        Ok(results)
    }
}

#[async_trait]
impl OcrProvider for WindowsNativeOcr {
    async fn extract_elements(
        &self,
        image: &[u8],
        _image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        let data = image.to_vec();
        tokio::task::spawn_blocking(move || Self::recognize_text_blocking(&data))
            .await
            .map_err(|e| CoreError::Internal {
                code: oneshim_core::error_codes::InternalCode::Generic,
                message: format!("Windows OCR task join error: {e}"),
            })?
    }

    fn provider_name(&self) -> &str {
        "windows-media-ocr"
    }

    fn is_external(&self) -> bool {
        false
    }
}
