use super::*;

// ── read_capability_token tests ─────────────────────────────────────

#[test]
fn token_header_is_enforced() {
    let headers = HeaderMap::new();
    let err = read_capability_token(&headers).unwrap_err();
    assert!(matches!(err, ApiError::Unauthorized(_)));
}

#[test]
fn token_header_rejects_empty_value() {
    let mut headers = HeaderMap::new();
    headers.insert(GUI_SESSION_HEADER, "".parse().unwrap());
    let err = read_capability_token(&headers).unwrap_err();
    assert!(matches!(err, ApiError::Unauthorized(_)));
}

#[test]
fn token_header_rejects_whitespace_only() {
    let mut headers = HeaderMap::new();
    headers.insert(GUI_SESSION_HEADER, "   ".parse().unwrap());
    let err = read_capability_token(&headers).unwrap_err();
    assert!(matches!(err, ApiError::Unauthorized(_)));
}

#[test]
fn token_header_accepts_valid_token() {
    let mut headers = HeaderMap::new();
    headers.insert(GUI_SESSION_HEADER, "abc123".parse().unwrap());
    let token = read_capability_token(&headers).unwrap();
    assert_eq!(token, "abc123");
}

#[test]
fn token_header_trims_whitespace() {
    let mut headers = HeaderMap::new();
    headers.insert(GUI_SESSION_HEADER, " tok123 ".parse().unwrap());
    let token = read_capability_token(&headers).unwrap();
    assert_eq!(token, "tok123");
}

// ── map_gui_error tests ─────────────────────────────────────────────

#[test]
fn maps_unauthorized_to_401() {
    let err = map_gui_error(GuiInteractionError::Unauthorized {
        code: oneshim_core::error_codes::GuiCode::Unauthorized,
    });
    assert!(matches!(err, ApiError::Unauthorized(_)));
}

#[test]
fn maps_not_found_to_404() {
    let err = map_gui_error(GuiInteractionError::NotFound {
        code: oneshim_core::error_codes::GuiCode::NotFound,
        name: "s1".to_string(),
    });
    assert!(matches!(err, ApiError::NotFound(_)));
}

#[test]
fn maps_bad_request_to_400() {
    let err = map_gui_error(GuiInteractionError::BadRequest {
        code: oneshim_core::error_codes::GuiCode::BadRequest,
        message: "bad".to_string(),
    });
    assert!(matches!(err, ApiError::BadRequest(_)));
}

#[test]
fn maps_forbidden_to_403() {
    let err = map_gui_error(GuiInteractionError::Forbidden {
        code: oneshim_core::error_codes::GuiCode::Forbidden,
        message: "denied".to_string(),
    });
    assert!(matches!(err, ApiError::Forbidden(_)));
}

#[test]
fn maps_focus_drift_to_409_conflict() {
    let err = map_gui_error(GuiInteractionError::FocusDrift {
        code: oneshim_core::error_codes::GuiCode::FocusDrift,
        message: "drift".to_string(),
    });
    assert!(matches!(err, ApiError::Conflict(_)));
}

#[test]
fn maps_ticket_invalid_to_422() {
    let err = map_gui_error(GuiInteractionError::TicketInvalid {
        code: oneshim_core::error_codes::GuiCode::TicketInvalid,
        message: "expired".to_string(),
    });
    assert!(matches!(err, ApiError::Unprocessable(_)));
}

#[test]
fn maps_unavailable_to_503() {
    let err = map_gui_error(GuiInteractionError::Unavailable {
        code: oneshim_core::error_codes::GuiCode::Unavailable,
        message: "down".to_string(),
    });
    assert!(matches!(err, ApiError::ServiceUnavailable(_)));
}

#[test]
fn maps_internal_to_500() {
    let err = map_gui_error(GuiInteractionError::Internal {
        code: oneshim_core::error_codes::GuiCode::InternalError,
        message: "crash".to_string(),
    });
    assert!(matches!(err, ApiError::Internal(_)));
}

// ── Schema version constant ─────────────────────────────────────────

#[test]
fn gui_schema_version_matches_core() {
    assert_eq!(
        GUI_SCHEMA_VERSION,
        oneshim_core::models::gui::GUI_INTERACTION_SCHEMA_VERSION
    );
}
