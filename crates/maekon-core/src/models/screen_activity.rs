//! Screen activity model: "did the screen change, and has it stopped changing".
//!
//! A [`ScreenSample`] is a grid of per-tile signatures, never pixels. Adapters
//! reduce a frame to one signature per [`SCREEN_TILE_PX`]-sized tile and wipe
//! the pixels; this module only compares signatures.
//!
//! [`ScreenActivity`] keeps the latest sample, the time of the last change and
//! a change sequence number that increases whenever a sample differs from the
//! previous one. [`SettleTracker`] uses it to decide when the screen has
//! stopped changing after an action, within a bounded wait.
//!
//! Time is injected as milliseconds from a caller-chosen origin, so every
//! decision here is deterministic and testable without a clock.

use std::fmt;

use super::dirty_region::DirtyRegions;

/// Tile edge, in sample pixels, shared by every signature grid.
pub const SCREEN_TILE_PX: u32 = 16;

/// Why a signature grid could not be built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenSampleError {
    /// The sampled image had no pixels.
    EmptyGeometry,
    /// The number of signatures does not match the tile grid of the geometry.
    TileCountMismatch { expected: usize, actual: usize },
}

impl fmt::Display for ScreenSampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyGeometry => write!(f, "screen sample has no pixels"),
            Self::TileCountMismatch { expected, actual } => write!(
                f,
                "screen sample expected {expected} tile signatures, got {actual}"
            ),
        }
    }
}

impl std::error::Error for ScreenSampleError {}

/// One observation of the screen as per-tile signatures (row-major).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenSample {
    width: u32,
    height: u32,
    tiles: Vec<u64>,
}

impl ScreenSample {
    /// Build a sample for a `width` x `height` image. `tiles` holds one
    /// signature per tile in row-major order; edge tiles may be partial.
    pub fn new(width: u32, height: u32, tiles: Vec<u64>) -> Result<Self, ScreenSampleError> {
        if width == 0 || height == 0 {
            return Err(ScreenSampleError::EmptyGeometry);
        }
        let expected =
            width.div_ceil(SCREEN_TILE_PX) as usize * height.div_ceil(SCREEN_TILE_PX) as usize;
        if tiles.len() != expected {
            return Err(ScreenSampleError::TileCountMismatch {
                expected,
                actual: tiles.len(),
            });
        }
        Ok(Self {
            width,
            height,
            tiles,
        })
    }

    /// Compare this (older) sample with a `newer` one. Tiles are compared one
    /// to one; when the size differs every tile of `newer` counts as changed.
    pub fn diff(&self, newer: &ScreenSample) -> TileDiff {
        if (self.width, self.height) != (newer.width, newer.height) {
            return TileDiff {
                changed_tiles: newer.tiles.len(),
                geometry_changed: true,
            };
        }
        let changed_tiles = self
            .tiles
            .iter()
            .zip(&newer.tiles)
            .filter(|(old, new)| old != new)
            .count();
        TileDiff {
            changed_tiles,
            geometry_changed: false,
        }
    }
}

/// How two samples differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TileDiff {
    /// Tiles whose signature changed (every tile when the size changed).
    pub changed_tiles: usize,
    /// The samples had different sizes, for example after a display switch.
    pub geometry_changed: bool,
}

impl TileDiff {
    pub fn is_changed(&self) -> bool {
        self.changed_tiles != 0
    }
}

/// Latest sample, time of the last change and a change sequence number.
#[derive(Debug, Clone, Default)]
pub struct ScreenActivity {
    latest: Option<ScreenSample>,
    seq: u64,
    last_change_ms: Option<u64>,
}

impl ScreenActivity {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a sample observed at `now_ms`. The first sample only sets the
    /// baseline: it does not count as a change.
    pub fn observe(&mut self, sample: ScreenSample, now_ms: u64) -> TileDiff {
        let diff = self
            .latest
            .as_ref()
            .map(|previous| previous.diff(&sample))
            .unwrap_or_default();
        if diff.is_changed() {
            self.seq = self.seq.saturating_add(1);
            self.last_change_ms = Some(now_ms);
        }
        self.latest = Some(sample);
        diff
    }

    /// Record regions an OS change source reported at `now_ms`. Any region
    /// counts as one change. The stored sample is kept, so a change that a
    /// later sample also shows moves `seq` again: compare `seq` values for
    /// equality only.
    pub fn observe_regions(&mut self, regions: &DirtyRegions, now_ms: u64) -> bool {
        let changed = !regions.is_empty();
        if changed {
            self.seq = self.seq.saturating_add(1);
            self.last_change_ms = Some(now_ms);
        }
        changed
    }

    /// Increases by one for every observed sample that differs from the
    /// previous one. Consumers keep the value they last acted on and skip
    /// work while it is unchanged.
    pub fn seq(&self) -> u64 {
        self.seq
    }
}

/// Bounds for waiting until the screen stops changing after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlePolicy {
    /// Delay between samples.
    pub poll_interval_ms: u64,
    /// The screen counts as settled once it changed and then stayed
    /// unchanged for this long. Kept below a typical caret blink half-period
    /// (about 500 ms) so a blinking caret alone does not hold the wait open.
    pub quiet_ms: u64,
    /// Upper bound on the whole wait, measured from the end of the action.
    pub timeout_ms: u64,
}

impl SettlePolicy {
    pub const DEFAULT_POLL_INTERVAL_MS: u64 = 100;
    pub const DEFAULT_QUIET_MS: u64 = 300;
    /// Caps an untrusted timeout so one action cannot sample without bound.
    pub const MAX_TIMEOUT_MS: u64 = 10_000;

    /// Default sampling cadence with the given upper bound, capped at
    /// [`Self::MAX_TIMEOUT_MS`].
    pub fn with_timeout(timeout_ms: u64) -> Self {
        Self {
            poll_interval_ms: Self::DEFAULT_POLL_INTERVAL_MS,
            quiet_ms: Self::DEFAULT_QUIET_MS,
            timeout_ms: timeout_ms.min(Self::MAX_TIMEOUT_MS),
        }
    }
}

/// What a bounded settle wait observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettleReport {
    /// Net change between the pre-action sample and the final sample.
    pub net: TileDiff,
    /// The screen ended unchanged for at least `quiet_ms`.
    pub settled: bool,
}

/// Decides when post-action sampling can stop.
///
/// The wait ends early only after the screen changed and then stayed quiet for
/// `quiet_ms`. A screen that has not changed yet keeps the wait open until
/// `timeout_ms`, because a slow application may not have started drawing.
#[derive(Debug, Clone)]
pub struct SettleTracker {
    policy: SettlePolicy,
    baseline: ScreenSample,
    activity: ScreenActivity,
    action_done_ms: u64,
    last_observed_ms: u64,
}

impl SettleTracker {
    /// `before` was sampled before the action; `action_done_ms` is when the
    /// action returned, on the same time origin as later observations. The
    /// policy is bounded here: a positive cadence and at most
    /// [`SettlePolicy::MAX_TIMEOUT_MS`].
    pub fn new(policy: SettlePolicy, before: ScreenSample, action_done_ms: u64) -> Self {
        let mut activity = ScreenActivity::new();
        activity.observe(before.clone(), action_done_ms);
        Self {
            policy: SettlePolicy {
                poll_interval_ms: policy.poll_interval_ms.max(1),
                quiet_ms: policy.quiet_ms,
                timeout_ms: policy.timeout_ms.min(SettlePolicy::MAX_TIMEOUT_MS),
            },
            baseline: before,
            activity,
            action_done_ms,
            last_observed_ms: action_done_ms,
        }
    }

    /// The bounded policy this tracker applies.
    pub fn policy(&self) -> SettlePolicy {
        self.policy
    }

    /// Feed one post-action sample taken at `now_ms`; `Some` ends the wait.
    /// Times that go backwards are treated as the latest time already seen.
    pub fn observe(&mut self, sample: ScreenSample, now_ms: u64) -> Option<SettleReport> {
        self.last_observed_ms = now_ms.max(self.last_observed_ms);
        self.activity.observe(sample, self.last_observed_ms);

        let last_change_ms = self.activity.last_change_ms;
        let quiet_ms = self.last_observed_ms - last_change_ms.unwrap_or(self.action_done_ms);
        let settled = quiet_ms >= self.policy.quiet_ms;
        let timed_out = self.last_observed_ms - self.action_done_ms >= self.policy.timeout_ms;
        if !(settled && last_change_ms.is_some() || timed_out) {
            return None;
        }
        let latest = self.activity.latest.as_ref().unwrap_or(&self.baseline);
        Some(SettleReport {
            net: self.baseline.diff(latest),
            settled,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 64; // 4 columns
    const H: u32 = 48; // 3 rows

    fn sample(tiles: &[u64]) -> ScreenSample {
        ScreenSample::new(W, H, tiles.to_vec()).unwrap()
    }

    fn blank() -> ScreenSample {
        sample(&[0; 12])
    }

    fn with_changed(indices: &[usize]) -> ScreenSample {
        let mut tiles = vec![0; 12];
        for index in indices {
            tiles[*index] = 1;
        }
        sample(&tiles)
    }

    fn policy(quiet_ms: u64, timeout_ms: u64) -> SettlePolicy {
        SettlePolicy {
            poll_interval_ms: 10,
            quiet_ms,
            timeout_ms,
        }
    }

    #[test]
    fn sample_rejects_empty_geometry_and_wrong_tile_count() {
        assert_eq!(
            ScreenSample::new(0, 10, vec![]),
            Err(ScreenSampleError::EmptyGeometry)
        );
        // 17x16 needs two columns (partial edge tile) and one row.
        assert_eq!(
            ScreenSample::new(17, 16, vec![0]),
            Err(ScreenSampleError::TileCountMismatch {
                expected: 2,
                actual: 1
            })
        );
        let edge = ScreenSample::new(17, 16, vec![0, 0]).unwrap();
        let single = ScreenSample::new(16, 16, vec![0]).unwrap();
        assert_eq!(single.diff(&edge).changed_tiles, 2);
    }

    #[test]
    fn sample_errors_explain_themselves() {
        assert_eq!(
            ScreenSampleError::EmptyGeometry.to_string(),
            "screen sample has no pixels"
        );
        assert_eq!(
            ScreenSampleError::TileCountMismatch {
                expected: 2,
                actual: 1
            }
            .to_string(),
            "screen sample expected 2 tile signatures, got 1"
        );
    }

    #[test]
    fn identical_samples_do_not_differ() {
        let diff = blank().diff(&blank());
        assert_eq!(diff, TileDiff::default());
        assert!(!diff.is_changed());
    }

    #[test]
    fn only_the_tiles_whose_signature_changed_are_counted() {
        let diff = blank().diff(&with_changed(&[0, 5, 10]));
        assert_eq!(
            diff,
            TileDiff {
                changed_tiles: 3,
                geometry_changed: false
            }
        );
        assert!(diff.is_changed());
    }

    #[test]
    fn a_geometry_change_counts_as_a_whole_screen_change() {
        let wider = ScreenSample::new(W + SCREEN_TILE_PX, H, vec![0; 15]).unwrap();
        let diff = blank().diff(&wider);
        assert_eq!(
            diff,
            TileDiff {
                changed_tiles: 15,
                geometry_changed: true
            }
        );
        assert!(diff.is_changed());
        // The same tile values at another height are still a different screen.
        let taller = ScreenSample::new(W, H + SCREEN_TILE_PX, vec![0; 16]).unwrap();
        assert!(blank().diff(&taller).geometry_changed);
    }

    #[test]
    fn seq_moves_only_when_a_sample_differs() {
        let mut activity = ScreenActivity::new();
        assert_eq!(activity.observe(blank(), 0), TileDiff::default());
        assert_eq!(activity.seq(), 0, "the baseline is not a change");
        assert_eq!(activity.last_change_ms, None);

        activity.observe(blank(), 10);
        assert_eq!(activity.seq(), 0);

        let diff = activity.observe(with_changed(&[6]), 20);
        assert_eq!(diff.changed_tiles, 1);
        assert_eq!(activity.seq(), 1);
        assert_eq!(activity.last_change_ms, Some(20));

        activity.observe(with_changed(&[6]), 30);
        assert_eq!(activity.seq(), 1, "a repeated screen keeps the same seq");
        assert_eq!(activity.last_change_ms, Some(20));
    }

    #[test]
    fn os_regions_move_seq_only_when_something_changed() {
        use crate::models::dirty_region::DirtyRect;

        let mut activity = ScreenActivity::new();
        let unchanged = DirtyRegions::new(W, H, []).unwrap();
        assert!(!activity.observe_regions(&unchanged, 10));
        assert_eq!(activity.seq(), 0);
        assert_eq!(activity.last_change_ms, None);

        let changed = DirtyRegions::new(
            W,
            H,
            [DirtyRect {
                left: 1,
                top: 2,
                right: 4,
                bottom: 6,
            }],
        )
        .unwrap();
        assert!(activity.observe_regions(&changed, 20));
        assert_eq!(activity.seq(), 1);
        assert_eq!(activity.last_change_ms, Some(20));

        // Regions do not replace the stored sample: the first sample after
        // them is still only the baseline.
        assert_eq!(activity.observe(blank(), 30), TileDiff::default());
        assert_eq!(activity.seq(), 1);
    }

    #[test]
    fn settles_once_the_screen_changed_and_stayed_quiet() {
        let mut tracker = SettleTracker::new(policy(30, 1_000), blank(), 0);
        assert_eq!(tracker.observe(with_changed(&[2]), 10), None);
        assert_eq!(tracker.observe(with_changed(&[2, 3]), 20), None);
        assert_eq!(tracker.observe(with_changed(&[2, 3]), 40), None);
        let report = tracker.observe(with_changed(&[2, 3]), 50).unwrap();
        assert!(report.settled);
        assert_eq!(report.net.changed_tiles, 2);
    }

    #[test]
    fn a_screen_that_keeps_changing_ends_at_the_timeout_unsettled() {
        // A bounded script, so a broken stop condition fails instead of hanging.
        let mut tracker = SettleTracker::new(policy(30, 100), blank(), 0);
        for (index, now) in [20, 40, 60, 80].into_iter().enumerate() {
            assert_eq!(tracker.observe(with_changed(&[index]), now), None);
        }
        let report = tracker.observe(with_changed(&[4]), 100).unwrap();
        assert!(!report.settled);
        assert!(report.net.is_changed());
    }

    #[test]
    fn an_unchanged_screen_waits_for_the_full_timeout() {
        // A slow application may not have drawn yet, so quiet alone does not
        // end the wait before the bound.
        let mut tracker = SettleTracker::new(policy(30, 100), blank(), 0);
        for now in [10, 40, 70] {
            assert_eq!(tracker.observe(blank(), now), None);
        }
        let report = tracker.observe(blank(), 100).unwrap();
        assert!(!report.net.is_changed());
        assert!(report.settled, "unchanged for the whole wait");
    }

    #[test]
    fn a_zero_timeout_takes_one_sample_and_cannot_claim_settled() {
        let mut tracker = SettleTracker::new(policy(30, 0), blank(), 0);
        let report = tracker.observe(with_changed(&[0]), 0).unwrap();
        assert!(report.net.is_changed());
        assert!(!report.settled);
    }

    #[test]
    fn a_change_that_reverts_ends_the_wait_without_a_net_change() {
        // The revert at 20 is a change, so the wait ends after 30 ms of quiet
        // instead of running to the 1000 ms bound.
        let mut tracker = SettleTracker::new(policy(30, 1_000), blank(), 0);
        assert_eq!(tracker.observe(with_changed(&[7]), 10), None);
        assert_eq!(tracker.observe(blank(), 20), None);
        let report = tracker.observe(blank(), 60).unwrap();
        assert!(report.settled);
        assert!(!report.net.is_changed());
    }

    #[test]
    fn the_bound_counts_from_the_end_of_the_action() {
        let mut tracker = SettleTracker::new(policy(30, 50), blank(), 100);
        assert_eq!(tracker.observe(with_changed(&[0]), 120), None);
        let report = tracker.observe(with_changed(&[1]), 150).unwrap();
        assert!(!report.settled);
    }

    #[test]
    fn backwards_time_does_not_underflow() {
        let mut tracker = SettleTracker::new(policy(30, 100), blank(), 50);
        assert_eq!(tracker.observe(with_changed(&[0]), 10), None);
        let report = tracker.observe(with_changed(&[0]), 80).unwrap();
        assert!(report.settled, "quiet since the change at 50");
        assert_eq!(report.net.changed_tiles, 1);
    }

    #[test]
    fn untrusted_timeouts_are_bounded() {
        let policy = SettlePolicy::with_timeout(u64::MAX);
        assert_eq!(policy.timeout_ms, SettlePolicy::MAX_TIMEOUT_MS);
        assert_eq!(
            policy.poll_interval_ms,
            SettlePolicy::DEFAULT_POLL_INTERVAL_MS
        );
        assert_eq!(policy.quiet_ms, SettlePolicy::DEFAULT_QUIET_MS);
        assert_eq!(SettlePolicy::with_timeout(250).timeout_ms, 250);

        let tracker = SettleTracker::new(
            SettlePolicy {
                poll_interval_ms: 0,
                quiet_ms: 7,
                timeout_ms: u64::MAX,
            },
            blank(),
            0,
        );
        assert_eq!(
            tracker.policy(),
            SettlePolicy {
                poll_interval_ms: 1,
                quiet_ms: 7,
                timeout_ms: SettlePolicy::MAX_TIMEOUT_MS,
            }
        );
    }
}
