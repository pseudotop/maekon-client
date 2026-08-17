//! SQLite implementation of the local WBS XLSX receipt spool (#10358).

use async_trait::async_trait;
use chrono::DateTime;
use maekon_core::error::CoreError;
use maekon_core::models::wbs_xlsx::{LocalWbsXlsxReceipt, WbsXlsxOutcome};
use maekon_core::ports::wbs_xlsx_receipt_store::{PendingWbsXlsxReceipt, WbsXlsxReceiptStore};
use rusqlite::{params, OptionalExtension};

use crate::error::StorageError;

use super::SqliteStorage;

#[async_trait]
impl WbsXlsxReceiptStore for SqliteStorage {
    async fn append_pending(
        &self,
        organization_id: &str,
        receipt: &LocalWbsXlsxReceipt,
    ) -> Result<(), CoreError> {
        validate_receipt(organization_id, receipt)?;
        let organization_id = organization_id.to_owned();
        let receipt = receipt.clone();
        let receipt_json = serde_json::to_string(&receipt).map_err(|error| {
            StorageError::Internal(format!("encode local XLSX receipt: {error}"))
        })?;
        self.with_conn(move |conn| {
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT organization_id, receipt_json
                     FROM wbs_xlsx_output_receipts WHERE receipt_id = ?1",
                    params![receipt.receipt_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|error| {
                    StorageError::Internal(format!("read local XLSX receipt: {error}"))
                })?;
            if let Some((stored_organization_id, stored_json)) = existing {
                if stored_organization_id == organization_id && stored_json == receipt_json {
                    return Ok(());
                }
                return Err(StorageError::Validation {
                    field: "receipt_id".into(),
                    message: "local XLSX receipt id already has different content".into(),
                });
            }
            conn.execute(
                "INSERT INTO wbs_xlsx_output_receipts (
                    receipt_id, organization_id, mapping_id, assignment_id,
                    receipt_json, produced_at, upload_state, uploaded_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', NULL)",
                params![
                    receipt.receipt_id,
                    organization_id,
                    receipt.mapping_id,
                    receipt.assignment_id,
                    receipt_json,
                    receipt.produced_at,
                ],
            )
            .map_err(|error| {
                StorageError::Internal(format!("append local XLSX receipt: {error}"))
            })?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn list_pending(&self) -> Result<Vec<PendingWbsXlsxReceipt>, CoreError> {
        self.with_conn_read(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT organization_id, receipt_json
                     FROM wbs_xlsx_output_receipts
                     WHERE upload_state = 'pending'
                     ORDER BY produced_at, receipt_id
                     LIMIT 100",
                )
                .map_err(|error| {
                    StorageError::Internal(format!("prepare pending XLSX receipts: {error}"))
                })?;
            let receipts = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(|error| {
                    StorageError::Internal(format!("query pending XLSX receipts: {error}"))
                })?
                .map(|row| {
                    let (organization_id, receipt_json) = row.map_err(|error| {
                        StorageError::Internal(format!("read pending XLSX receipt row: {error}"))
                    })?;
                    let receipt = serde_json::from_str(&receipt_json).map_err(|error| {
                        StorageError::Internal(format!("decode pending XLSX receipt: {error}"))
                    })?;
                    Ok(PendingWbsXlsxReceipt {
                        organization_id,
                        receipt,
                    })
                })
                .collect();
            receipts
        })
        .await
        .map_err(Into::into)
    }

    async fn mark_uploaded(&self, receipt_id: &str, uploaded_at: &str) -> Result<(), CoreError> {
        if receipt_id.is_empty() {
            return Err(StorageError::Validation {
                field: "receipt_id".into(),
                message: "local XLSX receipt id is empty".into(),
            }
            .into());
        }
        DateTime::parse_from_rfc3339(uploaded_at).map_err(|error| StorageError::Validation {
            field: "uploaded_at".into(),
            message: format!("local XLSX receipt upload timestamp is invalid: {error}"),
        })?;
        let receipt_id = receipt_id.to_owned();
        let uploaded_at = uploaded_at.to_owned();
        self.with_conn(move |conn| {
            let changed = conn
                .execute(
                    "UPDATE wbs_xlsx_output_receipts
                     SET upload_state = 'uploaded', uploaded_at = ?2
                     WHERE receipt_id = ?1",
                    params![receipt_id, uploaded_at],
                )
                .map_err(|error| {
                    StorageError::Internal(format!("mark local XLSX receipt uploaded: {error}"))
                })?;
            if changed != 1 {
                return Err(StorageError::Validation {
                    field: "receipt_id".into(),
                    message: "local XLSX receipt does not exist".into(),
                });
            }
            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

fn validate_receipt(
    organization_id: &str,
    receipt: &LocalWbsXlsxReceipt,
) -> Result<(), StorageError> {
    for (field, value) in [
        ("organization_id", organization_id),
        ("receipt_id", receipt.receipt_id.as_str()),
        ("mapping_id", receipt.mapping_id.as_str()),
        ("assignment_id", receipt.assignment_id.as_str()),
    ] {
        if value.is_empty() {
            return Err(StorageError::Validation {
                field: field.into(),
                message: "local XLSX receipt identifier is empty".into(),
            });
        }
    }
    let produced = DateTime::parse_from_rfc3339(&receipt.produced_at).map_err(|error| {
        StorageError::Validation {
            field: "produced_at".into(),
            message: format!("local XLSX receipt timestamp is invalid: {error}"),
        }
    })?;
    if produced.offset().local_minus_utc() != 0 || !is_canonical_utc(&receipt.produced_at) {
        return Err(StorageError::Validation {
            field: "produced_at".into(),
            message: "local XLSX receipt timestamp is not canonical UTC".into(),
        });
    }

    let artifacts = (
        receipt.artifact_sha256.as_ref(),
        receipt.row_count,
        receipt.escaped_cell_count,
    );
    let gates = (
        receipt.template_structure_hash.as_ref(),
        receipt.mapping_content_hash.as_ref(),
        receipt.approved_template_hash.as_ref(),
        receipt.assignment_hash.as_ref(),
        receipt.source_snapshot_hash.as_ref(),
        receipt.approval_seq,
        receipt.approved_at.as_ref(),
    );
    let valid = match receipt.outcome {
        WbsXlsxOutcome::Produced => {
            receipt.reason_code.is_none()
                && artifacts.0.is_some()
                && artifacts.1.is_some()
                && artifacts.2.is_some()
                && gates.0.is_some()
                && gates.1.is_some()
                && gates.2.is_some()
                && gates.3.is_some()
                && gates.4.is_some()
                && gates.5.is_some_and(|value| value >= 1)
                && gates.6.is_some()
        }
        WbsXlsxOutcome::GateRejected => {
            receipt
                .reason_code
                .as_ref()
                .is_some_and(|value| !value.is_empty())
                && artifacts.0.is_none()
                && artifacts.1.is_none()
                && artifacts.2.is_none()
                && gates.0.is_none()
                && gates.1.is_none()
                && gates.2.is_none()
                && gates.3.is_none()
                && gates.4.is_none()
                && gates.5.is_none()
                && gates.6.is_none()
        }
        WbsXlsxOutcome::HeaderDrift => {
            receipt
                .reason_code
                .as_ref()
                .is_some_and(|value| !value.is_empty())
                && artifacts.0.is_none()
                && artifacts.1.is_none()
                && artifacts.2.is_none()
                && gates.0.is_some()
                && gates.1.is_some()
                && gates.2.is_some()
                && gates.3.is_some()
                && gates.4.is_some()
                && gates.5.is_some_and(|value| value >= 1)
                && gates.6.is_some()
        }
    };
    if !valid {
        return Err(StorageError::Validation {
            field: "outcome".into(),
            message: "local XLSX receipt columns contradict its outcome".into(),
        });
    }
    for (field, digest) in [
        ("artifact_sha256", receipt.artifact_sha256.as_deref()),
        (
            "template_structure_hash",
            receipt.template_structure_hash.as_deref(),
        ),
    ] {
        if digest.is_some_and(|value| {
            value.len() != 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(StorageError::Validation {
                field: field.into(),
                message: "local XLSX receipt digest is not lowercase SHA-256".into(),
            });
        }
    }
    Ok(())
}

fn is_canonical_utc(value: &str) -> bool {
    let bytes = value.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':'), (16, b':')];
    if !matches!(bytes.len(), 25 | 32)
        || separators
            .iter()
            .any(|(index, expected)| bytes.get(*index) != Some(expected))
    {
        return false;
    }
    let base_digits = bytes[..19]
        .iter()
        .enumerate()
        .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit());
    base_digits
        && match bytes.len() {
            25 => &bytes[19..] == b"+00:00",
            32 => {
                bytes[19] == b'.'
                    && bytes[20..26].iter().all(u8::is_ascii_digit)
                    && &bytes[26..] == b"+00:00"
            }
            _ => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> LocalWbsXlsxReceipt {
        LocalWbsXlsxReceipt {
            receipt_id: "r1".into(),
            mapping_id: "m1".into(),
            assignment_id: "a1".into(),
            outcome: WbsXlsxOutcome::GateRejected,
            reason_code: Some("not_approved".into()),
            artifact_sha256: None,
            row_count: None,
            escaped_cell_count: None,
            template_structure_hash: None,
            mapping_content_hash: None,
            approved_template_hash: None,
            assignment_hash: None,
            source_snapshot_hash: None,
            approval_seq: None,
            approved_at: None,
            produced_at: "2026-08-16T00:00:00+00:00".into(),
        }
    }

    #[tokio::test]
    async fn append_is_idempotent_and_upload_keeps_local_evidence() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let receipt = receipt();
        storage.append_pending("org-1", &receipt).await.unwrap();
        storage.append_pending("org-1", &receipt).await.unwrap();
        assert_eq!(storage.list_pending().await.unwrap().len(), 1);
        storage
            .mark_uploaded("r1", "2026-08-16T00:01:00+00:00")
            .await
            .unwrap();
        assert!(storage.list_pending().await.unwrap().is_empty());
        let count: i64 = storage
            .with_conn_read(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM wbs_xlsx_output_receipts WHERE receipt_id = 'r1'",
                    [],
                    |row| row.get(0),
                )
                .map_err(StorageError::from)
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn mutation_control_rejects_same_id_with_changed_content() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let receipt = receipt();
        storage.append_pending("org-1", &receipt).await.unwrap();
        let mut changed = receipt;
        changed.reason_code = Some("template_stale".into());
        let error = storage
            .append_pending("org-1", &changed)
            .await
            .expect_err("a reused receipt id with changed content must fail closed");
        assert_eq!(error.code(), "validation.invalid_field");
        assert!(error.to_string().contains("already has different content"));
    }
}
