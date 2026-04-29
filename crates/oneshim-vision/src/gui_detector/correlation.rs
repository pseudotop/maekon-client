//! Click and typing correlation logic.

use oneshim_core::models::frame::{BoundingBox, OcrRegion};
use oneshim_core::models::gui_interaction::{GuiElement, GuiElementType};
use rstar::{PointDistance, RTree, RTreeObject, AABB};

use super::{
    GuiElementDetector, SPATIAL_INDEX_THRESHOLD, WORD_GROUP_GAP_FACTOR, WORD_GROUP_Y_TOLERANCE,
};

struct IndexedRegion<'a> {
    region: &'a OcrRegion,
    #[allow(dead_code)]
    index: usize,
}

impl RTreeObject for IndexedRegion<'_> {
    type Envelope = AABB<[f64; 2]>;

    fn envelope(&self) -> Self::Envelope {
        let b = &self.region.bbox;
        AABB::from_corners(
            [b.x as f64, b.y as f64],
            [(b.x + b.width) as f64, (b.y + b.height) as f64],
        )
    }
}

impl PointDistance for IndexedRegion<'_> {
    fn distance_2(&self, point: &[f64; 2]) -> f64 {
        self.envelope().distance_2(point)
    }
}

/// Y-tolerance for same-line word grouping (pixels).
impl GuiElementDetector {
    pub fn group_words(regions: &[OcrRegion]) -> Vec<OcrRegion> {
        if regions.is_empty() {
            return Vec::new();
        }

        let mut sorted: Vec<&OcrRegion> = regions.iter().collect();
        sorted.sort_by(|a, b| {
            let ay = a.bbox.y / (WORD_GROUP_Y_TOLERANCE + 1);
            let by = b.bbox.y / (WORD_GROUP_Y_TOLERANCE + 1);
            ay.cmp(&by).then(a.bbox.x.cmp(&b.bbox.x))
        });

        let mut grouped: Vec<OcrRegion> = Vec::new();

        for region in sorted {
            let should_merge = grouped.last().is_some_and(|prev: &OcrRegion| {
                // Same line check
                let y_diff = (prev.bbox.y as i64 - region.bbox.y as i64).unsigned_abs() as u32;
                if y_diff > WORD_GROUP_Y_TOLERANCE {
                    return false;
                }

                // Gap check
                let prev_right = prev.bbox.x + prev.bbox.width;
                let gap = region.bbox.x.saturating_sub(prev_right);

                // Estimate avg char width from the previous region
                let char_count = prev.text.chars().count().max(1) as f32;
                let avg_char_width = prev.bbox.width as f32 / char_count;
                let max_gap = (avg_char_width * WORD_GROUP_GAP_FACTOR) as u32;

                gap <= max_gap
            });

            if should_merge {
                let prev = grouped
                    .last_mut()
                    .expect("grouped is non-empty when merging");
                // Merge bounding boxes
                let min_x = prev.bbox.x.min(region.bbox.x);
                let min_y = prev.bbox.y.min(region.bbox.y);
                let prev_right = (prev.bbox.x as u64) + (prev.bbox.width as u64);
                let prev_bottom = (prev.bbox.y as u64) + (prev.bbox.height as u64);
                let region_right = (region.bbox.x as u64) + (region.bbox.width as u64);
                let region_bottom = (region.bbox.y as u64) + (region.bbox.height as u64);
                let max_x = prev_right.max(region_right).min(u32::MAX as u64) as u32;
                let max_y = prev_bottom.max(region_bottom).min(u32::MAX as u64) as u32;
                prev.bbox = BoundingBox {
                    x: min_x,
                    y: min_y,
                    width: max_x - min_x,
                    height: max_y - min_y,
                };
                // Concatenate text with space
                prev.text.push(' ');
                prev.text.push_str(&region.text);
                // Keep the lower confidence
                prev.confidence = prev.confidence.min(region.confidence);
            } else {
                grouped.push(region.clone());
            }
        }

        grouped
    }

    /// Given a mouse click position and OCR regions from the current frame,
    /// find which OCR region (if any) the click landed on.
    ///
    /// 1. Direct hit: smallest region containing the point.
    /// 2. Proximity fallback: nearest region within `proximity_threshold_px`.
    ///
    /// PII filter is applied to the resulting element text.
    pub fn correlate_click(
        &self,
        click_x: u32,
        click_y: u32,
        regions: &[OcrRegion],
    ) -> Option<GuiElement> {
        // Use R-tree spatial index for large region sets
        if regions.len() >= SPATIAL_INDEX_THRESHOLD {
            return self.correlate_click_spatial(click_x, click_y, regions);
        }

        // Linear scan for small region sets
        self.correlate_click_linear(click_x, click_y, regions)
    }

    /// Linear scan matching (O(n)) — used when region count < threshold.
    fn correlate_click_linear(
        &self,
        click_x: u32,
        click_y: u32,
        regions: &[OcrRegion],
    ) -> Option<GuiElement> {
        // 1. Direct hit — smallest containing region
        let direct = regions
            .iter()
            .filter(|r| r.bbox.contains_point(click_x, click_y))
            .min_by_key(|r| r.bbox.area());

        let matched = if let Some(r) = direct {
            Some(r)
        } else {
            // 2. Proximity fallback — nearest within threshold
            let threshold = self.proximity_threshold_px as f64;
            regions
                .iter()
                .filter_map(|r| {
                    let dist = Self::distance_to_bbox(click_x, click_y, &r.bbox);
                    if dist <= threshold {
                        Some((r, dist))
                    } else {
                        None
                    }
                })
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(r, _)| r)
        };

        matched.map(|r| self.build_gui_element(r))
    }

    /// R-tree spatial index matching (O(log n)) — used when region count >= threshold.
    fn correlate_click_spatial(
        &self,
        click_x: u32,
        click_y: u32,
        regions: &[OcrRegion],
    ) -> Option<GuiElement> {
        let indexed: Vec<IndexedRegion> = regions
            .iter()
            .enumerate()
            .map(|(i, r)| IndexedRegion {
                region: r,
                index: i,
            })
            .collect();
        let tree = RTree::bulk_load(indexed);

        let point = [click_x as f64, click_y as f64];

        // 1. Direct hit — containing regions, pick smallest
        let containing: Vec<_> = tree.locate_all_at_point(&point).collect();
        if let Some(hit) = containing.iter().min_by_key(|ir| ir.region.bbox.area()) {
            return Some(self.build_gui_element(hit.region));
        }

        // 2. Proximity fallback — nearest within threshold
        let threshold = self.proximity_threshold_px as f64;
        if let Some(nearest) = tree.nearest_neighbor(&point) {
            let dist = nearest.distance_2(&point).sqrt();
            if dist <= threshold {
                return Some(self.build_gui_element(nearest.region));
            }
        }

        None
    }

    pub fn correlate_typing(
        &self,
        regions: &[OcrRegion],
        cursor_x: u32,
        cursor_y: u32,
    ) -> Option<GuiElement> {
        self.correlate_click(cursor_x, cursor_y, regions)
            .map(|mut e| {
                e.element_type = GuiElementType::TextInput;
                e
            })
    }

    /// Euclidean distance from a point to the nearest edge of a bounding box.
    pub(super) fn distance_to_bbox(px: u32, py: u32, bbox: &BoundingBox) -> f64 {
        let cx = px as f64;
        let cy = py as f64;
        let bx_min = bbox.x as f64;
        let by_min = bbox.y as f64;
        let bx_max = (bbox.x + bbox.width) as f64;
        let by_max = (bbox.y + bbox.height) as f64;

        let dx = if cx < bx_min {
            bx_min - cx
        } else if cx > bx_max {
            cx - bx_max
        } else {
            0.0
        };
        let dy = if cy < by_min {
            by_min - cy
        } else if cy > by_max {
            cy - by_max
        } else {
            0.0
        };

        (dx * dx + dy * dy).sqrt()
    }
}
