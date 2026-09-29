#![cfg(test)]

use super::tests::effective_mapping;
use super::*;

#[test]
fn produced_result_keeps_the_flat_frontend_wire_contract() {
    let result = GenerateTmdXlsxResult::Produced(Box::new(ProducedTmdXlsx {
        receipt_id: "receipt-1".into(),
        assignment_receipt_id: "assignment-receipt-1".into(),
        assignment_id: "assignment-1".into(),
        mapping_id: "mapping-1".into(),
        output_path: "output.xlsx".into(),
        artifact_sha256: "artifact".into(),
        template_structure_hash: "template".into(),
        mapping_content_hash: "mapping".into(),
        approved_template_hash: "approved".into(),
        assignment_hash: "assignment".into(),
        source_snapshot_hash: "snapshot".into(),
        row_count: 7,
        escaped_cell_count: 2,
        receipt_upload_pending: true,
    }));
    assert_eq!(
        serde_json::to_value(result).expect("serialize produced result"),
        serde_json::json!({
            "outcome": "produced",
            "receipt_id": "receipt-1",
            "assignment_receipt_id": "assignment-receipt-1",
            "assignment_id": "assignment-1",
            "mapping_id": "mapping-1",
            "output_path": "output.xlsx",
            "artifact_sha256": "artifact",
            "template_structure_hash": "template",
            "mapping_content_hash": "mapping",
            "approved_template_hash": "approved",
            "assignment_hash": "assignment",
            "source_snapshot_hash": "snapshot",
            "row_count": 7,
            "escaped_cell_count": 2,
            "receipt_upload_pending": true
        })
    );
}

#[test]
fn post_gate_receipts_preserve_optional_counts_and_approval_provenance() {
    let effective = EffectiveWbsXlsxProjection {
        effective: effective_mapping(),
        projection: maekon_core::models::wbs_xlsx::WbsXlsxProjection {
            sheet: "WBS".into(),
            header: Vec::new(),
            rows: Vec::new(),
            rollup_groups: Vec::new(),
        },
    };
    for (counts, expected) in [(None, None), (Some(0), Some(0)), (Some(7), Some(7))] {
        let receipt = receipt_after_gate(
            &effective,
            WbsXlsxOutcome::Produced,
            None,
            None,
            Some("artifact".into()),
            counts,
            counts,
        )
        .expect("valid receipt counts");
        assert_eq!(receipt.row_count, expected);
        assert_eq!(receipt.escaped_cell_count, expected);
        assert_eq!(
            receipt.mapping_content_hash.as_deref(),
            Some(effective.effective.content_hash.as_str())
        );
        assert_eq!(
            receipt.assignment_hash.as_deref(),
            Some(effective.effective.assignment_hash.as_str())
        );
        assert_eq!(
            receipt.source_snapshot_hash.as_deref(),
            Some(effective.effective.source_snapshot_hash.as_str())
        );
        assert_eq!(receipt.approval_seq, Some(1));
    }
}
