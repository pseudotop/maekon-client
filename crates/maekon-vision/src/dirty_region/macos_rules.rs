//! Host-independent rules of the macOS change listener (`macos.rs`), kept
//! apart so the mutation gates measure them on Linux.
//!
//! ScreenCaptureKit attaches what changed to every frame of a display
//! stream: a frame status and, for a complete frame, the rectangles that were
//! redrawn or moved since the previous frame, in pixels of the frame. The
//! listener streams each display at its own pixel size, so a frame pixel is a
//! display pixel when the frame says its content was neither scaled nor
//! offset. Where it does not say so, or a rectangle cannot be read, a
//! complete frame reports its whole display.

use std::collections::BTreeSet;
use std::ops::RangeInclusive;

use super::mask::ScreenRect;

/// `SCFrameStatus` values (`ScreenCaptureKit/SCStream.h`).
const FRAME_COMPLETE: i64 = 0;
const FRAME_IDLE: i64 = 1;
const FRAME_STOPPED: i64 = 5;

/// The pixels per point that `SCStreamFrameInfoScaleFactor` documents.
const SCALE_FACTORS: RangeInclusive<f64> = 1.0..=4.0;

/// A `CGRect` as ScreenCaptureKit or CoreGraphics reports it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// A frame's attachments (`SCStreamFrameInfo` keys), read without
/// interpretation; `None` where a value is missing or of another type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FrameInfo {
    /// `SCStreamFrameInfoStatus`, an `SCFrameStatus`.
    pub status: Option<i64>,
    /// `SCStreamFrameInfoScaleFactor`: the display's pixels per point.
    pub scale_factor: Option<f64>,
    /// `SCStreamFrameInfoContentScale`: how much the content was scaled into
    /// the frame.
    pub content_scale: Option<f64>,
    /// `SCStreamFrameInfoContentRect`: where the content lies in the frame,
    /// in points.
    pub content_rect: Option<FrameRect>,
    /// `SCStreamFrameInfoDirtyRects`, in pixels of the frame; an element is
    /// `None` where it is not a rectangle.
    pub dirty_rects: Option<Vec<Option<FrameRect>>>,
}

/// What one frame says changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameChanges {
    Unchanged,
    /// These rectangles, in global display coordinates (points).
    Rects(Vec<ScreenRect>),
    /// The frame does not say what changed: the stream started or was
    /// suspended, or the display is blank.
    NotFollowed,
    /// The stream stopped.
    Stopped,
}

/// What `frame` says changed on `display`, the display's area in global
/// display coordinates.
pub fn frame_changes(frame: &FrameInfo, display: &ScreenRect) -> FrameChanges {
    match frame.status {
        Some(FRAME_COMPLETE) => {
            FrameChanges::Rects(dirty_on_desktop(frame, display).unwrap_or_else(|| vec![*display]))
        }
        Some(FRAME_IDLE) => FrameChanges::Unchanged,
        Some(FRAME_STOPPED) => FrameChanges::Stopped,
        _ => FrameChanges::NotFollowed,
    }
}

/// A complete frame's dirty rectangles on the desktop; `None` unless the
/// frame maps one to one onto `display` and every rectangle lies inside it.
fn dirty_on_desktop(frame: &FrameInfo, display: &ScreenRect) -> Option<Vec<ScreenRect>> {
    let scale = one_to_one_scale(frame)?;
    frame
        .dirty_rects
        .as_ref()?
        .iter()
        .map(|rect| on_desktop((*rect)?, scale, display))
        .collect()
}

/// The display's pixels per point, when the frame's content fills it
/// unscaled from its top-left corner. An unscaled stream reports exactly 1
/// and 0 there; anything else counts as scaled.
fn one_to_one_scale(frame: &FrameInfo) -> Option<f64> {
    let unscaled = frame.content_scale == Some(1.0);
    let at_origin = frame
        .content_rect
        .is_some_and(|rect| rect.x == 0.0 && rect.y == 0.0);
    frame
        .scale_factor
        .filter(|scale| unscaled && at_origin && SCALE_FACTORS.contains(scale))
}

/// A frame rectangle in pixels as global display coordinates: `display` is
/// the display's area in points and `scale` its pixels per point. Rounded
/// outwards; `None` unless it lies inside the display.
pub fn on_desktop(rect: FrameRect, scale: f64, display: &ScreenRect) -> Option<ScreenRect> {
    let [left, right] = axis(rect.x, rect.width, scale, display.left, display.right)?;
    let [top, bottom] = axis(rect.y, rect.height, scale, display.top, display.bottom)?;
    Some(ScreenRect {
        left,
        top,
        right,
        bottom,
    })
}

/// One axis of a frame rectangle, `start` and `length` in pixels, on the
/// display axis `low..=high` in points.
fn axis(start: f64, length: f64, scale: f64, low: i64, high: i64) -> Option<[i64; 2]> {
    let first = low as f64 + start / scale;
    let last = first + length / scale;
    let axis = low as f64..=high as f64;
    (axis.contains(&first) && axis.contains(&last) && first <= last)
        .then(|| [first.floor() as i64, last.ceil() as i64])
}

/// One on-screen window as `CGWindowListCopyWindowInfo` lists it: its window
/// number and the bits of its bounds in points.
pub type WindowEntry = (i64, [u64; 4]);

/// Whether the window layout changed between two reads of the window list.
/// Sets compare windows regardless of their stacking order: excluded windows
/// are cut out by their whole bounds, so restacking does not change what is
/// cut. A list that could not be read counts as a change.
pub fn layout_changed(
    before: Option<&BTreeSet<WindowEntry>>,
    now: Option<&BTreeSet<WindowEntry>>,
) -> bool {
    before.is_none() || before != now
}

/// What the streams of one capture session delivered since the poll thread
/// last took it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Inbox {
    pub rects: Vec<ScreenRect>,
    /// A frame arrived, so the window layout may have changed.
    pub frames: bool,
    /// A frame did not say what changed.
    pub not_followed: bool,
    /// A stream stopped, so the session has to start again.
    pub stopped: bool,
}

impl Inbox {
    pub fn receive(&mut self, changes: FrameChanges) {
        self.frames = true;
        match changes {
            FrameChanges::Unchanged => {}
            FrameChanges::Rects(rects) => self.rects.extend(rects),
            FrameChanges::NotFollowed => self.not_followed = true,
            FrameChanges::Stopped => self.stopped = true,
        }
    }
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

    fn pixels(x: f64, y: f64, width: f64, height: f64) -> FrameRect {
        FrameRect {
            x,
            y,
            width,
            height,
        }
    }

    /// A 1440 x 900 point display left of and above the main one.
    const DISPLAY: ScreenRect = ScreenRect {
        left: -1440,
        top: -300,
        right: 0,
        bottom: 600,
    };

    /// A complete frame of [`DISPLAY`] at 2 pixels per point, unscaled.
    fn complete(dirty: Vec<Option<FrameRect>>) -> FrameInfo {
        FrameInfo {
            status: Some(FRAME_COMPLETE),
            scale_factor: Some(2.0),
            content_scale: Some(1.0),
            content_rect: Some(pixels(0.0, 0.0, 1440.0, 900.0)),
            dirty_rects: Some(dirty),
        }
    }

    #[test]
    fn dirty_pixels_become_points_on_the_desktop_rounded_outwards() {
        let frame = complete(vec![
            Some(pixels(21.0, 41.0, 18.0, 8.0)),
            Some(pixels(0.0, 0.0, 2880.0, 1800.0)),
        ]);
        assert_eq!(
            frame_changes(&frame, &DISPLAY),
            FrameChanges::Rects(vec![rect(-1430, -280, -1420, -275), DISPLAY])
        );
        let at_one = FrameInfo {
            scale_factor: Some(1.0),
            ..complete(vec![Some(pixels(10.0, 20.0, 5.0, 5.0))])
        };
        assert_eq!(
            frame_changes(&at_one, &DISPLAY),
            FrameChanges::Rects(vec![rect(-1430, -280, -1425, -275)])
        );
        assert_eq!(
            frame_changes(&complete(Vec::new()), &DISPLAY),
            FrameChanges::Rects(Vec::new()),
            "a complete frame with nothing dirty changed nothing"
        );
    }

    #[test]
    fn a_frame_that_is_not_one_to_one_reports_its_whole_display() {
        let dirty = || complete(vec![Some(pixels(20.0, 40.0, 10.0, 10.0))]);
        let whole = FrameChanges::Rects(vec![DISPLAY]);
        for frame in [
            FrameInfo {
                content_scale: Some(0.5),
                ..dirty()
            },
            FrameInfo {
                content_scale: None,
                ..dirty()
            },
            FrameInfo {
                content_rect: Some(pixels(0.0, 90.0, 1440.0, 720.0)),
                ..dirty()
            },
            FrameInfo {
                content_rect: Some(pixels(160.0, 0.0, 1120.0, 900.0)),
                ..dirty()
            },
            FrameInfo {
                content_rect: None,
                ..dirty()
            },
            FrameInfo {
                scale_factor: Some(0.5),
                ..dirty()
            },
            FrameInfo {
                scale_factor: Some(8.0),
                ..dirty()
            },
            FrameInfo {
                scale_factor: None,
                ..dirty()
            },
            FrameInfo {
                dirty_rects: None,
                ..dirty()
            },
        ] {
            assert_eq!(frame_changes(&frame, &DISPLAY), whole, "{frame:?}");
        }
    }

    #[test]
    fn an_unreadable_or_outside_rectangle_reports_the_whole_display() {
        let whole = FrameChanges::Rects(vec![DISPLAY]);
        let inside = Some(pixels(20.0, 40.0, 10.0, 10.0));
        for odd in [
            None,
            Some(pixels(-2.0, 40.0, 10.0, 10.0)),
            Some(pixels(20.0, -2.0, 10.0, 10.0)),
            Some(pixels(2870.0, 40.0, 12.0, 10.0)),
            Some(pixels(20.0, 1790.0, 10.0, 12.0)),
            Some(pixels(20.0, 40.0, -4.0, 10.0)),
            Some(pixels(20.0, 40.0, 10.0, -4.0)),
            Some(pixels(f64::NAN, 40.0, 10.0, 10.0)),
            Some(pixels(20.0, 40.0, f64::INFINITY, 10.0)),
        ] {
            let frame = complete(vec![inside, odd]);
            assert_eq!(frame_changes(&frame, &DISPLAY), whole, "{odd:?}");
        }
    }

    #[test]
    fn only_a_complete_frame_has_rectangles() {
        let with_status = |status| FrameInfo {
            status,
            ..complete(vec![Some(pixels(20.0, 40.0, 10.0, 10.0))])
        };
        assert_eq!(
            frame_changes(&with_status(Some(FRAME_IDLE)), &DISPLAY),
            FrameChanges::Unchanged
        );
        assert_eq!(
            frame_changes(&with_status(Some(FRAME_STOPPED)), &DISPLAY),
            FrameChanges::Stopped
        );
        // Blank, Suspended, Started, a value outside the enum, and none.
        for status in [Some(2), Some(3), Some(4), Some(9), None] {
            assert_eq!(
                frame_changes(&with_status(status), &DISPLAY),
                FrameChanges::NotFollowed,
                "{status:?}"
            );
        }
    }

    #[test]
    fn the_window_layout_changes_when_its_windows_do_and_not_when_restacked() {
        let entry = |number, x: f64| (number, [x, 0.0, 100.0, 100.0].map(f64::to_bits));
        let layout = BTreeSet::from([entry(7, 0.0), entry(9, 50.0)]);
        // Restacked: the same windows listed in another order.
        let restacked = [entry(9, 50.0), entry(7, 0.0)].into_iter().collect();
        assert!(!layout_changed(Some(&layout), Some(&restacked)));

        let moved = BTreeSet::from([entry(7, 1.0), entry(9, 50.0)]);
        let closed = BTreeSet::from([entry(9, 50.0)]);
        assert!(layout_changed(Some(&layout), Some(&moved)));
        assert!(layout_changed(Some(&layout), Some(&closed)));
        assert!(layout_changed(Some(&layout), None), "unreadable now");
        assert!(layout_changed(None, Some(&layout)), "unreadable before");
        assert!(layout_changed(None, None));
    }

    #[test]
    fn the_inbox_collects_what_frames_said_until_taken() {
        let mut inbox = Inbox::default();
        inbox.receive(FrameChanges::Unchanged);
        assert_eq!(
            inbox,
            Inbox {
                frames: true,
                ..Inbox::default()
            }
        );
        inbox.receive(FrameChanges::Rects(vec![rect(0, 0, 1, 1)]));
        inbox.receive(FrameChanges::Rects(vec![rect(2, 2, 3, 3)]));
        inbox.receive(FrameChanges::NotFollowed);
        assert_eq!(
            std::mem::take(&mut inbox),
            Inbox {
                rects: vec![rect(0, 0, 1, 1), rect(2, 2, 3, 3)],
                frames: true,
                not_followed: true,
                stopped: false,
            }
        );
        inbox.receive(FrameChanges::Stopped);
        assert!(inbox.stopped && !inbox.not_followed && inbox.rects.is_empty());
    }
}
