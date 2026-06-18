//! Tests for the MAX_SESSIONS cap in `create_session`.
//!
//! Verifies that:
//! - Sessions below the cap succeed.
//! - The (MAX_SESSIONS + 1)-th call returns `GuiInteractionError::BadRequest`
//!   with code `gui.bad_request`.
//! - Expired sessions are purged before the cap is enforced so that they do
//!   not count against live callers.

use super::*;

/// The (MAX_SESSIONS + 1)-th `create_session` call on a service whose
/// sessions map is already full must be rejected with `BadRequest`.
#[tokio::test]
async fn create_session_rejects_when_cap_is_reached() {
    let scene = make_scene(vec![make_element("el-1", "OK", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    // Fill exactly MAX_SESSIONS slots.
    for _ in 0..MAX_SESSIONS {
        service
            .create_session(default_create_request())
            .await
            .expect("session below cap must succeed");
    }

    // The next call must fail.
    let err = service
        .create_session(default_create_request())
        .await
        .expect_err("session beyond cap must be rejected");

    match err {
        GuiInteractionError::BadRequest { code, ref message } => {
            assert_eq!(code.as_str(), "gui.bad_request");
            assert!(
                message.contains("Session limit"),
                "error message should mention the limit; got: {message}"
            );
        }
        other => panic!("expected BadRequest, got: {other:?}"),
    }
}

/// Expired sessions must not count against the cap: after all sessions
/// expire the next `create_session` call must succeed even though the map
/// previously held MAX_SESSIONS entries.
#[tokio::test]
async fn create_session_succeeds_after_expired_sessions_are_purged() {
    let scene = make_scene(vec![make_element("el-1", "OK", 0.9)]);
    let (service, _) = make_service(scene, make_focus());

    // Fill all slots with a minimal TTL (30 s is the enforced minimum).  We
    // cannot travel time in unit tests, so instead we write expired entries
    // directly into the sessions map to simulate the state that the cleanup
    // task would normally produce.
    {
        let mut sessions = service.sessions.write().await;
        for i in 0..MAX_SESSIONS {
            let id = format!("fake-session-{i}");
            let now = Utc::now();
            let expired_at = now - ChronoDuration::seconds(1);
            sessions.insert(
                id.clone(),
                crate::gui_interaction::types::StoredSession {
                    session: maekon_core::models::gui::GuiInteractionSession {
                        schema_version: "gui_interaction.v1".to_string(),
                        session_id: id,
                        state: maekon_core::models::gui::GuiSessionState::Proposed,
                        scene: maekon_core::models::ui_scene::UiScene {
                            schema_version: "ui_scene.v1".to_string(),
                            scene_id: format!("scene-{i}"),
                            app_name: None,
                            screen_id: None,
                            captured_at: now,
                            screen_width: 1920,
                            screen_height: 1080,
                            elements: vec![],
                        },
                        focus: maekon_core::models::gui::FocusSnapshot {
                            app_name: "X".to_string(),
                            window_title: "X".to_string(),
                            pid: 0,
                            bounds: None,
                            captured_at: now,
                            focus_hash: "x".to_string(),
                        },
                        candidates: vec![],
                        selected_element_id: None,
                        created_at: now,
                        updated_at: now,
                        expires_at: expired_at,
                    },
                    capability_token: "dummy-token".to_string(),
                    overlay_handle_id: None,
                    confirmed_action: None,
                    used_ticket_nonces: std::collections::HashSet::new(),
                },
            );
        }
    }

    // All MAX_SESSIONS slots are now occupied by already-expired sessions.
    // create_session must purge them first (lazy expiry) and then succeed.
    service
        .create_session(default_create_request())
        .await
        .expect("create_session must succeed after expired sessions are purged");
}
