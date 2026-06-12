use maekon_core::models::frame::{BoundingBox, OcrRegion};

fn normalized_scale(scale_factor: Option<f64>) -> Option<f64> {
    let scale = scale_factor?;
    if scale.is_finite() && scale > 1.0 {
        Some(scale)
    } else {
        None
    }
}

fn scale_coord(value: u32, scale: f64) -> u32 {
    ((value as f64) / scale).round().max(0.0) as u32
}

/// Convert OCR regions from captured physical pixels into logical pixels.
///
/// Retina/HiDPI captures can arrive at 2x physical resolution while window and
/// overlay coordinates are expressed in logical pixels. This helper keeps OCR
/// boxes aligned with the rest of the GUI coordinate system.
pub fn scale_ocr_regions_to_logical(
    regions: &[OcrRegion],
    scale_factor: Option<f64>,
) -> Vec<OcrRegion> {
    let Some(scale) = normalized_scale(scale_factor) else {
        return regions.to_vec();
    };

    regions
        .iter()
        .map(|region| OcrRegion {
            text: region.text.clone(),
            bbox: BoundingBox {
                x: scale_coord(region.bbox.x, scale),
                y: scale_coord(region.bbox.y, scale),
                width: scale_coord(region.bbox.width, scale),
                height: scale_coord(region.bbox.height, scale),
            },
            confidence: region.confidence,
        })
        .collect()
}
