//! Pure OCR age, attempt-budget and result-ordering metadata for #13684.
//!
//! This model does not capture, invoke OCR, store text, or grant permission.
//! One serialized capture owner must retain one instance and use its stamps and
//! tokens only with that instance. Observe metadata from the actual input sample
//! at its capture time; never restamp an old frame with its arrival time.
//! A replacement instance/time origin requires retiring every old completion.
//!
//! [ScreenActivity](super::screen_activity::ScreenActivity) and
//! [DirtyRegions](super::dirty_region::DirtyRegions) can supply change observations.
//! Their revisions are not native identity.
//! [DetectionSceneState](super::detection_scene::DetectionSceneState) owns the
//! separate running/latest-pending contract: this model has no worker slots,
//! queue, cancellation, or proof of provider termination.
//!
//! Runtime retains native provenance, live CapturePermit/own-field consent,
//! payload validation and checks immediately before prompt/store/queue/display.
//! Even an age-eligible result is metadata, not permission to use its payload.

/// A caller-computed proportion of changed input, bounded to 0..=10_000.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeBasisPoints(u16);

impl ChangeBasisPoints {
    pub fn new(value: u16) -> Result<Self, OcrError> {
        if value > 10_000 {
            return Err(OcrError::InvalidChange);
        }
        Ok(Self(value))
    }
}

/// Explicit policy; construction never chooses production timing defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrPolicy {
    min_interval_ms: u64,
    max_context_age_ms: u64,
    change_threshold: ChangeBasisPoints,
}

impl OcrPolicy {
    /// Require a positive interval, an age at least that interval, and a
    /// positive change threshold. An ROI change independently requests refresh.
    pub fn new(
        min_interval_ms: u64,
        max_context_age_ms: u64,
        change_threshold: ChangeBasisPoints,
    ) -> Result<Self, OcrError> {
        if min_interval_ms == 0 || min_interval_ms > max_context_age_ms || change_threshold.0 == 0 {
            return Err(OcrError::InvalidPolicy);
        }
        Ok(Self {
            min_interval_ms,
            max_context_age_ms,
            change_threshold,
        })
    }
}

/// Opaque epochs supplied by the actual capture owner, not app/title identity
/// or capture authority. A title-denied observation is not an OCR denial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrScope {
    pub scene_epoch: u64,
    pub monitor_epoch: u64,
    pub authority_epoch: u64,
}

/// Unavailable change observation is distinct from an observed zero change.
/// Availability of the source identity is separately represented by OcrScope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrObservation {
    Known {
        change: ChangeBasisPoints,
        roi_changed: bool,
    },
    Unavailable,
}

impl OcrObservation {
    fn requires_refresh(self, policy: OcrPolicy) -> bool {
        match self {
            Self::Known {
                change,
                roi_changed,
            } => roi_changed || change.0 >= policy.change_threshold.0,
            Self::Unavailable => false,
        }
    }
}

/// Issued for an ordered input observation. Private revision fences a previous
/// sample even when a later content change has the same millisecond timestamp.
/// Retain this with its actual frame; the model cannot authenticate the pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrSampleStamp {
    revision: u64,
    scope: OcrScope,
    sampled_at_ms: u64,
}

/// A reservation, never permission or an assertion that a worker has started.
/// Do not reconstruct this token from provider output or use another instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrRequestToken {
    generation: u64,
    sample: OcrSampleStamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrRefreshReason {
    NoContext,
    ContentChanged,
    ScopeChanged,
    MaxAge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrReadiness {
    /// No refresh is required by the known metadata and configured age bound.
    /// Unavailable observation quality remains explicit in OcrAssessment.
    Current,
    Due(OcrRefreshReason),
    Throttled {
        reason: OcrRefreshReason,
        remaining_ms: u64,
    },
}

/// Historical successful metadata. It remains visible after invalidation.
/// It is eligible only when the current assessment is Current and the caller's
/// retained payload token equals this request; runtime permission checks remain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrSuccess {
    pub request: OcrRequestToken,
    pub sampled_at_ms: u64,
    pub completed_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OcrAssessment {
    pub readiness: OcrReadiness,
    pub observation: OcrObservation,
    pub last_success: Option<OcrSuccess>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrOutcome {
    /// The caller has validated and classified the payload, which stays outside
    /// this model. Merely returning from a provider is not sufficient.
    NonEmptySuccess,
    Empty,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrCompletion {
    Accepted(OcrSuccess),
    Empty,
    Failed,
    FencedOut,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrError {
    InvalidChange,
    InvalidPolicy,
    Disabled,
    ScopeUnavailable,
    ObservationRequired,
    ClockRegression,
    GenerationExhausted,
    ObsoleteSample,
    SampleExpired,
    NotDue,
    Throttled { remaining_ms: u64 },
}

/// Constant-size, metadata-only state. Time uses one injected monotonic origin.
/// Failures consume the attempt budget, not the last successful sample's age.
/// Reset invalidates context but never refills the budget or rewinds generations.
#[derive(Debug)]
pub struct OcrContextState {
    policy: OcrPolicy,
    enabled: bool,
    scope: Option<OcrScope>,
    observation: OcrObservation,
    high_water_ms: u64,
    last_attempt_ms: Option<u64>,
    last_success: Option<OcrSuccess>,
    demand: Option<OcrRefreshReason>,
    generation: u64,
    exhausted: bool,
    input_revision: Option<u64>,
    latest: Option<OcrRequestToken>,
}

impl OcrContextState {
    pub fn new(policy: OcrPolicy) -> Self {
        Self {
            policy,
            enabled: false,
            scope: None,
            observation: OcrObservation::Unavailable,
            high_water_ms: 0,
            last_attempt_ms: None,
            last_success: None,
            demand: Some(OcrRefreshReason::NoContext),
            generation: 0,
            exhausted: false,
            input_revision: None,
            latest: None,
        }
    }

    /// Every reset requires a new input observation, even when re-enabling the
    /// same scene. Disabling still takes effect if the supplied clock regresses.
    pub fn reset(&mut self, now_ms: u64, enabled: bool) -> Result<(), OcrError> {
        self.enabled = enabled;
        self.scope = None;
        self.observation = OcrObservation::Unavailable;
        self.invalidate(OcrRefreshReason::NoContext);
        self.advance_time(now_ms)
    }

    /// Register the actual sample at its capture time before later operations.
    /// A delayed observation older than the time high-water mark is rejected.
    /// Known scope with unavailable change data supports bounded initial/age
    /// refresh; missing scope cannot identify a current result.
    pub fn observe(
        &mut self,
        sampled_at_ms: u64,
        scope: Option<OcrScope>,
        observation: OcrObservation,
    ) -> Result<OcrSampleStamp, OcrError> {
        self.check(sampled_at_ms)?;
        let Some(scope) = scope else {
            self.scope = None;
            self.observation = OcrObservation::Unavailable;
            self.invalidate(OcrRefreshReason::ScopeChanged);
            return Err(OcrError::ScopeUnavailable);
        };
        if self.scope != Some(scope) {
            let reason = if self.scope.is_some() {
                OcrRefreshReason::ScopeChanged
            } else {
                OcrRefreshReason::NoContext
            };
            self.invalidate(reason);
            self.scope = Some(scope);
        }
        self.observation = observation;
        if observation.requires_refresh(self.policy) {
            self.invalidate(OcrRefreshReason::ContentChanged);
        }
        let revision = match self.input_revision {
            Some(revision) => revision,
            None => {
                let revision = self.allocate_generation()?;
                self.input_revision = Some(revision);
                revision
            }
        };
        Ok(OcrSampleStamp {
            revision,
            scope,
            sampled_at_ms,
        })
    }

    /// Evaluate only the current metadata. This call also rejects clock reversal;
    /// a previously returned assessment cannot authorize a later reservation.
    pub fn assessment(&mut self, now_ms: u64) -> Result<OcrAssessment, OcrError> {
        self.check(now_ms)?;
        self.scope.ok_or(OcrError::ScopeUnavailable)?;
        self.input_revision.ok_or(OcrError::ObservationRequired)?;
        let reason = match self.last_success {
            Some(success)
                if now_ms.saturating_sub(success.sampled_at_ms)
                    < self.policy.max_context_age_ms =>
            {
                self.demand
            }
            _ => self.demand.or(Some(OcrRefreshReason::MaxAge)),
        };
        let remaining_ms = self.last_attempt_ms.map_or(0, |attempt| {
            self.policy
                .min_interval_ms
                .saturating_sub(now_ms.saturating_sub(attempt))
        });
        let readiness = match (reason, remaining_ms) {
            (None, _) => OcrReadiness::Current,
            (Some(reason), 0) => OcrReadiness::Due(reason),
            (Some(reason), remaining_ms) => OcrReadiness::Throttled {
                reason,
                remaining_ms,
            },
        };
        Ok(OcrAssessment {
            readiness,
            observation: self.observation,
            last_success: self.last_success,
        })
    }

    /// Atomically recheck eligibility and charge an attempt before dispatch.
    /// An abandoned reservation still consumes its budget. The caller must
    /// separately coordinate its real worker slot and live capture permission.
    pub fn reserve(
        &mut self,
        now_ms: u64,
        sample: OcrSampleStamp,
    ) -> Result<OcrRequestToken, OcrError> {
        let assessment = self.assessment(now_ms)?;
        if (Some(sample.scope), Some(sample.revision)) != (self.scope, self.input_revision) {
            return Err(OcrError::ObsoleteSample);
        }
        if now_ms.saturating_sub(sample.sampled_at_ms) >= self.policy.max_context_age_ms {
            return Err(OcrError::SampleExpired);
        }
        match assessment.readiness {
            OcrReadiness::Current => return Err(OcrError::NotDue),
            OcrReadiness::Throttled { remaining_ms, .. } => {
                return Err(OcrError::Throttled { remaining_ms });
            }
            OcrReadiness::Due(_) => {}
        }
        let token = OcrRequestToken {
            generation: self.allocate_generation()?,
            sample,
        };
        self.latest = Some(token);
        self.last_attempt_ms = Some(now_ms);
        Ok(token)
    }

    /// Consume only the current reservation once. Age is measured from capture,
    /// not completion; a delayed successful payload cannot rejuvenate old input.
    /// This does not free a worker slot or prove provider termination.
    pub fn complete(
        &mut self,
        now_ms: u64,
        token: OcrRequestToken,
        outcome: OcrOutcome,
    ) -> Result<OcrCompletion, OcrError> {
        self.check(now_ms)?;
        if self.latest != Some(token) {
            return Ok(OcrCompletion::FencedOut);
        }
        self.latest = None;
        match outcome {
            OcrOutcome::NonEmptySuccess => {
                if now_ms.saturating_sub(token.sample.sampled_at_ms)
                    >= self.policy.max_context_age_ms
                {
                    return Ok(OcrCompletion::Expired);
                }
                let success = OcrSuccess {
                    request: token,
                    sampled_at_ms: token.sample.sampled_at_ms,
                    completed_at_ms: now_ms,
                };
                self.last_success = Some(success);
                self.demand = None;
                Ok(OcrCompletion::Accepted(success))
            }
            OcrOutcome::Empty => Ok(OcrCompletion::Empty),
            OcrOutcome::Failed => Ok(OcrCompletion::Failed),
        }
    }

    fn invalidate(&mut self, reason: OcrRefreshReason) {
        self.input_revision = None;
        self.latest = None;
        self.demand = Some(reason);
    }

    fn advance_time(&mut self, now_ms: u64) -> Result<(), OcrError> {
        if now_ms < self.high_water_ms {
            self.invalidate(OcrRefreshReason::NoContext);
            return Err(OcrError::ClockRegression);
        }
        self.high_water_ms = now_ms;
        Ok(())
    }

    fn check(&mut self, now_ms: u64) -> Result<(), OcrError> {
        self.advance_time(now_ms)?;
        if !self.enabled {
            return Err(OcrError::Disabled);
        }
        if self.exhausted {
            return Err(OcrError::GenerationExhausted);
        }
        Ok(())
    }

    fn allocate_generation(&mut self) -> Result<u64, OcrError> {
        let Some(generation) = self.generation.checked_add(1) else {
            self.exhausted = true;
            self.invalidate(OcrRefreshReason::NoContext);
            return Err(OcrError::GenerationExhausted);
        };
        self.generation = generation;
        Ok(generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPE: OcrScope = OcrScope {
        scene_epoch: 0,
        monitor_epoch: 0,
        authority_epoch: 0,
    };

    fn change(amount: u16, roi_changed: bool) -> OcrObservation {
        OcrObservation::Known {
            change: ChangeBasisPoints::new(amount).unwrap(),
            roi_changed,
        }
    }

    fn state() -> OcrContextState {
        let policy = OcrPolicy::new(3, 10, ChangeBasisPoints::new(1_000).unwrap()).unwrap();
        let mut state = OcrContextState::new(policy);
        state.reset(0, true).unwrap();
        state
    }

    fn sample(state: &mut OcrContextState, at: u64) -> OcrSampleStamp {
        state.observe(at, Some(SCOPE), change(0, false)).unwrap()
    }

    fn start(state: &mut OcrContextState, at: u64) -> OcrRequestToken {
        let sample = sample(state, at);
        state.reserve(at, sample).unwrap()
    }

    #[track_caller]
    fn readiness(state: &mut OcrContextState, at: u64) -> OcrReadiness {
        state.assessment(at).unwrap().readiness
    }

    #[track_caller]
    fn assert_fenced_out(state: &mut OcrContextState, at: u64, token: OcrRequestToken) {
        assert_eq!(
            state.complete(at, token, OcrOutcome::NonEmptySuccess),
            Ok(OcrCompletion::FencedOut)
        );
    }

    fn succeed(state: &mut OcrContextState, at: u64, token: OcrRequestToken) -> OcrSuccess {
        let OcrCompletion::Accepted(success) = state
            .complete(at, token, OcrOutcome::NonEmptySuccess)
            .unwrap()
        else {
            panic!("expected a current nonempty result");
        };
        success
    }

    #[test]
    fn policy_rejects_invalid_ranges_and_accepts_exact_boundaries() {
        assert_eq!(ChangeBasisPoints::new(10_001), Err(OcrError::InvalidChange));
        assert_eq!(
            ChangeBasisPoints::new(u16::MAX),
            Err(OcrError::InvalidChange)
        );
        let zero = ChangeBasisPoints::new(0).unwrap();
        let one = ChangeBasisPoints::new(1).unwrap();
        let full = ChangeBasisPoints::new(10_000).unwrap();
        for (min, max, threshold) in [(0, 10, one), (1, 0, one), (11, 10, one), (3, 10, zero)] {
            assert_eq!(
                OcrPolicy::new(min, max, threshold),
                Err(OcrError::InvalidPolicy)
            );
        }
        for (min, max, threshold) in [(1, 1, one), (3, 10, full), (u64::MAX, u64::MAX, one)] {
            let policy = OcrPolicy::new(min, max, threshold).expect("valid boundary policy");
            assert_eq!(policy.min_interval_ms, min);
            assert_eq!(policy.max_context_age_ms, max);
            assert_eq!(policy.change_threshold, threshold);
        }
    }

    #[test]
    fn state_starts_disabled_and_needs_a_known_scope() {
        let policy = OcrPolicy::new(3, 10, ChangeBasisPoints::new(1_000).unwrap()).unwrap();
        let mut state = OcrContextState::new(policy);
        assert_eq!(state.assessment(0), Err(OcrError::Disabled));
        assert_eq!(
            state.observe(0, Some(SCOPE), change(0, false)),
            Err(OcrError::Disabled)
        );
        state.reset(0, true).unwrap();
        assert_eq!(state.assessment(0), Err(OcrError::ScopeUnavailable));
        let stamp = sample(&mut state, 0);
        assert_eq!(
            readiness(&mut state, 0),
            OcrReadiness::Due(OcrRefreshReason::NoContext)
        );
        let token = state.reserve(0, stamp).unwrap();
        succeed(&mut state, 0, token);
        assert_eq!(readiness(&mut state, 0), OcrReadiness::Current);
    }

    #[test]
    fn static_hundred_ticks_have_independent_exact_schedule_and_attempt_bound() {
        let mut state = state();
        let mut starts = Vec::new();
        for tick in 0..100 {
            let stamp = sample(&mut state, tick);
            if matches!(readiness(&mut state, tick), OcrReadiness::Due(_)) {
                let token = state.reserve(tick, stamp).unwrap();
                starts.push(tick);
                succeed(&mut state, tick, token);
            }
        }
        assert_eq!(starts, vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90]);
        assert!(starts.len() <= 1 + 99 / 3);
        let end = state.assessment(99).unwrap();
        assert_eq!(end.readiness, OcrReadiness::Current);
        assert_eq!(99 - end.last_success.unwrap().sampled_at_ms, 9);
    }

    #[test]
    fn failed_and_empty_hundred_tick_traces_consume_budget_without_success() {
        for outcome in [OcrOutcome::Failed, OcrOutcome::Empty] {
            let mut state = state();
            let mut starts = Vec::new();
            for tick in 0..100 {
                let stamp = sample(&mut state, tick);
                if matches!(readiness(&mut state, tick), OcrReadiness::Due(_)) {
                    let token = state.reserve(tick, stamp).unwrap();
                    starts.push(tick);
                    let expected = match outcome {
                        OcrOutcome::Failed => OcrCompletion::Failed,
                        _ => OcrCompletion::Empty,
                    };
                    assert_eq!(state.complete(tick, token, outcome), Ok(expected));
                }
                assert_eq!(state.assessment(tick).unwrap().last_success, None);
            }
            let expected: Vec<_> = (0..100).step_by(3).collect();
            assert_eq!(starts, expected);
            assert_eq!(starts.len(), 1 + 99 / 3);
            let token = start(&mut state, 102);
            succeed(&mut state, 102, token);
            assert_eq!(readiness(&mut state, 102), OcrReadiness::Current);
        }
    }

    #[test]
    fn minimum_interval_is_exact_and_abandoned_reservations_are_charged() {
        let mut state = state();
        let _abandoned = start(&mut state, 0);
        let early = sample(&mut state, 2);
        assert_eq!(
            state.reserve(2, early),
            Err(OcrError::Throttled { remaining_ms: 1 })
        );
        assert_eq!(
            readiness(&mut state, 2),
            OcrReadiness::Throttled {
                reason: OcrRefreshReason::NoContext,
                remaining_ms: 1,
            }
        );
        let token = start(&mut state, 3);
        let same_tick = sample(&mut state, 3);
        assert_eq!(
            state.reserve(3, same_tick),
            Err(OcrError::Throttled { remaining_ms: 3 })
        );
        succeed(&mut state, 3, token);
        assert_eq!(state.reserve(3, same_tick), Err(OcrError::NotDue));
    }

    #[test]
    fn max_age_uses_sample_time_and_rejects_exact_boundary_completion() {
        let mut accepted = state();
        let token = start(&mut accepted, 0);
        let success = succeed(&mut accepted, 9, token);
        assert_eq!((success.sampled_at_ms, success.completed_at_ms), (0, 9));
        assert_eq!(readiness(&mut accepted, 9), OcrReadiness::Current);
        assert_eq!(
            readiness(&mut accepted, 10),
            OcrReadiness::Due(OcrRefreshReason::MaxAge)
        );

        let mut expired = state();
        let token = start(&mut expired, 0);
        assert_eq!(
            expired.complete(10, token, OcrOutcome::NonEmptySuccess),
            Ok(OcrCompletion::Expired)
        );
        assert_eq!(expired.assessment(10).unwrap().last_success, None);
        assert_fenced_out(&mut expired, 10, token);
        let fresh = start(&mut expired, 10);
        succeed(&mut expired, 10, fresh);
    }

    #[test]
    fn stale_sample_is_rejected_without_spending_attempt_budget() {
        let mut state = state();
        let old = sample(&mut state, 0);
        assert_eq!(state.reserve(10, old), Err(OcrError::SampleExpired));
        let token = start(&mut state, 10);
        succeed(&mut state, 10, token);
    }

    #[test]
    fn same_scope_threshold_and_roi_changes_stay_due_through_unchanged_ticks() {
        // App/title labels would be constant here; they are deliberately not
        // part of this model's native scope or payload.
        for observation in [change(1_000, false), change(0, true)] {
            let mut state = state();
            let token = start(&mut state, 0);
            succeed(&mut state, 0, token);
            state.observe(1, Some(SCOPE), change(999, false)).unwrap();
            assert_eq!(readiness(&mut state, 1), OcrReadiness::Current);
            state.observe(1, Some(SCOPE), observation).unwrap();
            let unchanged = sample(&mut state, 2);
            assert_eq!(
                readiness(&mut state, 2),
                OcrReadiness::Throttled {
                    reason: OcrRefreshReason::ContentChanged,
                    remaining_ms: 1,
                }
            );
            assert_eq!(
                state.reserve(2, unchanged),
                Err(OcrError::Throttled { remaining_ms: 1 })
            );
            let token = start(&mut state, 3);
            succeed(&mut state, 3, token);
            assert_eq!(readiness(&mut state, 3), OcrReadiness::Current);
        }
    }

    #[test]
    fn unknown_observation_is_explicit_and_cannot_erase_detected_change() {
        let mut state = state();
        let unknown = state
            .observe(0, Some(SCOPE), OcrObservation::Unavailable)
            .unwrap();
        assert_eq!(
            state.assessment(0).unwrap().observation,
            OcrObservation::Unavailable
        );
        let token = state.reserve(0, unknown).unwrap();
        succeed(&mut state, 0, token);
        assert_eq!(
            state.assessment(0).unwrap().observation,
            OcrObservation::Unavailable
        );
        sample(&mut state, 1);
        assert_eq!(state.assessment(1).unwrap().observation, change(0, false));
        state.observe(2, Some(SCOPE), change(1_000, false)).unwrap();
        let unknown = state
            .observe(3, Some(SCOPE), OcrObservation::Unavailable)
            .unwrap();
        assert_eq!(
            readiness(&mut state, 3),
            OcrReadiness::Due(OcrRefreshReason::ContentChanged)
        );
        let token = state.reserve(3, unknown).unwrap();
        succeed(&mut state, 3, token);
        assert_eq!(
            readiness(&mut state, 13),
            OcrReadiness::Due(OcrRefreshReason::MaxAge)
        );
    }

    #[test]
    fn missing_scope_invalidates_requests_and_a_known_scope_can_recover() {
        let mut state = state();
        let old = start(&mut state, 0);
        assert_eq!(
            state.observe(1, None, OcrObservation::Unavailable),
            Err(OcrError::ScopeUnavailable)
        );
        assert_eq!(state.assessment(1), Err(OcrError::ScopeUnavailable));
        assert_fenced_out(&mut state, 1, old);
        let early = sample(&mut state, 2);
        assert_eq!(
            state.reserve(2, early),
            Err(OcrError::Throttled { remaining_ms: 1 })
        );
        let new = start(&mut state, 3);
        succeed(&mut state, 3, new);
    }

    #[test]
    fn every_capture_owner_epoch_invalidates_old_results_without_refilling_budget() {
        let scopes = [
            OcrScope {
                scene_epoch: 1,
                ..SCOPE
            },
            OcrScope {
                monitor_epoch: 1,
                ..SCOPE
            },
            OcrScope {
                authority_epoch: 1,
                ..SCOPE
            },
        ];
        for scope in scopes {
            let mut state = state();
            let old = start(&mut state, 0);
            let stamp = state.observe(1, Some(scope), change(0, false)).unwrap();
            assert_eq!(
                readiness(&mut state, 1),
                OcrReadiness::Throttled {
                    reason: OcrRefreshReason::ScopeChanged,
                    remaining_ms: 2,
                }
            );
            assert_fenced_out(&mut state, 1, old);
            assert_eq!(
                state.reserve(2, stamp),
                Err(OcrError::Throttled { remaining_ms: 1 })
            );
            let stamp = state.observe(3, Some(scope), change(0, false)).unwrap();
            let token = state.reserve(3, stamp).unwrap();
            succeed(&mut state, 3, token);
        }
    }

    #[test]
    fn failures_and_empty_results_preserve_historical_success_and_refresh_demand() {
        let mut state = state();
        let first = start(&mut state, 0);
        let success = succeed(&mut state, 0, first);
        state.observe(1, Some(SCOPE), change(1_000, false)).unwrap();
        for (tick, outcome) in [(3, OcrOutcome::Failed), (6, OcrOutcome::Empty)] {
            let token = start(&mut state, tick);
            state.complete(tick, token, outcome).unwrap();
            let assessment = state.assessment(tick).unwrap();
            assert_eq!(assessment.last_success, Some(success));
            assert_eq!(
                assessment.readiness,
                OcrReadiness::Throttled {
                    reason: OcrRefreshReason::ContentChanged,
                    remaining_ms: 3,
                }
            );
        }
        let token = start(&mut state, 9);
        let newer = succeed(&mut state, 9, token);
        assert_ne!(newer.request, success.request);
        assert_eq!(newer.sampled_at_ms, 9);
    }

    #[test]
    fn latest_request_wins_and_duplicate_completion_cannot_rewrite_success() {
        let mut state = state();
        let a = start(&mut state, 0);
        state.observe(1, Some(SCOPE), change(1_000, false)).unwrap();
        let b = start(&mut state, 3);
        let success = succeed(&mut state, 4, b);
        assert_fenced_out(&mut state, 5, a);
        assert_fenced_out(&mut state, 6, b);
        assert_eq!(state.assessment(6).unwrap().last_success, Some(success));
    }

    #[test]
    fn same_millisecond_content_change_fences_an_older_sample_stamp() {
        let mut state = state();
        let old = sample(&mut state, 0);
        let new = state.observe(0, Some(SCOPE), change(1_000, false)).unwrap();
        assert_eq!(state.reserve(0, old), Err(OcrError::ObsoleteSample));
        let token = state.reserve(0, new).unwrap();
        succeed(&mut state, 0, token);
    }

    #[test]
    fn off_on_same_scope_cannot_replay_context_or_reset_the_attempt_budget() {
        let mut state = state();
        let old = start(&mut state, 0);
        state.reset(1, false).unwrap();
        assert_eq!(
            state.complete(1, old, OcrOutcome::NonEmptySuccess),
            Err(OcrError::Disabled)
        );
        state.reset(2, true).unwrap();
        let early = sample(&mut state, 2);
        assert_eq!(
            state.reserve(2, early),
            Err(OcrError::Throttled { remaining_ms: 1 })
        );
        let new = start(&mut state, 3);
        succeed(&mut state, 3, new);
        assert_fenced_out(&mut state, 4, old);
        state.reset(4, true).unwrap();
        sample(&mut state, 4);
        assert_ne!(readiness(&mut state, 4), OcrReadiness::Current);
    }

    #[test]
    fn clock_regression_invalidates_without_rewinding_budget_and_catch_up_recovers() {
        let mut state = state();
        let old = start(&mut state, 5);
        assert_eq!(state.assessment(4), Err(OcrError::ClockRegression));
        assert_fenced_out(&mut state, 5, old);
        assert_eq!(state.assessment(5), Err(OcrError::ObservationRequired));
        assert_eq!(state.reset(4, true), Err(OcrError::ClockRegression));
        let early = sample(&mut state, 6);
        assert_eq!(
            state.reserve(6, early),
            Err(OcrError::Throttled { remaining_ms: 2 })
        );
        let new = start(&mut state, 8);
        succeed(&mut state, 8, new);
        assert_eq!(readiness(&mut state, 8), OcrReadiness::Current);
    }

    #[test]
    fn clock_regression_on_disable_still_disables_and_delayed_sample_is_not_restamped() {
        let mut state = state();
        let token = start(&mut state, 5);
        assert_eq!(
            state.observe(4, Some(SCOPE), change(0, false)),
            Err(OcrError::ClockRegression)
        );
        assert_fenced_out(&mut state, 5, token);
        assert_eq!(state.reset(4, false), Err(OcrError::ClockRegression));
        assert_eq!(
            state.observe(6, Some(SCOPE), change(0, false)),
            Err(OcrError::Disabled)
        );
        state.reset(6, true).unwrap();
        let new = start(&mut state, 8);
        succeed(&mut state, 8, new);
    }

    #[test]
    fn final_legal_generation_works_then_exhaustion_never_wraps_or_resets() {
        let mut state = state();
        state.generation = u64::MAX - 2;
        let token = start(&mut state, 0);
        assert_eq!(token.generation, u64::MAX);
        succeed(&mut state, 0, token);
        assert_eq!(readiness(&mut state, 9), OcrReadiness::Current);
        let stamp = sample(&mut state, 10);
        assert_eq!(state.reserve(10, stamp), Err(OcrError::GenerationExhausted));
        assert_eq!(state.assessment(10), Err(OcrError::GenerationExhausted));
        assert_eq!(
            state.complete(10, token, OcrOutcome::NonEmptySuccess),
            Err(OcrError::GenerationExhausted)
        );
        state.reset(11, false).unwrap();
        state.reset(12, true).unwrap();
        assert_eq!(
            state.observe(12, Some(SCOPE), change(0, false)),
            Err(OcrError::GenerationExhausted)
        );
        assert_eq!(state.generation, u64::MAX);
    }

    #[test]
    fn input_revision_exhaustion_also_invalidates_an_inflight_result() {
        let mut state = state();
        state.generation = u64::MAX - 2;
        let token = start(&mut state, 0);
        assert_eq!(
            state.observe(1, Some(SCOPE), change(1_000, false)),
            Err(OcrError::GenerationExhausted)
        );
        assert_eq!(
            state.complete(1, token, OcrOutcome::NonEmptySuccess),
            Err(OcrError::GenerationExhausted)
        );
        assert_eq!(state.last_success, None);
    }
}
