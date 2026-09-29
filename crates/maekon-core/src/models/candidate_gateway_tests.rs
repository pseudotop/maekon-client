use super::*;

fn authority(now: Instant, wall: SystemTime) -> GatewayAuthority {
    GatewayAuthority {
        account_reference: "test-account".into(),
        credential_profile: "gateway-test".into(),
        credential_revision: "key-revision-1".into(),
        credential_sha256: "a".repeat(64),
        price_reference: "account-price-1".into(),
        price: GatewayPrice {
            input_picos_per_token: 0,
            output_picos_per_token: 0,
            request_fee_picos: 0,
        },
        funding: GatewayFunding::ZeroPricePromotion,
        routing: observed_routing(),
        effective_route_reference: "typesafe-effective-route-1".into(),
        retention_reference: "account-retention-1".into(),
        region: GatewayRegion::ApprovedGlobal,
        typesafe_route_verified: true,
        gateway_key_only: true,
        fallback_disabled: true,
        valid_until: now + Duration::from_secs(60),
        wall_valid_until: wall + Duration::from_secs(60),
    }
}

fn authority_references(authority: &mut GatewayAuthority) -> [&mut String; 6] {
    [
        &mut authority.account_reference,
        &mut authority.credential_profile,
        &mut authority.credential_revision,
        &mut authority.price_reference,
        &mut authority.effective_route_reference,
        &mut authority.retention_reference,
    ]
}

#[test]
fn gateway_evidence_references_use_utf8_byte_limits_without_normalization() {
    for reference in [
        "r".into(),
        " opaque reference ".into(),
        "x".repeat(255),
        "x".repeat(256),
        "é".repeat(128),
        format!("{}a", "é".repeat(127)),
    ] {
        assert!(gateway_reference_valid(&reference), "{reference:?}");
    }
    for reference in [
        "".into(),
        " \u{2003} ".into(),
        "x".repeat(257),
        format!("{}a", "é".repeat(128)),
        "r\0x".into(),
        "r\nx".into(),
        "r\u{7f}x".into(),
        "r\u{85}x".into(),
    ] {
        assert!(!gateway_reference_valid(&reference), "{reference:?}");
    }
}

#[test]
fn gateway_authority_requires_every_reference_and_credential_digest() {
    let now = Instant::now();
    let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let good = authority(now, wall);
    assert_eq!(good.validate(now, wall), Ok(()));
    for field in 0..6 {
        for invalid in ["".into(), " \t".into(), "x".repeat(257), "r\u{85}x".into()] {
            let mut changed = good.clone();
            *authority_references(&mut changed)[field] = invalid;
            assert_eq!(
                changed.validate(now, wall),
                Err(DecisionUnavailable::ApprovalMissing),
                "reference {field}"
            );
        }
    }
    for invalid in [
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
    ] {
        let mut changed = good.clone();
        changed.credential_sha256 = invalid;
        assert_eq!(
            changed.validate(now, wall),
            Err(DecisionUnavailable::ApprovalMissing)
        );
    }
}

#[test]
fn gateway_authority_requires_each_route_flag_region_and_routing_identity() {
    let now = Instant::now();
    let wall = SystemTime::now();
    let good = authority(now, wall);
    assert_eq!(good.validate(now, wall), Ok(()));
    for field in 0..3 {
        let mut changed = good.clone();
        let flags = [
            &mut changed.typesafe_route_verified,
            &mut changed.gateway_key_only,
            &mut changed.fallback_disabled,
        ];
        *flags.into_iter().nth(field).unwrap() = false;
        assert_eq!(
            changed.validate(now, wall),
            Err(DecisionUnavailable::ApprovalMissing),
            "flag {field}"
        );
    }
    let mut changed = good.clone();
    changed.region = GatewayRegion::RequiredRegionUnsupported;
    assert_eq!(
        changed.validate(now, wall),
        Err(DecisionUnavailable::ApprovalMissing)
    );
    for field in 0..5 {
        let mut changed = good.clone();
        *routing_fields(&mut changed.routing)[field] = "jev-1.13.0".into();
        assert_eq!(
            changed.validate(now, wall),
            Err(DecisionUnavailable::ApprovalMissing),
            "routing {field}"
        );
    }
}

#[test]
fn gateway_authority_checks_each_clock_at_expiry_and_one_hour_boundaries() {
    let now = Instant::now();
    let wall = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
    let good = authority(now, wall);
    for clock in 0..2 {
        // Windows SystemTime stores 100 ns ticks, so a 1 ns wall-clock step
        // disappears. Keep Instant's nanosecond boundary and use a representable
        // wall-clock step for the same expiry and one-hour checks (#12749).
        let step = if clock == 0 {
            Duration::from_nanos(1)
        } else {
            Duration::from_micros(1)
        };
        for (remaining, expected) in [
            (None, Err(DecisionUnavailable::ApprovalMissing)),
            (
                Some(Duration::ZERO),
                Err(DecisionUnavailable::ApprovalMissing),
            ),
            (Some(step), Ok(())),
            (Some(Duration::from_secs(3599)), Ok(())),
            (Some(Duration::from_secs(3600)), Ok(())),
            (
                Some(Duration::from_secs(3600) + step),
                Err(DecisionUnavailable::ApprovalMissing),
            ),
        ] {
            let mut changed = good.clone();
            if clock == 0 {
                changed.valid_until = remaining.map_or(now - step, |d| now + d);
            } else {
                changed.wall_valid_until = remaining.map_or(wall - step, |d| wall + d);
            }
            assert_eq!(
                changed.validate(now, wall),
                expected,
                "clock {clock}: {remaining:?}"
            );
        }
    }
    assert_eq!(
        good.validate(good.valid_until, wall),
        Err(DecisionUnavailable::ApprovalMissing)
    );
    assert_eq!(
        good.validate(now, good.wall_valid_until),
        Err(DecisionUnavailable::ApprovalMissing)
    );
}

#[test]
fn gateway_promotion_requires_zero_input_output_and_request_prices() {
    let now = Instant::now();
    let wall = SystemTime::now();
    let good = authority(now, wall);
    assert_eq!(good.validate(now, wall), Ok(()));
    for field in 0..3 {
        let mut changed = good.clone();
        let prices = [
            &mut changed.price.input_picos_per_token,
            &mut changed.price.output_picos_per_token,
            &mut changed.price.request_fee_picos,
        ];
        *prices.into_iter().nth(field).unwrap() = 1;
        assert_eq!(
            changed.validate(now, wall),
            Err(DecisionUnavailable::Rejected)
        );
    }
    for field in 0..2 {
        let mut changed = good.clone();
        changed.funding = GatewayFunding::PaidApi;
        let prices = [
            &mut changed.price.input_picos_per_token,
            &mut changed.price.output_picos_per_token,
        ];
        *prices.into_iter().nth(field).unwrap() = u64::MAX;
        assert_eq!(
            changed.validate(now, wall),
            Err(DecisionUnavailable::ApprovalMissing)
        );
    }
}

#[test]
fn gateway_credits_require_exclusive_allocation_covering_worst_case_without_cash() {
    let now = Instant::now();
    let wall = SystemTime::now();
    let mut good = authority(now, wall);
    good.price = GatewayPrice {
        input_picos_per_token: 1,
        output_picos_per_token: 2,
        request_fee_picos: 3,
    };
    let reserved = good.price.reservation().unwrap();
    assert_eq!(reserved, 196_611);
    for (reference, limit, cash_disabled, expected) in [
        ("reserved-allocation", reserved, true, Ok(())),
        ("reserved-allocation", reserved + 1, true, Ok(())),
        (
            "reserved-allocation",
            reserved - 1,
            true,
            Err(DecisionUnavailable::Rejected),
        ),
        (
            "reserved-allocation",
            0,
            true,
            Err(DecisionUnavailable::Rejected),
        ),
        ("", reserved, true, Err(DecisionUnavailable::Rejected)),
        (
            "public\ncredit",
            reserved,
            true,
            Err(DecisionUnavailable::Rejected),
        ),
        (
            "reserved-allocation",
            reserved,
            false,
            Err(DecisionUnavailable::Rejected),
        ),
    ] {
        let mut changed = good.clone();
        changed.funding = GatewayFunding::FreeCredits {
            allocation_reference: reference.into(),
            limit_picos: limit,
            cash_fallback_disabled: cash_disabled,
        };
        assert_eq!(
            changed.validate(now, wall),
            expected,
            "{reference:?}/{limit}/{cash_disabled}"
        );
    }
    good.price = GatewayPrice {
        input_picos_per_token: 0,
        output_picos_per_token: 0,
        request_fee_picos: 0,
    };
    good.funding = GatewayFunding::FreeCredits {
        allocation_reference: "reserved-allocation".into(),
        limit_picos: 0,
        cash_fallback_disabled: true,
    };
    assert_eq!(good.validate(now, wall), Err(DecisionUnavailable::Rejected));
}

#[test]
fn gateway_funding_keeps_unknown_promotion_credit_and_paid_distinct() {
    let now = Instant::now();
    let wall = SystemTime::now();
    let mut observed = authority(now, wall);
    for (funding, billing, kind, validity) in [
        (
            GatewayFunding::Unknown,
            CandidateBilling::Unknown,
            None,
            Err(DecisionUnavailable::ApprovalMissing),
        ),
        (
            GatewayFunding::ZeroPricePromotion,
            CandidateBilling::Promotion {
                valid_until: observed.valid_until,
            },
            Some(GatewayFundingKind::Promotion),
            Ok(()),
        ),
        (
            GatewayFunding::FreeCredits {
                allocation_reference: "reserved-allocation".into(),
                limit_picos: 1,
                cash_fallback_disabled: true,
            },
            CandidateBilling::MeteredApi,
            Some(GatewayFundingKind::FreeCredits),
            Ok(()),
        ),
        (
            GatewayFunding::PaidApi,
            CandidateBilling::MeteredApi,
            Some(GatewayFundingKind::PaidApi),
            Ok(()),
        ),
    ] {
        observed.funding = funding;
        assert_eq!(observed.billing(), billing);
        assert_eq!(observed.funding_kind(), kind);
        assert_eq!(observed.validate(now, wall), validity);
    }
}

#[test]
fn gateway_exact_decimal_money_never_rounds_unknown_or_small_cost_to_zero() {
    for (text, expected) in [
        ("0", 0),
        ("0.000000042", 42_000),
        ("0.000000000001", 1),
        ("0.0001", 100_000_000),
        ("18446744.073709551615", u64::MAX),
    ] {
        assert_eq!(usd_picos(text), Some(expected), "{text}");
    }
    for text in [
        "",
        "-0",
        "-1",
        "+1",
        "0.+1",
        "1e-9",
        "NaN",
        "Infinity",
        " 0",
        "0 ",
        ".1",
        "1.",
        "00.1",
        "1.2.3",
        "0.0000000000001",
        "18446744.073709551616",
        "9999999999",
        "０",
    ] {
        assert_eq!(usd_picos(text), None, "{text}");
    }
}

#[test]
fn gateway_price_includes_fixed_fee_and_both_token_bounds() {
    let mut price = GatewayPrice {
        input_picos_per_token: 42_000,
        output_picos_per_token: 5_000,
        request_fee_picos: 100_000_000,
    };
    assert_eq!(
        price.estimate(DecisionUsage {
            input_tokens: 100,
            output_tokens: 200
        }),
        Some(105_200_000)
    );
    assert_eq!(
        price.estimate(DecisionUsage {
            input_tokens: 0,
            output_tokens: 0
        }),
        Some(100_000_000)
    );
    for usage in [
        DecisionUsage {
            input_tokens: 65_537,
            output_tokens: 0,
        },
        DecisionUsage {
            input_tokens: 0,
            output_tokens: 65_537,
        },
    ] {
        assert_eq!(price.estimate(usage), None);
    }
    assert_eq!(price.reservation(), Some(3_180_192_000));
    price.input_picos_per_token = u64::MAX;
    assert_eq!(price.reservation(), None);
}

fn observed_routing() -> GatewayRouting {
    GatewayRouting {
        response_model: GATEWAY_MODEL.into(),
        original_model_id: GATEWAY_MODEL.into(),
        canonical_slug: GATEWAY_MODEL.into(),
        resolved_provider: "typesafe-ai".into(),
        final_provider: "typesafe-ai".into(),
    }
}

fn routing_fields(routing: &mut GatewayRouting) -> [&mut String; 5] {
    [
        &mut routing.response_model,
        &mut routing.original_model_id,
        &mut routing.canonical_slug,
        &mut routing.resolved_provider,
        &mut routing.final_provider,
    ]
}

#[test]
fn gateway_routing_requires_each_observed_identity_without_alias_normalization() {
    assert_eq!(observed_routing().validate(), Ok(()));
    for field in 0..5 {
        for invalid in ["", "other", "jev-1.13.0", " typesafe-ai", "typesafe-ai "] {
            let mut routing = observed_routing();
            *routing_fields(&mut routing)[field] = invalid.into();
            assert_eq!(
                routing.validate(),
                Err(DecisionUnavailable::InvalidResponse),
                "field {field}: {invalid:?}"
            );
        }
        let mut routing = observed_routing();
        *routing_fields(&mut routing)[field] = if field < 3 {
            "typesafe-ai".into()
        } else {
            GATEWAY_MODEL.into()
        };
        assert_eq!(
            routing.validate(),
            Err(DecisionUnavailable::InvalidResponse)
        );
    }
}

fn cost_fields(costs: &mut GatewayReportedCosts) -> [&mut Option<String>; 4] {
    [
        &mut costs.cost,
        &mut costs.market_cost,
        &mut costs.surcharge_cost,
        &mut costs.gateway_cost,
    ]
}

#[test]
fn gateway_reported_costs_validate_every_present_field_independently() {
    assert_eq!(GatewayReportedCosts::default().validate(), Ok(()));
    for field in 0..4 {
        for valid in ["0", "0.000000000001", "18446744.073709551615"] {
            let mut costs = GatewayReportedCosts::default();
            *cost_fields(&mut costs)[field] = Some(valid.into());
            assert_eq!(costs.validate(), Ok(()), "field {field}: {valid}");
        }
        for invalid in ["", "-1", "NaN", "1e-9", " 0", "0.", "0.0000000000001"] {
            let mut costs = GatewayReportedCosts::default();
            *cost_fields(&mut costs)[field] = Some(invalid.into());
            assert_eq!(
                costs.validate(),
                Err(DecisionUnavailable::InvalidResponse),
                "field {field}: {invalid:?}"
            );
        }
    }
}

#[test]
fn gateway_digest_requires_exactly_64_lowercase_ascii_hex_bytes() {
    for valid in ["0".repeat(64), "a".repeat(64), "0123456789abcdef".repeat(4)] {
        assert!(gateway_digest_valid(&valid), "{valid}");
    }
    for invalid in [
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        format!("{}g", "a".repeat(63)),
        format!("{} ", "a".repeat(63)),
        format!("{}\0", "a".repeat(63)),
        "é".repeat(32),
    ] {
        assert!(!gateway_digest_valid(&invalid), "{invalid:?}");
    }
}

fn observation() -> GatewayObservation {
    GatewayObservation {
        routing: observed_routing(),
        usage: None,
        costs: GatewayReportedCosts::default(),
        generation_hash: None,
    }
}

#[test]
fn gateway_observation_accepts_unknown_evidence_and_distinguishes_reported_zero() {
    let mut observed = observation();
    assert_eq!(observed.validate(), Ok(()));
    let unknown = serde_json::to_value(&observed).unwrap();
    assert!(unknown["usage"].is_null());
    assert!(unknown["generation_hash"].is_null());
    for key in ["cost", "market_cost", "surcharge_cost", "gateway_cost"] {
        assert!(unknown["costs"][key].is_null());
    }
    observed.usage = Some(DecisionUsage {
        input_tokens: 0,
        output_tokens: 0,
    });
    observed.costs.cost = Some("0".into());
    observed.generation_hash = Some("a".repeat(64));
    assert_eq!(observed.validate(), Ok(()));
    let zero = serde_json::to_value(&observed).unwrap();
    assert_eq!(zero["usage"]["input_tokens"], 0);
    assert_eq!(zero["usage"]["output_tokens"], 0);
    assert_eq!(zero["costs"]["cost"], "0");
    assert!(zero["costs"]["market_cost"].is_null());
}

#[test]
fn gateway_observation_enforces_each_token_limit_inclusively() {
    for tokens in [0, 65_535, 65_536, 65_537, u64::MAX] {
        for usage in [
            DecisionUsage {
                input_tokens: tokens,
                output_tokens: 0,
            },
            DecisionUsage {
                input_tokens: 0,
                output_tokens: tokens,
            },
        ] {
            let mut observed = observation();
            observed.usage = Some(usage);
            let expected = if tokens <= 65_536 {
                Ok(())
            } else {
                Err(DecisionUnavailable::InvalidResponse)
            };
            assert_eq!(observed.validate(), expected, "{usage:?}");
        }
    }
}

#[test]
fn gateway_observation_propagates_routing_cost_and_digest_failures() {
    let mut wrong_route = observation();
    wrong_route.routing.canonical_slug = "other".into();
    let mut wrong_cost = observation();
    wrong_cost.costs.gateway_cost = Some("unknown".into());
    let mut wrong_hash = observation();
    wrong_hash.generation_hash = Some("a".repeat(63));
    for invalid in [wrong_route, wrong_cost, wrong_hash] {
        assert_eq!(
            invalid.validate(),
            Err(DecisionUnavailable::InvalidResponse)
        );
    }
}

fn choice_input() -> (DecisionText, GatewayChoice) {
    let text = DecisionText {
        goal: "Save".into(),
        candidates: vec![super::super::candidate_decision::DecisionOption {
            id: "c0".into(),
            text: "Save".into(),
            role: None,
            intent: None,
            state: None,
        }],
    };
    let choice = GatewayChoice {
        selected: "c0".into(),
        probabilities: [
            ("c0".into(), 0.8),
            ("none".into(), 0.1),
            ("delegate".into(), 0.1),
        ]
        .into_iter()
        .collect(),
        confidence: None,
    };
    (text, choice)
}

#[test]
fn gateway_choice_preserves_absent_confidence_ties_and_sum_tolerance() {
    for confidence in [None, Some(0.0), Some(1.0)] {
        for delta in [-0.5e-6, 0.0, 0.5e-6] {
            let (text, mut choice) = choice_input();
            choice.confidence = confidence;
            *choice.probabilities.get_mut("c0").unwrap() += delta;
            assert_eq!(choice.validate(&text), Ok(()));
            assert_eq!(choice.confidence, confidence);
        }
    }
    let (text, mut choice) = choice_input();
    choice.probabilities = [
        ("c0".into(), 0.5),
        ("none".into(), 0.5),
        ("delegate".into(), 0.0),
    ]
    .into_iter()
    .collect();
    for selected in ["c0", "none"] {
        choice.selected = selected.into();
        assert_eq!(choice.validate(&text), Ok(()));
    }
    for delta in [-1.5e-6, 1.5e-6] {
        let (text, mut choice) = choice_input();
        *choice.probabilities.get_mut("c0").unwrap() += delta;
        assert_eq!(
            choice.validate(&text),
            Err(DecisionUnavailable::InvalidResponse)
        );
    }
}

#[test]
fn gateway_choice_rejects_incomplete_options_nonfinite_values_and_nonmaximal_selections() {
    for case in 0..10 {
        let (text, mut choice) = choice_input();
        match case {
            0 => {
                choice.probabilities.remove("none");
            }
            1 => {
                choice.probabilities.insert("outside".into(), 0.0);
            }
            2 => {
                choice.probabilities.insert("c0".into(), f64::NAN);
            }
            3 => {
                choice.probabilities.insert("c0".into(), f64::INFINITY);
            }
            4 => {
                choice.probabilities.insert("none".into(), -0.1);
            }
            5 => {
                choice.probabilities.insert("c0".into(), 1.1);
            }
            6 => choice.selected = "outside".into(),
            7 => choice.selected = "none".into(),
            8 => choice.confidence = Some(f64::NAN),
            _ => choice.confidence = Some(1.1),
        }
        assert_eq!(
            choice.validate(&text),
            Err(DecisionUnavailable::InvalidResponse),
            "case {case}"
        );
    }
}

#[test]
fn gateway_response_value_failure_retains_original_wire_and_observation() {
    let response = GatewayResponse {
        value: 7,
        request_hash: "b".repeat(64),
        observation: observation(),
    };
    let preserved = serde_json::to_value(&response.observation).unwrap();
    let good = response
        .clone()
        .validate_value(|value| {
            assert_eq!(*value, 7);
            Ok(())
        })
        .unwrap();
    assert_eq!(good.value, 7);
    assert_eq!(good.request_hash, "b".repeat(64));
    let bad = response
        .validate_value(|_| Err(DecisionUnavailable::InvalidResponse))
        .unwrap_err();
    assert_eq!(bad.reason, DecisionUnavailable::InvalidResponse);
    assert!(bad.attempted);
    assert_eq!(bad.request_hash, Some("b".repeat(64)));
    assert_eq!(
        serde_json::to_value(bad.observation.unwrap()).unwrap(),
        preserved
    );
}

fn cost_attempt(reserved: u64, estimate: u64, reported: u64) -> (GatewayAttempt, GatewayAuthority) {
    let mut authority = authority(Instant::now(), SystemTime::now());
    authority.funding = GatewayFunding::PaidApi;
    authority.price.input_picos_per_token = estimate;
    let mut observed = observation();
    observed.usage = Some(DecisionUsage {
        input_tokens: 1,
        output_tokens: 0,
    });
    observed.costs.cost = Some(format!("0.{reported:012}"));
    (
        GatewayAttempt {
            id: Uuid::new_v4(),
            stage: DecisionStage::Choice,
            request_hash: Some("c".repeat(64)),
            send_state: GatewaySendState::Attempted,
            elapsed_ms: 1,
            reserved_picos: reserved,
            estimated_cost_picos: None,
            funding: GatewayFundingKind::PaidApi,
            observation: Some(Box::new(observed)),
            failure: None,
        },
        authority,
    )
}

#[test]
fn gateway_accounting_checks_both_budget_bounds_and_preserves_actual_overspend() {
    use DecisionUnavailable::{BudgetExceeded, Rejected};
    for (reserved, estimate, reported, expected) in [
        (0, 0, 0, Ok(())),
        (9, 9, 9, Ok(())),
        (10, 9, 9, Ok(())),
        (8, 9, 9, Err(BudgetExceeded)),
        (8, 7, 9, Err(BudgetExceeded)),
        (8, 9, 7, Err(BudgetExceeded)),
        (10, 8, 9, Err(Rejected)),
        (10, 9, 8, Err(Rejected)),
    ] {
        let (mut attempt, authority) = cost_attempt(reserved, estimate, reported);
        assert_eq!(attempt.account_observation(&authority), expected);
        assert_eq!(attempt.reserved_picos, reserved);
        assert_eq!(attempt.estimated_cost_picos, Some(estimate));
        assert_eq!(
            attempt.observation.as_ref().unwrap().costs.cost,
            Some(format!("0.{reported:012}"))
        );
    }
}

#[test]
fn gateway_accounting_distinguishes_pending_not_sent_and_unknown_usage() {
    for state in [
        GatewaySendState::Pending,
        GatewaySendState::NotSent,
        GatewaySendState::Attempted,
    ] {
        for observed in [false, true] {
            let (mut attempt, authority) = cost_attempt(9, 9, 9);
            attempt.send_state = state;
            attempt.estimated_cost_picos = Some(123);
            if !observed {
                attempt.observation = None;
            }
            let expected = match (state, observed) {
                (GatewaySendState::NotSent, false) | (GatewaySendState::Attempted, true) => Ok(()),
                _ => Err(DecisionUnavailable::InvalidResponse),
            };
            assert_eq!(attempt.account_observation(&authority), expected);
            assert_eq!(attempt.reserved_picos, 9);
            assert_eq!(
                attempt.estimated_cost_picos,
                if state == GatewaySendState::Attempted && observed {
                    Some(9)
                } else {
                    None
                }
            );
        }
    }
    for missing in [true, false] {
        let (mut attempt, authority) = cost_attempt(9, 9, 9);
        if missing {
            attempt.observation.as_mut().unwrap().usage = None;
        } else {
            attempt.observation.as_mut().unwrap().costs.cost = None;
        }
        assert_eq!(
            attempt.account_observation(&authority),
            Err(DecisionUnavailable::InvalidResponse)
        );
        assert_eq!(
            attempt.estimated_cost_picos,
            if missing { None } else { Some(9) }
        );
    }
    let (mut attempt, mut authority) = cost_attempt(9, 9, 9);
    authority.price.input_picos_per_token = u64::MAX;
    attempt
        .observation
        .as_mut()
        .unwrap()
        .usage
        .as_mut()
        .unwrap()
        .input_tokens = 2;
    assert_eq!(
        attempt.account_observation(&authority),
        Err(DecisionUnavailable::BudgetExceeded)
    );
    assert_eq!(attempt.reserved_picos, 9);
    assert_eq!(attempt.estimated_cost_picos, None);
}

#[test]
fn gateway_accounting_bounds_each_fee_and_requires_explicit_zero_promotion_fees() {
    for surcharge in [true, false] {
        for amount in [0, 8, 9, 10] {
            let (mut attempt, authority) = cost_attempt(9, 9, 9);
            let costs = &mut attempt.observation.as_mut().unwrap().costs;
            let slot = if surcharge {
                &mut costs.surcharge_cost
            } else {
                &mut costs.gateway_cost
            };
            *slot = Some(format!("0.{amount:012}"));
            assert_eq!(
                attempt.account_observation(&authority),
                if amount <= 9 {
                    Ok(())
                } else {
                    Err(DecisionUnavailable::Rejected)
                }
            );
        }
    }
    for fees in [
        (None, None),
        (Some("0"), None),
        (None, Some("0")),
        (Some("0"), Some("0")),
    ] {
        let (mut attempt, mut authority) = cost_attempt(0, 0, 0);
        authority.funding = GatewayFunding::ZeroPricePromotion;
        attempt.funding = GatewayFundingKind::Promotion;
        let costs = &mut attempt.observation.as_mut().unwrap().costs;
        costs.surcharge_cost = fees.0.map(str::to_owned);
        costs.gateway_cost = fees.1.map(str::to_owned);
        assert_eq!(
            attempt.account_observation(&authority),
            if fees == (Some("0"), Some("0")) {
                Ok(())
            } else {
                Err(DecisionUnavailable::Rejected)
            }
        );
        assert_eq!(attempt.estimated_cost_picos, Some(0));
        assert_eq!(attempt.reserved_picos, 0);
    }
}
