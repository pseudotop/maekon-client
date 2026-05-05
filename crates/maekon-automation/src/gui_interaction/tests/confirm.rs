use super::*;

// ── Confirm tests ───────────────────────────────────────────────────

#[tokio::test]
async fn confirm_transitions_to_confirmed_with_ticket() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    let ticket = service
        .confirm_candidate(
            &sid,
            &token,
            GuiConfirmRequest {
                candidate_id: "el-1".to_string(),
                action: GuiActionRequest {
                    action_type: GuiActionType::Click,
                    text: None,
                },
                ticket_ttl_secs: None,
            },
        )
        .await
        .unwrap();

    assert!(!ticket.signature.is_empty());
    assert_eq!(ticket.session_id, sid);
    assert_eq!(ticket.element_id, "el-1");

    let session = service.get_session(&sid, &token).await.unwrap();
    assert_eq!(session.state, GuiSessionState::Confirmed);
    assert_eq!(session.selected_element_id, Some("el-1".to_string()));
}

#[tokio::test]
async fn confirm_rejects_unknown_candidate() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    let err = service
        .confirm_candidate(
            &sid,
            &token,
            GuiConfirmRequest {
                candidate_id: "nonexistent".to_string(),
                action: GuiActionRequest {
                    action_type: GuiActionType::Click,
                    text: None,
                },
                ticket_ttl_secs: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::BadRequest { .. }));
}

#[tokio::test]
async fn confirm_rejects_when_focus_changed() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, probe) = make_service(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    // Simulate focus drift
    probe.set_validation_valid(false);

    let err = service
        .confirm_candidate(
            &sid,
            &token,
            GuiConfirmRequest {
                candidate_id: "el-1".to_string(),
                action: GuiActionRequest {
                    action_type: GuiActionType::Click,
                    text: None,
                },
                ticket_ttl_secs: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::FocusDrift { .. }));
}

#[tokio::test]
async fn confirm_type_text_requires_text() {
    let scene = make_scene(vec![make_element("el-1", "Input", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    let err = service
        .confirm_candidate(
            &sid,
            &token,
            GuiConfirmRequest {
                candidate_id: "el-1".to_string(),
                action: GuiActionRequest {
                    action_type: GuiActionType::TypeText,
                    text: None,
                },
                ticket_ttl_secs: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::BadRequest { .. }));
}

// ── Build candidates tests ──────────────────────────────────────────

#[test]
fn build_candidates_sorts_by_confidence_descending() {
    let scene = make_scene(vec![
        make_element("el-low", "A", 0.6),
        make_element("el-high", "B", 0.95),
        make_element("el-mid", "C", 0.8),
    ]);

    let candidates = build_candidates(&scene, 0.5, 10);

    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0].element.element_id, "el-high");
    assert_eq!(candidates[1].element.element_id, "el-mid");
    assert_eq!(candidates[2].element.element_id, "el-low");
}

#[test]
fn build_candidates_filters_below_min_confidence() {
    let scene = make_scene(vec![
        make_element("el-1", "A", 0.9),
        make_element("el-2", "B", 0.3),
    ]);

    let candidates = build_candidates(&scene, 0.5, 10);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].element.element_id, "el-1");
}

#[test]
fn build_candidates_truncates_to_max() {
    let elements: Vec<UiSceneElement> = (0..10)
        .map(|i| make_element(&format!("el-{i}"), &format!("Btn{i}"), 0.8))
        .collect();
    let scene = make_scene(elements);

    let candidates = build_candidates(&scene, 0.5, 3);
    assert_eq!(candidates.len(), 3);
}

// ── HMAC ticket signing tests ───────────────────────────────────────

#[test]
fn sign_and_verify_ticket_roundtrip() {
    let secret = TEST_HMAC_SECRET.as_bytes();
    let ticket = GuiExecutionTicket {
        schema_version: "automation.gui.ticket.v1".to_string(),
        ticket_id: "t-1".to_string(),
        session_id: "s-1".to_string(),
        scene_id: "sc-1".to_string(),
        element_id: "el-1".to_string(),
        action_hash: "ahash".to_string(),
        focus_hash: "fhash".to_string(),
        issued_at: Utc::now(),
        expires_at: Utc::now() + ChronoDuration::seconds(30),
        nonce: "nonce-1".to_string(),
        signature: String::new(),
    };

    let sig = sign_ticket(secret, &ticket).unwrap();
    let signed = GuiExecutionTicket {
        signature: sig,
        ..ticket
    };

    assert!(verify_ticket(secret, &signed).is_ok());
}

#[test]
fn verify_ticket_rejects_tampered_nonce() {
    let secret = TEST_HMAC_SECRET.as_bytes();
    let ticket = GuiExecutionTicket {
        schema_version: "automation.gui.ticket.v1".to_string(),
        ticket_id: "t-1".to_string(),
        session_id: "s-1".to_string(),
        scene_id: "sc-1".to_string(),
        element_id: "el-1".to_string(),
        action_hash: "ahash".to_string(),
        focus_hash: "fhash".to_string(),
        issued_at: Utc::now(),
        expires_at: Utc::now() + ChronoDuration::seconds(30),
        nonce: "nonce-1".to_string(),
        signature: String::new(),
    };

    let sig = sign_ticket(secret, &ticket).unwrap();
    let tampered = GuiExecutionTicket {
        signature: sig,
        nonce: "tampered-nonce".to_string(),
        ..ticket
    };

    assert!(verify_ticket(secret, &tampered).is_err());
}

// ── Action builder tests ────────────────────────────────────────────

#[test]
fn build_actions_click_generates_mouse_click() {
    let candidate = GuiCandidate {
        element: make_element("el-1", "Save", 0.9),
        ranking_reason: None,
        eligible: true,
    };
    let action = GuiActionRequest {
        action_type: GuiActionType::Click,
        text: None,
    };

    let actions = build_actions_for_candidate(&candidate, &action).unwrap();
    assert_eq!(actions.len(), 1);
    assert!(matches!(actions[0], AutomationAction::MouseClick { .. }));
}

#[test]
fn build_actions_double_click_generates_two_clicks() {
    let candidate = GuiCandidate {
        element: make_element("el-1", "File", 0.9),
        ranking_reason: None,
        eligible: true,
    };
    let action = GuiActionRequest {
        action_type: GuiActionType::DoubleClick,
        text: None,
    };

    let actions = build_actions_for_candidate(&candidate, &action).unwrap();
    assert_eq!(actions.len(), 2);
}

#[test]
fn build_actions_type_text_generates_click_then_type() {
    let candidate = GuiCandidate {
        element: make_element("el-1", "Input", 0.9),
        ranking_reason: None,
        eligible: true,
    };
    let action = GuiActionRequest {
        action_type: GuiActionType::TypeText,
        text: Some("hello".to_string()),
    };

    let actions = build_actions_for_candidate(&candidate, &action).unwrap();
    assert_eq!(actions.len(), 2);
    assert!(matches!(actions[0], AutomationAction::MouseClick { .. }));
    assert!(matches!(actions[1], AutomationAction::KeyType { .. }));
}

#[test]
fn build_actions_type_text_rejects_empty_text() {
    let candidate = GuiCandidate {
        element: make_element("el-1", "Input", 0.9),
        ranking_reason: None,
        eligible: true,
    };
    let action = GuiActionRequest {
        action_type: GuiActionType::TypeText,
        text: Some("  ".to_string()),
    };

    assert!(build_actions_for_candidate(&candidate, &action).is_err());
}
