//! Changes a listener thread collected and has not handed out yet.

use std::time::{Duration, Instant};

use maekon_core::error::CoreError;
use maekon_core::ports::vision::capture_denied;

use super::mask::ScreenRect;
use super::ListenerChanges;

/// Pending rectangles beyond this collapse into their bounding box, so a
/// burst of drawing between two takes cannot grow memory without bound and
/// still loses no change.
pub const MAX_PENDING_RECTS: usize = 256;

/// How long after a window layout change collected rectangles still count as
/// part of it. A listener learns of a moved, shown or hidden window before
/// the drawing that follows from it: a compositor repaints a frame or more
/// later, and DXGI and ScreenCaptureKit deliver that repaint later still. A
/// take between the two would otherwise report the change, and the next one
/// the repaint, over places where excluded windows were in between, as
/// observed. The length has not been measured on devices (#13655).
pub const LAYOUT_SETTLE: Duration = Duration::from_millis(250);

/// What a listener thread collected since the last drain.
#[derive(Debug, Default)]
pub struct PendingChanges {
    rects: Vec<ScreenRect>,
    windows_changed: bool,
    /// The end of the last layout change's [`LAYOUT_SETTLE`] period. A drain
    /// does not end it.
    settling_until: Option<Instant>,
    stopped: bool,
}

impl PendingChanges {
    pub fn push(&mut self, rect: ScreenRect) {
        self.push_at(rect, Instant::now());
    }

    /// [`Self::push`] at `now`: a rectangle collected while a layout change
    /// settles marks the interval as `windows_changed`.
    pub fn push_at(&mut self, rect: ScreenRect, now: Instant) {
        if self.settling_until.is_some_and(|until| now < until) {
            self.windows_changed = true;
        }
        if self.rects.len() < MAX_PENDING_RECTS {
            self.rects.push(rect);
            return;
        }
        let bounding = self.rects.drain(..).fold(rect, |a, b| ScreenRect {
            left: a.left.min(b.left),
            top: a.top.min(b.top),
            right: a.right.max(b.right),
            bottom: a.bottom.max(b.bottom),
        });
        self.rects.push(bounding);
    }

    pub fn windows_changed(&mut self) {
        self.windows_changed_at(Instant::now());
    }

    /// [`Self::windows_changed`] at `now`, which starts its settle period.
    pub fn windows_changed_at(&mut self, now: Instant) {
        self.windows_changed = true;
        self.settling_until = Some(now + LAYOUT_SETTLE);
    }

    /// Hand over what one poll of a listener thread read. `false`, after
    /// stopping, once the listener was stopped or `allowed` is false (its
    /// permit was refused).
    pub fn hand_over(
        &mut self,
        allowed: bool,
        rects: Vec<ScreenRect>,
        windows_changed: bool,
    ) -> bool {
        if !allowed || self.stopped {
            self.stop();
            return false;
        }
        if windows_changed {
            self.windows_changed();
        }
        for rect in rects {
            self.push(rect);
        }
        true
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Hand out what was collected; fails once stopped.
    pub fn drain(&mut self) -> Result<ListenerChanges, CoreError> {
        if self.stopped {
            return Err(capture_denied("change listener stopped"));
        }
        Ok(ListenerChanges {
            rects: std::mem::take(&mut self.rects),
            windows_changed: std::mem::take(&mut self.windows_changed),
        })
    }

    /// Discard what was collected and refuse later drains; `true` when this
    /// call is the one that stopped it.
    pub fn stop(&mut self) -> bool {
        self.rects.clear();
        self.windows_changed = false;
        !std::mem::replace(&mut self.stopped, true)
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

    #[test]
    fn a_drain_hands_out_everything_once() {
        let mut pending = PendingChanges::default();
        pending.push(rect(0, 0, 1, 1));
        pending.windows_changed();
        let changes = pending.drain().unwrap();
        assert_eq!(changes.rects, vec![rect(0, 0, 1, 1)]);
        assert!(changes.windows_changed);
        assert_eq!(pending.drain().unwrap(), ListenerChanges::default());
    }

    #[test]
    fn past_the_cap_everything_collapses_into_one_bounding_box() {
        let mut pending = PendingChanges::default();
        for i in 0..MAX_PENDING_RECTS as i64 {
            pending.push(rect(i, 10, i + 1, 11));
        }
        assert_eq!(pending.drain().unwrap().rects.len(), MAX_PENDING_RECTS);

        for i in 0..MAX_PENDING_RECTS as i64 {
            pending.push(rect(i, 10, i + 1, 11));
        }
        pending.push(rect(-5, 3, -4, 4));
        let rects = pending.drain().unwrap().rects;
        assert_eq!(
            rects,
            vec![rect(-5, 3, MAX_PENDING_RECTS as i64, 11)],
            "one box covers every collected change"
        );
    }

    #[test]
    fn a_stop_discards_what_was_collected_and_refuses_later_drains() {
        let mut pending = PendingChanges::default();
        pending.push(rect(0, 0, 1, 1));
        pending.windows_changed();
        assert!(!pending.is_stopped());
        assert!(pending.stop());
        assert!(pending.is_stopped());
        assert!(!pending.stop(), "only the first stop reports stopping");
        assert!(matches!(
            pending.drain().unwrap_err(),
            CoreError::PermissionDenied { .. }
        ));
        // Nothing collected before the stop survives it.
        assert!(pending.rects.is_empty());
        assert!(!pending.windows_changed);
    }

    #[test]
    fn rectangles_collected_while_a_layout_change_settles_belong_to_it() {
        let start = Instant::now();
        let mut pending = PendingChanges::default();
        pending.push_at(rect(0, 0, 1, 1), start);
        assert!(
            !pending.drain().unwrap().windows_changed,
            "no layout change"
        );

        pending.windows_changed_at(start);
        assert!(pending.drain().unwrap().windows_changed);
        // The repaint the change caused, collected after the drain that
        // reported the change.
        let repaint = start + LAYOUT_SETTLE - Duration::from_millis(1);
        pending.push_at(rect(0, 0, 1, 1), repaint);
        assert!(pending.drain().unwrap().windows_changed);

        pending.push_at(rect(2, 2, 3, 3), start + LAYOUT_SETTLE);
        assert_eq!(
            pending.drain().unwrap(),
            ListenerChanges {
                rects: vec![rect(2, 2, 3, 3)],
                windows_changed: false,
            },
            "settled"
        );
    }

    #[test]
    fn a_poll_hands_over_its_rectangles_and_window_changes() {
        let mut pending = PendingChanges::default();
        assert!(pending.hand_over(true, vec![rect(0, 0, 1, 1)], false));
        assert!(pending.hand_over(true, vec![rect(2, 2, 3, 3)], true));
        assert!(pending.hand_over(true, Vec::new(), false));
        assert_eq!(
            pending.drain().unwrap(),
            ListenerChanges {
                rects: vec![rect(0, 0, 1, 1), rect(2, 2, 3, 3)],
                windows_changed: true,
            }
        );
    }

    #[test]
    fn a_refused_permit_or_a_stop_ends_the_poll_loop() {
        let mut pending = PendingChanges::default();
        assert!(!pending.hand_over(false, vec![rect(0, 0, 1, 1)], true));
        assert!(pending.is_stopped(), "a refused permit stops the listener");
        assert!(matches!(
            pending.drain().unwrap_err(),
            CoreError::PermissionDenied { .. }
        ));

        let mut stopped = PendingChanges::default();
        stopped.stop();
        assert!(!stopped.hand_over(true, vec![rect(0, 0, 1, 1)], true));
        assert!(
            stopped.rects.is_empty(),
            "nothing is collected after a stop"
        );
    }
}
