//! Image preprocessing for ONNX GUI classifier.
//!
//! Converts RGBA image crops to the model's expected input format:
//! `[1, 3, 64, 64]` float32 tensor in CHW layout, normalized to [0, 1].

use oneshim_core::error::CoreError;

/// Target input size for the classifier model.
const TARGET_SIZE: u32 = 64;

/// Convert an RGBA image crop to a float32 CHW tensor normalized to [0, 1].
///
/// Steps:
/// 1. Create an `image::RgbaImage` from raw bytes
/// 2. Resize to 64×64 using nearest-neighbor (fast, adequate for classification)
/// 3. Convert to RGB channels-first (CHW) layout
/// 4. Normalize pixel values to [0.0, 1.0]
pub fn prepare_input(rgba: &[u8], width: u32, height: u32) -> Result<Vec<f32>, CoreError> {
    let img = image::RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or_else(|| {
        // Iter-105: caller passed mismatched buffer + dimensions — this is
        // input validation, not an internal failure. Aligns with iter-95
        // quantization/binary_quantizer fixes (wire: validation.invalid_arguments).
        CoreError::InvalidArguments {
            code: oneshim_core::error_codes::ValidationCode::InvalidArguments,
            message: format!(
                "invalid RGBA image: expected {} bytes, got {}",
                width * height * 4,
                rgba.len()
            ),
        }
    })?;

    let resized = image::imageops::resize(
        &img,
        TARGET_SIZE,
        TARGET_SIZE,
        image::imageops::FilterType::Nearest,
    );

    // Convert to CHW float32: [C=3, H=64, W=64]
    let pixels = TARGET_SIZE as usize * TARGET_SIZE as usize;
    let mut output = vec![0.0f32; 3 * pixels];

    for (i, pixel) in resized.pixels().enumerate() {
        output[i] = pixel[0] as f32 / 255.0; // R channel
        output[pixels + i] = pixel[1] as f32 / 255.0; // G channel
        output[2 * pixels + i] = pixel[2] as f32 / 255.0; // B channel
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_input_correct_dimensions() {
        let rgba = vec![128u8; 4 * 32 * 32]; // 32x32 RGBA
        let result = prepare_input(&rgba, 32, 32).unwrap();
        assert_eq!(result.len(), 3 * 64 * 64); // CHW: 3 channels × 64×64
    }

    #[test]
    fn prepare_input_normalized_range() {
        // All white pixels (255, 255, 255, 255)
        let rgba = vec![255u8; 4 * 8 * 8];
        let result = prepare_input(&rgba, 8, 8).unwrap();
        assert!(result.iter().all(|&v| (v - 1.0).abs() < f32::EPSILON));

        // All black pixels (0, 0, 0, 255)
        let mut rgba_black = vec![0u8; 4 * 8 * 8];
        for chunk in rgba_black.chunks_mut(4) {
            chunk[3] = 255; // alpha
        }
        let result = prepare_input(&rgba_black, 8, 8).unwrap();
        assert!(result.iter().all(|&v| v.abs() < f32::EPSILON));
    }

    #[test]
    fn prepare_input_rejects_invalid_size() {
        let rgba = vec![0u8; 100]; // too small for claimed dimensions
        let result = prepare_input(&rgba, 32, 32);
        assert!(result.is_err());
    }

    /// Iter-105 regression guard: buffer-dimension mismatch emits the
    /// `validation.invalid_arguments` wire code, not `internal.generic`.
    /// Aligns with iter-95's quantization/binary_quantizer fixes.
    #[test]
    fn prepare_input_mismatch_emits_invalid_arguments_code() {
        let rgba = vec![0u8; 100];
        let err = prepare_input(&rgba, 32, 32).unwrap_err();
        assert_eq!(err.code(), "validation.invalid_arguments");
    }
}
