use super::*;
use maekon_core::models::candidate_decision::CandidateSnapshot;
use maekon_core::models::candidate_gateway::{
    GatewayPrice, GatewayRegion, GatewayRouting, GATEWAY_MODEL,
};
use maekon_core::models::context::WindowInfo;
use maekon_core::models::gui::GuiCandidate;
use maekon_core::models::intent::ElementBounds;
use maekon_core::models::ui_scene::{NormalizedBounds, UiSceneElement};
use std::sync::Barrier;
use std::time::Duration;

const KEY: &str = "fake-gateway-key";

/// Smallest wall-clock step every platform can represent. Windows `SystemTime`
/// counts 100 ns ticks, so a 1 ns step vanishes there and turns "just before
/// the deadline" into "at the deadline" (#12745).
const WALL_STEP: Duration = Duration::from_micros(1);

fn request_at(now: Instant, generation: u64) -> CandidateDecisionRequest {
    CandidateDecisionRequest::new(
        CandidateSnapshot {
            goal: "Save".into(),
            scene_id: "private-scene".into(),
            frame_id: "private-frame".into(),
            generation,
            window: WindowInfo {
                title: "Editor".into(),
                app_name: "Editor".into(),
                app_bundle_id: None,
                pid: 1,
                bounds: None,
            },
            candidates: vec![GuiCandidate {
                eligible: true,
                ranking_reason: None,
                element: UiSceneElement {
                    element_id: "private-element".into(),
                    label: "Save".into(),
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
                    confidence: 0.9,
                    text_masked: None,
                    parent_id: None,
                },
            }],
        },
        now,
        Duration::from_secs(10),
    )
    .unwrap()
}

fn approval_at(now: Instant) -> GatewayCandidateApproval {
    GatewayCandidateApproval {
        approval_reference: "test-approval".into(),
        principal_namespace: "test-principal".into(),
        policy_revision: "policy-1".into(),
        max_attempts: 10,
        budget_picos: 0,
        expires_at: now + Duration::from_secs(30),
        authority: GatewayAuthority {
            account_reference: "test-account".into(),
            credential_profile: "test".into(),
            credential_revision: "key-1".into(),
            credential_sha256: digest_bytes(KEY.as_bytes()),
            price_reference: "account-promotion-1".into(),
            price: GatewayPrice {
                input_picos_per_token: 0,
                output_picos_per_token: 0,
                request_fee_picos: 0,
            },
            funding: GatewayFunding::ZeroPricePromotion,
            routing: GatewayRouting {
                response_model: GATEWAY_MODEL.into(),
                original_model_id: GATEWAY_MODEL.into(),
                canonical_slug: GATEWAY_MODEL.into(),
                resolved_provider: "typesafe-ai".into(),
                final_provider: "typesafe-ai".into(),
            },
            effective_route_reference: "typesafe-endpoint-policy".into(),
            retention_reference: "approved-gateway-retention".into(),
            region: GatewayRegion::ApprovedGlobal,
            typesafe_route_verified: true,
            gateway_key_only: true,
            fallback_disabled: true,
            valid_until: now + Duration::from_secs(60),
            wall_valid_until: SystemTime::now() + Duration::from_secs(60),
        },
    }
}

fn setup(
    approval: GatewayCandidateApproval,
) -> (
    GatewayCandidateControl,
    CandidateDecisionRequest,
    GatewayCandidateLease,
) {
    let control = GatewayCandidateControl::default();
    let request = request_at(Instant::now(), 1);
    control.approve(approval).unwrap();
    control.bind(&request).unwrap();
    let lease = control.lease(&request).unwrap();
    (control, request, lease)
}

#[test]
fn gateway_control_requires_binding_and_one_approval_per_runtime() {
    let control = GatewayCandidateControl::default();
    let request = request_at(Instant::now(), 1);
    assert_eq!(
        control.lease(&request).err(),
        Some(DecisionUnavailable::Off)
    );
    let mut approval = approval_at(Instant::now());
    control.approve(approval.clone()).unwrap();
    assert_eq!(
        control.lease(&request).err(),
        Some(DecisionUnavailable::Stale)
    );
    control.bind(&request).unwrap();
    let lease = control.lease(&request).unwrap();
    assert_eq!(lease.approval().approval_reference, "test-approval");
    assert_eq!(lease.approval().principal_namespace, "test-principal");
    assert_eq!(lease.approval().policy_revision, "policy-1");
    assert_eq!(lease.approval().max_attempts, 10);
    assert_eq!(control.checkpoint(&request, &lease), Ok(()));
    approval.budget_picos = 999;
    assert_eq!(
        control.approve(approval),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(control.state.lock().remaining_picos, 0);
    assert_eq!(lease.approval().budget_picos, 0);
}

#[test]
fn gateway_revoke_is_permanent_across_clones_and_before_approval() {
    let (control, request, lease) = setup(approval_at(Instant::now()));
    control.clone().revoke();
    assert_eq!(
        control.checkpoint(&request, &lease),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(
        control.reserve(&request, &lease),
        Err(DecisionUnavailable::Cancelled)
    );
    assert_eq!(control.bind(&request), Err(DecisionUnavailable::Cancelled));
    assert_eq!(
        control.approve(approval_at(Instant::now())),
        Err(DecisionUnavailable::Cancelled)
    );
    let fresh = GatewayCandidateControl::default();
    fresh.revoke();
    assert_eq!(
        fresh.approve(approval_at(Instant::now())),
        Err(DecisionUnavailable::Cancelled)
    );
    assert!(fresh.state.lock().approval.is_none());
}

#[test]
fn gateway_binding_aba_and_foreign_leases_cannot_reuse_permission() {
    let (first, request, lease) = setup(approval_at(Instant::now()));
    let second = GatewayCandidateControl::default();
    second.approve(approval_at(Instant::now())).unwrap();
    second.bind(&request).unwrap();
    assert_eq!(
        second.checkpoint(&request, &lease),
        Err(DecisionUnavailable::Stale)
    );
    assert_eq!(
        second.pin_privacy(&request, &lease, "consent"),
        Err(DecisionUnavailable::Stale)
    );
    assert_eq!(
        second.observe(&request, &lease, &lease.approval().authority),
        Err(DecisionUnavailable::Stale)
    );
    assert_eq!(
        second.check_secret(&request, &lease, KEY),
        Err(DecisionUnavailable::Stale)
    );
    assert_eq!(
        second.reserve(&request, &lease),
        Err(DecisionUnavailable::Stale)
    );
    assert!(second.state.lock().consent_revision.is_none());
    let own = second.lease(&request).unwrap();
    assert_eq!(second.checkpoint(&request, &own), Ok(()));
    first.bind(&request_at(Instant::now(), 2)).unwrap();
    first.bind(&request).unwrap();
    assert_eq!(
        first.checkpoint(&request, &lease),
        Err(DecisionUnavailable::Stale)
    );
    let renewed = first.lease(&request).unwrap();
    assert_eq!(first.checkpoint(&request, &renewed), Ok(()));
}

#[test]
fn gateway_approval_rejects_each_invalid_scope_and_lifetime() {
    let mutations: &[fn(&mut GatewayCandidateApproval)] = &[
        |a| a.approval_reference = "\u{1f}".into(),
        |a| a.principal_namespace = "\u{1f}".into(),
        |a| a.policy_revision = "\u{1f}".into(),
        |a| a.max_attempts = 0,
        |a| a.max_attempts = 1001,
        |a| a.expires_at = Instant::now(),
        |a| a.expires_at = a.authority.valid_until + WALL_STEP,
        |a| a.authority.credential_profile = "../typesafe".into(),
        |a| a.authority.credential_profile = " test".into(),
        |a| a.authority.credential_profile = "test ".into(),
        |a| a.authority.account_reference.clear(),
    ];
    for mutate in mutations {
        let control = GatewayCandidateControl::default();
        let mut approval = approval_at(Instant::now());
        mutate(&mut approval);
        assert_eq!(
            control.approve(approval),
            Err(DecisionUnavailable::ApprovalMissing)
        );
        assert!(control.state.lock().approval.is_none());
        control.approve(approval_at(Instant::now())).unwrap();
    }
    for attempts in [1, 1000] {
        let mut approval = approval_at(Instant::now());
        approval.max_attempts = attempts;
        approval.expires_at = approval.authority.valid_until;
        assert_eq!(GatewayCandidateControl::default().approve(approval), Ok(()));
    }
}

#[test]
fn gateway_approval_binds_worst_case_price_and_reserved_credit_limit() {
    for (budget, expected) in [
        (6, Err(DecisionUnavailable::ApprovalMissing)),
        (7, Ok(())),
        (8, Ok(())),
    ] {
        let mut approval = approval_at(Instant::now());
        approval.authority.funding = GatewayFunding::PaidApi;
        approval.authority.price.request_fee_picos = 7;
        approval.budget_picos = budget;
        assert_eq!(
            GatewayCandidateControl::default().approve(approval),
            expected
        );
    }
    for (budget, expected) in [
        (7, Ok(())),
        (8, Ok(())),
        (9, Err(DecisionUnavailable::ApprovalMissing)),
    ] {
        let mut approval = approval_at(Instant::now());
        approval.authority.price.request_fee_picos = 7;
        approval.authority.funding = GatewayFunding::FreeCredits {
            allocation_reference: "reserved-credit".into(),
            limit_picos: 8,
            cash_fallback_disabled: true,
        };
        approval.budget_picos = budget;
        assert_eq!(
            GatewayCandidateControl::default().approve(approval),
            expected
        );
    }
}

#[test]
fn gateway_atomic_attempt_and_money_limits_survive_concurrent_reservation() {
    for budget_limited in [false, true] {
        let mut approval = approval_at(Instant::now());
        approval.authority.funding = GatewayFunding::PaidApi;
        approval.authority.price.request_fee_picos = 7;
        approval.max_attempts = if budget_limited { 10 } else { 1 };
        approval.budget_picos = if budget_limited { 7 } else { 70 };
        let (control, request, _) = setup(approval);
        let barrier = Barrier::new(8);
        let results = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..8 {
                let (control, request, barrier) = (&control, &request, &barrier);
                handles.push(scope.spawn(move || {
                    let lease = control.lease(request).unwrap();
                    barrier.wait();
                    control.reserve(request, &lease)
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(results.iter().filter(|result| **result == Ok(7)).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| **result == Err(DecisionUnavailable::BudgetExceeded))
                .count(),
            7
        );
        let remaining = if budget_limited { 0 } else { 63 };
        assert_eq!(control.state.lock().remaining_picos, remaining);
        control.bind(&request).unwrap();
        let lease = control.lease(&request).unwrap();
        assert_eq!(
            control.reserve(&request, &lease),
            Err(DecisionUnavailable::BudgetExceeded)
        );
        control.clone().revoke();
        assert_eq!(control.state.lock().remaining_picos, remaining);
    }
    let mut approval = approval_at(Instant::now());
    approval.max_attempts = 1;
    let (control, request, lease) = setup(approval);
    assert_eq!(control.reserve(&request, &lease), Ok(0));
    assert_eq!(
        control.reserve(&request, &lease),
        Err(DecisionUnavailable::BudgetExceeded)
    );
}

#[test]
fn gateway_observation_changes_revoke_every_clone() {
    let mutations: &[fn(&mut GatewayAuthority)] = &[
        |a| a.account_reference.push('2'),
        |a| a.credential_profile.push('2'),
        |a| a.credential_revision.push('2'),
        |a| a.credential_sha256 = digest_bytes(b"different-key"),
        |a| a.price_reference.push('2'),
        |a| a.price.request_fee_picos = 1,
        |a| a.funding = GatewayFunding::PaidApi,
        |a| a.effective_route_reference.push('2'),
        |a| a.retention_reference.push('2'),
        |a| a.routing.response_model = "different".into(),
        |a| a.routing.original_model_id = "different".into(),
        |a| a.routing.canonical_slug = "different".into(),
        |a| a.routing.resolved_provider = "different".into(),
        |a| a.routing.final_provider = "different".into(),
        |a| a.region = GatewayRegion::RequiredRegionUnsupported,
        |a| a.typesafe_route_verified = false,
        |a| a.gateway_key_only = false,
        |a| a.fallback_disabled = false,
        |a| a.valid_until += Duration::from_secs(1),
        |a| a.wall_valid_until += Duration::from_secs(1),
    ];
    for mutate in mutations {
        let (control, request, lease) = setup(approval_at(Instant::now()));
        let original = &lease.approval().authority;
        assert_eq!(control.observe(&request, &lease, original), Ok(()));
        let mut changed = original.clone();
        mutate(&mut changed);
        assert_eq!(
            control.observe(&request, &lease, &changed),
            Err(DecisionUnavailable::Rejected)
        );
        assert_eq!(
            control.clone().observe(&request, &lease, original),
            Err(DecisionUnavailable::Cancelled)
        );
        assert_eq!(
            control.reserve(&request, &lease),
            Err(DecisionUnavailable::Cancelled)
        );
    }
}

#[test]
fn gateway_privacy_revision_is_pinned_across_bindings() {
    let (control, request, lease) = setup(approval_at(Instant::now()));
    assert_eq!(control.pin_privacy(&request, &lease, "consent-1"), Ok(()));
    assert_eq!(control.pin_privacy(&request, &lease, "consent-1"), Ok(()));
    control.bind(&request).unwrap();
    let renewed = control.lease(&request).unwrap();
    assert_eq!(
        control.pin_privacy(&request, &renewed, "consent-2"),
        Err(DecisionUnavailable::ConsentOrPolicyDenied)
    );
    assert_eq!(
        control.clone().checkpoint(&request, &renewed),
        Err(DecisionUnavailable::Cancelled)
    );
    for invalid in ["", " ", "\u{1f}", &"x".repeat(257)] {
        let (control, request, lease) = setup(approval_at(Instant::now()));
        assert_eq!(
            control.pin_privacy(&request, &lease, invalid),
            Err(DecisionUnavailable::ConsentOrPolicyDenied)
        );
        assert!(control.state.lock().consent_revision.is_none());
        assert_eq!(
            control.checkpoint(&request, &lease),
            Err(DecisionUnavailable::Cancelled)
        );
    }
}

#[test]
fn gateway_secret_checks_digest_and_bounded_noncontrol_bytes() {
    let (control, request, lease) = setup(approval_at(Instant::now()));
    assert_eq!(control.check_secret(&request, &lease, KEY), Ok(()));
    assert_eq!(
        control.check_secret(&request, &lease, "other-key"),
        Err(DecisionUnavailable::CredentialUnavailable)
    );
    assert_eq!(
        control.check_secret(&request, &lease, KEY),
        Err(DecisionUnavailable::Cancelled)
    );
    for (key, valid) in [
        ("".into(), false),
        (" \u{2003}".into(), false),
        ("x".repeat(4095), true),
        ("x".repeat(4096), true),
        ("x".repeat(4097), false),
        ("key\u{0}".into(), false),
        ("key\r\n".into(), false),
        ("key\u{85}".into(), false),
    ] {
        let mut approval = approval_at(Instant::now());
        approval.authority.credential_sha256 = digest_bytes(key.as_bytes());
        let (control, request, lease) = setup(approval);
        let expected = if valid {
            Ok(())
        } else {
            Err(DecisionUnavailable::CredentialUnavailable)
        };
        assert_eq!(control.check_secret(&request, &lease, &key), expected);
        if !valid {
            assert_eq!(
                control.checkpoint(&request, &lease),
                Err(DecisionUnavailable::Cancelled)
            );
        }
    }
}

#[test]
fn gateway_wall_deadline_is_strict_and_expiry_is_permanent() {
    for offset in [Duration::ZERO, WALL_STEP] {
        let (control, request, lease) = setup(approval_at(Instant::now()));
        let deadline = lease.approval().authority.wall_valid_until;
        {
            let _state = control.state.lock();
            assert_eq!(
                control.checkpoint_at(&request, &lease, deadline - WALL_STEP),
                Ok(())
            );
            assert_eq!(
                control.checkpoint_at(&request, &lease, deadline + offset),
                Err(DecisionUnavailable::Expired)
            );
        }
        assert_eq!(
            control.checkpoint(&request, &lease),
            Err(DecisionUnavailable::Cancelled)
        );
    }
}

#[test]
fn gateway_every_boundary_observes_shared_clock_rollback_and_authority_expiry() {
    for operation in 0..6 {
        for rollback in [true, false] {
            let (control, request, mut lease) = setup(approval_at(Instant::now()));
            if rollback {
                control
                    .lifecycle
                    .set_clock_observation_for_test(SystemTime::now() + Duration::from_secs(60));
            } else {
                // Model elapsed wall time without sleeping or changing the OS clock.
                let expired = SystemTime::now() - Duration::from_secs(1);
                lease.approval.authority.wall_valid_until = expired;
                control
                    .state
                    .lock()
                    .approval
                    .as_mut()
                    .unwrap()
                    .authority
                    .wall_valid_until = expired;
            }
            let result = match operation {
                0 => control.lease(&request).map(|_| ()),
                1 => control.checkpoint(&request, &lease),
                2 => control.pin_privacy(&request, &lease, "consent-1"),
                3 => control.observe(&request, &lease, &lease.approval().authority),
                4 => control.check_secret(&request, &lease, KEY),
                5 => control.reserve(&request, &lease).map(|_| ()),
                _ => unreachable!(),
            };
            assert_eq!(result, Err(DecisionUnavailable::Expired));
            assert_eq!(
                control.checkpoint(&request, &lease),
                Err(DecisionUnavailable::Cancelled)
            );
            assert!(control.state.lock().consent_revision.is_none());
        }
    }
}
