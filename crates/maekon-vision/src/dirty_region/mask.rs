//! Rectangles in the platform's screen coordinates, and the arithmetic that
//! keeps changed regions out of excluded windows and inside one display.

use maekon_core::models::context::WindowBounds;
use maekon_core::models::dirty_region::DirtyRect;

/// A rectangle in the platform's screen coordinates (the space of
/// [`WindowBounds`]); `right` and `bottom` are exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenRect {
    pub left: i64,
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
}

impl ScreenRect {
    pub fn from_bounds(bounds: &WindowBounds) -> Self {
        let left = i64::from(bounds.x);
        let top = i64::from(bounds.y);
        Self {
            left,
            top,
            right: left + i64::from(bounds.width),
            bottom: top + i64::from(bounds.height),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    /// The parts of this rectangle outside `hole`: at most a band above, a
    /// band below, and the pieces left and right of it.
    fn subtract(&self, hole: &ScreenRect) -> impl Iterator<Item = ScreenRect> {
        let top = self.top.max(hole.top).min(self.bottom);
        let bottom = self.bottom.min(hole.bottom).max(top);
        let left = self.left.max(hole.left).min(self.right);
        let right = self.right.min(hole.right).max(left);
        [
            ScreenRect {
                bottom: top,
                ..*self
            },
            ScreenRect {
                top: bottom,
                ..*self
            },
            ScreenRect {
                top,
                bottom,
                right: left,
                ..*self
            },
            ScreenRect {
                top,
                bottom,
                left: right,
                ..*self
            },
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
    }

    /// This rectangle in the pixel coordinates of `display`, clamped to it.
    /// A rectangle off the display comes out empty, and [`DirtyRegions`]
    /// drops empty rectangles.
    ///
    /// [`DirtyRegions`]: maekon_core::models::dirty_region::DirtyRegions
    pub fn on_display(&self, display: &ScreenRect) -> DirtyRect {
        let clamp = |value: i64, low: i64, high: i64| (value.clamp(low, high) - low) as u32;
        DirtyRect {
            left: clamp(self.left, display.left, display.right),
            top: clamp(self.top, display.top, display.bottom),
            right: clamp(self.right, display.left, display.right),
            bottom: clamp(self.bottom, display.top, display.bottom),
        }
    }
}

/// The parts of `rects` that no `hole` covers.
pub fn subtract_all(
    rects: Vec<ScreenRect>,
    holes: impl IntoIterator<Item = ScreenRect>,
) -> Vec<ScreenRect> {
    holes.into_iter().fold(rects, |rects, hole| {
        rects.iter().flat_map(|rect| rect.subtract(&hole)).collect()
    })
}

#[cfg(test)]
mod tests {
    use maekon_core::models::dirty_region::DirtyRegions;

    use super::*;

    fn rect(left: i64, top: i64, right: i64, bottom: i64) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    fn area(rects: &[ScreenRect]) -> i64 {
        rects
            .iter()
            .map(|r| (r.right - r.left) * (r.bottom - r.top))
            .sum()
    }

    #[test]
    fn window_bounds_become_exclusive_edges() {
        let bounds = WindowBounds {
            x: -10,
            y: 20,
            width: 30,
            height: 40,
        };
        assert_eq!(ScreenRect::from_bounds(&bounds), rect(-10, 20, 20, 60));
    }

    #[test]
    fn a_rectangle_without_area_is_empty() {
        assert!(!rect(0, 0, 1, 1).is_empty());
        assert!(rect(0, 0, 0, 1).is_empty());
        assert!(rect(0, 0, 1, 0).is_empty());
        assert!(rect(5, 0, 1, 1).is_empty());
    }

    #[test]
    fn a_hole_in_the_middle_leaves_four_pieces_around_it() {
        let parts = subtract_all(vec![rect(0, 0, 10, 10)], [rect(3, 4, 6, 8)]);
        assert_eq!(
            parts,
            vec![
                rect(0, 0, 10, 4),
                rect(0, 8, 10, 10),
                rect(0, 4, 3, 8),
                rect(6, 4, 10, 8)
            ]
        );
        assert_eq!(area(&parts), 100 - 12);
    }

    #[test]
    fn a_hole_that_misses_leaves_the_rectangle_whole() {
        let whole = rect(0, 0, 10, 10);
        for hole in [
            rect(10, 0, 20, 10),
            rect(-5, 0, 0, 10),
            rect(0, 10, 10, 20),
            rect(0, -5, 10, 0),
        ] {
            assert_eq!(subtract_all(vec![whole], [hole]), vec![whole], "{hole:?}");
        }
    }

    #[test]
    fn a_covering_hole_leaves_nothing() {
        assert!(subtract_all(vec![rect(2, 2, 4, 4)], [rect(0, 0, 10, 10)]).is_empty());
    }

    #[test]
    fn a_hole_over_one_edge_trims_that_side_only() {
        assert_eq!(
            subtract_all(vec![rect(0, 0, 10, 10)], [rect(7, -5, 15, 15)]),
            vec![rect(0, 0, 7, 10)]
        );
    }

    #[test]
    fn every_hole_is_taken_out() {
        let parts = subtract_all(
            vec![rect(0, 0, 10, 2), rect(20, 0, 30, 2)],
            [rect(0, 0, 5, 2), rect(25, 0, 30, 2)],
        );
        assert_eq!(parts, vec![rect(5, 0, 10, 2), rect(20, 0, 25, 2)]);
    }

    /// What `rect` adds to the changed regions of `display`.
    fn regions_of(rect: ScreenRect, display: &ScreenRect) -> Vec<DirtyRect> {
        let width = (display.right - display.left) as u32;
        let height = (display.bottom - display.top) as u32;
        DirtyRegions::new(width, height, [rect.on_display(display)])
            .unwrap()
            .rects()
            .to_vec()
    }

    #[test]
    fn screen_rectangles_map_into_display_pixels() {
        // A second display to the right of a 1920 px wide one.
        let display = rect(1920, 0, 3840, 1080);
        assert_eq!(
            regions_of(rect(1900, -10, 1930, 20), &display),
            vec![DirtyRect {
                left: 0,
                top: 0,
                right: 10,
                bottom: 20
            }]
        );
        assert_eq!(
            regions_of(rect(3830, 1070, 3900, 1100), &display),
            vec![DirtyRect {
                left: 1910,
                top: 1070,
                right: 1920,
                bottom: 1080
            }]
        );
        assert_eq!(regions_of(rect(0, 0, 1920, 1080), &display), vec![]);
        assert_eq!(regions_of(rect(2000, 1080, 2100, 1200), &display), vec![]);
    }
}
