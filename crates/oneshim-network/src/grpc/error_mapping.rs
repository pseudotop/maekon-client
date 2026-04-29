use crate::error::NetworkError;
use tonic::{Code, Status};

const DEFAULT_RETRY_AFTER_SECS: u64 = 60;

pub fn map_grpc_status_error(operation: &str, status: Status) -> NetworkError {
    let code = status.code();
    let message = status.message().to_string();

    match code {
        Code::Unauthenticated | Code::PermissionDenied => {
            NetworkError::Auth(format!("{operation}: {message}"))
        }
        Code::NotFound => NetworkError::NotFound {
            resource_type: operation.to_string(),
            id: message,
        },
        Code::InvalidArgument | Code::FailedPrecondition | Code::OutOfRange => {
            NetworkError::Validation {
                field: "grpc_request".to_string(),
                message: format!("{operation}: {message} ({code})"),
            }
        }
        Code::ResourceExhausted => NetworkError::RateLimited {
            retry_after_secs: extract_retry_after_secs(&status),
        },
        Code::Unavailable => NetworkError::ServiceUnavailable(format!("{operation}: {message}")),
        Code::DeadlineExceeded => NetworkError::Timeout {
            // gRPC client-side deadline elapsed. We don't know the exact timeout
            // value from Status alone, so use 0 as a sentinel; actual request
            // timeout is already logged at request-site.
            timeout_ms: 0,
        },
        Code::Unimplemented => NetworkError::NotFound {
            // Server doesn't implement this RPC — semantically the RPC resource
            // is missing, not a generic HTTP failure. Non-retryable (server
            // won't grow the method on retry); wire code `not_found.resource_missing`
            // helps telemetry isolate client/server version-skew scenarios.
            resource_type: format!("grpc_method:{operation}"),
            id: message,
        },
        // Iter-92: previously these codes fell into the Http wildcard, losing
        // their semantic signal for telemetry and retry decisions.
        Code::Internal | Code::DataLoss => {
            NetworkError::Internal(format!("{operation}: server-side {code} — {message}"))
        }
        Code::AlreadyExists => NetworkError::Validation {
            // Conflict — client attempted to create an entity that already
            // exists. Non-retryable; semantically a client-side request
            // failure surfaced as validation.
            field: "grpc_request".to_string(),
            message: format!("{operation}: already exists — {message}"),
        },
        Code::Aborted => NetworkError::ServiceUnavailable(format!(
            "{operation}: transaction aborted (concurrency) — {message}"
        )),
        _ => NetworkError::Http(format!("{operation}: {message} ({code})")),
    }
}

fn extract_retry_after_secs(status: &Status) -> u64 {
    status
        .metadata()
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            status
                .metadata()
                .get("x-retry-after-seconds")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(DEFAULT_RETRY_AFTER_SECS)
}

#[cfg(test)]
mod tests {
    use tonic::Status;

    use super::*;

    #[test]
    fn maps_unauthenticated_to_auth_error() {
        let err = map_grpc_status_error("grpc login", Status::unauthenticated("bad token"));
        assert!(matches!(err, NetworkError::Auth(_)));
    }

    #[test]
    fn maps_invalid_argument_to_validation_error() {
        let err = map_grpc_status_error("grpc upload", Status::invalid_argument("missing id"));

        match err {
            NetworkError::Validation { field, message } => {
                assert_eq!(field, "grpc_request");
                assert!(message.contains("missing id"));
            }
            _ => panic!("expected validation error"),
        }
    }

    #[test]
    fn maps_resource_exhausted_to_rate_limit_with_default_retry() {
        let err = map_grpc_status_error("grpc list", Status::resource_exhausted("busy"));
        assert!(matches!(
            err,
            NetworkError::RateLimited {
                retry_after_secs: DEFAULT_RETRY_AFTER_SECS
            }
        ));
    }

    #[test]
    fn maps_unavailable_to_service_unavailable() {
        let err = map_grpc_status_error("grpc heartbeat", Status::unavailable("down"));
        assert!(matches!(err, NetworkError::ServiceUnavailable(_)));
    }

    /// iter-52 regression guard: gRPC Code::DeadlineExceeded is a client-
    /// observed timeout and must map to NetworkError::Timeout (not fall into
    /// the Http wildcard). This preserves retry semantics — Timeout is
    /// explicitly retryable via is_retryable, Http is retryable-ambiguous,
    /// and downstream CoreError wire code `network.timeout` vs `network.generic`
    /// lets telemetry isolate timeout issues from generic HTTP failures.
    #[test]
    fn maps_deadline_exceeded_to_timeout() {
        let err =
            map_grpc_status_error("grpc streaming", Status::deadline_exceeded("took too long"));
        assert!(
            matches!(err, NetworkError::Timeout { .. }),
            "DeadlineExceeded must map to NetworkError::Timeout, got: {err:?}"
        );
    }

    /// iter-53 regression guard: gRPC Code::Unimplemented means the server
    /// doesn't implement the RPC — semantically the method is missing.
    /// Maps to NetworkError::NotFound so downstream CoreError is
    /// `not_found.resource_missing` (not retryable; clearly signals
    /// client/server version skew in telemetry).
    #[test]
    fn maps_unimplemented_to_not_found() {
        let err = map_grpc_status_error(
            "CreateSession",
            Status::unimplemented("method not found on server"),
        );
        match err {
            NetworkError::NotFound { resource_type, id } => {
                assert!(
                    resource_type.contains("CreateSession"),
                    "resource_type should mention the operation, got: {resource_type}"
                );
                assert!(
                    id.contains("method not found"),
                    "id should preserve server message, got: {id}"
                );
            }
            other => panic!("Unimplemented must map to NetworkError::NotFound, got: {other:?}"),
        }
    }

    /// Iter-92 regression guard: Code::Internal means the server had an
    /// internal error — wire this through NetworkError::Internal (wire code
    /// `internal.generic`) so telemetry can distinguish server-internal from
    /// generic HTTP failures. Pre-iter-92 this fell into the Http wildcard
    /// (wire code `network.generic`), conflating the two.
    #[test]
    fn maps_internal_to_internal_error() {
        let err = map_grpc_status_error("Rpc", Status::internal("db connection lost"));
        match err {
            NetworkError::Internal(msg) => {
                assert!(msg.contains("Rpc"), "expected operation in msg, got: {msg}");
                assert!(msg.contains("db connection"), "expected detail, got: {msg}");
            }
            other => panic!("Code::Internal must map to NetworkError::Internal, got: {other:?}"),
        }
    }

    /// Iter-92 regression guard: Code::DataLoss is catastrophic. Route
    /// through NetworkError::Internal (not Http wildcard) so downstream
    /// wire code is `internal.generic` and alerts can fire on frequency.
    #[test]
    fn maps_data_loss_to_internal_error() {
        let err = map_grpc_status_error("Rpc", Status::data_loss("storage corruption detected"));
        assert!(
            matches!(err, NetworkError::Internal(_)),
            "Code::DataLoss must map to NetworkError::Internal, got: {err:?}"
        );
    }

    /// Iter-92 regression guard: Code::AlreadyExists is a client-side
    /// conflict — entity being created already exists. Maps to Validation
    /// so wire code signals a request-error (non-retryable) distinct from
    /// generic HTTP failures.
    #[test]
    fn maps_already_exists_to_validation_error() {
        let err = map_grpc_status_error("CreateUser", Status::already_exists("user id taken"));
        match err {
            NetworkError::Validation { field, message } => {
                assert_eq!(field, "grpc_request");
                assert!(
                    message.contains("already exists"),
                    "expected 'already exists' in message, got: {message}"
                );
            }
            other => {
                panic!("Code::AlreadyExists must map to NetworkError::Validation, got: {other:?}")
            }
        }
    }

    /// Iter-92 regression guard: Code::Aborted is a transient concurrency
    /// failure (typically optimistic-concurrency retry). Maps to
    /// ServiceUnavailable so downstream retry logic treats it as
    /// retryable-with-backoff (wire code `service.unavailable`).
    #[test]
    fn maps_aborted_to_service_unavailable() {
        let err = map_grpc_status_error("UpdateSession", Status::aborted("txn conflict"));
        assert!(
            matches!(err, NetworkError::ServiceUnavailable(_)),
            "Code::Aborted must map to NetworkError::ServiceUnavailable, got: {err:?}"
        );
    }
}
