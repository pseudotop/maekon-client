//! Authenticated Maekon→Console pending handoff transport (#9628).
//!
//! The POST has no body, query or identity parameter. The existing shared
//! `TokenManager` supplies the bearer inside Rust and the server derives both
//! actor and organization from it.

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::{AuthCode, NetworkCode, PolicyCode, ServiceCode, ValidationCode};
use maekon_core::models::console_handoff::ConsoleHandoffIssue;
use maekon_core::ports::console_handoff_client::ConsoleHandoffClient;
use maekon_http_core::outbound::{read_text_capped, BodyReadError};
use maekon_http_core::resilience::extract_retry_after;

use crate::http_client::HttpApiClient;

pub const CONSOLE_HANDOFF_ISSUE_PATH: &str = "/api/v1/user-context/context-home/console-handoffs";
pub const MAX_CONSOLE_HANDOFF_BYTES: u64 = 16 * 1024;

#[async_trait]
impl ConsoleHandoffClient for HttpApiClient {
    async fn issue_console_handoff(&self) -> Result<ConsoleHandoffIssue, CoreError> {
        let request = self
            .authorized_request(reqwest::Method::POST, CONSOLE_HANDOFF_ISSUE_PATH)
            .await
            .map_err(CoreError::from)?;
        let response = request.send().await.map_err(|error| {
            if error.is_timeout() {
                CoreError::RequestTimeout {
                    code: NetworkCode::Timeout,
                    timeout_ms: 0,
                }
            } else {
                CoreError::Network {
                    code: NetworkCode::Generic,
                    message: format!("console handoff request failed: {error}"),
                }
            }
        })?;

        let status = response.status();
        if !status.is_success() {
            return Err(map_failure_status(
                status.as_u16(),
                extract_retry_after(&response),
            ));
        }

        let body = read_text_capped(response, MAX_CONSOLE_HANDOFF_BYTES)
            .await
            .map_err(|error| match error {
                BodyReadError::TooLarge { len, cap } => CoreError::Validation {
                    code: ValidationCode::InvalidField,
                    field: "body".to_string(),
                    message: format!("console handoff response too large ({len} > {cap} bytes)"),
                },
                BodyReadError::Transport(error) if error.is_timeout() => {
                    CoreError::RequestTimeout {
                        code: NetworkCode::Timeout,
                        timeout_ms: 0,
                    }
                }
                BodyReadError::Transport(error) => CoreError::Network {
                    code: NetworkCode::Generic,
                    message: format!("console handoff response read failed: {error}"),
                },
            })?;
        let receipt: ConsoleHandoffIssue =
            serde_json::from_str(&body).map_err(|error| CoreError::Validation {
                code: ValidationCode::InvalidField,
                field: "body".to_string(),
                message: format!("console handoff response is invalid: {error}"),
            })?;
        if !receipt.is_valid_contract() {
            return Err(CoreError::Validation {
                code: ValidationCode::InvalidField,
                field: "body".to_string(),
                message: "console handoff response failed contract validation".to_string(),
            });
        }
        Ok(receipt)
    }
}

pub(crate) fn map_failure_status(status: u16, retry_after_secs: u64) -> CoreError {
    match status {
        401 => CoreError::Auth {
            code: AuthCode::Failed,
            message: "console handoff session expired".to_string(),
        },
        403 | 404 => CoreError::PolicyDenied {
            code: PolicyCode::Denied,
            message: "console handoff is unavailable for this actor".to_string(),
        },
        408 | 504 => CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 0,
        },
        429 => CoreError::RateLimit {
            code: NetworkCode::RateLimit,
            retry_after_secs,
        },
        500..=599 => CoreError::ServiceUnavailable {
            code: ServiceCode::Unavailable,
            message: format!("console handoff unavailable ({status})"),
        },
        400..=499 => CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: "request".to_string(),
            message: format!("console handoff request rejected ({status})"),
        },
        other => CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("console handoff returned an unexpected status ({other})"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_has_no_identity_token_or_query() {
        assert!(!CONSOLE_HANDOFF_ISSUE_PATH.contains('?'));
        for forbidden in ["actor", "user_id", "organization", "org_id", "token"] {
            assert!(!CONSOLE_HANDOFF_ISSUE_PATH.contains(forbidden));
        }
    }

    #[test]
    fn status_classes_keep_reauth_policy_and_retry_distinct() {
        assert_eq!(map_failure_status(401, 0).code(), "auth.failed");
        assert_eq!(map_failure_status(404, 0).code(), "policy.denied");
        assert_eq!(
            map_failure_status(409, 0).code(),
            "validation.invalid_field"
        );
        assert_eq!(map_failure_status(503, 0).code(), "service.unavailable");
        assert_eq!(map_failure_status(504, 0).code(), "network.timeout");
        assert_eq!(map_failure_status(429, 12).code(), "network.rate_limit");
    }
}
