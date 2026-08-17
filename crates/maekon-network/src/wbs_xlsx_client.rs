//! Authenticated adapter for standalone WBS XLSX projection and receipts (#10358).

use async_trait::async_trait;
use maekon_api_contracts::effective_mapping::{EffectiveMapping422Dto, RequestValidationLocation};
use maekon_api_contracts::wbs_xlsx_client::{
    EffectiveWbsXlsxProjectionDto, LocalWbsXlsxReceiptDto, ReceiptConflictDto,
    UploadedWbsXlsxReceiptDto,
};
use maekon_core::error::CoreError;
use maekon_core::error_codes::ValidationCode;
use maekon_core::models::effective_mapping::EffectiveMapping;
use maekon_core::models::wbs_xlsx::{
    EffectiveWbsXlsxProjection, EffectiveWbsXlsxProjectionResolution, LocalWbsXlsxReceipt,
    UploadedWbsXlsxReceipt, WbsXlsxProjection,
};
use maekon_core::ports::wbs_xlsx_client::WbsXlsxClient;
use maekon_http_core::resilience::extract_retry_after;

use crate::effective_mapping::{
    invalid_response, map_failure_status, map_transport_error, read_mapping_body,
    require_identifier, validate_effective_mapping,
};
use crate::http_client::HttpApiClient;

#[async_trait]
impl WbsXlsxClient for HttpApiClient {
    async fn resolve_effective_projection(
        &self,
        organization_id: &str,
        mapping_id: &str,
        assignment_id: &str,
    ) -> Result<EffectiveWbsXlsxProjectionResolution, CoreError> {
        require_identifier(organization_id, "organization_id")?;
        require_identifier(mapping_id, "mapping_id")?;
        require_identifier(assignment_id, "assignment_id")?;

        let base_path = format!(
            "/api/v1/organizations/{organization_id}/template-mappings/{mapping_id}/effective-projection"
        );
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("assignment_id", assignment_id)
            .finish();
        let response = self
            .authorized_request(reqwest::Method::GET, &format!("{base_path}?{query}"))
            .await
            .map_err(CoreError::from)?
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        let retry_after = extract_retry_after(&response);
        let body = read_mapping_body(response).await?;

        if status.is_success() {
            let dto: EffectiveWbsXlsxProjectionDto =
                serde_json::from_str(&body).map_err(|error| {
                    invalid_response(format!("invalid projection response: {error}"))
                })?;
            let effective: EffectiveMapping = dto.effective.into();
            validate_effective_mapping(&effective, organization_id, mapping_id, assignment_id)?;
            if dto.projection.sheet.is_empty()
                || dto.projection.header.is_empty()
                || dto.projection.rows.is_empty()
            {
                return Err(invalid_response(
                    "effective projection contains an empty sheet, header, or row set".into(),
                ));
            }
            return Ok(EffectiveWbsXlsxProjectionResolution::Effective(Box::new(
                EffectiveWbsXlsxProjection {
                    effective,
                    projection: WbsXlsxProjection {
                        sheet: dto.projection.sheet,
                        header: dto.projection.header,
                        rows: dto.projection.rows,
                        rollup_groups: dto.projection.rollup_groups,
                    },
                },
            )));
        }

        if status.as_u16() == 422 {
            return projection_rejection(&body, mapping_id, assignment_id);
        }
        Err(map_failure_status(status.as_u16(), retry_after))
    }

    async fn append_local_receipt(
        &self,
        organization_id: &str,
        receipt: &LocalWbsXlsxReceipt,
    ) -> Result<UploadedWbsXlsxReceipt, CoreError> {
        require_identifier(organization_id, "organization_id")?;
        require_identifier(&receipt.mapping_id, "mapping_id")?;
        require_identifier(&receipt.assignment_id, "assignment_id")?;
        require_identifier(&receipt.receipt_id, "receipt_id")?;

        let path = format!("/api/v1/organizations/{organization_id}/wbs/xlsx-output-receipts");
        let response = self
            .authorized_request(reqwest::Method::POST, &path)
            .await
            .map_err(CoreError::from)?
            .json(&LocalWbsXlsxReceiptDto::from(receipt))
            .send()
            .await
            .map_err(map_transport_error)?;
        let status = response.status();
        let retry_after = extract_retry_after(&response);
        let body = read_mapping_body(response).await?;

        if status.is_success() {
            let uploaded: UploadedWbsXlsxReceipt =
                serde_json::from_str::<UploadedWbsXlsxReceiptDto>(&body)
                    .map_err(|error| {
                        invalid_response(format!("invalid receipt response: {error}"))
                    })?
                    .into();
            validate_uploaded_receipt(&uploaded, organization_id, receipt)?;
            return Ok(uploaded);
        }
        if status.as_u16() == 409 {
            let conflict: ReceiptConflictDto = serde_json::from_str(&body).map_err(|error| {
                invalid_response(format!("invalid receipt conflict response: {error}"))
            })?;
            if conflict.code != "receipt_id_conflict" || conflict.receipt_id != receipt.receipt_id {
                return Err(invalid_response(
                    "receipt conflict response does not match the request".into(),
                ));
            }
            return Err(CoreError::Validation {
                code: ValidationCode::InvalidField,
                field: "receipt_id".into(),
                message: "receipt_id already exists with different content".into(),
            });
        }
        Err(map_failure_status(status.as_u16(), retry_after))
    }
}

fn projection_rejection(
    body: &str,
    mapping_id: &str,
    assignment_id: &str,
) -> Result<EffectiveWbsXlsxProjectionResolution, CoreError> {
    match serde_json::from_str::<EffectiveMapping422Dto>(body) {
        Ok(EffectiveMapping422Dto::Rejection(rejection)) => {
            if rejection.mapping_id != mapping_id || rejection.assignment_id != assignment_id {
                return Err(invalid_response(
                    "projection gate rejection identifiers do not match the request".into(),
                ));
            }
            Ok(EffectiveWbsXlsxProjectionResolution::Rejected(
                rejection.into(),
            ))
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
                message: format!("effective projection request validation failed: {detail}"),
            })
        }
        Err(error) => Err(invalid_response(format!(
            "invalid 422 response from effective projection gate: {error}"
        ))),
    }
}

fn validate_uploaded_receipt(
    uploaded: &UploadedWbsXlsxReceipt,
    organization_id: &str,
    request: &LocalWbsXlsxReceipt,
) -> Result<(), CoreError> {
    if uploaded.organization_id != organization_id
        || uploaded.receipt != *request
        || uploaded.origin != "client_local"
        || uploaded.actor_id.as_deref().is_none_or(str::is_empty)
        || uploaded.synthetic
        || uploaded.seed_namespace.is_some()
    {
        return Err(invalid_response(
            "uploaded receipt response does not preserve the authenticated local receipt".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use maekon_core::models::wbs_xlsx::{EffectiveWbsXlsxProjectionResolution, WbsXlsxOutcome};

    use super::*;
    use crate::auth::TokenManager;

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
            .login(
                "test@example.com",
                &String::from_utf8(vec![b'x'; 16]).unwrap(),
            )
            .await
            .unwrap();
        login.assert_async().await;
        HttpApiClient::new(&server.url(), manager, Duration::from_secs(5)).unwrap()
    }

    fn projection_body() -> String {
        let content = serde_json::json!({
            "schema_version": "tmd/1", "sheet": "WBS", "first_data_row": 2, "steps": []
        })
        .to_string();
        serde_json::json!({
            "effective": {
                "mapping_id": "map-1", "organization_id": "org-1", "version_id": "v1",
                "version_seq": 1, "content_hash": EffectiveMapping::hash_content(&content),
                "content": content, "approval_seq": 1,
                "approved_at": "2026-08-16T00:00:00+00:00", "approved_by_user_id": "u1",
                "approved_template_hash": "a".repeat(64), "assignment_id": "asg-1",
                "assignment_hash": "b".repeat(64), "source_snapshot_hash": "c".repeat(64)
            },
            "projection": {
                "sheet": "WBS", "header": ["레벨"], "rows": [{"level": 1}],
                "rollup_groups": []
            }
        })
        .to_string()
    }

    fn receipt() -> LocalWbsXlsxReceipt {
        LocalWbsXlsxReceipt {
            receipt_id: "receipt-1".into(),
            mapping_id: "map-1".into(),
            assignment_id: "asg-1".into(),
            outcome: WbsXlsxOutcome::Produced,
            reason_code: None,
            artifact_sha256: Some("d".repeat(64)),
            row_count: Some(1),
            escaped_cell_count: Some(0),
            template_structure_hash: Some("e".repeat(64)),
            mapping_content_hash: Some("f".repeat(64)),
            approved_template_hash: Some("a".repeat(64)),
            assignment_hash: Some("b".repeat(64)),
            source_snapshot_hash: Some("c".repeat(64)),
            approval_seq: Some(1),
            approved_at: Some("2026-08-16T00:00:00+00:00".into()),
            produced_at: "2026-08-16T00:00:01+00:00".into(),
        }
    }

    #[tokio::test]
    async fn projection_success_is_scope_bound_and_typed() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let request = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective-projection",
            )
            .match_query(mockito::Matcher::UrlEncoded(
                "assignment_id".into(),
                "asg-1".into(),
            ))
            .match_header("authorization", "Bearer test_jwt")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(projection_body())
            .create_async()
            .await;
        let result = client
            .resolve_effective_projection("org-1", "map-1", "asg-1")
            .await
            .unwrap();
        let EffectiveWbsXlsxProjectionResolution::Effective(result) = result else {
            panic!("expected an effective projection")
        };
        assert!(matches!(
            result.projection.rows[0]["level"],
            maekon_core::models::wbs_xlsx::ProjectionCellValue::Integer(1)
        ));
        request.assert_async().await;
    }

    #[tokio::test]
    async fn gate_rejection_remains_typed_for_local_receipt_creation() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let request = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective-projection",
            )
            .match_query(mockito::Matcher::Any)
            .with_status(422)
            .with_header("content-type", "application/json")
            .with_body(r#"{"reason_code":"template_stale","mapping_id":"map-1","assignment_id":"asg-1","message":"stale","expected":"a","actual":"b"}"#)
            .create_async()
            .await;
        let result = client
            .resolve_effective_projection("org-1", "map-1", "asg-1")
            .await
            .unwrap();
        assert!(matches!(
            result,
            EffectiveWbsXlsxProjectionResolution::Rejected(_)
        ));
        request.assert_async().await;
    }

    #[tokio::test]
    async fn uploaded_receipt_must_preserve_body_and_server_owned_fields() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let receipt = receipt();
        let mut response = serde_json::to_value(&receipt).unwrap();
        let object = response.as_object_mut().unwrap();
        object.insert("organization_id".into(), "org-1".into());
        object.insert("origin".into(), "client_local".into());
        object.insert("actor_id".into(), "user-1".into());
        object.insert("synthetic".into(), false.into());
        object.insert("seed_namespace".into(), serde_json::Value::Null);
        let request = server
            .mock(
                "POST",
                "/api/v1/organizations/org-1/wbs/xlsx-output-receipts",
            )
            .match_header("authorization", "Bearer test_jwt")
            .match_body(mockito::Matcher::Json(
                serde_json::to_value(&receipt).unwrap(),
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(response.to_string())
            .create_async()
            .await;
        let uploaded = client
            .append_local_receipt("org-1", &receipt)
            .await
            .unwrap();
        assert_eq!(uploaded.receipt, receipt);
        assert_eq!(uploaded.actor_id.as_deref(), Some("user-1"));
        request.assert_async().await;
    }

    #[tokio::test]
    async fn mutation_control_rejects_cross_scope_projection() {
        let mut server = mockito::Server::new_async().await;
        let client = client(&mut server).await;
        let mut body: serde_json::Value = serde_json::from_str(&projection_body()).unwrap();
        body["effective"]["organization_id"] = "other-org".into();
        let request = server
            .mock(
                "GET",
                "/api/v1/organizations/org-1/template-mappings/map-1/effective-projection",
            )
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create_async()
            .await;
        let error = client
            .resolve_effective_projection("org-1", "map-1", "asg-1")
            .await
            .unwrap_err();
        assert_eq!(error.code(), "validation.invalid_field");
        request.assert_async().await;
    }
}
