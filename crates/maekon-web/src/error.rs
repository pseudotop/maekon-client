use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use maekon_api_contracts::error::ErrorResponse;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("Internal server error: {0}")]
    Internal(String),

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    #[error("Unauthorized request: {0}")]
    Unauthorized(String),

    #[error("Forbidden request: {0}")]
    Forbidden(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Unprocessable request: {0}")]
    Unprocessable(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("{message}")]
    Coded {
        status: u16,
        code: String,
        message: String,
    },
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            ApiError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal.generic".to_string(),
                msg.clone(),
            ),
            ApiError::NotFound(msg) => (
                StatusCode::NOT_FOUND,
                "not_found.resource_missing".to_string(),
                msg.clone(),
            ),
            ApiError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "validation.invalid_arguments".to_string(),
                msg.clone(),
            ),
            ApiError::Unauthorized(msg) => (
                StatusCode::UNAUTHORIZED,
                "auth.failed".to_string(),
                msg.clone(),
            ),
            ApiError::Forbidden(msg) => (
                StatusCode::FORBIDDEN,
                "permission.permission_denied".to_string(),
                msg.clone(),
            ),
            ApiError::Conflict(msg) => (
                StatusCode::CONFLICT,
                "conflict.resource_state".to_string(),
                msg.clone(),
            ),
            ApiError::Unprocessable(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation.invalid_arguments".to_string(),
                msg.clone(),
            ),
            ApiError::ServiceUnavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "service.unavailable".to_string(),
                msg.clone(),
            ),
            ApiError::Coded {
                status,
                code,
                message,
            } => (
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                code.clone(),
                message.clone(),
            ),
        };

        let body = ErrorResponse {
            code,
            error: message,
            status: status.as_u16(),
        };

        (status, Json(body)).into_response()
    }
}

impl From<maekon_core::error::CoreError> for ApiError {
    fn from(err: maekon_core::error::CoreError) -> Self {
        use maekon_core::error::CoreError;
        let code = err.code().to_string();
        match err {
            CoreError::Validation { field, message, .. } => ApiError::Coded {
                status: StatusCode::BAD_REQUEST.as_u16(),
                code,
                message: format!("{field}: {message}"),
            },
            CoreError::Auth { message, .. }
            | CoreError::ConsentRequired { message, .. }
            | CoreError::OAuthError { message, .. }
            | CoreError::OAuthRefreshError { message, .. } => ApiError::Coded {
                status: StatusCode::UNAUTHORIZED.as_u16(),
                code,
                message,
            },
            CoreError::ConsentExpired { .. } => ApiError::Coded {
                status: StatusCode::UNAUTHORIZED.as_u16(),
                code,
                message: "consent expired".to_string(),
            },
            CoreError::NotFound {
                resource_type, id, ..
            } => ApiError::Coded {
                status: StatusCode::NOT_FOUND.as_u16(),
                code,
                message: format!("{resource_type}: {id}"),
            },
            CoreError::ServiceUnavailable { message, .. }
            | CoreError::SandboxUnsupported { message, .. }
            // #6280: a network/transport failure (upstream unreachable, connection
            // reset, etc.) is NOT a client error — map it to 503 alongside
            // RateLimit/RequestTimeout, not 400 BadRequest.
            | CoreError::Network { message, .. } => ApiError::Coded {
                status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                code,
                message,
            },
            rate_or_timeout @ (CoreError::RateLimit { .. } | CoreError::RequestTimeout { .. }) => {
                ApiError::Coded {
                    status: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                    code,
                    message: rate_or_timeout.to_string(),
                }
            }
            CoreError::PolicyDenied { message, .. }
            | CoreError::PrivacyDenied { message, .. }
            | CoreError::PermissionDenied { message, .. } => ApiError::Coded {
                status: StatusCode::FORBIDDEN.as_u16(),
                code,
                message,
            },
            CoreError::InvalidArguments { message, .. }
            | CoreError::Config { message, .. }
            | CoreError::OcrError { message, .. }
            | CoreError::SecretStoreError { message, .. }
            | CoreError::SandboxInit { message, .. }
            | CoreError::SandboxExecution { message, .. }
            | CoreError::TimeWindow { message, .. } => ApiError::Coded {
                status: StatusCode::BAD_REQUEST.as_u16(),
                code,
                message,
            },
            CoreError::ElementNotFound { name, .. } => ApiError::Coded {
                status: StatusCode::BAD_REQUEST.as_u16(),
                code,
                message: name,
            },

            other => ApiError::Coded {
                status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
                code,
                message: other.to_string(),
            },
        }
    }
}

impl From<maekon_core::models::ai_session::SessionInputLimitError> for ApiError {
    fn from(err: maekon_core::models::ai_session::SessionInputLimitError) -> Self {
        ApiError::Coded {
            status: StatusCode::PAYLOAD_TOO_LARGE.as_u16(),
            code: err.code.to_string(),
            message: err.message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_coded(api: ApiError, expected_status: StatusCode, expected_code: &str) {
        match api {
            ApiError::Coded { status, code, .. } => {
                assert_eq!(status, expected_status.as_u16());
                assert_eq!(code, expected_code);
            }
            other => panic!("expected coded ApiError, got: {other:?}"),
        }
    }

    #[test]
    fn error_display() {
        let err = ApiError::NotFound("session".to_string());
        assert!(err.to_string().contains("session"));
    }

    #[tokio::test]
    async fn conflict_response_uses_conflict_wire_code() {
        let response = ApiError::Conflict("preset already exists".to_string()).into_response();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("conflict response body must be readable");
        let body: serde_json::Value =
            serde_json::from_slice(&body).expect("conflict response body must be JSON");

        assert_eq!(body["status"], StatusCode::CONFLICT.as_u16());
        assert_eq!(body["code"], "conflict.resource_state");
        assert_eq!(body["error"], "preset already exists");
    }

    /// Regression guard: CoreError::PermissionDenied must map to HTTP 403
    /// Forbidden (not 500 Internal). Was dropping into the wildcard arm
    /// before iter-40 drift audit; caught by noting sibling semantic-
    /// denial variants (PolicyDenied, PrivacyDenied)
    /// already mapped to Forbidden.
    #[test]
    fn permission_denied_maps_to_forbidden() {
        let core = maekon_core::error::CoreError::PermissionDenied {
            code: maekon_core::error_codes::PermissionCode::PermissionDenied,
            message: "macOS Accessibility denied".to_string(),
        };
        let api: ApiError = core.into();
        assert_coded(api, StatusCode::FORBIDDEN, "permission.permission_denied");
    }

    /// Regression guard: CoreError::ConsentExpired must map to HTTP 401
    /// Unauthorized (not 500 Internal). Parallel to sibling ConsentRequired
    /// already mapped to 401 — both represent consent-state issues the
    /// client should re-prompt for, not server-side bugs. Caught by iter-41.
    #[test]
    fn consent_expired_maps_to_unauthorized() {
        let core = maekon_core::error::CoreError::ConsentExpired {
            code: maekon_core::error_codes::ConsentCode::Expired,
        };
        let api: ApiError = core.into();
        assert_coded(api, StatusCode::UNAUTHORIZED, "consent.expired");
    }

    /// Regression guard: transient-unavailability variants (RateLimit,
    /// RequestTimeout) must map to HTTP 503 ServiceUnavailable (not 500
    /// Internal). These represent upstream-service issues the client
    /// should retry, not server-side bugs. Caught by iter-41 drift audit.
    #[test]
    fn rate_limit_and_timeout_map_to_service_unavailable() {
        let rate = maekon_core::error::CoreError::RateLimit {
            code: maekon_core::error_codes::NetworkCode::RateLimit,
            retry_after_secs: 30,
        };
        let api: ApiError = rate.into();
        assert_coded(api, StatusCode::SERVICE_UNAVAILABLE, "network.rate_limit");

        let timeout = maekon_core::error::CoreError::RequestTimeout {
            code: maekon_core::error_codes::NetworkCode::Timeout,
            timeout_ms: 5000,
        };
        let api: ApiError = timeout.into();
        assert_coded(api, StatusCode::SERVICE_UNAVAILABLE, "network.timeout");
    }

    /// #6280 regression guard: CoreError::Network (transport/upstream failure)
    /// must map to HTTP 503 ServiceUnavailable, not 400 BadRequest — it is not a
    /// client error. Parallel to RateLimit/RequestTimeout above.
    #[test]
    fn network_error_maps_to_service_unavailable() {
        let core = maekon_core::error::CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: "connection reset by upstream".to_string(),
        };
        let api: ApiError = core.into();
        assert_coded(api, StatusCode::SERVICE_UNAVAILABLE, "network.generic");
    }

    /// Regression guard: CoreError::TimeWindow (InvertedBounds) must map to
    /// HTTP 400 BadRequest (not 500 Internal). TimeWindow validation errors
    /// reflect malformed client input (start > end), not server-side bugs.
    #[test]
    fn time_window_inverted_bounds_maps_to_bad_request() {
        let core = maekon_core::error::CoreError::TimeWindow {
            code: maekon_core::error_codes::TimeWindowCode::InvertedBounds,
            message: "start > end".to_string(),
        };
        let api: ApiError = core.into();
        assert_coded(api, StatusCode::BAD_REQUEST, "time_window.inverted_bounds");
    }

    /// Regression guard: CoreError::TimeWindow (ParseFailed) must map to
    /// HTTP 400 BadRequest (not 500 Internal). RFC3339 parsing errors
    /// reflect malformed client input, not server-side bugs.
    #[test]
    fn time_window_parse_failed_maps_to_bad_request() {
        let core = maekon_core::error::CoreError::TimeWindow {
            code: maekon_core::error_codes::TimeWindowCode::ParseFailed,
            message: "not a date".to_string(),
        };
        let api: ApiError = core.into();
        assert_coded(api, StatusCode::BAD_REQUEST, "time_window.parse_failed");
    }
}
