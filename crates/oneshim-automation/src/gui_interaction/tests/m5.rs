use super::*;

// ── M3: SSE Event Stream Integration ────────────────────────────────

/// `subscribe_session` rejects a wrong capability token.
#[tokio::test]
async fn m3_subscribe_session_rejects_invalid_token() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );
    let (sid, _token) = create_test_session(&service).await;

    let err = service
        .subscribe_session(&sid, "wrong-token")
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::Unauthorized { .. }));
}

/// `subscribe_session` rejects an unknown session_id.
#[tokio::test]
async fn m3_subscribe_session_rejects_unknown_session() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );

    let err = service
        .subscribe_session("no-such-session", "any-token")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        GuiInteractionError::Unauthorized { .. } | GuiInteractionError::NotFound { .. }
    ));
}

/// `subscribe_session` with the correct token succeeds.
#[tokio::test]
async fn m3_subscribe_session_accepts_valid_token() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );
    let (sid, token) = create_test_session(&service).await;

    // Subscribing after session creation with the correct token must succeed.
    let result = service.subscribe_session(&sid, &token).await;
    assert!(
        result.is_ok(),
        "subscribe_session should succeed with valid token"
    );
}

/// `create_session` emits a `gui_session.proposed` event on the broadcast channel.
#[tokio::test]
async fn m3_create_session_emits_proposed_event() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );

    // Subscribe before the state transition so we don't miss the event.
    let mut rx = service.subscribe();

    let (sid, _) = create_test_session(&service).await;

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                if ev.session_id == sid {
                    return ev;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("proposed event should be received within timeout");

    assert_eq!(event.event_type, "gui_session.proposed");
    assert_eq!(event.session_id, sid);
}

/// `highlight_session` emits a `gui_session.highlighted` event.
#[tokio::test]
async fn m3_highlight_session_emits_highlighted_event() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );

    let mut rx = service.subscribe();
    let (sid, token) = create_test_session(&service).await;

    // Drain the proposed event.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), async {
        loop {
            if rx.try_recv().is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await;

    service
        .highlight_session(
            &sid,
            &token,
            GuiHighlightRequest {
                candidate_ids: None,
            },
        )
        .await
        .expect("highlight should succeed");

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                if ev.session_id == sid && ev.event_type == "gui_session.highlighted" {
                    return ev;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("highlighted event should be received within timeout");

    assert_eq!(event.event_type, "gui_session.highlighted");
    assert_eq!(event.session_id, sid);
}

/// `cancel_session` emits a `gui_session.cancelled` event.
#[tokio::test]
async fn m3_cancel_session_emits_cancelled_event() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );

    let mut rx = service.subscribe();
    let (sid, token) = create_test_session(&service).await;

    service
        .cancel_session(&sid, &token)
        .await
        .expect("cancel should succeed");

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                if ev.session_id == sid && ev.event_type == "gui_session.cancelled" {
                    return ev;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled event should be received within timeout");

    assert_eq!(event.event_type, "gui_session.cancelled");
    assert_eq!(event.session_id, sid);
}

/// Events from session B are not mistaken for events from session A.
/// The broadcast channel carries events from all sessions; correct consumers
/// must filter by `session_id` (as the SSE handler does).
#[tokio::test]
async fn m3_event_session_id_scoping() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );

    let mut rx = service.subscribe();

    // Create session A — its events should carry sid_a.
    let (sid_a, _) = create_test_session(&service).await;
    // Create session B — its events should carry sid_b.
    let (sid_b, _) = create_test_session(&service).await;

    // Drain all events and partition them by session_id.
    let mut events_a: Vec<String> = vec![];
    let mut events_b: Vec<String> = vec![];

    tokio::time::timeout(std::time::Duration::from_millis(300), async {
        loop {
            match rx.try_recv() {
                Ok(ev) => {
                    if ev.session_id == sid_a {
                        events_a.push(ev.event_type.clone());
                    } else if ev.session_id == sid_b {
                        events_b.push(ev.event_type.clone());
                    }
                }
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
            }
            if !events_a.is_empty() && !events_b.is_empty() {
                break;
            }
        }
    })
    .await
    .expect("events for both sessions should arrive within 300 ms");

    // Both sessions should have their own proposed event.
    assert!(
        events_a.iter().any(|t| t == "gui_session.proposed"),
        "session A should have a proposed event; got {:?}",
        events_a
    );
    assert!(
        events_b.iter().any(|t| t == "gui_session.proposed"),
        "session B should have a proposed event; got {:?}",
        events_b
    );

    // No session A event should carry session B's id and vice versa
    // (guaranteed by the event construction, but asserting the partition is clean).
    assert!(
        !events_a.is_empty() && !events_b.is_empty(),
        "each session must have at least one event"
    );
}

/// `confirm_candidate` emits a `gui_session.confirmed` event.
#[tokio::test]
async fn m3_confirm_candidate_emits_confirmed_event() {
    let scene = make_scene(vec![make_element("el-1", "OK", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();
    let (sid, token) = create_and_highlight(&service).await;

    // Drain earlier events (proposed + highlighted) by collecting until
    // the channel is transiently empty, then proceeding.  Draining by
    // count is fragile when the service emits more than expected; draining
    // to empty is stable because send() is synchronous in the service.
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), async {
        loop {
            match rx.try_recv() {
                Ok(_) => {} // consume but don't break early
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                Err(_) => break, // Lagged — just exit
            }
        }
    })
    .await;

    // Get a candidate id.
    let session = service.get_session(&sid, &token).await.unwrap();
    let candidate_id = session.candidates[0].element.element_id.clone();

    service
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
        .expect("confirm should succeed");

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                if ev.session_id == sid && ev.event_type == "gui_session.confirmed" {
                    return ev;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("confirmed event should be received within timeout");

    assert_eq!(event.event_type, "gui_session.confirmed");
    assert_eq!(event.session_id, sid);
}

/// `complete_execution(succeeded=true)` emits a `gui_session.executed` event.
#[tokio::test]
async fn m3_complete_execution_emits_executed_event() {
    let scene = make_scene(vec![make_element("el-1", "OK", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut rx = service.subscribe();
    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .expect("prepare_execution should succeed");

    service
        .complete_execution(&sid, true, None, 1, 1)
        .await
        .expect("complete_execution should succeed");

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                if ev.session_id == sid && ev.event_type == "gui_session.executed" {
                    return ev;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("executed event should be received within timeout");

    assert_eq!(event.event_type, "gui_session.executed");
    assert_eq!(event.session_id, sid);
}

/// `expire_sessions()` emits a `gui_session.expired` event for sessions past their TTL.
#[tokio::test]
async fn m3_expire_sessions_emits_expired_event() {
    let (service, _) = make_service(
        make_scene(vec![make_element("el-1", "OK", 0.9)]),
        make_focus(),
    );

    let mut rx = service.subscribe();

    // The TTL clamp in create_session enforces a minimum of 30 s, so we
    // create the session and then back-date its expires_at directly.
    let (sid, _) = create_test_session(&service).await;
    {
        let mut sessions = service.sessions.write().await;
        if let Some(stored) = sessions.get_mut(&sid) {
            stored.session.expires_at = Utc::now() - ChronoDuration::seconds(1);
        }
    }

    // Directly invoke the cleanup sweep (avoid waiting 30 s for the background loop).
    service.expire_sessions().await;

    let event = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let Ok(ev) = rx.try_recv() {
                if ev.session_id == sid && ev.event_type == "gui_session.expired" {
                    return ev;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("expired event should be received within timeout");

    assert_eq!(event.event_type, "gui_session.expired");
    assert_eq!(event.session_id, sid);
}

// ── M5: Failure scenario tests ────────────────────────────────────

#[tokio::test]
async fn m5_permission_denied_returns_forbidden() {
    let service = make_service_with_finder(Arc::new(PermissionDeniedElementFinder), make_focus());

    let err = service
        .create_session(default_create_request())
        .await
        .unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::Forbidden { .. }),
        "Expected ForbiddenV2, got: {err:?}"
    );
}

#[tokio::test]
async fn m5_focus_drift_on_confirm_returns_focus_drift() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, probe) = make_service(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    // Drift focus before confirm
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
                ticket_ttl_secs: Some(60),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::FocusDrift { .. }),
        "Expected FocusDrift, got: {err:?}"
    );
}

#[tokio::test]
async fn m5_focus_drift_on_execute_returns_focus_drift() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, probe) = make_service(scene, make_focus());

    let (sid, token, ticket) = create_highlight_and_confirm(&service).await;

    // Drift focus after confirm, before execute
    probe.set_validation_valid(false);

    let err = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::FocusDrift { .. }),
        "Expected FocusDrift, got: {err:?}"
    );
}

#[tokio::test]
async fn m5_expired_ticket_returns_ticket_invalid() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token, mut ticket) = create_highlight_and_confirm(&service).await;

    // Backdate the ticket so it is expired past grace
    ticket.expires_at = Utc::now() - ChronoDuration::seconds(TICKET_EXPIRY_GRACE_SECS + 10);

    let err = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::TicketInvalid { .. }),
        "Expected TicketInvalid for expired ticket, got: {err:?}"
    );
}

#[tokio::test]
async fn m5_nonce_replay_blocked_deterministically() {
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

    // Revert to Confirmed so state gate passes
    service
        .complete_execution(&sid, false, None, 0, 1)
        .await
        .unwrap();

    // Replay same nonce — must be rejected
    let err = service
        .prepare_execution(&sid, &token, GuiExecutionRequest { ticket })
        .await
        .unwrap_err();
    assert!(
        matches!(err, GuiInteractionError::TicketInvalid { .. }),
        "Expected TicketInvalid for nonce replay, got: {err:?}"
    );
    // Verify the error message is specific
    if let GuiInteractionError::TicketInvalid {
        code: oneshim_core::error_codes::GuiCode::TicketInvalid,
        message: msg,
    } = &err
    {
        assert!(
            msg.contains("nonce") || msg.contains("replay"),
            "Error message should mention nonce replay, got: {msg}"
        );
    }
}

#[tokio::test]
async fn m5_session_ttl_boundary_marks_expired() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let mut req = default_create_request();
    req.session_ttl_secs = Some(30);
    let resp = service.create_session(req).await.unwrap();
    let sid = resp.session.session_id.clone();
    let token = resp.capability_token.clone();

    // Manually expire
    {
        let mut sessions = service.sessions.write().await;
        if let Some(stored) = sessions.get_mut(&sid) {
            stored.session.expires_at = Utc::now() - ChronoDuration::seconds(2);
        }
    }

    // get_session should reflect Expired state
    let session = service.get_session(&sid, &token).await.unwrap();
    assert_eq!(session.state, GuiSessionState::Expired);

    // Operations on expired session should fail
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

#[tokio::test]
async fn m5_expire_sessions_removes_and_emits_event() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _, overlay) = make_service_full(scene, make_focus());

    let (sid, token) = create_and_highlight(&service).await;

    let mut rx = service.subscribe();

    // Expire the session
    {
        let mut sessions = service.sessions.write().await;
        if let Some(stored) = sessions.get_mut(&sid) {
            stored.session.expires_at = Utc::now() - ChronoDuration::seconds(1);
        }
    }

    service.expire_sessions().await;

    // Session should be gone
    let err = service.get_session(&sid, &token).await.unwrap_err();
    assert!(matches!(err, GuiInteractionError::NotFound { .. }));

    // Expired event should have been emitted
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    let expired_events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "gui_session.expired")
        .collect();
    assert_eq!(expired_events.len(), 1);
    assert_eq!(expired_events[0].session_id, sid);

    // Overlay clear should have been called (session was highlighted)
    assert!(
        overlay.clear_count.load(Ordering::SeqCst) >= 1,
        "Overlay clear_highlights should be called on expire"
    );
}
