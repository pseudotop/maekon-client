//! Bind a live effective TMD document to its server projection (#10358).
//!
//! Projection row keys are TMD `source_field` names, not spreadsheet columns.
//! This adapter parses the live-gated canonical document and produces explicit
//! cell coordinates only after the sheet, header, kind, row and rollup
//! invariants agree.

use std::collections::{BTreeSet, HashSet};

use maekon_core::models::wbs_xlsx::{
    EffectiveWbsXlsxProjection, ProjectionCellValue, WbsXlsxProjection,
};
use serde::Deserialize;

use crate::tmd_xlsx_writer::{
    fill_workbook, rollup_sum_formula, CellUpdate, CellValue, HeaderExpectation, XlsxFillOutcome,
    XlsxWriterError,
};

const MAX_EXCEL_ROW: u32 = 1_048_576;
const MAX_EXCEL_COLUMN: u32 = 16_384;

#[derive(Clone, Debug, PartialEq)]
pub struct TmdXlsxFillPlan {
    pub sheet: String,
    pub headers: Vec<HeaderExpectation>,
    pub updates: Vec<CellUpdate>,
    pub row_count: usize,
}

#[derive(Debug, Deserialize)]
struct TmdDocument {
    schema_version: String,
    sheet: String,
    first_data_row: u32,
    steps: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MapColumnsStep {
    #[serde(rename = "type")]
    step_type: String,
    bindings: Vec<ColumnBinding>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ColumnBinding {
    source_field: String,
    column: String,
    kind: ColumnKind,
    header: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ColumnKind {
    Text,
    Int,
    Number,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RollupStep {
    #[serde(rename = "type")]
    step_type: String,
    strategy: String,
    target_column: String,
    applies_to: String,
}

pub fn build_fill_plan(
    input: &EffectiveWbsXlsxProjection,
) -> Result<TmdXlsxFillPlan, XlsxWriterError> {
    if !input.effective.content_hash_matches() {
        return Err(contract_error(
            "effective mapping content hash does not match its canonical content",
        ));
    }
    let document: TmdDocument = serde_json::from_str(&input.effective.content)
        .map_err(|error| contract_error(format!("invalid effective TMD document: {error}")))?;
    if document.schema_version != "tmd/1" {
        return Err(contract_error(format!(
            "unsupported TMD schema version: {}",
            document.schema_version
        )));
    }
    if document.first_data_row < 2 || document.first_data_row > MAX_EXCEL_ROW {
        return Err(contract_error(format!(
            "first_data_row is outside the Excel data range: {}",
            document.first_data_row
        )));
    }
    if document.sheet != input.projection.sheet {
        return Err(contract_error(format!(
            "projection sheet does not match the effective TMD: {:?} != {:?}",
            input.projection.sheet, document.sheet
        )));
    }

    let mut bindings = parse_bindings(&document.steps)?;
    bindings.sort_by_key(|binding| column_index(&binding.column).unwrap_or(u32::MAX));
    validate_bindings(&bindings)?;
    validate_projection_shape(&input.projection, &bindings)?;

    let mut updates = row_updates(&input.projection, &bindings, document.first_data_row)?;
    updates.extend(rollup_updates(
        &input.projection,
        &document.steps,
        &bindings,
        document.first_data_row,
    )?);
    updates.sort_by_key(|update| cell_sort_key(&update.coordinate).unwrap_or((u32::MAX, u32::MAX)));

    Ok(TmdXlsxFillPlan {
        sheet: document.sheet,
        headers: bindings
            .iter()
            .map(|binding| HeaderExpectation {
                column: binding.column.clone(),
                expected: binding.header.clone(),
            })
            .collect(),
        updates,
        row_count: input.projection.rows.len(),
    })
}

pub fn fill_projected_workbook(
    template: &[u8],
    input: &EffectiveWbsXlsxProjection,
) -> Result<XlsxFillOutcome, XlsxWriterError> {
    let plan = build_fill_plan(input)?;
    fill_workbook(template, &plan.sheet, &plan.headers, &plan.updates)
}

fn parse_bindings(steps: &[serde_json::Value]) -> Result<Vec<ColumnBinding>, XlsxWriterError> {
    let mut bindings = Vec::new();
    for step in steps {
        if step.get("type").and_then(serde_json::Value::as_str) == Some("map_columns") {
            let parsed: MapColumnsStep = serde_json::from_value(step.clone())
                .map_err(|error| contract_error(format!("invalid map_columns step: {error}")))?;
            if parsed.step_type != "map_columns" {
                return Err(contract_error("map_columns step discriminator changed"));
            }
            bindings.extend(parsed.bindings);
        }
    }
    if bindings.is_empty() {
        return Err(contract_error("effective TMD has no map_columns bindings"));
    }
    Ok(bindings)
}

fn validate_bindings(bindings: &[ColumnBinding]) -> Result<(), XlsxWriterError> {
    let mut columns = HashSet::new();
    let mut fields = HashSet::new();
    for binding in bindings {
        column_index(&binding.column)?;
        if binding.source_field.is_empty() || binding.header.is_empty() {
            return Err(contract_error(
                "map_columns source_field and header must be non-empty",
            ));
        }
        if !columns.insert(binding.column.as_str()) {
            return Err(contract_error(format!(
                "duplicate map_columns column: {}",
                binding.column
            )));
        }
        if !fields.insert(binding.source_field.as_str()) {
            return Err(contract_error(format!(
                "duplicate map_columns source_field: {}",
                binding.source_field
            )));
        }
    }
    Ok(())
}

fn validate_projection_shape(
    projection: &WbsXlsxProjection,
    bindings: &[ColumnBinding],
) -> Result<(), XlsxWriterError> {
    let expected_header = bindings
        .iter()
        .map(|binding| binding.header.as_str())
        .collect::<Vec<_>>();
    let actual_header = projection
        .header
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if actual_header != expected_header {
        return Err(contract_error(
            "projection header does not match the effective TMD bindings",
        ));
    }
    let expected_fields = bindings
        .iter()
        .map(|binding| binding.source_field.as_str())
        .collect::<BTreeSet<_>>();
    for (offset, row) in projection.rows.iter().enumerate() {
        let actual_fields = row.keys().map(String::as_str).collect::<BTreeSet<_>>();
        if actual_fields != expected_fields {
            return Err(contract_error(format!(
                "projection row {} keys do not match the effective TMD bindings",
                offset + 1
            )));
        }
    }
    Ok(())
}

fn row_updates(
    projection: &WbsXlsxProjection,
    bindings: &[ColumnBinding],
    first_data_row: u32,
) -> Result<Vec<CellUpdate>, XlsxWriterError> {
    let mut updates = Vec::with_capacity(projection.rows.len() * bindings.len());
    for (offset, row) in projection.rows.iter().enumerate() {
        let row_number = first_data_row
            .checked_add(u32::try_from(offset).map_err(|_| contract_error("too many rows"))?)
            .ok_or_else(|| contract_error("projection row overflow"))?;
        if row_number > MAX_EXCEL_ROW {
            return Err(contract_error("projection exceeds the Excel row limit"));
        }
        for binding in bindings {
            let value = row
                .get(&binding.source_field)
                .ok_or_else(|| contract_error("projection row lost a validated field"))?;
            updates.push(CellUpdate {
                coordinate: format!("{}{row_number}", binding.column),
                value: convert_value(value, binding)?,
            });
        }
    }
    Ok(updates)
}

fn convert_value(
    value: &ProjectionCellValue,
    binding: &ColumnBinding,
) -> Result<CellValue, XlsxWriterError> {
    match (binding.kind, value) {
        (ColumnKind::Text, ProjectionCellValue::Text(value)) => Ok(CellValue::Text(value.clone())),
        (ColumnKind::Text, ProjectionCellValue::Integer(value)) => {
            Ok(CellValue::Text(value.to_string()))
        }
        (ColumnKind::Text, ProjectionCellValue::Number(value)) if value.is_finite() => {
            Ok(CellValue::Text(value.to_string()))
        }
        (ColumnKind::Int, ProjectionCellValue::Integer(value)) => Ok(CellValue::Integer(*value)),
        (ColumnKind::Number, ProjectionCellValue::Integer(value)) => Ok(CellValue::Integer(*value)),
        (ColumnKind::Number, ProjectionCellValue::Number(value)) if value.is_finite() => {
            Ok(CellValue::Number(*value))
        }
        _ => Err(contract_error(format!(
            "projection value kind does not match source_field {:?}",
            binding.source_field
        ))),
    }
}

fn rollup_updates(
    projection: &WbsXlsxProjection,
    steps: &[serde_json::Value],
    bindings: &[ColumnBinding],
    first_data_row: u32,
) -> Result<Vec<CellUpdate>, XlsxWriterError> {
    if projection.rollup_groups.is_empty() {
        return Ok(Vec::new());
    }
    let steps = steps
        .iter()
        .filter(|step| step.get("type").and_then(serde_json::Value::as_str) == Some("rollup"))
        .map(|step| {
            serde_json::from_value::<RollupStep>(step.clone())
                .map_err(|error| contract_error(format!("invalid rollup step: {error}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    if steps.len() != 1 {
        return Err(contract_error(
            "a projection with rollup groups requires exactly one rollup step",
        ));
    }
    let step = &steps[0];
    if step.step_type != "rollup"
        || step.strategy != "sum_cells"
        || step.applies_to != "non_leaf_rows"
    {
        return Err(contract_error("unsupported rollup contract"));
    }
    column_index(&step.target_column)?;
    if !bindings
        .iter()
        .any(|binding| binding.column == step.target_column)
    {
        return Err(contract_error(
            "rollup target column is outside the header-validated bindings",
        ));
    }
    let last_data_row = first_data_row
        .checked_add(
            u32::try_from(projection.rows.len()).map_err(|_| contract_error("too many rows"))?,
        )
        .and_then(|value| value.checked_sub(1))
        .ok_or_else(|| contract_error("projection row range overflow"))?;
    let mut parents = HashSet::new();
    let mut updates = Vec::with_capacity(projection.rollup_groups.len());
    for group in &projection.rollup_groups {
        if !parents.insert(group.parent_row)
            || group.child_rows.is_empty()
            || std::iter::once(group.parent_row)
                .chain(group.child_rows.iter().copied())
                .any(|row| row < first_data_row || row > last_data_row)
        {
            return Err(contract_error(
                "rollup group is duplicated, empty, or outside the projected row range",
            ));
        }
        updates.push(CellUpdate {
            coordinate: format!("{}{row}", step.target_column, row = group.parent_row),
            value: CellValue::Formula(rollup_sum_formula(&step.target_column, &group.child_rows)?),
        });
    }
    Ok(updates)
}

fn column_index(column: &str) -> Result<u32, XlsxWriterError> {
    if column.is_empty()
        || column.len() > 3
        || !column.bytes().all(|byte| byte.is_ascii_uppercase())
    {
        return Err(contract_error(format!("invalid Excel column: {column:?}")));
    }
    let index = column
        .bytes()
        .try_fold(0_u32, |acc, byte| {
            acc.checked_mul(26)?.checked_add(u32::from(byte - b'A' + 1))
        })
        .ok_or_else(|| contract_error("Excel column overflow"))?;
    if index == 0 || index > MAX_EXCEL_COLUMN {
        return Err(contract_error(format!(
            "Excel column exceeds XFD: {column}"
        )));
    }
    Ok(index)
}

fn cell_sort_key(coordinate: &str) -> Result<(u32, u32), XlsxWriterError> {
    let split = coordinate
        .find(|character: char| character.is_ascii_digit())
        .ok_or_else(|| contract_error("cell coordinate has no row"))?;
    let column = column_index(&coordinate[..split])?;
    let row = coordinate[split..]
        .parse::<u32>()
        .map_err(|error| contract_error(format!("invalid cell row: {error}")))?;
    Ok((row, column))
}

fn contract_error(message: impl Into<String>) -> XlsxWriterError {
    XlsxWriterError::Malformed(message.into())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use maekon_core::models::effective_mapping::EffectiveMapping;
    use maekon_core::models::wbs_xlsx::{RollupCellGroup, WbsXlsxProjection};

    use super::*;

    fn input() -> EffectiveWbsXlsxProjection {
        let content = serde_json::json!({
            "schema_version": "tmd/1",
            "sheet": "WBS",
            "first_data_row": 2,
            "template_ref": {"template_id": "t", "version_label": "1", "content_hash": "a"},
            "input": {"sources": []},
            "steps": [
                {"type": "map_columns", "bindings": [
                    {"source_field": "name", "column": "C", "kind": "text", "header": "작업명"},
                    {"source_field": "level", "column": "B", "kind": "int", "header": "레벨"},
                    {"source_field": "effort", "column": "I", "kind": "number", "header": "공수"}
                ]},
                {"type": "rollup", "strategy": "sum_cells", "target_column": "I", "applies_to": "non_leaf_rows"}
            ]
        }).to_string();
        let mut mapping = EffectiveMapping {
            mapping_id: "map-1".into(),
            organization_id: "org-1".into(),
            version_id: "v1".into(),
            version_seq: 1,
            content_hash: String::new(),
            content,
            approval_seq: 1,
            approved_at: "2026-08-16T00:00:00+00:00".into(),
            approved_by_user_id: "u1".into(),
            approved_template_hash: "a".repeat(64),
            assignment_id: "asg-1".into(),
            assignment_hash: "b".repeat(64),
            source_snapshot_hash: "c".repeat(64),
        };
        mapping.content_hash = EffectiveMapping::hash_content(&mapping.content);
        EffectiveWbsXlsxProjection {
            effective: mapping,
            projection: WbsXlsxProjection {
                sheet: "WBS".into(),
                header: vec!["레벨".into(), "작업명".into(), "공수".into()],
                rows: vec![
                    BTreeMap::from([
                        ("name".into(), ProjectionCellValue::Text("루트".into())),
                        ("level".into(), ProjectionCellValue::Integer(1)),
                        ("effort".into(), ProjectionCellValue::Number(3.5)),
                    ]),
                    BTreeMap::from([
                        ("name".into(), ProjectionCellValue::Text("분석".into())),
                        ("level".into(), ProjectionCellValue::Integer(2)),
                        ("effort".into(), ProjectionCellValue::Number(3.5)),
                    ]),
                ],
                rollup_groups: vec![RollupCellGroup {
                    parent_row: 2,
                    child_rows: vec![3],
                }],
            },
        }
    }

    #[test]
    fn binds_source_fields_to_sorted_columns_and_rollup_formula() {
        let plan = build_fill_plan(&input()).unwrap();
        assert_eq!(
            plan.headers
                .iter()
                .map(|h| h.column.as_str())
                .collect::<Vec<_>>(),
            ["B", "C", "I"]
        );
        assert!(plan.updates.contains(&CellUpdate {
            coordinate: "B2".into(),
            value: CellValue::Integer(1)
        }));
        assert!(plan.updates.contains(&CellUpdate {
            coordinate: "C3".into(),
            value: CellValue::Text("분석".into())
        }));
        assert!(plan.updates.contains(&CellUpdate {
            coordinate: "I2".into(),
            value: CellValue::Formula("SUM(I3)".into())
        }));
    }

    #[test]
    fn mutation_control_rejects_header_or_row_key_drift() {
        let mut header = input();
        header.projection.header[0] = "깊이".into();
        let error = build_fill_plan(&header).expect_err("header drift must fail closed");
        assert!(matches!(
            &error,
            XlsxWriterError::Malformed(message) if message.contains("projection header")
        ));

        let mut row = input();
        row.projection.rows[0].remove("effort");
        let error = build_fill_plan(&row).expect_err("row key drift must fail closed");
        assert!(matches!(
            &error,
            XlsxWriterError::Malformed(message) if message.contains("projection row 1 keys")
        ));
    }

    #[test]
    fn mutation_control_rejects_rollup_outside_written_rows() {
        let mut value = input();
        value.projection.rollup_groups[0].child_rows = vec![99];
        let error = build_fill_plan(&value).expect_err("out-of-range rollup rows must fail closed");
        assert!(matches!(
            &error,
            XlsxWriterError::Malformed(message) if message.contains("outside the projected row range")
        ));
    }
}
