//! Windows native OCR via WinRT `Windows.Media.Ocr.OcrEngine`.
//!
//! Available since Windows 10 1507. GPU-accelerated, zero external
//! dependencies — uses the OS-shipped language packs from the user profile.
//!
//! WinRT async operations (`IAsyncOperation`) are resolved synchronously
//! via `.join()` inside a `spawn_blocking` context. The `windows` crate
//! auto-initializes COM MTA on first WinRT factory activation.
//!
//! Stock Windows' WIC (`Windows::Graphics::Imaging::BitmapDecoder`) has no
//! built-in WebP codec, so WebP frames from the capture pipeline
//! (`processor.rs`) cannot be decoded via `BitmapDecoder::CreateAsync` —
//! that call errors on every WebP frame, producing zero OCR regions on
//! every capture (#7574). WebP input is instead decoded ourselves via the
//! platform-independent `super::webp_decode::decode_to_rgba` helper and
//! handed to WinRT as a raw BGRA8 pixel buffer
//! (`SoftwareBitmap::CreateCopyWithAlphaFromBuffer`), bypassing WIC
//! entirely for that format. Formats WIC already ships a codec for (PNG,
//! BMP, JPEG, ...) keep using the original `BitmapDecoder` path.

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::ports::ocr_provider::{OcrProvider, OcrResult};

/// Windows WinRT native OCR provider.
pub(crate) struct WindowsNativeOcr;

impl WindowsNativeOcr {
    /// Synchronous OCR via WinRT. Called from `spawn_blocking`.
    fn recognize_text_blocking(
        image_data: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        // 1. Create OcrEngine from user profile languages
        let engine =
            windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages().map_err(|e| {
                CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("OcrEngine creation failed: {e}"),
                }
            })?;

        // 2. Decode image bytes to SoftwareBitmap
        let bitmap = Self::decode_to_software_bitmap(image_data, image_format)?;

        // 3. Run OCR
        let ocr_result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("RecognizeAsync failed: {e}"),
            })?
            .join()
            .map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("RecognizeAsync get failed: {e}"),
            })?;

        // 4. Extract results — iterate lines → words with bounding rectangles
        let mut results = Vec::new();
        let lines = ocr_result.Lines().map_err(|e| CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Lines failed: {e}"),
        })?;
        for line in &lines {
            let words = line.Words().map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Words failed: {e}"),
            })?;
            for word in &words {
                let text = word
                    .Text()
                    .map_err(|e| CoreError::OcrError {
                        code: maekon_core::error_codes::ProviderCode::OcrFailed,
                        message: format!("Text failed: {e}"),
                    })?
                    .to_string_lossy();
                let rect = word.BoundingRect().map_err(|e| CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
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

    /// Decode `image_data` (encoded as `image_format`) into a WinRT
    /// `SoftwareBitmap` ready for `OcrEngine::RecognizeAsync`.
    ///
    /// WebP frames route through `webp_to_software_bitmap` (manual decode,
    /// since WIC has no WebP codec — #7574). Every other format keeps using
    /// the original `InMemoryRandomAccessStream` + `BitmapDecoder` path,
    /// which WIC natively supports (PNG, BMP, JPEG, ...).
    fn decode_to_software_bitmap(
        image_data: &[u8],
        image_format: &str,
    ) -> Result<windows::Graphics::Imaging::SoftwareBitmap, CoreError> {
        if image_format.eq_ignore_ascii_case("webp") {
            return Self::webp_to_software_bitmap(image_data);
        }

        // Write bytes into InMemoryRandomAccessStream, then decode via BitmapDecoder.
        let stream = windows::Storage::Streams::InMemoryRandomAccessStream::new().map_err(|e| {
            CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("Stream creation failed: {e}"),
            }
        })?;
        {
            let writer =
                windows::Storage::Streams::DataWriter::CreateDataWriter(&stream).map_err(|e| {
                    CoreError::OcrError {
                        code: maekon_core::error_codes::ProviderCode::OcrFailed,
                        message: format!("DataWriter failed: {e}"),
                    }
                })?;
            writer
                .WriteBytes(image_data)
                .map_err(|e| CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("WriteBytes failed: {e}"),
                })?;
            writer
                .StoreAsync()
                .map_err(|e| CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("StoreAsync failed: {e}"),
                })?
                .join()
                .map_err(|e| CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("StoreAsync get failed: {e}"),
                })?;
            writer
                .FlushAsync()
                .map_err(|e| CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("FlushAsync failed: {e}"),
                })?
                .join()
                .map_err(|e| CoreError::OcrError {
                    code: maekon_core::error_codes::ProviderCode::OcrFailed,
                    message: format!("FlushAsync get failed: {e}"),
                })?;
            writer.DetachStream().map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("DetachStream failed: {e}"),
            })?;
        }

        // Reset stream position to start
        stream.Seek(0).map_err(|e| CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("Seek failed: {e}"),
        })?;

        let decoder = windows::Graphics::Imaging::BitmapDecoder::CreateAsync(&stream)
            .map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("BitmapDecoder failed: {e}"),
            })?
            .join()
            .map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("BitmapDecoder get failed: {e}"),
            })?;

        decoder
            .GetSoftwareBitmapAsync()
            .map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("GetSoftwareBitmap failed: {e}"),
            })?
            .join()
            .map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("GetSoftwareBitmap get failed: {e}"),
            })
    }

    /// Decode WebP bytes to raw RGBA via the platform-independent
    /// `super::webp_decode::decode_to_rgba` helper, then build a
    /// `SoftwareBitmap` directly from the pixel buffer — no WIC codec
    /// involved, so this works regardless of what image codecs are
    /// installed on the machine.
    ///
    /// `OcrEngine::RecognizeAsync` only accepts `Gray8`, `Nv12`, or `Bgra8`
    /// `SoftwareBitmap`s, so the decoded RGBA buffer is converted to BGRA8
    /// (byte-swap red/blue, alpha stays last) before
    /// `CreateCopyWithAlphaFromBuffer`.
    fn webp_to_software_bitmap(
        image_data: &[u8],
    ) -> Result<windows::Graphics::Imaging::SoftwareBitmap, CoreError> {
        let (width, height, mut pixels) = super::webp_decode::decode_to_rgba(image_data, "webp")?;

        // RGBA -> BGRA in place: swap the R and B bytes of every pixel.
        // Alpha values come straight from the decoded image (not
        // premultiplied), so this pairs with `BitmapAlphaMode::Straight`
        // below.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }

        let writer =
            windows::Storage::Streams::DataWriter::new().map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("DataWriter creation failed (webp): {e}"),
            })?;
        writer
            .WriteBytes(&pixels)
            .map_err(|e| CoreError::OcrError {
                code: maekon_core::error_codes::ProviderCode::OcrFailed,
                message: format!("WriteBytes failed (webp BGRA): {e}"),
            })?;
        let buffer = writer.DetachBuffer().map_err(|e| CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("DetachBuffer failed (webp): {e}"),
        })?;

        windows::Graphics::Imaging::SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
            &buffer,
            windows::Graphics::Imaging::BitmapPixelFormat::Bgra8,
            width as i32,
            height as i32,
            windows::Graphics::Imaging::BitmapAlphaMode::Straight,
        )
        .map_err(|e| CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("CreateCopyWithAlphaFromBuffer failed (webp): {e}"),
        })
    }
}

#[async_trait]
impl OcrProvider for WindowsNativeOcr {
    async fn extract_elements(
        &self,
        image: &[u8],
        image_format: &str,
    ) -> Result<Vec<OcrResult>, CoreError> {
        let data = image.to_vec();
        let format = image_format.to_string();
        tokio::task::spawn_blocking(move || Self::recognize_text_blocking(&data, &format))
            .await
            .map_err(|e| CoreError::Internal {
                code: maekon_core::error_codes::InternalCode::Generic,
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
