use chrono::{DateTime, Duration as ChronoDuration, Utc};

use oneshim_core::error::CoreError;
use oneshim_core::models::gui::{GuiActionType, GuiCandidate};
use oneshim_core::models::ui_scene::UiScene;

use super::types::GuiInteractionError;
use crate::controller::AutomationAction;

#[tracing::instrument(skip_all, fields(element_count = scene.elements.len()))]
pub(super) fn build_candidates(
    scene: &UiScene,
    min_confidence: f64,
    max_candidates: usize,
) -> Vec<GuiCandidate> {
    let mut candidates: Vec<GuiCandidate> = scene
        .elements
        .iter()
        .filter(|element| element.confidence >= min_confidence)
        .map(|element| GuiCandidate {
            element: element.clone(),
            ranking_reason: Some(format!("confidence={:.2}", element.confidence)),
            eligible: true,
        })
        .collect();

    candidates.sort_by(|a, b| {
        b.element
            .confidence
            .partial_cmp(&a.element.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(max_candidates);
    candidates
}

pub(super) fn build_actions_for_candidate(
    candidate: &GuiCandidate,
    action: &oneshim_core::models::gui::GuiActionRequest,
) -> Result<Vec<AutomationAction>, GuiInteractionError> {
    let center_x = candidate.element.bbox_abs.x + (candidate.element.bbox_abs.width as i32 / 2);
    let center_y = candidate.element.bbox_abs.y + (candidate.element.bbox_abs.height as i32 / 2);

    let actions = match action.action_type {
        GuiActionType::Click => vec![AutomationAction::MouseClick {
            button: "left".to_string(),
            x: center_x,
            y: center_y,
        }],
        GuiActionType::DoubleClick => vec![
            AutomationAction::MouseClick {
                button: "left".to_string(),
                x: center_x,
                y: center_y,
            },
            AutomationAction::MouseClick {
                button: "left".to_string(),
                x: center_x,
                y: center_y,
            },
        ],
        GuiActionType::RightClick => vec![AutomationAction::MouseClick {
            button: "right".to_string(),
            x: center_x,
            y: center_y,
        }],
        GuiActionType::TypeText => {
            let text = action
                .text
                .clone()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| GuiInteractionError::BadRequest {
                    code: oneshim_core::error_codes::GuiCode::BadRequest,
                    message: "type_text action requires non-empty text".to_string(),
                })?;
            vec![
                AutomationAction::MouseClick {
                    button: "left".to_string(),
                    x: center_x,
                    y: center_y,
                },
                AutomationAction::KeyType { text },
            ]
        }
    };

    Ok(actions)
}

pub(super) fn is_expired(expires_at: &DateTime<Utc>) -> bool {
    *expires_at <= Utc::now()
}

pub(super) fn is_expired_past_grace(expires_at: &DateTime<Utc>, grace_secs: i64) -> bool {
    *expires_at + ChronoDuration::seconds(grace_secs) <= Utc::now()
}

pub(super) fn map_core_error(err: CoreError) -> GuiInteractionError {
    // Iter-91: expanded semantic mapping. Pre-iter-91 this match only handled
    // PolicyDenied/PrivacyDenied (Forbidden), InvalidArguments/ElementNotFound
    // (BadRequest), and ServiceUnavailable/SandboxUnsupported/SandboxInit
    // (Unavailable). Everything else fell through to Internal, labelling auth
    // failures, permission denials, consent expiries, timeouts, and rate
    // limits all as `gui.internal_error`. Frontend i18n cannot distinguish
    // them via code alone — it has to substring-match the message. Fix: route
    // each into the semantically correct GuiInteractionError variant so
    // `err.code()` carries the denial/timeout/unauthorized intent.
    match err {
        // Auth failures surface the auth-domain wire code. The Unauthorized
        // variant is message-less by design (session token invalid); the
        // original message is retained on CoreError before the conversion
        // for server-side logging but intentionally not forwarded to the
        // frontend to avoid leaking token details.
        CoreError::Auth { .. } => GuiInteractionError::Unauthorized {
            code: oneshim_core::error_codes::GuiCode::Unauthorized,
        },
        CoreError::PolicyDenied { message: msg, .. }
        | CoreError::PrivacyDenied { message: msg, .. }
        | CoreError::PermissionDenied { message: msg, .. }
        | CoreError::ConsentRequired { message: msg, .. } => GuiInteractionError::Forbidden {
            code: oneshim_core::error_codes::GuiCode::Forbidden,
            message: msg,
        },
        CoreError::ConsentExpired { .. } => GuiInteractionError::Forbidden {
            code: oneshim_core::error_codes::GuiCode::Forbidden,
            message: "consent expired — re-authorization required".to_string(),
        },
        CoreError::InvalidArguments { message: msg, .. } => GuiInteractionError::BadRequest {
            code: oneshim_core::error_codes::GuiCode::BadRequest,
            message: msg,
        },
        CoreError::ElementNotFound { name: msg, .. } => GuiInteractionError::BadRequest {
            code: oneshim_core::error_codes::GuiCode::BadRequest,
            message: msg,
        },
        CoreError::ServiceUnavailable { message: msg, .. }
        | CoreError::SandboxUnsupported { message: msg, .. }
        | CoreError::SandboxInit { message: msg, .. } => GuiInteractionError::Unavailable {
            code: oneshim_core::error_codes::GuiCode::Unavailable,
            message: msg,
        },
        // Timeouts and rate-limits are transient availability issues — the
        // GUI runtime is effectively unavailable right now but may recover.
        CoreError::RequestTimeout { timeout_ms, .. } => GuiInteractionError::Unavailable {
            code: oneshim_core::error_codes::GuiCode::Unavailable,
            message: format!("request timed out after {timeout_ms}ms"),
        },
        CoreError::RateLimit {
            retry_after_secs, ..
        } => GuiInteractionError::Unavailable {
            code: oneshim_core::error_codes::GuiCode::Unavailable,
            message: format!("rate limited; retry after {retry_after_secs}s"),
        },
        other => GuiInteractionError::Internal {
            code: oneshim_core::error_codes::GuiCode::InternalError,
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod map_core_error_tests {
    use super::*;
    use oneshim_core::error_codes::{
        AuthCode, ConsentCode, NetworkCode, PermissionCode, PolicyCode, SandboxCode, ServiceCode,
        ValidationCode,
    };

    /// Regression guard (iter-91): CoreError::Auth must map to
    /// GuiInteractionError::Unauthorized with wire code `gui.unauthorized`,
    /// not fall through to `gui.internal_error`.
    #[test]
    fn auth_maps_to_unauthorized() {
        let err = CoreError::Auth {
            code: AuthCode::Failed,
            message: "token rejected".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.unauthorized");
    }

    /// Regression guard (iter-91): PermissionDenied joins PolicyDenied +
    /// PrivacyDenied under Forbidden instead of falling through to Internal.
    #[test]
    fn permission_denied_maps_to_forbidden() {
        let err = CoreError::PermissionDenied {
            code: PermissionCode::PermissionDenied,
            message: "Accessibility not granted".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.forbidden");
    }

    /// Regression guard (iter-91): ConsentRequired is a denial variant and
    /// must share the Forbidden bucket (gui.forbidden).
    #[test]
    fn consent_required_maps_to_forbidden() {
        let err = CoreError::ConsentRequired {
            code: ConsentCode::Required,
            message: "consent pending".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.forbidden");
    }

    /// Regression guard (iter-91): ConsentExpired is also a denial — user
    /// needs to re-authorize.
    #[test]
    fn consent_expired_maps_to_forbidden() {
        let err = CoreError::ConsentExpired {
            code: ConsentCode::Expired,
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.forbidden");
    }

    /// Regression guard (iter-91): RequestTimeout is transient unavailability
    /// (gui.unavailable), not a GUI runtime internal error.
    #[test]
    fn request_timeout_maps_to_unavailable() {
        let err = CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 5_000,
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.unavailable");
    }

    /// Regression guard (iter-91): RateLimit is transient unavailability.
    #[test]
    fn rate_limit_maps_to_unavailable() {
        let err = CoreError::RateLimit {
            code: NetworkCode::RateLimit,
            retry_after_secs: 30,
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.unavailable");
    }

    /// Pre-iter-91 mappings that were already correct — guard to make sure
    /// we did not regress them while expanding coverage.
    #[test]
    fn policy_denied_still_maps_to_forbidden() {
        let err = CoreError::PolicyDenied {
            code: PolicyCode::Denied,
            message: "policy blocks action".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.forbidden");
    }

    #[test]
    fn invalid_arguments_still_maps_to_bad_request() {
        let err = CoreError::InvalidArguments {
            code: ValidationCode::InvalidArguments,
            message: "missing field".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.bad_request");
    }

    #[test]
    fn service_unavailable_still_maps_to_unavailable() {
        let err = CoreError::ServiceUnavailable {
            code: ServiceCode::Unavailable,
            message: "downstream down".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.unavailable");
    }

    #[test]
    fn sandbox_unsupported_still_maps_to_unavailable() {
        let err = CoreError::SandboxUnsupported {
            code: SandboxCode::UnsupportedPlatform,
            message: "platform has no sandbox".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.unavailable");
    }

    /// Catch-all arm: truly unmapped CoreError variants must still surface
    /// as GUI internal (gui.internal_error) — not a panic / not a swallowed
    /// error. Uses CoreError::Storage as a stand-in since it has no semantic
    /// peer in GuiInteractionError.
    #[test]
    fn unmapped_variant_falls_through_to_internal() {
        let err = CoreError::Storage {
            code: oneshim_core::error_codes::StorageCode::Failed,
            message: "disk I/O".into(),
        };
        let gui = map_core_error(err);
        assert_eq!(gui.code(), "gui.internal_error");
    }
}
