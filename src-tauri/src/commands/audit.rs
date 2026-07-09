//! Tauri IPC: audit log integrity verification (#4834, E20).
//!
//! #7600: `export_audit_log` (local JSON/CSV file export via native save-dialog,
//! #4819) was removed as a dead IPC duplicate — the React frontend never called
//! it, and `GET /audit/export` in the embedded HTTP API delivers the same
//! capability (a `Vec<AuditEntry>` JSON export).
//!
//! `verify_audit_log` remains: it **MUST be included in the default OSS build**,
//! so it is not placed behind a feature gate.

/// Verify the integrity of the local audit log's hash chain (#4834, E20).
///
/// Verifies the `audit_log` SHA-256 hash chain in durable `SqliteStorage`
/// (`AppState.storage`) and returns an [`AuditChainReport`]. This is tamper-**evident**
/// verification: it detects accidental/partial corruption, simple row edits, deletions, and
/// reordering (defense against a full-rewrite insider threat is out-of-scope — that needs
/// HMAC/Ed25519).
#[tauri::command]
pub async fn verify_audit_log(
    state: tauri::State<'_, crate::runtime_state::AppState>,
) -> Result<maekon_core::models::audit::AuditChainReport, crate::ipc_error::IpcError> {
    let storage = state.storage.clone();
    let report = tokio::task::spawn_blocking(move || storage.verify_audit_chain())
        .await
        .map_err(|e| {
            crate::ipc_error::IpcError::new(
                "internal.generic",
                format!("audit chain verify task failed: {e}"),
            )
        })?;
    Ok(report)
}
