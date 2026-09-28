use super::*;
use maekon_core::models::candidate_decision::CandidateSnapshot;
use maekon_core::models::context::WindowInfo;
use maekon_core::models::gui::GuiCandidate;
use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::{NormalizedBounds, UiSceneElement};

/// Smallest wall-clock step every platform can represent. Windows `SystemTime`
/// counts 100 ns ticks, so a 1 ns step vanishes there: "just before the
/// deadline" becomes "at the deadline" and a 1 ns regression is no regression
/// (#12745).
const WALL_STEP: Duration = Duration::from_micros(1);

fn request(goal: &str, labels: &[&str]) -> CandidateDecisionRequest {
    CandidateDecisionRequest::new(
        CandidateSnapshot {
            goal: goal.into(),
            scene_id: "scene".into(),
            frame_id: "frame".into(),
            generation: 1,
            window: WindowInfo {
                title: "Editor".into(),
                app_name: "Editor".into(),
                app_bundle_id: None,
                pid: 1,
                bounds: None,
            },
            candidates: labels
                .iter()
                .enumerate()
                .map(|(index, label)| GuiCandidate {
                    eligible: true,
                    ranking_reason: None,
                    element: UiSceneElement {
                        element_id: format!("private-id-{index}"),
                        label: (*label).into(),
                        bbox_abs: ElementBounds {
                            x: 1,
                            y: 2,
                            width: 3,
                            height: 4,
                        },
                        bbox_norm: NormalizedBounds::new(0.1, 0.2, 0.3, 0.4),
                        role: Some("button".into()),
                        intent: None,
                        state: Some("enabled".into()),
                        confidence: if index == 0 { 0.99 } else { 0.2 },
                        text_masked: None,
                        parent_id: None,
                    },
                })
                .collect(),
        },
        Instant::now(),
        Duration::from_secs(10),
    )
    .unwrap()
}

fn local_approval() -> LocalAssessmentApproval {
    LocalAssessmentApproval {
        approval_reference: "approved".into(),
        principal_namespace: "principal".into(),
        policy_revision: "policy".into(),
        configuration_revision: "config".into(),
        trigger: CandidateTrigger::UserRequest,
        max_attempts: 2,
        expires_at: Instant::now() + Duration::from_secs(60),
        model: None,
    }
}

fn bound_control(request: &CandidateDecisionRequest) -> LocalAssessmentControl {
    let control = LocalAssessmentControl::default();
    control.approve(local_approval()).unwrap();
    control.bind(request).unwrap();
    control
}

#[test]
fn local_control_requires_approval_binding_and_one_shot_approval() {
    let request = request("Save", &["Save"]);
    let control = LocalAssessmentControl::default();
    assert_eq!(
        control.lease(&request).err(),
        Some(DecisionUnavailable::Off)
    );
    control.approve(local_approval()).unwrap();
    assert_eq!(
        control.lease(&request).err(),
        Some(DecisionUnavailable::Stale)
    );
    control.bind(&request).unwrap();
    let lease = control.lease(&request).unwrap();
    assert_eq!(lease.approval().principal_namespace, "principal");
    assert_eq!(lease.approval().max_attempts, 2);
    assert_eq!(control.checkpoint(&request, &lease), Ok(()));
    assert_eq!(
        control.approve(local_approval()),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(control.state.lock().remaining_attempts, 2);
}

#[test]
fn local_control_revoke_is_permanent_across_clones() {
    let request = request("Save", &["Save"]);
    let control = bound_control(&request);
    let lease = control.lease(&request).unwrap();
    control.reserve_attempt(&request, &lease).unwrap();
    control.clone().revoke();
    assert_eq!(
        control.checkpoint(&request, &lease),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(
        control.reserve_attempt(&request, &lease),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(control.bind(&request), Err(DecisionUnavailable::Cancelled));
    assert_eq!(
        control.approve(local_approval()),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(control.state.lock().remaining_attempts, 1);
    let fresh = LocalAssessmentControl::default();
    fresh.revoke();
    assert_eq!(
        fresh.approve(local_approval()),
        Err(DecisionUnavailable::Cancelled)
    );
    assert!(fresh.state.lock().approval.is_none());
}

#[test]
fn local_control_aba_invalidates_the_old_lease_without_restoring_quota() {
    let original = request("Save", &["Save"]);
    let other = request("Other", &["Other"]);
    let control = bound_control(&original);
    let lease = control.lease(&original).unwrap();
    control.reserve_attempt(&original, &lease).unwrap();
    control.bind(&other).unwrap();
    control.bind(&original).unwrap();
    assert_eq!(
        control.checkpoint(&original, &lease),
        Err(DecisionUnavailable::Stale)
    );
    assert_eq!(
        control.reserve_attempt(&original, &lease),
        Err(DecisionUnavailable::Stale)
    );
    let renewed = control.lease(&original).unwrap();
    assert_eq!(control.checkpoint(&original, &renewed), Ok(()));
    control.reserve_attempt(&original, &renewed).unwrap();
    assert_eq!(
        control.reserve_attempt(&original, &renewed),
        Err(DecisionUnavailable::BudgetExceeded)
    );
}

#[test]
fn local_control_does_not_accept_another_runtime_lease_at_the_same_epoch() {
    let request = request("Save", &["Save"]);
    let first = bound_control(&request);
    let second = bound_control(&request);
    let lease = first.lease(&request).unwrap();
    assert_eq!(first.state.lock().epoch, second.state.lock().epoch);
    assert_eq!(
        second.checkpoint(&request, &lease),
        Err(DecisionUnavailable::Stale)
    );
    assert_eq!(
        second.reserve_attempt(&request, &lease),
        Err(DecisionUnavailable::Stale)
    );
    assert_eq!(second.state.lock().remaining_attempts, 2);
    assert_eq!(first.checkpoint(&request, &lease), Ok(()));
}

#[test]
fn local_control_reservations_are_atomic_under_concurrent_calls() {
    let request = request("Save", &["Save"]);
    let control = LocalAssessmentControl::default();
    let mut approval = local_approval();
    approval.max_attempts = 1;
    control.approve(approval).unwrap();
    control.bind(&request).unwrap();
    let lease = control.lease(&request).unwrap();
    let barrier = std::sync::Barrier::new(3);
    let outcomes = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            control.reserve_attempt(&request, &lease)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            control.reserve_attempt(&request, &lease)
        });
        barrier.wait();
        [a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(
        outcomes.iter().filter(|result| **result == Ok(())).count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|result| **result == Err(DecisionUnavailable::BudgetExceeded))
            .count(),
        1
    );
    assert_eq!(control.state.lock().remaining_attempts, 0);
}

#[test]
fn local_control_checks_observed_time_exact_deadline_and_epoch_exhaustion() {
    let request = request("Save", &["Save"]);
    let control = bound_control(&request);
    let epoch = control.state.lock().epoch;
    let mut state = control.state.lock();
    assert_eq!(
        state.check(&request, epoch, request.binding().observed_at),
        Ok(())
    );
    assert_eq!(
        state.check(&request, epoch, request.binding().observed_at - WALL_STEP),
        Err(DecisionUnavailable::Expired)
    );
    assert_eq!(
        state.check(&request, epoch, request.binding().deadline - WALL_STEP),
        Ok(())
    );
    assert_eq!(
        state.check(&request, epoch, request.binding().deadline),
        Err(DecisionUnavailable::Expired)
    );
    state.epoch = u64::MAX;
    drop(state);
    assert_eq!(control.bind(&request), Err(DecisionUnavailable::Cancelled));
    assert!(control.state.lock().revoked);
}

#[test]
fn local_control_validates_approval_identity_quota_and_exact_time_limits() {
    let now = Instant::now();
    let mut valid = local_approval();
    valid.expires_at = now + Duration::from_secs(3600);
    valid.max_attempts = 1000;
    valid.approval_reference = "x".repeat(256);
    valid.principal_namespace = "x".repeat(256);
    valid.policy_revision = "x".repeat(256);
    valid.configuration_revision = "x".repeat(256);
    assert_eq!(valid.validate(now), Ok(()));
    for index in 0..4 {
        for value in [" \t".to_owned(), "x".repeat(257)] {
            let mut invalid = valid.clone();
            match index {
                0 => invalid.approval_reference = value,
                1 => invalid.principal_namespace = value,
                2 => invalid.policy_revision = value,
                _ => invalid.configuration_revision = value,
            }
            assert_eq!(
                invalid.validate(now),
                Err(DecisionUnavailable::ApprovalMissing)
            );
        }
    }
    for count in [0, 1001] {
        let mut invalid = valid.clone();
        invalid.max_attempts = count;
        assert_eq!(
            invalid.validate(now),
            Err(DecisionUnavailable::ApprovalMissing)
        );
    }
    for expires in [
        now,
        now - WALL_STEP,
        now + Duration::from_secs(3600) + WALL_STEP,
    ] {
        let mut invalid = valid.clone();
        invalid.expires_at = expires;
        assert_eq!(
            invalid.validate(now),
            Err(DecisionUnavailable::ApprovalMissing)
        );
    }
}

#[test]
fn local_control_uses_the_shorter_approval_or_model_deadline() {
    let request = request("Save", &["Save"]);
    for with_model in [false, true] {
        let control = LocalAssessmentControl::default();
        let mut approved = local_approval();
        let short = request.binding().observed_at + Duration::from_secs(1);
        if with_model {
            approved.model = Some(LocalModelApproval {
                daemon_reference: "daemon".into(),
                configuration_revision: "config".into(),
                endpoint_origin: "http://127.0.0.1:11434".into(),
                model: "fixture:fixed".into(),
                model_digest: format!("sha256:{}", "a".repeat(64)),
                expires_at: short,
            });
            assert_eq!(approved.validate(Instant::now()), Ok(()));
            let mut invalid = approved.clone();
            invalid.model.as_mut().unwrap().model_digest = "invalid".into();
            assert_eq!(
                invalid.validate(Instant::now()),
                Err(DecisionUnavailable::ApprovalMissing)
            );
        } else {
            approved.expires_at = short;
        }
        control.approve(approved).unwrap();
        control.bind(&request).unwrap();
        let epoch = control.state.lock().epoch;
        assert_eq!(
            control
                .state
                .lock()
                .check(&request, epoch, short - WALL_STEP),
            Ok(())
        );
        assert_eq!(
            control.state.lock().check(&request, epoch, short),
            Err(DecisionUnavailable::Expired)
        );
    }
}

#[test]
fn local_control_clock_maps_both_directions_and_allows_equal_wall_time() {
    let monotonic = Instant::now();
    let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let mut clock = ClockFence {
        monotonic,
        wall,
        last_wall: wall,
    };
    assert_eq!(clock.wall_at(monotonic), Some(wall));
    assert_eq!(
        clock.wall_at(monotonic + Duration::from_secs(1)),
        Some(wall + Duration::from_secs(1))
    );
    assert_eq!(
        clock.wall_at(monotonic - Duration::from_secs(1)),
        Some(wall - Duration::from_secs(1))
    );
    let deadline = monotonic + Duration::from_secs(10);
    assert_eq!(clock.check(deadline, wall), Ok(()));
    assert_eq!(clock.check(deadline, wall), Ok(()));
    assert_eq!(clock.check(deadline, wall + WALL_STEP), Ok(()));
    assert_eq!(
        clock.check(deadline, wall),
        Err(DecisionUnavailable::Expired)
    );
    assert_eq!(
        clock.check(deadline, wall + Duration::from_secs(10)),
        Err(DecisionUnavailable::Expired)
    );
}

#[test]
fn local_control_wall_deadline_survives_suspension_and_rebinding() {
    let original = request("Save", &["Save"]);
    let control = bound_control(&original);
    let deadline = control
        .state
        .lock()
        .clock
        .wall_at(original.binding().deadline)
        .unwrap();
    let now = Instant::now();
    let epoch = control.state.lock().epoch;
    assert_eq!(
        control
            .state
            .lock()
            .check_at(&original, epoch, now, deadline - WALL_STEP),
        Ok(())
    );
    control.bind(&request("Other", &["Other"])).unwrap();
    control.bind(&original).unwrap();
    let epoch = control.state.lock().epoch;
    assert_eq!(
        control
            .state
            .lock()
            .clock
            .wall_at(original.binding().deadline),
        Some(deadline)
    );
    assert_eq!(
        control
            .state
            .lock()
            .check_at(&original, epoch, now, deadline),
        Err(DecisionUnavailable::Expired)
    );
    assert!(control.state.lock().revoked);
    assert_eq!(control.bind(&original), Err(DecisionUnavailable::Cancelled));
}

#[test]
fn local_control_wall_regression_and_shorter_approvals_revoke_permanently() {
    for case in 0..3 {
        let request = request("Save", &["Save"]);
        let control = LocalAssessmentControl::default();
        let mut approved = local_approval();
        if case == 2 {
            approved.model = Some(LocalModelApproval {
                daemon_reference: "daemon".into(),
                configuration_revision: "config".into(),
                endpoint_origin: "http://127.0.0.1:11434".into(),
                model: "fixture:fixed".into(),
                model_digest: format!("sha256:{}", "a".repeat(64)),
                expires_at: approved.expires_at,
            });
        }
        control.approve(approved).unwrap();
        control.bind(&request).unwrap();
        let mut state = control.state.lock();
        let now = Instant::now();
        let wall = state.clock.wall_at(now).unwrap();
        let epoch = state.epoch;
        assert_eq!(state.check_at(&request, epoch, now, wall), Ok(()));
        let bound = if case == 0 {
            wall - WALL_STEP
        } else {
            let short = now + Duration::from_secs(1);
            let approval = state.approval.as_mut().unwrap();
            if case == 1 {
                approval.expires_at = short;
            } else {
                approval.model.as_mut().unwrap().expires_at = short;
            }
            state.clock.wall_at(short).unwrap()
        };
        assert_eq!(
            state.check_at(&request, epoch, now, bound),
            Err(DecisionUnavailable::Expired)
        );
        assert!(state.revoked);
        assert_eq!(
            state.check_at(&request, epoch, now, wall),
            Err(DecisionUnavailable::Cancelled)
        );
    }
}
