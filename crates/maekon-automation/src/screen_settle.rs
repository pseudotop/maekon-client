//! Post-action screen verification: sample the screen before an action, then
//! keep sampling after it until the screen changed and went quiet, or until
//! the bound, and report only what was observed.

use std::time::Duration;

use maekon_core::error::CoreError;
use maekon_core::models::intent::VerificationResult;
use maekon_core::models::screen_activity::{
    ScreenSample, SettlePolicy, SettleReport, SettleTracker,
};
use maekon_core::ports::screen_probe::ScreenChangeProbe;
use tokio::time::Instant;
use tracing::debug;

/// The pre-action sample, or `None` when the screen cannot be observed.
pub(crate) async fn sample_before_action(
    probe: Option<&dyn ScreenChangeProbe>,
) -> Option<ScreenSample> {
    let probe = probe?;
    match probe.sample().await {
        Ok(sample) => Some(sample),
        Err(error) => {
            debug!(
                probe = probe.name(),
                err.code = %error.code(),
                "pre-action screen sample unavailable; verification skipped"
            );
            None
        }
    }
}

/// Compare the screen after the action with the pre-action sample.
///
/// Reports `None` when there is no probe, no pre-action sample, or a later
/// sample fails: an unobserved screen is never reported as changed or
/// unchanged. Without an observation the fixed `verify_delay_ms` pause is
/// kept, so preset timing does not change when verification is unavailable.
pub(crate) async fn verify_after_action(
    probe: Option<&dyn ScreenChangeProbe>,
    before: Option<ScreenSample>,
    verify_delay_ms: u64,
) -> Option<VerificationResult> {
    let (Some(probe), Some(before)) = (probe, before) else {
        tokio::time::sleep(Duration::from_millis(verify_delay_ms)).await;
        return None;
    };
    let policy = SettlePolicy::with_timeout(verify_delay_ms);
    match observe_until_settled(probe, before, policy).await {
        Ok(report) => {
            debug!(
                probe = probe.name(),
                changed_tiles = report.net.changed_tiles,
                settled = report.settled,
                "post-action screen verification"
            );
            Some(VerificationResult {
                screen_changed: report.net.is_changed(),
                changed_regions: report.net.changed_tiles,
                text_found: None,
                settled: Some(report.settled),
            })
        }
        Err(error) => {
            debug!(
                probe = probe.name(),
                err.code = %error.code(),
                "post-action screen sample unavailable; verification skipped"
            );
            None
        }
    }
}

/// Sample after an action until [`SettleTracker`] ends the wait.
///
/// Time is measured from the call, which the executor makes as soon as the
/// action returns. The first sample comes one poll interval after the call
/// (or at the bound, if that is sooner). Any sampling error ends the wait at
/// once, so a revoked capture permit stops further samples.
async fn observe_until_settled(
    probe: &dyn ScreenChangeProbe,
    before: ScreenSample,
    policy: SettlePolicy,
) -> Result<SettleReport, CoreError> {
    let started = Instant::now();
    let mut tracker = SettleTracker::new(policy, before, 0);
    let policy = tracker.policy();
    loop {
        let remaining_ms = policy.timeout_ms.saturating_sub(millis_since(started));
        tokio::time::sleep(Duration::from_millis(
            policy.poll_interval_ms.min(remaining_ms),
        ))
        .await;
        let sample = probe.sample().await?;
        if let Some(report) = tracker.observe(sample, millis_since(started)) {
            return Ok(report);
        }
    }
}

fn millis_since(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
pub(crate) mod test_probe {
    //! A scripted [`ScreenChangeProbe`] shared by the executor tests.

    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::models::screen_activity::ScreenSample;
    use maekon_core::ports::screen_probe::ScreenChangeProbe;

    /// One scripted answer: a screen whose first tile carries the mark, or a
    /// denied capture.
    #[derive(Clone, Copy)]
    pub(crate) enum ProbeStep {
        Screen(u64),
        Denied,
    }

    /// Replays scripted answers, repeating the last one, and counts calls.
    pub(crate) struct ScriptedProbe {
        steps: Vec<ProbeStep>,
        calls: AtomicUsize,
    }

    impl ScriptedProbe {
        pub(crate) fn new(steps: Vec<ProbeStep>) -> Arc<Self> {
            Arc::new(Self {
                steps,
                calls: AtomicUsize::new(0),
            })
        }

        pub(crate) fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ScreenChangeProbe for ScriptedProbe {
        async fn sample(&self) -> Result<ScreenSample, CoreError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            match self.steps.get(call).or_else(|| self.steps.last()) {
                Some(ProbeStep::Screen(mark)) => Ok(screen(*mark)),
                _ => Err(maekon_core::ports::vision::capture_denied(
                    "test revocation",
                )),
            }
        }

        fn name(&self) -> &str {
            "scripted"
        }
    }

    /// A 2x2-tile screen whose first tile carries `mark`.
    pub(crate) fn screen(mark: u64) -> ScreenSample {
        ScreenSample::new(32, 32, vec![mark, 0, 0, 0]).expect("2x2 tile grid")
    }

    /// Fail instead of hanging when a wait does not end within `limit_ms`.
    pub(crate) async fn within<F: Future>(limit_ms: u64, future: F) -> F::Output {
        tokio::time::timeout(Duration::from_millis(limit_ms), future)
            .await
            .expect("the wait must end within its bound")
    }
}

#[cfg(test)]
mod tests {
    use super::test_probe::{screen, within, ProbeStep, ScriptedProbe};
    use super::*;

    #[tokio::test]
    async fn the_pre_action_sample_needs_a_probe_and_a_permitted_capture() {
        assert_eq!(sample_before_action(None).await, None);
        let denied = ScriptedProbe::new(vec![ProbeStep::Denied]);
        assert_eq!(sample_before_action(Some(denied.as_ref())).await, None);
        let probe = ScriptedProbe::new(vec![ProbeStep::Screen(7)]);
        assert_eq!(
            sample_before_action(Some(probe.as_ref())).await,
            Some(screen(7))
        );
    }

    #[tokio::test]
    async fn a_change_that_settles_ends_the_wait_before_the_bound() {
        // The action redraws one tile and the screen then stays put.
        let probe = ScriptedProbe::new(vec![ProbeStep::Screen(1)]);
        let started = Instant::now();
        let verification = within(
            5_000,
            verify_after_action(Some(probe.as_ref()), Some(screen(0)), 3_000),
        )
        .await
        .expect("observed");
        assert!(verification.screen_changed);
        assert_eq!(verification.changed_regions, 1);
        assert_eq!(verification.text_found, None);
        assert_eq!(verification.settled, Some(true));
        assert!(
            started.elapsed() < Duration::from_millis(2_000),
            "a settled screen ends the wait before the bound"
        );
    }

    #[tokio::test]
    async fn an_unchanged_screen_is_sampled_until_the_bound() {
        // Nothing is redrawn, so nothing is reported as changed.
        let probe = ScriptedProbe::new(vec![ProbeStep::Screen(0)]);
        let started = Instant::now();
        let verification = within(
            5_000,
            verify_after_action(Some(probe.as_ref()), Some(screen(0)), 400),
        )
        .await
        .expect("observed");
        assert!(!verification.screen_changed);
        assert_eq!(verification.changed_regions, 0);
        assert_eq!(verification.settled, Some(true));
        assert!(started.elapsed() >= Duration::from_millis(400));
        assert!(probe.calls() >= 2);
    }

    #[tokio::test]
    async fn a_screen_still_changing_at_the_bound_is_not_settled() {
        let probe = ScriptedProbe::new((1..=50).map(ProbeStep::Screen).collect());
        let verification = within(
            5_000,
            verify_after_action(Some(probe.as_ref()), Some(screen(0)), 250),
        )
        .await
        .expect("observed");
        assert!(verification.screen_changed);
        assert_eq!(verification.settled, Some(false));
    }

    #[tokio::test]
    async fn a_denied_sample_ends_the_wait_unverified() {
        // A permit revoked after the action stops sampling at once and leaves
        // the result unverified instead of guessing.
        let probe = ScriptedProbe::new(vec![ProbeStep::Denied]);
        let started = Instant::now();
        let verification = within(
            5_000,
            verify_after_action(Some(probe.as_ref()), Some(screen(0)), 3_000),
        )
        .await;
        assert!(verification.is_none());
        assert_eq!(probe.calls(), 1, "no sample after the denial");
        assert!(started.elapsed() < Duration::from_millis(2_000));
    }

    #[tokio::test]
    async fn without_an_observation_the_fixed_pause_is_kept() {
        // No probe, or no pre-action sample to compare with: no verification,
        // and the pause that presets may rely on stays in place.
        let probe = ScriptedProbe::new(vec![ProbeStep::Screen(1)]);
        let cases: [(Option<&dyn ScreenChangeProbe>, Option<ScreenSample>); 2] =
            [(None, Some(screen(0))), (Some(probe.as_ref()), None)];
        for (case_probe, before) in cases {
            let started = Instant::now();
            assert!(verify_after_action(case_probe, before, 40).await.is_none());
            assert!(started.elapsed() >= Duration::from_millis(40));
        }
        assert_eq!(probe.calls(), 0, "nothing to compare with, so no sampling");
    }
}
