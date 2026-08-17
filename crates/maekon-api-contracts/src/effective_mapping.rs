//! Transport DTOs for the server-gated effective mapping endpoint (#10358).

use maekon_core::models::effective_mapping::{
    EffectiveMapping, MappingResolutionReason, MappingResolutionRejection,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveMappingDto {
    pub mapping_id: String,
    pub organization_id: String,
    pub version_id: String,
    pub version_seq: i64,
    pub content_hash: String,
    pub content: String,
    pub approval_seq: i64,
    pub approved_at: String,
    pub approved_by_user_id: String,
    pub approved_template_hash: String,
    pub assignment_id: String,
    pub assignment_hash: String,
    pub source_snapshot_hash: String,
}

impl From<EffectiveMappingDto> for EffectiveMapping {
    fn from(value: EffectiveMappingDto) -> Self {
        Self {
            mapping_id: value.mapping_id,
            organization_id: value.organization_id,
            version_id: value.version_id,
            version_seq: value.version_seq,
            content_hash: value.content_hash,
            content: value.content,
            approval_seq: value.approval_seq,
            approved_at: value.approved_at,
            approved_by_user_id: value.approved_by_user_id,
            approved_template_hash: value.approved_template_hash,
            assignment_id: value.assignment_id,
            assignment_hash: value.assignment_hash,
            source_snapshot_hash: value.source_snapshot_hash,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingResolutionRejectionDto {
    pub reason_code: MappingResolutionReason,
    pub mapping_id: String,
    pub assignment_id: String,
    pub message: String,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

impl From<MappingResolutionRejectionDto> for MappingResolutionRejection {
    fn from(value: MappingResolutionRejectionDto) -> Self {
        Self {
            reason_code: value.reason_code,
            mapping_id: value.mapping_id,
            assignment_id: value.assignment_id,
            message: value.message,
            expected: value.expected,
            actual: value.actual,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestValidationLocation {
    Text(String),
    Index(i64),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestValidationErrorDetail {
    pub loc: Vec<RequestValidationLocation>,
    pub msg: String,
    #[serde(rename = "type")]
    pub error_type: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestValidationErrorResponse {
    pub detail: Vec<RequestValidationErrorDetail>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum EffectiveMapping422Dto {
    Rejection(MappingResolutionRejectionDto),
    Validation(RequestValidationErrorResponse),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_effective_mapping_contract() {
        let dto: EffectiveMappingDto = serde_json::from_value(serde_json::json!({
            "mapping_id": "map-1",
            "organization_id": "org-1",
            "version_id": "ver-1",
            "version_seq": 4,
            "content_hash": "a".repeat(64),
            "content": "{\"fields\":[]}",
            "approval_seq": 2,
            "approved_at": "2026-08-15T00:00:00Z",
            "approved_by_user_id": "user-1",
            "approved_template_hash": "b".repeat(64),
            "assignment_id": "asg-1",
            "assignment_hash": "c".repeat(64),
            "source_snapshot_hash": "d".repeat(64)
        }))
        .expect("the exact server response must parse");
        assert_eq!(dto.version_seq, 4);
    }

    #[test]
    fn discriminates_gate_rejection_from_fastapi_validation() {
        let rejection: EffectiveMapping422Dto = serde_json::from_value(serde_json::json!({
            "reason_code": "template_stale",
            "mapping_id": "map-1",
            "assignment_id": "asg-1",
            "message": "template changed",
            "expected": "a",
            "actual": "b"
        }))
        .expect("a gate rejection must parse");
        assert!(matches!(rejection, EffectiveMapping422Dto::Rejection(_)));

        let validation: EffectiveMapping422Dto = serde_json::from_value(serde_json::json!({
            "detail": [{
                "loc": ["query", "assignment_id", 0],
                "msg": "field required",
                "type": "missing",
                "input": null
            }]
        }))
        .expect("a FastAPI validation response may contain extra detail fields");
        assert!(matches!(validation, EffectiveMapping422Dto::Validation(_)));
    }

    #[test]
    fn accepts_all_documented_rejection_reasons() {
        for reason in [
            "not_approved",
            "mapping_hash_mismatch",
            "template_stale",
            "receipt_contract_mismatch",
            "receipt_instance_stale",
        ] {
            let value = serde_json::json!({
                "reason_code": reason,
                "mapping_id": "map-1",
                "assignment_id": "asg-1",
                "message": "rejected",
                "expected": null,
                "actual": null
            });
            let parsed: EffectiveMapping422Dto =
                serde_json::from_value(value).expect("documented reason must parse");
            assert!(matches!(parsed, EffectiveMapping422Dto::Rejection(_)));
        }
    }

    #[test]
    fn optional_rejection_anchors_may_be_omitted_by_the_server() {
        let omitted = serde_json::json!({
            "reason_code": "not_approved",
            "mapping_id": "map-1",
            "assignment_id": "asg-1",
            "message": "rejected"
        });
        assert!(matches!(
            serde_json::from_value::<EffectiveMapping422Dto>(omitted).unwrap(),
            EffectiveMapping422Dto::Rejection(MappingResolutionRejectionDto {
                expected: None,
                actual: None,
                ..
            })
        ));
    }
}
