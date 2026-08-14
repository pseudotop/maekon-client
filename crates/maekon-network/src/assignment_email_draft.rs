//! Authenticated receipt-only assignment email draft adapter (#9627).

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::error_codes::{
    AuthCode, NetworkCode, NotFoundCode, PolicyCode, ServiceCode, ValidationCode,
};
use maekon_core::models::assignment_email_draft::AssignmentEmailDraft;
use maekon_core::ports::assignment_email_draft_client::AssignmentEmailDraftClient;
use maekon_http_core::outbound::{read_text_capped, BodyReadError};
use maekon_http_core::resilience::extract_retry_after;
use serde::Serialize;

use crate::http_client::HttpApiClient;

pub const DRAFT_PATH: &str = "/api/v1/user-context/suggestions/email-draft";
pub const MAX_DRAFT_BYTES: u64 = 128 * 1024;

#[derive(Serialize)]
struct DraftRequest<'a> {
    assignment_receipt_id: &'a str,
}

#[async_trait]
impl AssignmentEmailDraftClient for HttpApiClient {
    async fn generate(
        &self,
        assignment_receipt_id: &str,
    ) -> Result<AssignmentEmailDraft, CoreError> {
        require_identifier(assignment_receipt_id, "assignment_receipt_id")?;
        let request = self
            .authorized_request(reqwest::Method::POST, DRAFT_PATH)
            .await
            .map_err(CoreError::from)?
            .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
            .json(&DraftRequest {
                assignment_receipt_id,
            });
        read_draft(request.send().await).await
    }

    async fn load(&self, draft_id: &str) -> Result<AssignmentEmailDraft, CoreError> {
        require_identifier(draft_id, "draft_id")?;
        let path = format!("{DRAFT_PATH}/{draft_id}");
        let request = self
            .authorized_request(reqwest::Method::GET, &path)
            .await
            .map_err(CoreError::from)?;
        read_draft(request.send().await).await
    }

    async fn regenerate(
        &self,
        draft_id: &str,
        assignment_receipt_id: &str,
    ) -> Result<AssignmentEmailDraft, CoreError> {
        require_identifier(draft_id, "draft_id")?;
        require_identifier(assignment_receipt_id, "assignment_receipt_id")?;
        let path = format!("{DRAFT_PATH}/{draft_id}/regenerate");
        let request = self
            .authorized_request(reqwest::Method::POST, &path)
            .await
            .map_err(CoreError::from)?
            .header("Idempotency-Key", uuid::Uuid::new_v4().to_string())
            .json(&DraftRequest {
                assignment_receipt_id,
            });
        read_draft(request.send().await).await
    }
}

fn require_identifier(value: &str, field: &str) -> Result<(), CoreError> {
    if value.is_empty()
        || value.len() > 200
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: field.to_string(),
            message: format!("{field} is not a safe single-line identifier"),
        });
    }
    Ok(())
}

async fn read_draft(
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<AssignmentEmailDraft, CoreError> {
    let response = response.map_err(|error| {
        if error.is_timeout() {
            CoreError::RequestTimeout {
                code: NetworkCode::Timeout,
                timeout_ms: 0,
            }
        } else {
            CoreError::Network {
                code: NetworkCode::Generic,
                message: format!("assignment email draft request failed: {error}"),
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
    let body = read_text_capped(response, MAX_DRAFT_BYTES)
        .await
        .map_err(|error| match error {
            BodyReadError::TooLarge { len, cap } => CoreError::Validation {
                code: ValidationCode::InvalidField,
                field: "body".into(),
                message: format!("assignment email draft response too large ({len} > {cap})"),
            },
            BodyReadError::Transport(error) if error.is_timeout() => CoreError::RequestTimeout {
                code: NetworkCode::Timeout,
                timeout_ms: 0,
            },
            BodyReadError::Transport(error) => CoreError::Network {
                code: NetworkCode::Generic,
                message: format!("assignment email draft response read failed: {error}"),
            },
        })?;
    let draft = serde_json::from_str::<AssignmentEmailDraft>(&body).map_err(|error| {
        CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: "body".into(),
            message: format!("assignment email draft response is invalid: {error}"),
        }
    })?;
    if !draft.has_reserved_synthetic_recipient() {
        return Err(CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: "recipient".into(),
            message: "assignment email draft recipient is not reserved synthetic data".into(),
        });
    }
    Ok(draft)
}

fn map_failure_status(status: u16, retry_after_secs: u64) -> CoreError {
    match status {
        401 => CoreError::Auth {
            code: AuthCode::Failed,
            message: "assignment email draft session expired".into(),
        },
        403 => CoreError::PolicyDenied {
            code: PolicyCode::Denied,
            message: "assignment email draft access denied".into(),
        },
        404 => CoreError::NotFound {
            code: NotFoundCode::ResourceMissing,
            resource_type: "assignment_email_draft".into(),
            id: "draft".into(),
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
            message: format!("assignment email draft unavailable ({status})"),
        },
        400..=499 => CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: "request".into(),
            message: format!("assignment email draft request rejected ({status})"),
        },
        other => CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("assignment email draft returned unexpected status ({other})"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_and_request_carry_no_identity_or_recipient_fields() {
        for forbidden in ["actor", "organization", "recipient", "subject", "body"] {
            assert!(!DRAFT_PATH.contains(forbidden));
        }
        let json = serde_json::to_value(DraftRequest {
            assignment_receipt_id: "ercv-1",
        })
        .unwrap();
        assert_eq!(json.as_object().unwrap().len(), 1);
        assert_eq!(json["assignment_receipt_id"], "ercv-1");
    }

    #[test]
    fn unsafe_path_identifiers_are_rejected_before_url_construction() {
        for value in [
            "",
            "../other",
            "draft/other",
            "draft\r\nheader",
            "draft?org=other",
        ] {
            let error = require_identifier(value, "draft_id").unwrap_err();
            assert_eq!(error.code(), "validation.invalid_field", "{value:?}");
        }
        require_identifier("emd-draft_01", "draft_id")
            .expect("a safe single-line identifier must be accepted");
    }

    #[test]
    fn conflict_is_permanent_and_service_errors_are_retryable() {
        assert_eq!(
            map_failure_status(409, 0).code(),
            "validation.invalid_field"
        );
        assert_eq!(map_failure_status(503, 0).code(), "service.unavailable");
        assert_eq!(map_failure_status(504, 0).code(), "network.timeout");
    }

    #[test]
    fn response_cap_is_small_and_nonzero() {
        const {
            assert!(MAX_DRAFT_BYTES >= 32 * 1024);
            assert!(MAX_DRAFT_BYTES <= 512 * 1024);
        }
    }
}
