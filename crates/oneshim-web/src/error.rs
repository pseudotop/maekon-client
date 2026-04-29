use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use oneshim_api_contracts::error::ErrorResponse;
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
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ApiError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ApiError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ApiError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            ApiError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            ApiError::Unprocessable(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg.clone()),
            ApiError::ServiceUnavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.clone()),
        };

        let body = ErrorResponse {
            error: message,
            status: status.as_u16(),
        };

        (status, Json(body)).into_response()
    }
}

impl From<oneshim_core::error::CoreError> for ApiError {
    fn from(err: oneshim_core::error::CoreError) -> Self {
        use oneshim_core::error::CoreError;
        match err {
            CoreError::Validation { field, message, .. } => {
                ApiError::BadRequest(format!("{field}: {message}"))
            }
            CoreError::Auth { message, .. }
            | CoreError::ConsentRequired { message, .. }
            | CoreError::OAuthError { message, .. }
            | CoreError::OAuthRefreshError { message, .. } => ApiError::Unauthorized(message),
            CoreError::ConsentExpired { .. } => {
                ApiError::Unauthorized("consent expired".to_string())
            }
            CoreError::NotFound {
                resource_type, id, ..
            } => ApiError::NotFound(format!("{resource_type}: {id}")),
            CoreError::ServiceUnavailable { message, .. }
            | CoreError::SandboxUnsupported { message, .. } => {
                ApiError::ServiceUnavailable(message)
            }
            rate_or_timeout @ (CoreError::RateLimit { .. } | CoreError::RequestTimeout { .. }) => {
                ApiError::ServiceUnavailable(rate_or_timeout.to_string())
            }
            CoreError::PolicyDenied { message, .. }
            | CoreError::PrivacyDenied { message, .. }
            | CoreError::PermissionDenied { message, .. } => ApiError::Forbidden(message),
            CoreError::InvalidArguments { message, .. }
            | CoreError::Config { message, .. }
            | CoreError::Network { message, .. }
            | CoreError::OcrError { message, .. }
            | CoreError::SecretStoreError { message, .. }
            | CoreError::SandboxInit { message, .. }
            | CoreError::SandboxExecution { message, .. }
            | CoreError::TimeWindow { message, .. } => ApiError::BadRequest(message),
            CoreError::ElementNotFound { name, .. } => ApiError::BadRequest(name),

            other => ApiError::Internal(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display() {
        let err = ApiError::NotFound("session".to_string());
        assert!(err.to_string().contains("session"));
    }

    /// Regression guard: CoreError::PermissionDenied must map to HTTP 403
    /// Forbidden (not 500 Internal). Was dropping into the wildcard arm
    /// before iter-40 drift audit; caught by noting sibling semantic-
    /// denial variants (PolicyDenied, PrivacyDenied)
    /// already mapped to Forbidden.
    #[test]
    fn permission_denied_maps_to_forbidden() {
        let core = oneshim_core::error::CoreError::PermissionDenied {
            code: oneshim_core::error_codes::PermissionCode::PermissionDenied,
            message: "macOS Accessibility denied".to_string(),
        };
        let api: ApiError = core.into();
        assert!(
            matches!(api, ApiError::Forbidden(_)),
            "PermissionDenied must map to 403 Forbidden, got: {api:?}"
        );
    }

    /// Regression guard: CoreError::ConsentExpired must map to HTTP 401
    /// Unauthorized (not 500 Internal). Parallel to sibling ConsentRequired
    /// already mapped to 401 — both represent consent-state issues the
    /// client should re-prompt for, not server-side bugs. Caught by iter-41.
    #[test]
    fn consent_expired_maps_to_unauthorized() {
        let core = oneshim_core::error::CoreError::ConsentExpired {
            code: oneshim_core::error_codes::ConsentCode::Expired,
        };
        let api: ApiError = core.into();
        assert!(
            matches!(api, ApiError::Unauthorized(_)),
            "ConsentExpired must map to 401 Unauthorized, got: {api:?}"
        );
    }

    /// Regression guard: transient-unavailability variants (RateLimit,
    /// RequestTimeout) must map to HTTP 503 ServiceUnavailable (not 500
    /// Internal). These represent upstream-service issues the client
    /// should retry, not server-side bugs. Caught by iter-41 drift audit.
    #[test]
    fn rate_limit_and_timeout_map_to_service_unavailable() {
        let rate = oneshim_core::error::CoreError::RateLimit {
            code: oneshim_core::error_codes::NetworkCode::RateLimit,
            retry_after_secs: 30,
        };
        let api: ApiError = rate.into();
        assert!(
            matches!(api, ApiError::ServiceUnavailable(_)),
            "RateLimit must map to 503 ServiceUnavailable, got: {api:?}"
        );

        let timeout = oneshim_core::error::CoreError::RequestTimeout {
            code: oneshim_core::error_codes::NetworkCode::Timeout,
            timeout_ms: 5000,
        };
        let api: ApiError = timeout.into();
        assert!(
            matches!(api, ApiError::ServiceUnavailable(_)),
            "RequestTimeout must map to 503 ServiceUnavailable, got: {api:?}"
        );
    }

    /// Regression guard: CoreError::TimeWindow (InvertedBounds) must map to
    /// HTTP 400 BadRequest (not 500 Internal). TimeWindow validation errors
    /// reflect malformed client input (start > end), not server-side bugs.
    #[test]
    fn time_window_inverted_bounds_maps_to_bad_request() {
        let core = oneshim_core::error::CoreError::TimeWindow {
            code: oneshim_core::error_codes::TimeWindowCode::InvertedBounds,
            message: "start > end".to_string(),
        };
        let api: ApiError = core.into();
        assert!(
            matches!(api, ApiError::BadRequest(_)),
            "TimeWindow::InvertedBounds must map to 400 BadRequest, got: {api:?}"
        );
    }

    /// Regression guard: CoreError::TimeWindow (ParseFailed) must map to
    /// HTTP 400 BadRequest (not 500 Internal). RFC3339 parsing errors
    /// reflect malformed client input, not server-side bugs.
    #[test]
    fn time_window_parse_failed_maps_to_bad_request() {
        let core = oneshim_core::error::CoreError::TimeWindow {
            code: oneshim_core::error_codes::TimeWindowCode::ParseFailed,
            message: "not a date".to_string(),
        };
        let api: ApiError = core.into();
        assert!(
            matches!(api, ApiError::BadRequest(_)),
            "TimeWindow::ParseFailed must map to 400 BadRequest, got: {api:?}"
        );
    }
}
