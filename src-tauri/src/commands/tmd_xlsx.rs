//! Standalone native-file WBS XLSX flow (#10358).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use maekon_core::models::effective_mapping::{MappingResolutionReason, MappingResolutionRejection};
use maekon_core::models::wbs_xlsx::{
    EffectiveWbsXlsxProjection, EffectiveWbsXlsxProjectionResolution, LocalWbsXlsxReceipt,
    WbsXlsxOutcome,
};
use maekon_core::ports::effective_mapping_cache::EffectiveMappingCache;
use maekon_core::ports::wbs_xlsx_client::WbsXlsxClient;
use maekon_core::ports::wbs_xlsx_receipt_store::WbsXlsxReceiptStore;
use maekon_storage::tmd_xlsx_plan::fill_projected_workbook;
use maekon_storage::tmd_xlsx_writer::{persist_atomic, template_structure_hash, XlsxFillOutcome};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::State;
use uuid::Uuid;

use crate::ipc_error::IpcError;

const CODE_UNAVAILABLE: &str = "service.unavailable";

pub struct TmdXlsxState {
    client: Mutex<Option<Arc<dyn WbsXlsxClient>>>,
    receipts: Mutex<Option<Arc<dyn WbsXlsxReceiptStore>>>,
    cache: Mutex<Option<Arc<dyn EffectiveMappingCache>>>,
}

impl TmdXlsxState {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            client: Mutex::new(None),
            receipts: Mutex::new(None),
            cache: Mutex::new(None),
        }
    }

    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    pub fn set(
        &self,
        client: Arc<dyn WbsXlsxClient>,
        receipts: Arc<dyn WbsXlsxReceiptStore>,
        cache: Arc<dyn EffectiveMappingCache>,
    ) {
        *self
            .client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(client);
        *self
            .receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(receipts);
        *self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(cache);
    }

    fn get(
        &self,
    ) -> Option<(
        Arc<dyn WbsXlsxClient>,
        Arc<dyn WbsXlsxReceiptStore>,
        Arc<dyn EffectiveMappingCache>,
    )> {
        let client = self
            .client
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        let receipts = self
            .receipts
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        let cache = self
            .cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()?;
        Some((client, receipts, cache))
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum GenerateTmdXlsxResult {
    Cancelled,
    GateRejected {
        receipt_id: String,
        reason_code: String,
        receipt_upload_pending: bool,
    },
    HeaderDrift {
        receipt_id: String,
        first_mismatch_column: String,
        receipt_upload_pending: bool,
    },
    Produced {
        receipt_id: String,
        output_path: String,
        artifact_sha256: String,
        row_count: usize,
        escaped_cell_count: usize,
        receipt_upload_pending: bool,
    },
}

#[tauri::command]
pub async fn generate_tmd_xlsx(
    app: tauri::AppHandle,
    organization_id: String,
    mapping_id: String,
    assignment_id: String,
    state: State<'_, TmdXlsxState>,
) -> Result<GenerateTmdXlsxResult, IpcError> {
    let (client, receipts, cache) = state.get().ok_or_else(|| {
        IpcError::new(
            CODE_UNAVAILABLE,
            "standalone TMD XLSX transport is not wired in this build",
        )
    })?;

    flush_pending_receipts(&client, &receipts).await;

    let resolution = client
        .resolve_effective_projection(&organization_id, &mapping_id, &assignment_id)
        .await
        .map_err(IpcError::from)?;
    let effective = match resolution {
        EffectiveWbsXlsxProjectionResolution::Effective(value) => {
            if let Err(error) = cache
                .store_server_validated(&value.effective, &canonical_now())
                .await
            {
                tracing::warn!(
                    error.code = error.code(),
                    "live WBS XLSX projection succeeded but its revalidation candidate was not cached"
                );
            }
            *value
        }
        EffectiveWbsXlsxProjectionResolution::Rejected(rejection) => {
            if let Err(error) = cache
                .invalidate(
                    &organization_id,
                    &rejection.mapping_id,
                    &rejection.assignment_id,
                )
                .await
            {
                tracing::warn!(
                    error.code = error.code(),
                    "WBS XLSX projection rejection could not invalidate its cached candidate"
                );
            }
            return record_gate_rejection(client, receipts, &organization_id, rejection).await;
        }
    };

    let Some(input_path) = pick_input_path(&app).await? else {
        return Ok(GenerateTmdXlsxResult::Cancelled);
    };
    let Some(output_path) = pick_output_path(&app, &assignment_id).await? else {
        return Ok(GenerateTmdXlsxResult::Cancelled);
    };
    let same_file_input = input_path.clone();
    let same_file_output = output_path.clone();
    let overwrites_template = tokio::task::spawn_blocking(move || {
        paths_refer_to_same_file(&same_file_input, &same_file_output)
    })
    .await
    .map_err(|error| {
        IpcError::new(
            "internal.generic",
            format!("file identity check task failed: {error}"),
        )
    })?;
    if overwrites_template {
        return Err(IpcError::new(
            "validation.invalid_arguments",
            "output path must not overwrite the selected template",
        ));
    }
    let template = tokio::fs::read(&input_path).await.map_err(IpcError::from)?;
    let structure_hash = template_structure_hash(&template).map_err(writer_error)?;
    let outcome = fill_projected_workbook(&template, &effective).map_err(writer_error)?;
    match outcome {
        XlsxFillOutcome::HeaderDrift(drift) => {
            let receipt = receipt_after_gate(
                &effective,
                WbsXlsxOutcome::HeaderDrift,
                Some("header_drift".into()),
                Some(structure_hash),
                None,
                None,
                None,
            );
            let upload_pending =
                record_and_upload(client, receipts, &organization_id, &receipt).await?;
            Ok(GenerateTmdXlsxResult::HeaderDrift {
                receipt_id: receipt.receipt_id,
                first_mismatch_column: drift.first_mismatch_column,
                receipt_upload_pending: upload_pending,
            })
        }
        XlsxFillOutcome::Written {
            bytes,
            escaped_cell_count,
        } => {
            let artifact_sha256 = sha256_hex(&bytes);
            let receipt = receipt_after_gate(
                &effective,
                WbsXlsxOutcome::Produced,
                None,
                Some(structure_hash),
                Some(artifact_sha256.clone()),
                Some(effective.projection.rows.len()),
                Some(escaped_cell_count),
            );
            receipts
                .append_pending(&organization_id, &receipt)
                .await
                .map_err(IpcError::from)?;
            let persisted_path = output_path.clone();
            tokio::task::spawn_blocking(move || persist_atomic(&persisted_path, &bytes))
                .await
                .map_err(|error| {
                    IpcError::new(
                        "internal.generic",
                        format!("XLSX write task failed: {error}"),
                    )
                })?
                .map_err(writer_error)?;
            let upload_pending =
                upload_recorded_receipt(client, receipts, &organization_id, &receipt).await?;
            Ok(GenerateTmdXlsxResult::Produced {
                receipt_id: receipt.receipt_id,
                output_path: output_path.display().to_string(),
                artifact_sha256,
                row_count: effective.projection.rows.len(),
                escaped_cell_count,
                receipt_upload_pending: upload_pending,
            })
        }
    }
}

async fn pick_input_path(app: &tauri::AppHandle) -> Result<Option<PathBuf>, IpcError> {
    use tauri_plugin_dialog::DialogExt;
    let dialog = app.dialog().clone();
    let selected = tokio::task::spawn_blocking(move || {
        dialog
            .file()
            .add_filter("Excel workbook", &["xlsx"])
            .blocking_pick_file()
    })
    .await
    .map_err(|error| {
        IpcError::new(
            "internal.generic",
            format!("input dialog task failed: {error}"),
        )
    })?;
    selected
        .map(|path| {
            path.as_path()
                .map(PathBuf::from)
                .ok_or_else(|| IpcError::new("validation.invalid_arguments", "invalid input path"))
        })
        .transpose()
}

async fn pick_output_path(
    app: &tauri::AppHandle,
    assignment_id: &str,
) -> Result<Option<PathBuf>, IpcError> {
    use tauri_plugin_dialog::DialogExt;
    let dialog = app.dialog().clone();
    let file_name = format!("wbs-{}.xlsx", safe_file_stem(assignment_id));
    let selected = tokio::task::spawn_blocking(move || {
        dialog
            .file()
            .set_file_name(file_name)
            .add_filter("Excel workbook", &["xlsx"])
            .blocking_save_file()
    })
    .await
    .map_err(|error| {
        IpcError::new(
            "internal.generic",
            format!("output dialog task failed: {error}"),
        )
    })?;
    selected
        .map(|path| {
            path.as_path()
                .map(PathBuf::from)
                .ok_or_else(|| IpcError::new("validation.invalid_arguments", "invalid output path"))
        })
        .transpose()
}

async fn record_gate_rejection(
    client: Arc<dyn WbsXlsxClient>,
    receipts: Arc<dyn WbsXlsxReceiptStore>,
    organization_id: &str,
    rejection: MappingResolutionRejection,
) -> Result<GenerateTmdXlsxResult, IpcError> {
    let reason_code = reason_code(rejection.reason_code).to_owned();
    let receipt = LocalWbsXlsxReceipt {
        receipt_id: Uuid::new_v4().to_string(),
        mapping_id: rejection.mapping_id,
        assignment_id: rejection.assignment_id,
        outcome: WbsXlsxOutcome::GateRejected,
        reason_code: Some(reason_code.clone()),
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
        produced_at: canonical_now(),
    };
    let upload_pending = record_and_upload(client, receipts, organization_id, &receipt).await?;
    Ok(GenerateTmdXlsxResult::GateRejected {
        receipt_id: receipt.receipt_id,
        reason_code,
        receipt_upload_pending: upload_pending,
    })
}

fn receipt_after_gate(
    effective: &EffectiveWbsXlsxProjection,
    outcome: WbsXlsxOutcome,
    reason_code: Option<String>,
    template_structure_hash: Option<String>,
    artifact_sha256: Option<String>,
    row_count: Option<usize>,
    escaped_cell_count: Option<usize>,
) -> LocalWbsXlsxReceipt {
    LocalWbsXlsxReceipt {
        receipt_id: Uuid::new_v4().to_string(),
        mapping_id: effective.effective.mapping_id.clone(),
        assignment_id: effective.effective.assignment_id.clone(),
        outcome,
        reason_code,
        artifact_sha256,
        row_count: row_count.map(|value| {
            u64::try_from(value).expect("supported XLSX targets use at most 64-bit usize")
        }),
        escaped_cell_count: escaped_cell_count.map(|value| {
            u64::try_from(value).expect("supported XLSX targets use at most 64-bit usize")
        }),
        template_structure_hash,
        mapping_content_hash: Some(effective.effective.content_hash.clone()),
        approved_template_hash: Some(effective.effective.approved_template_hash.clone()),
        assignment_hash: Some(effective.effective.assignment_hash.clone()),
        source_snapshot_hash: Some(effective.effective.source_snapshot_hash.clone()),
        approval_seq: Some(effective.effective.approval_seq),
        approved_at: Some(effective.effective.approved_at.clone()),
        produced_at: canonical_now(),
    }
}

async fn record_and_upload(
    client: Arc<dyn WbsXlsxClient>,
    receipts: Arc<dyn WbsXlsxReceiptStore>,
    organization_id: &str,
    receipt: &LocalWbsXlsxReceipt,
) -> Result<bool, IpcError> {
    receipts
        .append_pending(organization_id, receipt)
        .await
        .map_err(IpcError::from)?;
    upload_recorded_receipt(client, receipts, organization_id, receipt).await
}

async fn upload_recorded_receipt(
    client: Arc<dyn WbsXlsxClient>,
    receipts: Arc<dyn WbsXlsxReceiptStore>,
    organization_id: &str,
    receipt: &LocalWbsXlsxReceipt,
) -> Result<bool, IpcError> {
    match client.append_local_receipt(organization_id, receipt).await {
        Ok(_) => {
            receipts
                .mark_uploaded(&receipt.receipt_id, &canonical_now())
                .await
                .map_err(IpcError::from)?;
            Ok(false)
        }
        Err(error) => {
            tracing::warn!(
                receipt_id = %receipt.receipt_id,
                error.code = error.code(),
                "WBS XLSX receipt upload deferred; durable local receipt remains pending"
            );
            Ok(true)
        }
    }
}

async fn flush_pending_receipts(
    client: &Arc<dyn WbsXlsxClient>,
    receipts: &Arc<dyn WbsXlsxReceiptStore>,
) {
    let pending = match receipts.list_pending().await {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(
                error.code = error.code(),
                "could not load pending WBS XLSX receipts for retry"
            );
            return;
        }
    };
    for pending in pending {
        match client
            .append_local_receipt(&pending.organization_id, &pending.receipt)
            .await
        {
            Ok(_) => {
                if let Err(error) = receipts
                    .mark_uploaded(&pending.receipt.receipt_id, &canonical_now())
                    .await
                {
                    tracing::warn!(
                        receipt_id = %pending.receipt.receipt_id,
                        error.code = error.code(),
                        "uploaded WBS XLSX receipt could not be marked locally"
                    );
                }
            }
            Err(error) => {
                tracing::warn!(
                    receipt_id = %pending.receipt.receipt_id,
                    error.code = error.code(),
                    "pending WBS XLSX receipt retry deferred"
                );
            }
        }
    }
}

fn safe_file_stem(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = sanitized.trim_matches('.');
    if sanitized.is_empty() {
        "assignment".to_owned()
    } else {
        sanitized.chars().take(120).collect()
    }
}

fn paths_refer_to_same_file(input: &std::path::Path, output: &std::path::Path) -> bool {
    if input == output {
        return true;
    }
    if matches!(
        (std::fs::canonicalize(input), std::fs::canonicalize(output)),
        (Ok(input), Ok(output)) if input == output
    ) {
        return true;
    }
    // #11068: catches the case the two checks above miss — two different paths
    // that are hard links to one file, where writing the output would silently
    // destroy the input.
    //
    // This was three hand-written cfg branches. The Windows one used
    // `volume_serial_number`/`file_index`, which are unstable
    // (`windows_by_handle`, rust-lang/rust#63010), so `maekon-app` could never
    // compile for Windows on stable. `same_file` does exactly this job —
    // `dev`+`ino` on Unix, `GetFileInformationByHandle` on Windows — on stable,
    // and it was already in Cargo.lock and already audited, so it adds no
    // supply-chain surface.
    //
    // `unwrap_or(false)`: an unreadable path is not evidence that the two are
    // the same file, and the caller uses `true` to refuse the write.
    same_file::is_same_file(input, output).unwrap_or(false)
}

fn canonical_now() -> String {
    chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.6f+00:00")
        .to_string()
}

fn reason_code(reason: MappingResolutionReason) -> &'static str {
    match reason {
        MappingResolutionReason::NotApproved => "not_approved",
        MappingResolutionReason::MappingHashMismatch => "mapping_hash_mismatch",
        MappingResolutionReason::TemplateStale => "template_stale",
        MappingResolutionReason::ReceiptContractMismatch => "receipt_contract_mismatch",
        MappingResolutionReason::ReceiptInstanceStale => "receipt_instance_stale",
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn writer_error(error: maekon_storage::tmd_xlsx_writer::XlsxWriterError) -> IpcError {
    IpcError::new("validation.invalid_arguments", error.to_string())
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use maekon_core::error::CoreError;
    use maekon_core::error_codes::ServiceCode;
    use maekon_core::models::wbs_xlsx::UploadedWbsXlsxReceipt;
    use maekon_core::ports::wbs_xlsx_receipt_store::PendingWbsXlsxReceipt;

    use super::*;

    struct SequenceClient {
        events: Arc<Mutex<Vec<&'static str>>>,
        succeeds: bool,
    }

    #[async_trait]
    impl WbsXlsxClient for SequenceClient {
        async fn resolve_effective_projection(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<EffectiveWbsXlsxProjectionResolution, CoreError> {
            unreachable!("sequence test does not resolve a projection")
        }

        async fn append_local_receipt(
            &self,
            organization_id: &str,
            receipt: &LocalWbsXlsxReceipt,
        ) -> Result<UploadedWbsXlsxReceipt, CoreError> {
            self.events.lock().unwrap().push("upload");
            if !self.succeeds {
                return Err(CoreError::ServiceUnavailable {
                    code: ServiceCode::Unavailable,
                    message: "offline".into(),
                });
            }
            Ok(UploadedWbsXlsxReceipt {
                receipt: receipt.clone(),
                organization_id: organization_id.into(),
                origin: "client_local".into(),
                actor_id: Some("actor-1".into()),
                synthetic: false,
                seed_namespace: None,
            })
        }
    }

    struct SequenceReceiptStore {
        events: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait]
    impl WbsXlsxReceiptStore for SequenceReceiptStore {
        async fn append_pending(&self, _: &str, _: &LocalWbsXlsxReceipt) -> Result<(), CoreError> {
            self.events.lock().unwrap().push("append");
            Ok(())
        }

        async fn list_pending(&self) -> Result<Vec<PendingWbsXlsxReceipt>, CoreError> {
            Ok(Vec::new())
        }

        async fn mark_uploaded(&self, _: &str, _: &str) -> Result<(), CoreError> {
            self.events.lock().unwrap().push("mark");
            Ok(())
        }
    }

    fn gate_receipt() -> LocalWbsXlsxReceipt {
        LocalWbsXlsxReceipt {
            receipt_id: "receipt-1".into(),
            mapping_id: "mapping-1".into(),
            assignment_id: "assignment-1".into(),
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

    #[test]
    fn canonical_timestamp_matches_server_receipt_contract() {
        let value = canonical_now();
        assert_eq!(value.len(), 32);
        assert!(value.ends_with("+00:00"));
        assert_eq!(value.as_bytes()[19], b'.');
    }

    #[test]
    fn every_gate_reason_has_the_server_wire_value() {
        assert_eq!(
            reason_code(MappingResolutionReason::NotApproved),
            "not_approved"
        );
        assert_eq!(
            reason_code(MappingResolutionReason::TemplateStale),
            "template_stale"
        );
        assert_eq!(
            reason_code(MappingResolutionReason::ReceiptInstanceStale),
            "receipt_instance_stale"
        );
    }

    #[test]
    fn suggested_file_name_is_portable_across_supported_operating_systems() {
        assert_eq!(safe_file_stem("asg:2026/08\\16"), "asg_2026_08_16");
        assert_eq!(safe_file_stem("..."), "assignment");
        assert!(safe_file_stem(&"a".repeat(200)).len() <= 120);
    }

    #[test]
    fn output_alias_cannot_overwrite_the_selected_template() {
        let directory = tempfile::tempdir().unwrap();
        let input = directory.path().join("template.xlsx");
        let alias = directory.path().join("alias.xlsx");
        std::fs::write(&input, b"template").unwrap();
        std::fs::hard_link(&input, &alias).unwrap();

        assert!(paths_refer_to_same_file(&input, &input));
        assert!(paths_refer_to_same_file(&input, &alias));
        assert!(!paths_refer_to_same_file(
            &input,
            &directory.path().join("new-output.xlsx")
        ));
    }

    #[tokio::test]
    async fn local_receipt_is_recorded_before_additive_upload() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let client: Arc<dyn WbsXlsxClient> = Arc::new(SequenceClient {
            events: events.clone(),
            succeeds: true,
        });
        let receipts: Arc<dyn WbsXlsxReceiptStore> = Arc::new(SequenceReceiptStore {
            events: events.clone(),
        });

        assert!(
            !record_and_upload(client, receipts, "org-1", &gate_receipt())
                .await
                .unwrap()
        );
        assert_eq!(*events.lock().unwrap(), ["append", "upload", "mark"]);
    }

    #[tokio::test]
    async fn failed_additive_upload_leaves_the_local_receipt_pending() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let client: Arc<dyn WbsXlsxClient> = Arc::new(SequenceClient {
            events: events.clone(),
            succeeds: false,
        });
        let receipts: Arc<dyn WbsXlsxReceiptStore> = Arc::new(SequenceReceiptStore {
            events: events.clone(),
        });

        assert!(
            record_and_upload(client, receipts, "org-1", &gate_receipt())
                .await
                .unwrap()
        );
        assert_eq!(*events.lock().unwrap(), ["append", "upload"]);
    }
}
