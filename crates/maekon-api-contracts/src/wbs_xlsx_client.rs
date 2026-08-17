//! Wire DTOs for standalone WBS XLSX projection and receipt APIs (#10358).

use maekon_core::models::wbs_xlsx::{
    LocalWbsXlsxReceipt, RollupCellGroup, UploadedWbsXlsxReceipt, WbsXlsxOutcome,
};
use serde::{Deserialize, Serialize};

use crate::effective_mapping::EffectiveMappingDto;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WbsXlsxProjectionDto {
    pub sheet: String,
    pub header: Vec<String>,
    pub rows:
        Vec<std::collections::BTreeMap<String, maekon_core::models::wbs_xlsx::ProjectionCellValue>>,
    pub rollup_groups: Vec<RollupCellGroup>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveWbsXlsxProjectionDto {
    pub effective: EffectiveMappingDto,
    pub projection: WbsXlsxProjectionDto,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalWbsXlsxReceiptDto {
    pub receipt_id: String,
    pub mapping_id: String,
    pub assignment_id: String,
    pub outcome: WbsXlsxOutcome,
    pub reason_code: Option<String>,
    pub artifact_sha256: Option<String>,
    pub row_count: Option<u64>,
    pub escaped_cell_count: Option<u64>,
    pub template_structure_hash: Option<String>,
    pub mapping_content_hash: Option<String>,
    pub approved_template_hash: Option<String>,
    pub assignment_hash: Option<String>,
    pub source_snapshot_hash: Option<String>,
    pub approval_seq: Option<i64>,
    pub approved_at: Option<String>,
    pub produced_at: String,
}

impl From<&LocalWbsXlsxReceipt> for LocalWbsXlsxReceiptDto {
    fn from(value: &LocalWbsXlsxReceipt) -> Self {
        Self {
            receipt_id: value.receipt_id.clone(),
            mapping_id: value.mapping_id.clone(),
            assignment_id: value.assignment_id.clone(),
            outcome: value.outcome,
            reason_code: value.reason_code.clone(),
            artifact_sha256: value.artifact_sha256.clone(),
            row_count: value.row_count,
            escaped_cell_count: value.escaped_cell_count,
            template_structure_hash: value.template_structure_hash.clone(),
            mapping_content_hash: value.mapping_content_hash.clone(),
            approved_template_hash: value.approved_template_hash.clone(),
            assignment_hash: value.assignment_hash.clone(),
            source_snapshot_hash: value.source_snapshot_hash.clone(),
            approval_seq: value.approval_seq,
            approved_at: value.approved_at.clone(),
            produced_at: value.produced_at.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UploadedWbsXlsxReceiptDto {
    #[serde(flatten)]
    pub receipt: LocalWbsXlsxReceiptDto,
    pub organization_id: String,
    pub origin: String,
    pub actor_id: Option<String>,
    pub synthetic: bool,
    pub seed_namespace: Option<String>,
}

impl From<UploadedWbsXlsxReceiptDto> for UploadedWbsXlsxReceipt {
    fn from(value: UploadedWbsXlsxReceiptDto) -> Self {
        Self {
            receipt: LocalWbsXlsxReceipt {
                receipt_id: value.receipt.receipt_id,
                mapping_id: value.receipt.mapping_id,
                assignment_id: value.receipt.assignment_id,
                outcome: value.receipt.outcome,
                reason_code: value.receipt.reason_code,
                artifact_sha256: value.receipt.artifact_sha256,
                row_count: value.receipt.row_count,
                escaped_cell_count: value.receipt.escaped_cell_count,
                template_structure_hash: value.receipt.template_structure_hash,
                mapping_content_hash: value.receipt.mapping_content_hash,
                approved_template_hash: value.receipt.approved_template_hash,
                assignment_hash: value.receipt.assignment_hash,
                source_snapshot_hash: value.receipt.source_snapshot_hash,
                approval_seq: value.receipt.approval_seq,
                approved_at: value.receipt.approved_at,
                produced_at: value.receipt.produced_at,
            },
            organization_id: value.organization_id,
            origin: value.origin,
            actor_id: value.actor_id,
            synthetic: value.synthetic,
            seed_namespace: value.seed_namespace,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
pub struct ReceiptConflictDto {
    pub code: String,
    pub receipt_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_preserves_integer_number_and_text_values() {
        let body = serde_json::json!({
            "effective": {
                "mapping_id": "map-1", "organization_id": "org-1", "version_id": "v1",
                "version_seq": 1, "content_hash": "a", "content": "{}", "approval_seq": 1,
                "approved_at": "2026-08-16T00:00:00+00:00", "approved_by_user_id": "u1",
                "approved_template_hash": "b", "assignment_id": "asg-1",
                "assignment_hash": "c", "source_snapshot_hash": "d"
            },
            "projection": {
                "sheet": "WBS", "header": ["Level", "Effort", "Name"],
                "rows": [{"level": 2, "effort": 3.5, "name": "분석"}],
                "rollup_groups": [{"parent_row": 2, "child_rows": [3, 4]}]
            }
        });
        let parsed: EffectiveWbsXlsxProjectionDto = serde_json::from_value(body).unwrap();
        let row = &parsed.projection.rows[0];
        assert!(matches!(
            row["level"],
            maekon_core::models::wbs_xlsx::ProjectionCellValue::Integer(2)
        ));
        assert!(
            matches!(row["effort"], maekon_core::models::wbs_xlsx::ProjectionCellValue::Number(value) if value == 3.5)
        );
        assert!(
            matches!(&row["name"], maekon_core::models::wbs_xlsx::ProjectionCellValue::Text(value) if value == "분석")
        );
    }
}
