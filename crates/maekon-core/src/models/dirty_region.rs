//! Screen regions the operating system reports as changed, and the rule that
//! maps them onto the [`SCREEN_TILE_PX`] signature grid of a
//! [`ScreenSample`](super::screen_activity::ScreenSample).
//!
//! An OS change source (X11 DAMAGE, Windows Desktop Duplication, macOS
//! ScreenCaptureKit) reports rectangles in display pixels. [`DirtyRegions`]
//! keeps the parts that lie on the display. [`DirtyRegions::touched_tiles`]
//! names the tiles of a downscaled signature grid that a region overlaps, so
//! both sources describe a change in the same tiles.

use std::collections::BTreeSet;

use super::screen_activity::{ScreenSampleError, SCREEN_TILE_PX};

/// A rectangle in display pixels, origin at the display's top-left corner.
/// `right` and `bottom` are exclusive, as in a Windows `RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DirtyRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl DirtyRect {
    /// The part of this rectangle that lies on a `width` x `height` display.
    pub fn clip(&self, width: u32, height: u32) -> Option<DirtyRect> {
        let right = self.right.min(width);
        let bottom = self.bottom.min(height);
        (self.left < right && self.top < bottom).then_some(DirtyRect {
            right,
            bottom,
            ..*self
        })
    }
}

/// Regions of one display that changed since its source last reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyRegions {
    display_width: u32,
    display_height: u32,
    rects: Vec<DirtyRect>,
}

impl DirtyRegions {
    /// Keep the parts of `rects` that lie on a `display_width` x
    /// `display_height` display. No rectangles means "observed, unchanged".
    pub fn new(
        display_width: u32,
        display_height: u32,
        rects: impl IntoIterator<Item = DirtyRect>,
    ) -> Result<Self, ScreenSampleError> {
        if display_width == 0 || display_height == 0 {
            return Err(ScreenSampleError::EmptyGeometry);
        }
        let rects = rects
            .into_iter()
            .filter_map(|rect| rect.clip(display_width, display_height))
            .collect();
        Ok(Self {
            display_width,
            display_height,
            rects,
        })
    }

    /// Nothing on the display changed.
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// The changed rectangles, each inside the display.
    pub fn rects(&self) -> &[DirtyRect] {
        &self.rects
    }

    /// Row-major indices of the tiles of a `sample_width` x `sample_height`
    /// signature grid, the display reduced to that size, that any region
    /// overlaps. Region edges are widened to whole sample pixels, so a region
    /// one display pixel wide still marks its tile: rounding never drops a
    /// change.
    pub fn touched_tiles(&self, sample_width: u32, sample_height: u32) -> Vec<usize> {
        let columns = sample_width.div_ceil(SCREEN_TILE_PX) as usize;
        let mut touched = BTreeSet::new();
        for rect in &self.rects {
            let (first_col, end_col) =
                tile_range(rect.left, rect.right, self.display_width, sample_width);
            let (first_row, end_row) =
                tile_range(rect.top, rect.bottom, self.display_height, sample_height);
            for row in first_row..end_row {
                touched.extend((first_col..end_col).map(|col| row * columns + col));
            }
        }
        touched.into_iter().collect()
    }
}

/// First tile and the tile past the last one of a `sample`-pixel axis that
/// the display span `start..end` of a `display`-pixel axis covers. The start
/// is rounded down and the end up.
fn tile_range(start: u32, end: u32, display: u32, sample: u32) -> (usize, usize) {
    let unit = u64::from(display) * u64::from(SCREEN_TILE_PX);
    let first = u64::from(start) * u64::from(sample) / unit;
    let end = (u64::from(end) * u64::from(sample)).div_ceil(unit);
    (first as usize, end as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: u32, top: u32, right: u32, bottom: u32) -> DirtyRect {
        DirtyRect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn regions_keep_only_the_part_on_the_display() {
        let regions = DirtyRegions::new(
            100,
            50,
            [
                rect(90, 40, 110, 60), // crosses the right and bottom edges
                rect(100, 0, 105, 5),  // right of the display
                rect(0, 50, 5, 55),    // below the display
                rect(10, 10, 10, 15),  // no width
                rect(10, 10, 15, 10),  // no height
                rect(20, 20, 10, 30),  // inverted
            ],
        )
        .unwrap();
        assert_eq!(regions.rects(), &[rect(90, 40, 100, 50)]);
        assert!(!regions.is_empty());
    }

    #[test]
    fn no_regions_means_observed_and_unchanged() {
        let regions = DirtyRegions::new(100, 50, []).unwrap();
        assert!(regions.is_empty());
        assert!(regions.rects().is_empty());
        assert!(regions.touched_tiles(100, 50).is_empty());
    }

    #[test]
    fn a_display_without_pixels_is_rejected() {
        assert_eq!(
            DirtyRegions::new(0, 50, [rect(0, 0, 1, 1)]),
            Err(ScreenSampleError::EmptyGeometry)
        );
        assert_eq!(
            DirtyRegions::new(100, 0, [rect(0, 0, 1, 1)]),
            Err(ScreenSampleError::EmptyGeometry)
        );
    }

    #[test]
    fn at_full_size_a_region_marks_exactly_the_tiles_it_covers() {
        // 64 x 48 -> 4 columns x 3 rows.
        let regions = DirtyRegions::new(64, 48, [rect(16, 16, 48, 32)]).unwrap();
        assert_eq!(regions.touched_tiles(64, 48), vec![5, 6]);
        // One pixel into the next tile marks it too.
        let regions = DirtyRegions::new(64, 48, [rect(15, 31, 17, 33)]).unwrap();
        assert_eq!(regions.touched_tiles(64, 48), vec![4, 5, 8, 9]);
    }

    #[test]
    fn a_one_pixel_change_survives_downscaling() {
        // 1920 x 1080 reduced to 960 x 540: 60 columns, 34 rows.
        let regions = DirtyRegions::new(1920, 1080, [rect(1919, 1079, 1920, 1080)]).unwrap();
        assert_eq!(regions.touched_tiles(960, 540), vec![33 * 60 + 59]);
        let regions = DirtyRegions::new(1920, 1080, [rect(0, 0, 1, 1)]).unwrap();
        assert_eq!(regions.touched_tiles(960, 540), vec![0]);
        // A region ending on a tile edge does not spill into the next tile.
        let regions = DirtyRegions::new(1920, 1080, [rect(32, 0, 64, 32)]).unwrap();
        assert_eq!(regions.touched_tiles(960, 540), vec![1]);
        // Neither does one ending inside the last sample pixel of a tile.
        let regions = DirtyRegions::new(1920, 1080, [rect(33, 34, 63, 63)]).unwrap();
        assert_eq!(regions.touched_tiles(960, 540), vec![61]);
    }

    #[test]
    fn overlapping_regions_name_each_tile_once_in_row_major_order() {
        let regions = DirtyRegions::new(
            64,
            48,
            [rect(40, 40, 48, 48), rect(0, 0, 20, 4), rect(2, 2, 4, 4)],
        )
        .unwrap();
        assert_eq!(regions.touched_tiles(64, 48), vec![0, 1, 10]);
    }

    #[test]
    fn a_grid_without_pixels_has_no_tiles() {
        let regions = DirtyRegions::new(64, 48, [rect(0, 0, 64, 48)]).unwrap();
        assert!(regions.touched_tiles(0, 48).is_empty());
        assert!(regions.touched_tiles(64, 0).is_empty());
    }
}
