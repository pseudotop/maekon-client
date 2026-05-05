use super::*;

// ── Highlight tests ─────────────────────────────────────────────────

#[tokio::test]
async fn highlight_transitions_to_highlighted() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_test_session(&service).await;
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
}

#[tokio::test]
async fn highlight_filters_by_candidate_ids() {
    let scene = make_scene(vec![
        make_element("el-1", "Save", 0.9),
        make_element("el-2", "Cancel", 0.8),
    ]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, token) = create_test_session(&service).await;

    // highlight only el-1
    let session = service
        .highlight_session(
            &sid,
            &token,
            GuiHighlightRequest {
                candidate_ids: Some(vec!["el-1".to_string()]),
            },
        )
        .await
        .unwrap();

    assert_eq!(session.state, GuiSessionState::Highlighted);
}

#[tokio::test]
async fn highlight_rejects_invalid_token() {
    let scene = make_scene(vec![make_element("el-1", "Save", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    let (sid, _) = create_test_session(&service).await;
    let err = service
        .highlight_session(
            &sid,
            "wrong-token",
            GuiHighlightRequest {
                candidate_ids: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, GuiInteractionError::Unauthorized { .. }));
}
