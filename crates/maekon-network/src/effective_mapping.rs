//! Authenticated live-gate adapter for effective TMD mappings (#10358).

use async_trait::async_trait;
use chrono::DateTime;
use maekon_api_contracts::effective_mapping::{
    EffectiveMapping422Dto, EffectiveMappingDto, RequestValidationLocation,
};
use maekon_core::error::CoreError;
use maekon_core::error_codes::{AuthCode, NetworkCode, PolicyCode, ServiceCode, ValidationCode};
use maekon_core::models::effective_mapping::{EffectiveMapping, EffectiveMappingResolution};
use maekon_core::ports::effective_mapping_client::EffectiveMappingClient;
use maekon_http_core::outbound::{read_text_capped, BodyReadError};
use maekon_http_core::resilience::extract_retry_after;

use crate::http_client::HttpApiClient;

const MAX_EFFECTIVE_MAPPING_BYTES: u64 = 512 * 1024;

#[async_trait]
impl EffectiveMappingClient for HttpApiClient {
    async fn resolve_effective_mapping(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<EffectiveMappingResolution, CoreError> {
        require_identifier(organization_id, "organization_id")?;
        require_identifier(mapping_id, "mapping_id")?;
        require_identifier(assignment_id, "assignment_id")?;

        let base_path = format!(
            "/api/v1/organizations/{organization_id}/template-mappings/{mapping_id}/effective"
        );
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("assignment_id", assignment_id)
            .finish();
        let path = format!("{base_path}?{query}");
        let response = self
            .authorized_request(reqwest::Method::GET, &path)
            .await
            .map_err(CoreError::from)?
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        let retry_after = extract_retry_after(&response);

        if status.is_success() {
            let body = read_mapping_body(response).await?;
            let mapping: EffectiveMapping = serde_json::from_str::<EffectiveMappingDto>(&body)
                .map_err(|error| invalid_response(format!("invalid success response: {error}")))?
                .into();
            validate_effective_mapping(&mapping, organization_id, mapping_id, assignment_id)?;
            return Ok(EffectiveMappingResolution::Effective(mapping));
        }

        if status.as_u16() == 422 {
            let body = read_mapping_body(response).await?;
            return match serde_json::from_str::<EffectiveMapping422Dto>(&body) {
                Ok(EffectiveMapping422Dto::Rejection(rejection)) => {
                    if rejection.mapping_id != mapping_id
                        || rejection.assignment_id != assignment_id
                    {
                        return Err(invalid_response(
                            "gate rejection identifiers do not match the request".into(),
                        ));
                    }
                    Ok(EffectiveMappingResolution::Rejected(rejection.into()))
                }
                Ok(EffectiveMapping422Dto::Validation(validation)) => {
                    let detail = validation
                        .detail
                        .iter()
                        .map(|item| {
                            let location = item
                                .loc
                                .iter()
                                .map(|part| match part {
                                    RequestValidationLocation::Text(value) => value.clone(),
                                    RequestValidationLocation::Index(value) => value.to_string(),
                                })
                                .collect::<Vec<_>>()
                                .join(".");
                            format!("{location}: {} ({})", item.msg, item.error_type)
                        })
                        .collect::<Vec<_>>()
                        .join("; ");
                    Err(CoreError::Validation {
                        code: ValidationCode::InvalidField,
                        field: "request".into(),
                        message: format!("effective mapping request validation failed: {detail}"),
                    })
                }
                Err(error) => Err(invalid_response(format!(
                    "invalid 422 response from effective mapping gate: {error}"
                ))),
            };
        }

        Err(map_failure_status(status.as_u16(), retry_after))
    }
}

pub(crate) fn require_identifier(value: &str, field: &str) -> Result<(), CoreError> {
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

pub(crate) fn validate_effective_mapping(
    mapping: &EffectiveMapping,
    organization_id: &str,
    mapping_id: &str,
    assignment_id: &str,
) -> Result<(), CoreError> {
    if mapping.organization_id != organization_id
        || mapping.mapping_id != mapping_id
        || mapping.assignment_id != assignment_id
    {
        return Err(invalid_response(
            "effective mapping identifiers do not match the request".into(),
        ));
    }
    for (field, digest) in [
        ("content_hash", &mapping.content_hash),
        ("approved_template_hash", &mapping.approved_template_hash),
        ("assignment_hash", &mapping.assignment_hash),
        ("source_snapshot_hash", &mapping.source_snapshot_hash),
    ] {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid_response(format!(
                "effective mapping {field} is not a lowercase SHA-256 digest"
            )));
        }
    }
    if !mapping.content_hash_matches() {
        return Err(invalid_response(
            "effective mapping content hash does not match the canonical content bytes".into(),
        ));
    }
    DateTime::parse_from_rfc3339(&mapping.approved_at).map_err(|error| {
        invalid_response(format!(
            "effective mapping approved_at is not RFC 3339: {error}"
        ))
    })?;
    Ok(())
}

pub(crate) async fn read_mapping_body(response: reqwest::Response) -> Result<String, CoreError> {
    read_text_capped(response, MAX_EFFECTIVE_MAPPING_BYTES)
        .await
        .map_err(|error| match error {
            BodyReadError::TooLarge { len, cap } => invalid_response(format!(
                "effective mapping response is too large ({len} > {cap})"
            )),
            BodyReadError::Transport(error) if error.is_timeout() => CoreError::RequestTimeout {
                code: NetworkCode::Timeout,
                timeout_ms: 0,
            },
            BodyReadError::Transport(error) => CoreError::Network {
                code: NetworkCode::Generic,
                message: format!("effective mapping response read failed: {error}"),
            },
        })
}

pub(crate) fn map_transport_error(error: reqwest::Error) -> CoreError {
    if error.is_timeout() {
        CoreError::RequestTimeout {
            code: NetworkCode::Timeout,
            timeout_ms: 0,
        }
    } else {
        CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("effective mapping request failed: {error}"),
        }
    }
}

pub(crate) fn invalid_response(message: String) -> CoreError {
    CoreError::Validation {
        code: ValidationCode::InvalidField,
        field: "body".into(),
        message,
    }
}

pub(crate) fn map_failure_status(status: u16, retry_after_secs: u64) -> CoreError {
    match status {
        401 => CoreError::Auth {
            code: AuthCode::Failed,
            message: "effective mapping session expired".into(),
        },
        403 => CoreError::PolicyDenied {
            code: PolicyCode::Denied,
            message: "effective mapping access denied".into(),
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
            message: format!("effective mapping gate unavailable ({status})"),
        },
        400..=499 => CoreError::Validation {
            code: ValidationCode::InvalidField,
            field: "request".into(),
            message: format!("effective mapping request rejected ({status})"),
        },
        other => CoreError::Network {
            code: NetworkCode::Generic,
            message: format!("effective mapping gate returned unexpected status ({other})"),
        },
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::auth::TokenManager;

    fn primary_password() -> String {
        String::from_utf8(vec![b'x'; 16]).expect("password fixture bytes must be UTF-8")
    }

    async fn client(server: &mut mockito::ServerGuard) -> HttpApiClient {
        let login = server
            .mock("POST", "/api/v1/auth/tokens")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"test_jwt","refresh_token":"ref","expires_in":3600}"#)
            .create_async()
            .await;
        let manager = Arc::new(TokenManager::new(&server.url()));
        manager
            .login("test@example.com", &primary_password())
            .await
            .expect("test login must succeed");
        login.assert_async().await;
        HttpApiClient::new(&server.url(), manager, Duration::from_secs(5))
            .expect("test client must build")
    }

    fn body() -> String {
        let content = "{\"fields\":[]}";
        let hash = EffectiveMapping::hash_content(content);
        serde_json::json!({
            "mapping_id": "map-1",
            "organization_id": "org-1",
            "version_id": "ver-1",
            "version_seq": 4,
            "content_hash": hash,
            "content": content,
            "approval_seq": 2,
            "approved_at": "2026-08-15T00:00:00Z",
            "approved_by_user_id": "user-1",
            "approved_template_hash": "b".repeat(64),
            "assignment_id": "asg-1",
            "assignment_hash": "c".repeat(64),
            "source_snapshot_hash": "d".repeat(64)
        })
        .to_string()
    }

    #[tokio::test]
    async fn live_success_requires_matching_anchors_and_content_hash() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let gate = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective",
            )
            .match_query(mockito::Matcher::UrlEncoded(
                "assignment_id".into(),
                "asg-1".into(),
            ))
            .match_header("authorization", "Bearer test_jwt")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body())
            .create_async()
            .await;
        let result = client
            .resolve_effective_mapping("org-1", "map-1", "asg-1")
            .await
            .expect("live gate must succeed");
        assert!(matches!(result, EffectiveMappingResolution::Effective(_)));
        gate.assert_async().await;
    }

    #[tokio::test]
    async fn gate_rejection_is_typed_data() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let gate = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective",
            )
            .match_query(mockito::Matcher::UrlEncoded(
                "assignment_id".into(),
                "asg-1".into(),
            ))
            .with_status(422)
            .with_header("content-type", "application/json")
            .with_body(r#"{"reason_code":"not_approved","mapping_id":"map-1","assignment_id":"asg-1","message":"approval required","expected":null,"actual":null}"#)
            .create_async()
            .await;
        let result = client
            .resolve_effective_mapping("org-1", "map-1", "asg-1")
            .await
            .expect("a documented gate rejection must remain typed data");
        assert!(matches!(result, EffectiveMappingResolution::Rejected(_)));
        gate.assert_async().await;
    }

    #[tokio::test]
    async fn fastapi_request_validation_is_an_error_not_a_gate_rejection() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let validation = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective",
            )
            .match_query(mockito::Matcher::UrlEncoded(
                "assignment_id".into(),
                "asg-1".into(),
            ))
            .with_status(422)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"detail":[{"loc":["query","assignment_id"],"msg":"invalid","type":"value_error"}]}"#,
            )
            .create_async()
            .await;
        let error = client
            .resolve_effective_mapping("org-1", "map-1", "asg-1")
            .await
            .expect_err("request validation must remain an error");
        assert_eq!(error.code(), "validation.invalid_field");
        validation.assert_async().await;
    }

    #[tokio::test]
    async fn mutation_control_rejects_mismatched_content_hash() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let mut payload: serde_json::Value = serde_json::from_str(&body()).unwrap();
        payload["content_hash"] = serde_json::Value::String("0".repeat(64));
        let gate = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective",
            )
            .match_query(mockito::Matcher::UrlEncoded(
                "assignment_id".into(),
                "asg-1".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(payload.to_string())
            .create_async()
            .await;
        let error = client
            .resolve_effective_mapping("org-1", "map-1", "asg-1")
            .await
            .expect_err("a mismatched content hash must fail closed");
        assert_eq!(error.code(), "validation.invalid_field");
        gate.assert_async().await;
    }

    #[tokio::test]
    async fn mutation_control_rejects_response_from_another_scope() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let mut payload: serde_json::Value = serde_json::from_str(&body()).unwrap();
        payload["organization_id"] = serde_json::Value::String("org-other".into());
        let gate = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective",
            )
            .match_query(mockito::Matcher::UrlEncoded(
                "assignment_id".into(),
                "asg-1".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(payload.to_string())
            .create_async()
            .await;
        let error = client
            .resolve_effective_mapping("org-1", "map-1", "asg-1")
            .await
            .expect_err("a cross-scope response must fail closed");
        assert_eq!(error.code(), "validation.invalid_field");
        gate.assert_async().await;
    }

    #[test]
    fn rejects_path_and_query_injection_before_request_construction() {
        for value in ["", "../other", "x/y", "x?assignment_id=other", "x\r\ny"] {
            let error = require_identifier(value, "mapping_id")
                .expect_err("path and query injection must fail closed");
            assert!(
                matches!(&error, CoreError::Validation { .. }),
                "{value:?}: {error}"
            );
            assert_eq!(error.code(), "validation.invalid_field", "{value:?}");
        }
        require_identifier("map-1_2.v3", "mapping_id").expect("a safe identifier must be accepted");
    }
}
