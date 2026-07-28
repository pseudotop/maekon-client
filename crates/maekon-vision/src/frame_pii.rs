//! Screenshot-frame pixel-level PII redaction (#8042).
//!
//! The capture pipeline already masks OCR *text* (`ProcessedFrame.ocr_text` and
//! the per-region `ocr_regions[].text`) with the configured [`PiiFilterLevel`],
//! but before this module the stored screenshot *pixels* were written
//! byte-for-byte: a card number / SSN / private message legible on screen landed
//! unmasked in the local frame store. At-rest AES-GCM encryption (#8039) is the
//! only prior mitigation, and it does not cover replay-viewing, automation OCR,
//! or any future egress of the decrypted frame.
//!
//! This module closes that gap by destructively masking the pixel regions of PII
//! text that OCR already located, reusing the same PII predicate
//! ([`is_sensitive_segment_with_level`]) as the text path and the external-OCR
//! egress path.
//!
//! ## Coordinate contract (HiDPI)
//!
//! Callers MUST pass OCR regions in the SAME pixel coordinate space as the image
//! being masked. In the capture pipeline that is the *physical* source-pixel
//! space: the regions produced by the OCR engine and the captured frame are both
//! physical pixels, so masking is applied BEFORE
//! [`crate::ocr_geometry::scale_ocr_regions_to_logical`] converts the exposed
//! `ocr_regions` to logical pixels. Feeding logical-scaled boxes to a physical
//! image (or vice versa) would mask the wrong region on HiDPI displays.
//!
//! ## Relationship to [`crate::privacy_gateway`]
//!
//! `PrivacyGateway` runs the analogous detect+mask for the *external OCR egress*
//! boundary, but operates on its own `TextBox`/`SensitiveRegion` types (`i32`
//! coordinates, decoupled from the capture types) and re-runs OCR itself. This
//! module operates directly on the capture pipeline's already-extracted
//! [`OcrRegion`] boxes (no second OCR pass) and shares the [`PiiMarker`] pattern
//! SSOT via [`is_sensitive_segment_with_level`]. The grouping/merge geometry is
//! intentionally parallel to `PrivacyGateway::detect_sensitive_regions`; keep the
//! two in sync when the grouping heuristic changes.

use image::{DynamicImage, Rgba, RgbaImage};

use maekon_core::config::PiiFilterLevel;
use maekon_core::models::frame::{BoundingBox, OcrRegion};

use crate::encoder::{self, WebPQuality};
use crate::error::VisionError;
use crate::privacy::is_sensitive_segment_with_level;

/// A redacted frame ready for storage: `(webp_bytes, rgba_bytes)`. `rgba_bytes`
/// is the matching redacted RGBA buffer, reused as the ML-classifier crop input
/// so the classifier never sees PII pixels the stored WebP no longer contains.
pub type RedactedFrame = (Vec<u8>, Vec<u8>);

/// Vertical tolerance (px) for treating adjacent OCR word boxes as being on the
/// same text line when testing multi-word PII segments. Mirrors the external-OCR
/// path's `line_threshold` (`PrivacyGateway::detect_sensitive_regions`).
const LINE_THRESHOLD_PX: i64 = 14;

/// Maximum horizontal/vertical gap (px) between two sensitive boxes that are
/// still merged into a single redaction rectangle. Mirrors the external-OCR
/// path's `gap` (`PrivacyGateway::merge_sensitive_regions`).
const REGION_MERGE_GAP_PX: u32 = 10;

/// Pixels added on every side of a detected PII box before masking, so the glyph
/// anti-aliasing fringe just outside the OCR box is also covered. Mirrors the
/// external-OCR path's `margin` (`PrivacyGateway::apply_region_redaction`).
const REDACTION_MARGIN_PX: u32 = 4;

/// Detect the merged pixel rectangles of OCR word boxes whose text is PII at the
/// given [`PiiFilterLevel`].
///
/// Returns bounding boxes in the SAME pixel coordinate space as `regions` (see
/// the module-level coordinate contract). Returns an empty vector when
/// `level == PiiFilterLevel::Off` (explicit operator opt-out — the whole frame
/// is left untouched, matching the text path) or when no region is PII.
///
/// Both single-region matches (`admin@corp.com`) and multi-word runs that are
/// only sensitive once concatenated (`4111 1111 1111 1111` split across word
/// boxes) are detected, then overlapping/adjacent matches are merged so a single
/// opaque rectangle covers a contiguous run.
pub fn detect_pii_bounding_boxes(regions: &[OcrRegion], level: PiiFilterLevel) -> Vec<BoundingBox> {
    use std::collections::HashSet;

    if level == PiiFilterLevel::Off || regions.is_empty() {
        return Vec::new();
    }

    // Stable top-to-bottom, left-to-right order so the sliding window groups
    // reading-order-adjacent boxes.
    let mut indexed: Vec<(usize, &OcrRegion)> = regions.iter().enumerate().collect();
    indexed.sort_by_key(|(_, r)| (r.bbox.y, r.bbox.x));

    let mut sensitive: HashSet<usize> = HashSet::new();

    // Single-box matches.
    for (idx, region) in &indexed {
        if is_sensitive_segment_with_level(&region.text, level) {
            sensitive.insert(*idx);
        }
    }

    // Multi-word matches: slide windows of 2..=5 boxes on the same line and test
    // both the space-joined and the concatenated form (numeric PII is frequently
    // split across word boxes with or without separators).
    for window_size in 2..=5 {
        if indexed.len() < window_size {
            break;
        }
        for window in indexed.windows(window_size) {
            let y_min = window
                .iter()
                .map(|(_, r)| i64::from(r.bbox.y))
                .min()
                .unwrap_or(0);
            let y_max = window
                .iter()
                .map(|(_, r)| i64::from(r.bbox.y))
                .max()
                .unwrap_or(0);
            if (y_max - y_min).abs() > LINE_THRESHOLD_PX {
                continue;
            }
            let compact = window
                .iter()
                .map(|(_, r)| r.text.as_str())
                .collect::<Vec<_>>()
                .join("");
            let spaced = window
                .iter()
                .map(|(_, r)| r.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if is_sensitive_segment_with_level(&compact, level)
                || is_sensitive_segment_with_level(&spaced, level)
            {
                for (idx, _) in window {
                    sensitive.insert(*idx);
                }
            }
        }
    }

    if sensitive.is_empty() {
        return Vec::new();
    }

    let raw: Vec<BoundingBox> = regions
        .iter()
        .enumerate()
        .filter(|(idx, _)| sensitive.contains(idx))
        .map(|(_, r)| BoundingBox {
            x: r.bbox.x,
            y: r.bbox.y,
            width: r.bbox.width.max(1),
            height: r.bbox.height.max(1),
        })
        .collect();

    merge_bounding_boxes(raw)
}

/// Merge overlapping or near-adjacent (within [`REGION_MERGE_GAP_PX`]) boxes into
/// their union so a single opaque rectangle covers a contiguous PII run.
fn merge_bounding_boxes(mut boxes: Vec<BoundingBox>) -> Vec<BoundingBox> {
    if boxes.is_empty() {
        return boxes;
    }
    boxes.sort_by_key(|b| (b.y, b.x));

    let mut merged: Vec<BoundingBox> = Vec::new();
    for candidate in boxes {
        let mut absorbed = false;
        for existing in &mut merged {
            let existing_right = existing.x.saturating_add(existing.width);
            let existing_bottom = existing.y.saturating_add(existing.height);
            let candidate_right = candidate.x.saturating_add(candidate.width);
            let candidate_bottom = candidate.y.saturating_add(candidate.height);

            let near_x = candidate.x <= existing_right.saturating_add(REGION_MERGE_GAP_PX)
                && candidate_right.saturating_add(REGION_MERGE_GAP_PX) >= existing.x;
            let near_y = candidate.y <= existing_bottom.saturating_add(REGION_MERGE_GAP_PX)
                && candidate_bottom.saturating_add(REGION_MERGE_GAP_PX) >= existing.y;

            if near_x && near_y {
                let left = existing.x.min(candidate.x);
                let top = existing.y.min(candidate.y);
                let right = existing_right.max(candidate_right);
                let bottom = existing_bottom.max(candidate_bottom);
                existing.x = left;
                existing.y = top;
                existing.width = right.saturating_sub(left).max(1);
                existing.height = bottom.saturating_sub(top).max(1);
                absorbed = true;
                break;
            }
        }
        if !absorbed {
            merged.push(candidate);
        }
    }
    merged
}

/// Destructively overwrite every pixel inside each box (expanded by
/// [`REDACTION_MARGIN_PX`] and clamped to the image bounds) with solid, fully
/// opaque black. Coordinates are in the image's own pixel space. Returns the
/// number of pixels overwritten.
///
/// Uses an irreversible constant fill, not a recoverable blur: the resulting
/// region is zero-variance so no reconstructable PII remains even under
/// deconvolution / CNN deblurring (matches the external-OCR path's #7069
/// treatment).
pub fn redact_pii_bounding_boxes(img: &mut RgbaImage, boxes: &[BoundingBox]) -> usize {
    const FILL: Rgba<u8> = Rgba([0, 0, 0, 255]);
    let (img_w, img_h) = img.dimensions();
    let mut filled = 0usize;

    for b in boxes {
        let x0 = b.x.saturating_sub(REDACTION_MARGIN_PX).min(img_w);
        let y0 = b.y.saturating_sub(REDACTION_MARGIN_PX).min(img_h);
        let x1 =
            b.x.saturating_add(b.width)
                .saturating_add(REDACTION_MARGIN_PX)
                .min(img_w);
        let y1 =
            b.y.saturating_add(b.height)
                .saturating_add(REDACTION_MARGIN_PX)
                .min(img_h);

        for py in y0..y1 {
            for px in x0..x1 {
                img.put_pixel(px, py, FILL);
                filled += 1;
            }
        }
    }
    filled
}

/// Mask `boxes` on a copy of `frame`'s pixels and encode the redacted image to
/// WebP for storage.
///
/// Returns `(webp_bytes, rgba_bytes)`. `rgba_bytes` is the same redacted RGBA
/// buffer, returned so the caller can reuse it as the ML-classifier crop input
/// without a second `to_rgba8()` and, more importantly, so the classifier never
/// sees PII pixels the stored frame no longer contains.
///
/// CPU-heavy (RGBA clone + WebP encode); call from a blocking context
/// (`spawn_blocking`), never on the async reactor.
pub fn encode_frame_with_pii_boxes(
    frame: &DynamicImage,
    boxes: &[BoundingBox],
    quality: WebPQuality,
) -> Result<RedactedFrame, VisionError> {
    let mut rgba = frame.to_rgba8();
    redact_pii_bounding_boxes(&mut rgba, boxes);
    let dynamic = DynamicImage::ImageRgba8(rgba);
    let webp = encoder::encode_webp(&dynamic, quality)?;
    Ok((webp, dynamic.into_rgba8().into_vec()))
}

/// Detect PII in `regions` and, if any is present, return the redacted
/// `(webp_bytes, rgba_bytes)` for the stored frame; return `Ok(None)` when there
/// is nothing to mask (level `Off`, no regions, or no PII detected) so the caller
/// keeps its original encode untouched.
///
/// `regions` MUST be in the same PHYSICAL pixel space as `frame` (module-level
/// coordinate contract). Both the PII text scan and the WebP re-encode are
/// CPU-heavy — call from a blocking context (`spawn_blocking`), never on the
/// async reactor.
pub fn redact_frame_if_pii(
    frame: &DynamicImage,
    regions: &[OcrRegion],
    level: PiiFilterLevel,
    quality: WebPQuality,
) -> Result<Option<RedactedFrame>, VisionError> {
    let boxes = detect_pii_bounding_boxes(regions, level);
    if boxes.is_empty() {
        return Ok(None);
    }
    Ok(Some(encode_frame_with_pii_boxes(frame, &boxes, quality)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(text: &str, x: u32, y: u32, w: u32, h: u32) -> OcrRegion {
        OcrRegion {
            text: text.to_string(),
            bbox: BoundingBox {
                x,
                y,
                width: w,
                height: h,
            },
            confidence: 0.95,
        }
    }

    #[test]
    fn detect_single_email_region() {
        // The benign word is on a different text line (y far apart), so the
        // multi-word sliding window cannot group it with the email — only the
        // email box is returned.
        let regions = vec![
            region("greeting", 10, 20, 60, 18),
            region("admin@company.com", 80, 200, 160, 18),
        ];
        let boxes = detect_pii_bounding_boxes(&regions, PiiFilterLevel::Standard);
        assert_eq!(boxes.len(), 1, "only the email box must be detected");
        assert_eq!(boxes[0].x, 80);
        assert_eq!(boxes[0].y, 200);
    }

    #[test]
    fn detect_merges_benign_word_adjacent_to_pii_on_same_line() {
        // When a benign word is on the SAME line immediately before the email,
        // the space-joined window ("contact admin@company.com") is sensitive, so
        // both boxes are masked and merged — conservative over-masking that
        // matches the external-OCR egress path.
        let regions = vec![
            region("contact", 10, 20, 60, 18),
            region("admin@company.com", 80, 20, 160, 18),
        ];
        let boxes = detect_pii_bounding_boxes(&regions, PiiFilterLevel::Standard);
        assert_eq!(boxes.len(), 1, "adjacent sensitive boxes merge into one");
        assert_eq!(boxes[0].x, 10, "merged box starts at the benign word");
        assert!(
            boxes[0].x.saturating_add(boxes[0].width) >= 240,
            "merged box spans through the email"
        );
    }

    #[test]
    fn detect_returns_empty_when_level_off() {
        let regions = vec![region("admin@company.com", 80, 20, 160, 18)];
        let boxes = detect_pii_bounding_boxes(&regions, PiiFilterLevel::Off);
        assert!(
            boxes.is_empty(),
            "PiiFilterLevel::Off must be an explicit opt-out (no boxes, whole frame kept)"
        );
    }

    #[test]
    fn detect_preserves_non_pii_regions() {
        let regions = vec![
            region("just plain words", 10, 20, 200, 18),
            region("nothing sensitive", 10, 60, 200, 18),
        ];
        let boxes = detect_pii_bounding_boxes(&regions, PiiFilterLevel::Standard);
        assert!(boxes.is_empty(), "non-PII text must not be masked");
    }

    #[test]
    fn detect_multi_word_card_number_split_across_boxes() {
        // A card number split across four adjacent word boxes on the same line
        // is only sensitive once concatenated; the sliding window must catch it.
        let regions = vec![
            region("4111", 10, 40, 40, 18),
            region("1111", 55, 40, 40, 18),
            region("1111", 100, 40, 40, 18),
            region("1111", 145, 40, 40, 18),
        ];
        let boxes = detect_pii_bounding_boxes(&regions, PiiFilterLevel::Standard);
        assert!(
            !boxes.is_empty(),
            "a card number split across word boxes must be detected via the sliding window"
        );
        // The merged rectangle must span from the first to the last box.
        let covering = boxes
            .iter()
            .any(|b| b.x <= 10 && b.x.saturating_add(b.width) >= 185 && b.y <= 40);
        assert!(
            covering,
            "adjacent sensitive boxes must merge into one rectangle: {boxes:?}"
        );
    }

    #[test]
    fn redact_fills_region_opaque_black_and_counts_pixels() {
        let mut img = RgbaImage::from_pixel(100, 60, Rgba([200, 100, 50, 255]));
        let boxes = vec![BoundingBox {
            x: 20,
            y: 20,
            width: 30,
            height: 20,
        }];
        let filled = redact_pii_bounding_boxes(&mut img, &boxes);
        // (30+2*4) * (20+2*4) = 38 * 28 = 1064 pixels.
        assert_eq!(filled, 1064);
        // A pixel inside the box is opaque black; the region is constant.
        assert_eq!(*img.get_pixel(30, 30), Rgba([0, 0, 0, 255]));
        assert_eq!(*img.get_pixel(20, 20), *img.get_pixel(49, 39));
        // A pixel well outside the (margin-expanded) box is untouched.
        assert_eq!(*img.get_pixel(90, 55), Rgba([200, 100, 50, 255]));
    }

    #[test]
    fn redact_clamps_boxes_to_image_bounds() {
        let mut img = RgbaImage::from_pixel(40, 40, Rgba([10, 20, 30, 255]));
        // Box extends past the bottom-right corner; must not panic and must
        // mask only in-bounds pixels.
        let boxes = vec![BoundingBox {
            x: 30,
            y: 30,
            width: 100,
            height: 100,
        }];
        let filled = redact_pii_bounding_boxes(&mut img, &boxes);
        assert!(filled > 0);
        assert_eq!(*img.get_pixel(39, 39), Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn encode_frame_with_pii_boxes_produces_decodable_masked_image() {
        let frame =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(120, 80, Rgba([255, 255, 255, 255])));
        let boxes = vec![BoundingBox {
            x: 40,
            y: 30,
            width: 30,
            height: 20,
        }];
        let (webp, rgba_bytes) =
            encode_frame_with_pii_boxes(&frame, &boxes, WebPQuality::High).unwrap();
        assert!(!webp.is_empty());
        // The returned RGBA buffer is the redacted one (box center is black).
        let rebuilt = RgbaImage::from_raw(120, 80, rgba_bytes).expect("valid rgba buffer");
        assert_eq!(*rebuilt.get_pixel(55, 40), Rgba([0, 0, 0, 255]));
        assert_eq!(*rebuilt.get_pixel(5, 5), Rgba([255, 255, 255, 255]));
        // The encoded WebP round-trips back to a masked image (lossy codec, so
        // assert the region is near-black rather than exactly black).
        let decoded = image::load_from_memory(&webp)
            .expect("redacted webp must decode")
            .to_rgba8();
        let p = decoded.get_pixel(55, 40).0;
        assert!(
            p[0] < 40 && p[1] < 40 && p[2] < 40,
            "masked region must decode near-black, got {p:?}"
        );
    }
}
