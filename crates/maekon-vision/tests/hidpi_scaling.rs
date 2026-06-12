use maekon_core::models::frame::{BoundingBox, OcrRegion};
use maekon_vision::ocr_geometry::scale_ocr_regions_to_logical;

#[test]
fn crt_prv_cap_009_hidpi_physical_ocr_boxes_become_logical_pixels() {
    let regions = vec![OcrRegion {
        text: "Hello".to_string(),
        bbox: BoundingBox {
            x: 200,
            y: 100,
            width: 160,
            height: 48,
        },
        confidence: 0.97,
    }];

    let scaled = scale_ocr_regions_to_logical(&regions, Some(2.0));

    assert_eq!(scaled.len(), 1);
    assert_eq!(scaled[0].text, "Hello");
    assert_eq!(scaled[0].confidence, 0.97);
    assert_eq!(
        scaled[0].bbox,
        BoundingBox {
            x: 100,
            y: 50,
            width: 80,
            height: 24,
        },
        "HiDPI OCR boxes must be exposed in logical pixels, not raw physical pixels"
    );
}
