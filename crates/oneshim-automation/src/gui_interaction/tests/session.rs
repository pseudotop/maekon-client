use super::*;

// ── Utility tests ───────────────────────────────────────────────────

#[test]
fn hex_roundtrip() {
    let data = b"hello";
    let encoded = encode_hex(data);
    let decoded = decode_hex(&encoded).unwrap();
    assert_eq!(decoded, data);
}

#[test]
fn decode_hex_rejects_invalid_length() {
    assert!(decode_hex("abc").is_none());
}

// ── Session creation tests ──────────────────────────────────────────

#[tokio::test]
async fn create_session_returns_proposed_state() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let resp = service
        .create_session(default_create_request())
        .await
        .unwrap();

    assert_eq!(resp.session.state, GuiSessionState::Proposed);
    assert!(!resp.capability_token.is_empty());
    assert!(!resp.session.session_id.is_empty());
    assert_eq!(resp.session.candidates.len(), 1);
    assert_eq!(resp.session.candidates[0].element.label, "Save");
}

#[tokio::test]
async fn create_session_filters_low_confidence_candidates() {
    let scene = make_scene(vec![
        make_element("el-high", "Save", 0.9),
        make_element("el-low", "Cancel", 0.2),
    ]);
    let (service, _) = make_service(scene, make_focus());

    let resp = service
        .create_session(default_create_request())
        .await
        .unwrap();

    assert_eq!(resp.session.candidates.len(), 1);
    assert_eq!(resp.session.candidates[0].element.element_id, "el-high");
}

#[tokio::test]
async fn create_session_respects_max_candidates() {
    let elements: Vec<UiSceneElement> = (0..10)
        .map(|i| make_element(&format!("el-{i}"), &format!("Btn{i}"), 0.8))
        .collect();
    let scene = make_scene(elements);
    let (service, _) = make_service(scene, make_focus());

    let mut req = default_create_request();
    req.max_candidates = Some(3);

    let resp = service.create_session(req).await.unwrap();
    assert_eq!(resp.session.candidates.len(), 3);
}

#[tokio::test]
async fn create_session_rejects_empty_scene() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.1)]);
    let (service, _) = make_service(scene, make_focus());

    let mut req = default_create_request();
    req.min_confidence = Some(0.99);

    let err = service.create_session(req).await.unwrap_err();
    assert!(matches!(err, GuiInteractionError::BadRequest { .. }));
}

#[tokio::test]
async fn create_session_requires_hmac_secret() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let service = GuiInteractionService::new(
        Arc::new(MockElementFinder::new(scene)),
        Arc::new(MockFocusProbe::new(make_focus())),
        Arc::new(MockOverlayDriver::new()),
        None, // no HMAC secret
    );

    let err = service
        .create_session(default_create_request())
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::Unavailable { .. }));
}

// ── Get session tests ───────────────────────────────────────────────

#[tokio::test]
async fn get_session_returns_current_state() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_test_session(&service).await;
    let session = service.get_session(&sid, &token).await.unwrap();

    assert_eq!(session.state, GuiSessionState::Proposed);
    assert_eq!(session.session_id, sid);
}

#[tokio::test]
async fn get_session_rejects_invalid_token() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, _) = create_test_session(&service).await;
    let err = service.get_session(&sid, "wrong-token").await.unwrap_err();
    assert!(matches!(err, GuiInteractionError::Unauthorized { .. }));
}

#[tokio::test]
async fn get_session_rejects_unknown_session() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let err = service
        .get_session("nonexistent", "some-token")
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::NotFound { .. }));
}
