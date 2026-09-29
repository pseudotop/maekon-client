//! Platform-independent WebP → raw RGBA8 decode helper.
//!
//! Extracted into its own module (unconditionally compiled, unlike
//! `windows.rs`/`macos.rs`) so the decode path can be unit-tested on any
//! development host, including macOS. Stock Windows has no built-in WebP
//! codec in WIC (`Windows::Graphics::Imaging::BitmapDecoder`), so
//! `WindowsNativeOcr` (`windows.rs`) cannot hand WebP bytes to
//! `BitmapDecoder::CreateAsync` — it must decode WebP itself first and
//! build a `SoftwareBitmap` from raw pixels instead (#7574).

use maekon_core::error::CoreError;

/// Decode `bytes` (encoded as `format`) into raw RGBA8 pixel data.
///
/// Returns `(width, height, rgba_pixels)` where
/// `rgba_pixels.len() == width as usize * height as usize * 4`.
///
/// Only `"webp"` is currently supported — this is the one capture format
/// stock Windows' WIC cannot decode natively, so it is the only format the
/// Windows native-OCR glue needs a manual decode path for. Every other
/// format WIC already ships a codec for (PNG, BMP, JPEG, ...) keeps using
/// `BitmapDecoder` directly and never reaches this helper.
pub fn decode_to_rgba(bytes: &[u8], format: &str) -> Result<(u32, u32, Vec<u8>), CoreError> {
    match format {
        "webp" => {
            let decoded =
                webp::Decoder::new(bytes)
                    .decode()
                    .ok_or_else(|| CoreError::OcrError {
                        code: maekon_core::error_codes::ProviderCode::OcrFailed,
                        message: "WebP decode failed: invalid or unsupported bitstream".to_string(),
                    })?;
            // `to_image()` always yields RGB8 or RGBA8 depending on whether the
            // source had an alpha channel; normalize to RGBA8 so callers get a
            // fixed 4-byte-per-pixel layout regardless of the source.
            let rgba = decoded.to_image().to_rgba8();
            let (width, height) = (rgba.width(), rgba.height());
            Ok((width, height, rgba.into_raw()))
        }
        other => Err(CoreError::OcrError {
            code: maekon_core::error_codes::ProviderCode::OcrFailed,
            message: format!("decode_to_rgba: unsupported image format `{other}`"),
        }),
    }
}

/// Swap the red and blue bytes of every whole pixel in place, turning RGBA8
/// into the BGRA8 layout `OcrEngine::RecognizeAsync` accepts on Windows.
///
/// Lives here rather than in `windows.rs` so any development host tests it
/// (#12500). A trailing partial pixel, which `decode_to_rgba` never returns,
/// is left untouched, as the previous `chunks_exact_mut(4)` loop did.
pub fn rgba_to_bgra_in_place(pixels: &mut [u8]) {
    let (whole_pixels, _partial) = pixels.as_chunks_mut::<4>();
    for pixel in whole_pixels {
        pixel.swap(0, 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_to_bgra_swaps_red_and_blue_and_keeps_alpha() {
        let mut pixels = vec![10, 20, 30, 255, 1, 2, 3, 0];
        rgba_to_bgra_in_place(&mut pixels);
        assert_eq!(pixels, [30, 20, 10, 255, 3, 2, 1, 0]);
    }

    #[test]
    fn rgba_to_bgra_leaves_a_trailing_partial_pixel_and_empty_input_alone() {
        // Same contract as the `chunks_exact_mut(4)` loop it replaces.
        let mut pixels = vec![10, 20, 30, 40, 7, 8, 9];
        rgba_to_bgra_in_place(&mut pixels);
        assert_eq!(pixels, [30, 20, 10, 40, 7, 8, 9]);
        let mut empty: Vec<u8> = Vec::new();
        rgba_to_bgra_in_place(&mut empty);
        assert!(empty.is_empty());
    }

    /// Encode a tiny known RGBA image to WebP (same encoder
    /// `encoder::encode_webp` uses in the capture pipeline), then decode it
    /// back through `decode_to_rgba` and assert dimensions + non-empty RGBA
    /// output. This is a regression test for #7574: `decode_to_rgba` is the
    /// exact decode path `windows.rs::webp_to_software_bitmap` relies on to
    /// feed OCR pixels to WinRT.
    ///
    /// Before this fix, `WindowsNativeOcr` handed the same WebP bytes
    /// straight to `windows::Graphics::Imaging::BitmapDecoder::CreateAsync`
    /// (WIC). Stock Windows has no built-in WebP codec, so `CreateAsync`
    /// errors on every single frame — the pre-fix code path never reaches a
    /// valid `SoftwareBitmap` at all, so `OcrEngine::RecognizeAsync` is never
    /// invoked and every capture on Windows silently produces zero OCR
    /// regions. This test proves the replacement decode path (this helper)
    /// actually produces usable RGBA pixels from a WebP payload, which is
    /// the missing link that made the old WIC-only path a dead end.
    #[test]
    fn decode_to_rgba_round_trips_a_tiny_webp_image() {
        let width = 4u32;
        let height = 3u32;
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                rgba.push(((x * 40) % 256) as u8);
                rgba.push(((y * 60) % 256) as u8);
                rgba.push(128);
                rgba.push(255);
            }
        }
        let encoder = webp::Encoder::from_rgba(&rgba, width, height);
        let encoded = encoder.encode(80.0);
        let encoded_bytes = encoded.to_vec();

        let (decoded_width, decoded_height, decoded_rgba) = decode_to_rgba(&encoded_bytes, "webp")
            .expect("decode_to_rgba must succeed for a valid WebP payload");

        assert_eq!(decoded_width, width);
        assert_eq!(decoded_height, height);
        assert_eq!(decoded_rgba.len(), (width * height * 4) as usize);
        assert!(!decoded_rgba.is_empty());
    }

    #[test]
    fn decode_to_rgba_rejects_garbage_bytes() {
        let err = decode_to_rgba(&[0u8, 1, 2, 3], "webp")
            .expect_err("garbage bytes are not a valid WebP bitstream");
        let CoreError::OcrError { message, .. } = err else {
            panic!("expected CoreError::OcrError, got {err:?}");
        };
        assert!(message.contains("WebP decode failed"));
    }

    #[test]
    fn decode_to_rgba_rejects_unsupported_format() {
        let err = decode_to_rgba(&[], "bmp")
            .expect_err("unsupported format must error, not silently decode");
        let CoreError::OcrError { message, .. } = err else {
            panic!("expected CoreError::OcrError, got {err:?}");
        };
        assert!(message.contains("bmp"));
    }
}
