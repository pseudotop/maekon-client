//! Typed contracts for the standalone, server-gated WBS XLSX flow (#10358).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::effective_mapping::EffectiveMapping;
use super::effective_mapping::MappingResolutionRejection;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProjectionCellValue {
    Integer(i64),
    Number(f64),
    Text(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RollupCellGroup {
    pub parent_row: u32,
    pub child_rows: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WbsXlsxProjection {
    pub sheet: String,
    pub header: Vec<String>,
    pub rows: Vec<std::collections::BTreeMap<String, ProjectionCellValue>>,
    pub rollup_groups: Vec<RollupCellGroup>,
}

impl WbsXlsxProjection {
    /// Server-compatible canonical hash over sheet, header and projected rows.
    pub fn content_hash(&self) -> Result<String, serde_json::Error> {
        #[derive(Serialize)]
        struct CanonicalProjection<'a> {
            header: &'a [String],
            rows: &'a [std::collections::BTreeMap<String, ProjectionCellValue>],
            sheet: &'a str,
        }

        let bytes = serde_json::to_vec(&CanonicalProjection {
            header: &self.header,
            rows: &self.rows,
            sheet: &self.sheet,
        })?;
        let digest = Sha256::digest(bytes);
        Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveWbsXlsxProjection {
    pub effective: EffectiveMapping,
    pub projection: WbsXlsxProjection,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EffectiveWbsXlsxProjectionResolution {
    Effective(Box<EffectiveWbsXlsxProjection>),
    Rejected(MappingResolutionRejection),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbsXlsxOutcome {
    Produced,
    GateRejected,
    HeaderDrift,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalWbsXlsxReceipt {
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UploadedWbsXlsxReceipt {
    #[serde(flatten)]
    pub receipt: LocalWbsXlsxReceipt,
    pub organization_id: String,
    pub origin: String,
    pub actor_id: Option<String>,
    pub synthetic: bool,
    pub seed_namespace: Option<String>,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn projection_hash_matches_the_server_cross_language_golden() {
        let projection = WbsXlsxProjection {
            sheet: "WBS".into(),
            header: vec!["레벨".into(), "작업명".into(), "공수".into()],
            rows: vec![
                BTreeMap::from([
                    ("level".into(), ProjectionCellValue::Integer(1)),
                    ("name".into(), ProjectionCellValue::Text("루트".into())),
                    ("effort".into(), ProjectionCellValue::Number(3.5)),
                ]),
                BTreeMap::from([
                    ("level".into(), ProjectionCellValue::Integer(2)),
                    ("name".into(), ProjectionCellValue::Text("=분석".into())),
                    ("effort".into(), ProjectionCellValue::Number(3.5)),
                ]),
            ],
            rollup_groups: Vec::new(),
        };
        assert_eq!(
            projection.content_hash().unwrap(),
            "91e0baac58cc469f28b39bfdaa6749755c183d58faa754acf4e55512b212bd61"
        );
    }
}
