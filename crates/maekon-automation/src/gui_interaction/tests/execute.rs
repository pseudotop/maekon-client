use super::*;

// ── Execution tests ─────────────────────────────────────────────────

#[tokio::test]
async fn prepare_execution_transitions_to_executing() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    let plan = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    assert_eq!(plan.session_id, sid);
    assert!(!plan.actions.is_empty());

    let session = service.get_session(&sid, &token).await.unwrap();
    assert_eq!(session.state, GuiSessionState::Executing);
}

#[tokio::test]
async fn prepare_execution_rejects_nonce_replay() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    // First execution succeeds
    let _ = service
        .prepare_execution(
            &sid,
            &token,
            GuiExecutionRequest {
                ticket: ticket.clone(),
            },
        )
        .await
        .unwrap();

    // Complete execution to go back to Confirmed for re-test
    service
        .complete_execution(&sid, false, None, 0, 1)
        .await
        .unwrap();

    // Replay same ticket nonce — should be rejected
    let err = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::TicketInvalid { .. }));
}

#[tokio::test]
async fn prepare_execution_rejects_tampered_signature() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, mut ticket) = create_highlight_and_confirm(&service).await;

    ticket.signature = "00".repeat(32); // tampered

    let err = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::TicketInvalid { .. }));
}

#[tokio::test]
async fn prepare_execution_rejects_wrong_session_state() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    // Session is only Proposed (not Confirmed), so prepare_execution should fail
    let (sid, token) = create_test_session(&service).await;
    let dummy_ticket = GuiExecutionTicket {
        schema_version: "automation.gui.ticket.v1".to_string(),
        ticket_id: "t1".to_string(),
        session_id: sid.clone(),
        scene_id: "s1".to_string(),
        element_id: "el-1".to_string(),
        action_hash: "hash".to_string(),
        focus_hash: "focus".to_string(),
        issued_at: Utc::now(),
        expires_at: Utc::now() + ChronoDuration::seconds(60),
        nonce: "nonce1".to_string(),
        signature: "sig".to_string(),
    };

    let err = service
        .prepare_execution(
            &sid,
            &token,
            GuiExecutionRequest {
                ticket: dummy_ticket,
            },
        )
        .await
        .unwrap_err();
    // Should fail because there is no confirmed_action
    assert!(matches!(err, GuiInteractionError::TicketInvalid { .. }));
}

#[tokio::test]
async fn prepare_execution_rejects_focus_drift() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, probe) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    // Focus drifts after confirm
    probe.set_validation_valid(false);

    let err = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::FocusDrift { .. }));
}

// ── Complete execution tests ────────────────────────────────────────

#[tokio::test]
async fn complete_execution_success_transitions_to_executed() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    let outcome = service
        .complete_execution(&sid, true, None, 1, 1)
        .await
        .unwrap();

    assert!(outcome.succeeded);
    assert_eq!(outcome.session.state, GuiSessionState::Executed);
}

#[tokio::test]
async fn complete_execution_failure_reverts_to_confirmed() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    let outcome = service
        .complete_execution(&sid, false, Some("click missed".to_string()), 0, 1)
        .await
        .unwrap();

    assert!(!outcome.succeeded);
    assert_eq!(outcome.session.state, GuiSessionState::Confirmed);
    assert_eq!(outcome.detail, Some("click missed".to_string()));
}

// ── Cancel tests ────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_transitions_to_cancelled() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_test_session(&service).await;
    let session = service.cancel_session(&sid, &token).await.unwrap();

    assert_eq!(session.state, GuiSessionState::Cancelled);
}

#[tokio::test]
async fn cancel_rejects_invalid_token() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, _) = create_test_session(&service).await;
    let err = service
        .cancel_session(&sid, "wrong-token")
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::Unauthorized { .. }));
}

// ── Expiry tests ────────────────────────────────────────────────────

#[tokio::test]
async fn expired_session_is_detected_on_get() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut req = default_create_request();
    req.session_ttl_secs = Some(30); // minimum allowed

    let resp = service.create_session(req).await.unwrap();
    let sid = resp.session.session_id.clone();
    let token = resp.capability_token.clone();

    // Manually expire the session by setting expires_at in the past
    {
        let mut sessions = service.sessions.write().await;
        if let Some(stored) = sessions.get_mut(&sid) {
            stored.session.expires_at = Utc::now() - ChronoDuration::seconds(1);
        }
    }

    let session = service.get_session(&sid, &token).await.unwrap();
    assert_eq!(session.state, GuiSessionState::Expired);
}

#[tokio::test]
async fn highlight_rejects_expired_session() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_test_session(&service).await;

    // Expire it
    {
        let mut sessions = service.sessions.write().await;
        if let Some(stored) = sessions.get_mut(&sid) {
            stored.session.expires_at = Utc::now() - ChronoDuration::seconds(1);
        }
    }

    let err = service
        .highlight_session(
            &sid,
            &token,
            GuiHighlightRequest {
                candidate_ids: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::TicketInvalid { .. }));
}

// ── Full flow integration test ──────────────────────────────────────

#[tokio::test]
async fn full_propose_highlight_confirm_execute_flow() {
    let scene = make_scene(vec![
        make_element("el-1", "Save", 0.95),
        make_element("el-2", "Cancel", 0.85),
    ]);
    let (service, _) = make_service(scene, make_focus());

    // 1. Propose
    let resp = service
        .create_session(default_create_request())
        .await
        .unwrap();
    assert_eq!(resp.session.state, GuiSessionState::Proposed);
    let sid = resp.session.session_id.clone();
    let token = resp.capability_token.clone();

    // 2. Highlight
    let session = service
        .highlight_session(
            &sid,
            &token,
            GuiHighlightRequest {
                candidate_ids: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(session.state, GuiSessionState::Highlighted);

    // 3. Confirm
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
                ticket_ttl_secs: Some(60),
            },
        )
        .await
        .unwrap();
    assert!(!ticket.signature.is_empty());

    let session = service.get_session(&sid, &token).await.unwrap();
    assert_eq!(session.state, GuiSessionState::Confirmed);

    // 4. Execute
    let plan = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();
    assert!(!plan.actions.is_empty());

    let session = service.get_session(&sid, &token).await.unwrap();
    assert_eq!(session.state, GuiSessionState::Executing);

    // 5. Complete
    let outcome = service
        .complete_execution(&sid, true, None, 1, 1)
        .await
        .unwrap();
    assert!(outcome.succeeded);
    assert_eq!(outcome.session.state, GuiSessionState::Executed);
}

// ── M3: Event subscription / SSE integration tests ─────────────────

#[tokio::test]
async fn subscribe_receives_session_events() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();

    let _ = service
        .create_session(default_create_request())
        .await
        .unwrap();

    let event = rx.try_recv().unwrap();
    assert_eq!(event.event_type, "gui_session.proposed");
    assert_eq!(event.state, GuiSessionState::Proposed);
}

#[tokio::test]
async fn subscribe_session_requires_valid_token() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, _token) = create_test_session(&service).await;

    let err = service
        .subscribe_session(&sid, "wrong-token")
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::Unauthorized { .. }));
}

#[tokio::test]
async fn subscribe_session_rejects_unknown_session() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let err = service
        .subscribe_session("nonexistent-session", "any-token")
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::NotFound { .. }));
}

#[tokio::test]
async fn subscribe_session_succeeds_with_valid_token() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_test_session(&service).await;

    let rx = service.subscribe_session(&sid, &token).await;
    assert!(rx.is_ok(), "Valid token should allow subscription");
}

#[tokio::test]
async fn event_stream_full_lifecycle_proposed_to_executed() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();

    // 1. Create session → Proposed
    let resp = service
        .create_session(default_create_request())
        .await
        .unwrap();
    let sid = resp.session.session_id;
    let token = resp.capability_token;

    // 2. Highlight → Highlighted
    service
        .highlight_session(
            &sid,
            &token,
            GuiHighlightRequest {
                candidate_ids: None,
            },
        )
        .await
        .unwrap();

    // 3. Confirm → Confirmed
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

    // 4. Prepare execution → Executing
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    // 5. Complete → Executed
    service
        .complete_execution(&sid, true, None, 1, 1)
        .await
        .unwrap();

    // Collect all events
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert_eq!(
        types,
        vec![
            "gui_session.proposed",
            "gui_session.highlighted",
            "gui_session.confirmed",
            "gui_session.executing",
            "gui_session.executed",
        ],
        "Events should arrive in state machine order"
    );

    // All events belong to the same session
    assert!(
        events.iter().all(|e| e.session_id == sid),
        "All events must reference the same session"
    );
}

#[tokio::test]
async fn event_stream_cancel_emits_cancelled_event() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();

    let (sid, token) = create_test_session(&service).await;
    service.cancel_session(&sid, &token).await.unwrap();

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    assert_eq!(events.len(), 2); // proposed + cancelled
    assert_eq!(events[0].event_type, "gui_session.proposed");
    assert_eq!(events[1].event_type, "gui_session.cancelled");
    assert_eq!(events[1].state, GuiSessionState::Cancelled);
}

#[tokio::test]
async fn event_stream_execution_failure_emits_failure_event() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();
    service
        .complete_execution(&sid, false, Some("click missed".to_string()), 0, 1)
        .await
        .unwrap();

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    let types: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(
        types.contains(&"gui_session.execution_failed"),
        "Should emit execution_failed event, got: {types:?}"
    );
}

#[tokio::test]
async fn event_schema_version_is_consistent() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();

    let _ = service
        .create_session(default_create_request())
        .await
        .unwrap();

    let event = rx.try_recv().unwrap();
    assert_eq!(
        event.schema_version, "automation.gui.event.v1",
        "Event schema version must match contract"
    );
}

#[tokio::test]
async fn events_are_session_scoped_in_broadcast() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    // Create two sessions
    let (sid1, _token1) = create_test_session(&service).await;
    let (sid2, _token2) = create_test_session(&service).await;

    // Subscribe AFTER both sessions exist
    let mut rx = service.subscribe();

    // Cancel session 1 — should emit event for sid1 only
    service.cancel_session(&sid1, &_token1).await.unwrap();

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    // Filter to session 1 events only (simulating handler-level filtering)
    let sid1_events: Vec<_> = events.iter().filter(|e| e.session_id == sid1).collect();
    let sid2_events: Vec<_> = events.iter().filter(|e| e.session_id == sid2).collect();

    assert_eq!(sid1_events.len(), 1);
    assert_eq!(sid1_events[0].event_type, "gui_session.cancelled");
    assert!(
        sid2_events.is_empty(),
        "Session 2 should have no events after session 1 cancel"
    );
}

#[tokio::test]
async fn event_includes_message_from_confirm() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    // Drain events to find the executing event with ticket_id message
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }

    let executing_event = events
        .iter()
        .find(|e| e.event_type == "gui_session.executing")
        .expect("Should have executing event");

    assert!(
        executing_event.message.is_some(),
        "Executing event should contain ticket_id in message"
    );
    assert!(
        executing_event
            .message
            .as_ref()
            .unwrap()
            .contains("ticket_id="),
        "Message should contain ticket_id reference"
    );
}

#[test]
fn event_channel_capacity_is_reasonable() {
    assert!(
        std::hint::black_box(GUI_EVENT_CHANNEL_CAPACITY) >= 64
            && std::hint::black_box(GUI_EVENT_CHANNEL_CAPACITY) <= 1024,
        "Event channel capacity should be between 64 and 1024"
    );
}

// ── M2: Focus drift recovery tests ─────────────────────────────────

#[tokio::test]
async fn prepare_execution_recovers_from_transient_drift() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, probe) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    // confirm_candidate already made call 0 (valid).
    // In prepare_execution: call 1 → drift, call 2 → recover.
    probe.set_drift_recover_after(2);

    let plan = service
        .prepare_execution(
            &sid,
            &token,
            GuiExecutionRequest {
                ticket: ticket.clone(),
            },
        )
        .await;
    assert!(plan.is_ok(), "Should recover after transient drift");
    // 1 (confirm) + 2 (prepare: drift then recover) = 3
    assert_eq!(
        probe.validation_call_count.load(Ordering::SeqCst),
        3,
        "Should have retried focus validation"
    );
}

#[tokio::test]
async fn prepare_execution_recovers_after_two_drifts() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, probe) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    // confirm_candidate already made call 0 (valid).
    // In prepare_execution: call 1 → drift, call 2 → drift, call 3 → recover.
    probe.set_drift_recover_after(3);

    let plan = service
        .prepare_execution(
            &sid,
            &token,
            GuiExecutionRequest {
                ticket: ticket.clone(),
            },
        )
        .await;
    assert!(plan.is_ok(), "Should recover after two drifts");
    // 1 (confirm) + 3 (prepare: drift, drift, recover) = 4
    assert_eq!(
        probe.validation_call_count.load(Ordering::SeqCst),
        4,
        "Should have attempted initial + 2 retries in prepare_execution"
    );
}

#[tokio::test]
async fn prepare_execution_fails_after_max_drift_retries() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, probe) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    // Never recover
    probe.set_validation_valid(false);

    let err = service
        .prepare_execution(
            &sid,
            &token,
            GuiExecutionRequest {
                ticket: ticket.clone(),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::FocusDrift { .. }));
    // confirm_candidate calls validate once, then prepare_execution calls
    // initial + MAX_RETRIES = 1 + (1 + MAX_RETRIES) = MAX_RETRIES + 2
    assert_eq!(
        probe.validation_call_count.load(Ordering::SeqCst),
        FOCUS_DRIFT_MAX_RETRIES + 2,
        "Should have exhausted all retry attempts"
    );
}

// ── M2: Overlay cleanup tests ──────────────────────────────────────

#[tokio::test]
async fn complete_execution_clears_overlay_on_failure() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _, overlay) = make_service_full(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    // Overlay was shown during highlight
    assert!(overlay.show_count.load(Ordering::SeqCst) >= 1);

    // Confirm the candidate
    let session = service.get_session(&sid, &token).await.unwrap();
    let candidate_id = session.candidates[0].element.element_id.clone();
    let _ticket = service
        .confirm_candidate(
            &sid,
            &token,
            GuiConfirmRequest {
                candidate_id,
                action: GuiActionRequest {
                    action_type: GuiActionType::Click,
                    text: None,
                },
                ticket_ttl_secs: Some(60),
            },
        )
        .await
        .unwrap();

    let clear_before = overlay.clear_count.load(Ordering::SeqCst);

    // Complete with failure
    let outcome = service
        .complete_execution(&sid, false, Some("action failed".to_string()), 0, 1)
        .await
        .unwrap();
    assert!(!outcome.succeeded);

    // Overlay should be cleared even on failure
    assert!(
        overlay.clear_count.load(Ordering::SeqCst) > clear_before,
        "Overlay should be cleared on execution failure"
    );
}

#[tokio::test]
async fn complete_execution_clears_overlay_on_success() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _, overlay) = make_service_full(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;
    let session = service.get_session(&sid, &token).await.unwrap();
    let candidate_id = session.candidates[0].element.element_id.clone();
    let _ticket = service
        .confirm_candidate(
            &sid,
            &token,
            GuiConfirmRequest {
                candidate_id,
                action: GuiActionRequest {
                    action_type: GuiActionType::Click,
                    text: None,
                },
                ticket_ttl_secs: Some(60),
            },
        )
        .await
        .unwrap();

    let clear_before = overlay.clear_count.load(Ordering::SeqCst);

    let outcome = service
        .complete_execution(&sid, true, None, 1, 1)
        .await
        .unwrap();
    assert!(outcome.succeeded);

    assert!(
        overlay.clear_count.load(Ordering::SeqCst) > clear_before,
        "Overlay should be cleared on execution success"
    );
}

#[tokio::test]
async fn cancel_session_clears_overlay() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _, overlay) = make_service_full(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    let clear_before = overlay.clear_count.load(Ordering::SeqCst);

    let session = service.cancel_session(&sid, &token).await.unwrap();
    assert_eq!(session.state, GuiSessionState::Cancelled);

    assert!(
        overlay.clear_count.load(Ordering::SeqCst) > clear_before,
        "Overlay should be cleared on session cancel"
    );
}

// ── M2: Execution constants ────────────────────────────────────────

#[test]
fn focus_drift_retry_constants_are_reasonable() {
    assert!(
        std::hint::black_box(FOCUS_DRIFT_MAX_RETRIES) <= 5,
        "Max retries should be bounded"
    );
    assert!(
        std::hint::black_box(FOCUS_DRIFT_RETRY_DELAY_MS) >= 100
            && std::hint::black_box(FOCUS_DRIFT_RETRY_DELAY_MS) <= 5000,
        "Retry delay should be between 100ms and 5s"
    );
}

// ── M2 P2: Ticket expiry grace period ────────────────────────────────

#[test]
fn ticket_expiry_grace_secs_is_reasonable() {
    assert!(
        std::hint::black_box(TICKET_EXPIRY_GRACE_SECS) >= 1
            && std::hint::black_box(TICKET_EXPIRY_GRACE_SECS) <= 30,
        "Grace period should be between 1s and 30s"
    );
    assert!(
        std::hint::black_box(TICKET_EXPIRY_GRACE_SECS)
            < std::hint::black_box(DEFAULT_TICKET_TTL_SECS),
        "Grace period must be shorter than ticket TTL"
    );
}

#[test]
fn is_expired_past_grace_rejects_well_past_deadline() {
    let well_expired = Utc::now() - ChronoDuration::seconds(60);
    assert!(
        is_expired_past_grace(&well_expired, TICKET_EXPIRY_GRACE_SECS),
        "Ticket expired 60s ago should fail even with grace"
    );
}

#[test]
fn is_expired_past_grace_allows_within_grace_window() {
    // Expired 2s ago, but grace is 5s — should still be valid
    let just_expired = Utc::now() - ChronoDuration::seconds(2);
    assert!(
        !is_expired_past_grace(&just_expired, TICKET_EXPIRY_GRACE_SECS),
        "Ticket expired 2s ago should be allowed within 5s grace"
    );
}

#[test]
fn is_expired_past_grace_rejects_past_grace_boundary() {
    // Expired 10s ago, grace is 5s — should fail
    let past_grace = Utc::now() - ChronoDuration::seconds(10);
    assert!(
        is_expired_past_grace(&past_grace, TICKET_EXPIRY_GRACE_SECS),
        "Ticket expired 10s ago should fail with 5s grace"
    );
}

#[test]
fn is_expired_still_strict_for_sessions() {
    // Session expiry uses strict is_expired (no grace)
    let just_expired = Utc::now() - ChronoDuration::seconds(1);
    assert!(
        is_expired(&just_expired),
        "Session expiry should remain strict"
    );
}

#[tokio::test]
async fn prepare_execution_allows_ticket_within_grace_window() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    // Confirm with a 1-second TTL so it expires quickly
    let (sid, token, _) = create_highlight_and_confirm(&service).await;
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
                ticket_ttl_secs: Some(1),
            },
        )
        .await
        .unwrap();

    // Wait for ticket to nominally expire (1s), but grace (5s) keeps it valid
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    let result = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await;

    assert!(
        result.is_ok(),
        "Ticket expired 1.2s ago should be accepted within 5s grace window"
    );
}

#[tokio::test]
async fn prepare_execution_rejects_ticket_past_grace_window() {
    // Test the is_expired_past_grace function directly since we can't
    // tamper with expires_at without breaking the HMAC signature
    let past_grace = Utc::now() - ChronoDuration::seconds(60);
    assert!(
        is_expired_past_grace(&past_grace, TICKET_EXPIRY_GRACE_SECS),
        "Ticket expired 60s ago should be rejected even with grace"
    );

    let within_grace = Utc::now() - ChronoDuration::seconds(2);
    assert!(
        !is_expired_past_grace(&within_grace, TICKET_EXPIRY_GRACE_SECS),
        "Ticket expired 2s ago should pass with 5s grace"
    );
}

// ── M2 P2: Partial execution step tracking ──────────────────────────

#[tokio::test]
async fn complete_execution_tracks_step_counts() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    // Simulate partial execution: 2 of 5 steps completed
    let outcome = service
        .complete_execution(&sid, false, Some("step 3 failed".to_string()), 2, 5)
        .await
        .unwrap();

    assert!(!outcome.succeeded);
    assert_eq!(outcome.steps_completed, 2);
    assert_eq!(outcome.total_steps, 5);
    assert_eq!(outcome.session.state, GuiSessionState::Confirmed);
}

#[tokio::test]
async fn complete_execution_full_success_step_counts() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    let outcome = service
        .complete_execution(&sid, true, None, 3, 3)
        .await
        .unwrap();

    assert!(outcome.succeeded);
    assert_eq!(outcome.steps_completed, 3);
    assert_eq!(outcome.total_steps, 3);
    assert_eq!(outcome.session.state, GuiSessionState::Executed);
}

#[tokio::test]
async fn partial_execution_allows_retry_with_new_ticket() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;
    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap();

    // Partial failure reverts to Confirmed
    let outcome = service
        .complete_execution(&sid, false, Some("step 2 failed".to_string()), 1, 3)
        .await
        .unwrap();
    assert_eq!(outcome.session.state, GuiSessionState::Confirmed);

    // Client can re-confirm to get a new ticket
    let new_ticket = service
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

    // New ticket should work for retry
    let plan = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket: new_ticket })
        .await;
    assert!(plan.is_ok(), "Retry with new ticket should succeed");
}
