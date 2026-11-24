//! Host-independent rules of the Windows change listener (`windows.rs`),
//! kept apart so the mutation gates measure them on Linux.
//!
//! DXGI Desktop Duplication reports, per output, what changed in the
//! output's duplicated image: dirty rectangles, and the destinations of move
//! rectangles (content copied from elsewhere in the image; DXGI reports what
//! a move uncovers as dirty). Out-of-context window events
//! (`SetWinEventHook`) report windows that appeared, disappeared or moved.

use super::mask::ScreenRect;

/// `DXGI_MODE_ROTATION_UNSPECIFIED` and `DXGI_MODE_ROTATION_IDENTITY`.
const NOT_ROTATED: [i32; 2] = [0, 1];

/// The window events the listener hooks, as inclusive ranges:
/// `EVENT_OBJECT_DESTROY`..`EVENT_OBJECT_HIDE` (destroy, show, hide),
/// `EVENT_OBJECT_LOCATIONCHANGE`, and
/// `EVENT_OBJECT_CLOAKED`..`EVENT_OBJECT_UNCLOAKED`. Restacking is left out:
/// excluded windows are cut out by their whole bounds, so the stacking order
/// does not change what is cut.
pub const EVENT_RANGES: [(u32, u32); 3] = [(0x8001, 0x8003), (0x800B, 0x800B), (0x8017, 0x8018)];

/// What DXGI says about an output's duplicated image (`DXGI_OUTDUPL_DESC`):
/// its size in pixels and its `DXGI_MODE_ROTATION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicatedImage {
    pub width: i64,
    pub height: i64,
    pub rotation: i32,
}

/// A rectangle of an output's duplicated image in virtual-screen coordinates
/// (the space of window bounds); `output` is the output's desktop area
/// (`DXGI_OUTPUT_DESC::DesktopCoordinates`). Where the image does not map one
/// to one onto that area, a change reports the whole output: the image of a
/// rotated output is not rotated, and that mapping has not been checked on a
/// device; an image of another size means this process sees DPI-virtualized
/// desktop coordinates.
pub fn on_desktop(rect: ScreenRect, image: &DuplicatedImage, output: &ScreenRect) -> ScreenRect {
    let one_to_one = NOT_ROTATED.contains(&image.rotation)
        && image.width == output.right - output.left
        && image.height == output.bottom - output.top;
    if !one_to_one {
        return *output;
    }
    ScreenRect {
        left: output.left + rect.left,
        top: output.top + rect.top,
        right: output.left + rect.right,
        bottom: output.top + rect.bottom,
    }
}

/// Whether a hooked window event is about a window itself
/// (`OBJID_WINDOW`, `CHILDID_SELF`) rather than its caret, cursor or a child
/// element, and is one of [`EVENT_RANGES`].
pub fn is_layout_event(event: u32, object: i32, child: i32) -> bool {
    object == 0
        && child == 0
        && EVENT_RANGES
            .iter()
            .any(|&(first, last)| (first..=last).contains(&event))
}

/// Whether a window event's window is top-level or already gone. `root` is
/// `GetAncestor(window, GA_ROOT)`, 0 when it failed. Out-of-context events
/// arrive after the fact, so a top-level window destroyed by then has no
/// root and still changed the layout.
pub fn is_top_level_or_gone(window: isize, root: isize) -> bool {
    root == 0 || root == window
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: i64, top: i64, right: i64, bottom: i64) -> ScreenRect {
        ScreenRect {
            left,
            top,
            right,
            bottom,
        }
    }

    /// A monitor left of and below the primary one.
    const OUTPUT: ScreenRect = ScreenRect {
        left: -1280,
        top: 200,
        right: 0,
        bottom: 1224,
    };

    fn image(width: i64, height: i64, rotation: i32) -> DuplicatedImage {
        DuplicatedImage {
            width,
            height,
            rotation,
        }
    }

    #[test]
    fn an_unrotated_image_of_the_output_size_offsets_by_its_position() {
        for rotation in NOT_ROTATED {
            assert_eq!(
                on_desktop(rect(10, 20, 30, 50), &image(1280, 1024, rotation), &OUTPUT),
                rect(-1270, 220, -1250, 250),
                "rotation {rotation}"
            );
        }
    }

    #[test]
    fn an_image_that_does_not_map_one_to_one_reports_the_whole_output() {
        let changed = rect(10, 20, 30, 50);
        // DXGI_MODE_ROTATION_ROTATE90, ROTATE180, ROTATE270, and a value
        // outside the enum.
        for rotation in [2, 3, 4, 7] {
            assert_eq!(
                on_desktop(changed, &image(1280, 1024, rotation), &OUTPUT),
                OUTPUT,
                "rotation {rotation}"
            );
        }
        // DPI-virtualized desktop coordinates: the image has more pixels.
        assert_eq!(on_desktop(changed, &image(1920, 1024, 1), &OUTPUT), OUTPUT);
        assert_eq!(on_desktop(changed, &image(1280, 1536, 1), &OUTPUT), OUTPUT);
    }

    #[test]
    fn only_events_about_a_window_itself_change_the_layout() {
        for event in [0x8001, 0x8002, 0x8003, 0x800B, 0x8017, 0x8018] {
            assert!(is_layout_event(event, 0, 0), "event {event:#x}");
            // OBJID_CURSOR, OBJID_CARET and OBJID_CLIENT.
            for object in [-9, -8, -4] {
                assert!(!is_layout_event(event, object, 0), "object {object}");
            }
            assert!(!is_layout_event(event, 0, 3), "a child element");
        }
        // Create, reorder, selection, name change: outside the ranges.
        for event in [0x8000, 0x8004, 0x8006, 0x800C, 0x8016, 0x8019] {
            assert!(!is_layout_event(event, 0, 0), "event {event:#x}");
        }
    }

    #[test]
    fn a_top_level_or_vanished_window_counts_and_a_child_does_not() {
        assert!(is_top_level_or_gone(0x10, 0x10));
        assert!(
            is_top_level_or_gone(0x10, 0),
            "gone before the event arrived"
        );
        assert!(!is_top_level_or_gone(0x10, 0x20), "a child of 0x20");
    }
}
