//! Durable, privacy-safe audit rows for capture/privacy state transitions (#8094).
//!
//! Consent grant/revoke already write durable `audit_log` rows (see
//! [`crate::commands::consent`]). This module extends the same durable,
//! SHA-256-hash-chained trail to the OTHER privacy transitions the QC flagged as
//! silent — capture pause/resume and focus-mode enter/exit — so a reviewer can
//! reconstruct "when did capture/focus state change" from the exported audit
//! trail (`/audit/export`) and verify its integrity (`/audit/verify`).
//!
//! # Correlation id
//!
//! These rows use the `"system.privacy"` sentinel for `session_id`/`command_id`
//! (distinct from consent's `"system.consent"` and from per-automation-command
//! correlation ids), so they are queryable as a group without colliding with
//! automation-session audit correlation.
//!
//! # No raw sensitive context
//!
//! `details` carries only booleans/enums describing the transition (paused,
//! active, auto, duration) — never window titles, app names, OCR text, or any
//! captured content. This matches the ADR-072 privacy-safe audit contract.

use maekon_core::models::audit::{AuditEntry, AuditStatus};
use maekon_storage::sqlite::SqliteStorage;

/// The `session_id`/`command_id` sentinel for privacy/capture transition rows.
pub(crate) const PRIVACY_AUDIT_ID: &str = "system.privacy";

/// Stable `action_type` for a capture pause/resume transition.
///
/// Snake_case, mirroring the existing `consent_granted`/`consent_revoked` names.
pub fn capture_pause_action(paused: bool) -> &'static str {
    if paused {
        "capture_paused"
    } else {
        "capture_resumed"
    }
}

/// Stable `action_type` for a focus-mode enter/exit transition.
pub fn focus_mode_action(active: bool) -> &'static str {
    if active {
        "focus_mode_entered"
    } else {
        "focus_mode_exited"
    }
}

/// Writes one durable, privacy-safe `audit_log` row for a privacy/capture
/// transition.
///
/// Blocking SQLite write — callers on an async reactor must offload it (e.g.
/// `tauri::async_runtime::spawn_blocking`), matching the consent audit path.
/// Best-effort by contract: a failed audit write must never fail the underlying
/// privacy action (the pause/focus toggle itself already succeeded).
pub fn audit_privacy_transition(
    storage: &SqliteStorage,
    action_type: &str,
    details: serde_json::Value,
) {
    storage.save_audit_entry(&AuditEntry {
        entry_id: maekon_core::id_generation::generate_id("audit"),
        timestamp: chrono::Utc::now(),
        session_id: PRIVACY_AUDIT_ID.into(),
        command_id: PRIVACY_AUDIT_ID.into(),
        action_type: action_type.into(),
        // There is no dedicated transition variant; a recorded transition is a
        // completed side effect (mirrors the consent audit's `Completed`).
        status: AuditStatus::Completed,
        details: Some(details.to_string()),
        execution_time_ms: None,
    });
}

/// Records a capture pause/resume transition (offloaded, best-effort).
///
/// `details` carries only the resulting `paused` boolean — no captured content.
/// `tauri::async_runtime::spawn` is safe from any caller context (including the
/// tray callback, which may not be inside a Tokio runtime), and the inner
/// `tokio::task::spawn_blocking` keeps the blocking SQLite write off the async
/// worker (F-RR-06 async-safety).
pub fn spawn_audit_capture_pause(storage: std::sync::Arc<SqliteStorage>, paused: bool) {
    let action = capture_pause_action(paused);
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            audit_privacy_transition(&storage, action, serde_json::json!({ "paused": paused }));
        })
        .await;
    });
}

/// Records a focus-mode enter/exit transition (offloaded, best-effort).
///
/// `details` carries only booleans/duration — no captured content.
pub fn spawn_audit_focus_mode(
    storage: std::sync::Arc<SqliteStorage>,
    active: bool,
    auto: bool,
    duration_minutes: Option<u32>,
) {
    let action = focus_mode_action(active);
    tauri::async_runtime::spawn(async move {
        let _ = tokio::task::spawn_blocking(move || {
            audit_privacy_transition(
                &storage,
                action,
                serde_json::json!({
                    "active": active,
                    "auto": auto,
                    "duration_minutes": duration_minutes,
                }),
            );
        })
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_pause_action_names_are_stable() {
        assert_eq!(capture_pause_action(true), "capture_paused");
        assert_eq!(capture_pause_action(false), "capture_resumed");
    }

    #[test]
    fn focus_mode_action_names_are_stable() {
        assert_eq!(focus_mode_action(true), "focus_mode_entered");
        assert_eq!(focus_mode_action(false), "focus_mode_exited");
    }

    /// A privacy transition writes a durable `audit_log` row that is queryable by
    /// the `system.privacy` correlation id — the same durable trail consent uses,
    /// so it is exportable via `/audit/export` and covered by the hash chain.
    #[test]
    fn audit_privacy_transition_writes_durable_queryable_row() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap();

        audit_privacy_transition(
            &storage,
            capture_pause_action(true),
            serde_json::json!({ "paused": true }),
        );

        let rows = storage.entries_by_command_id(PRIVACY_AUDIT_ID, 10);
        let entry = rows
            .iter()
            .find(|e| e.action_type == "capture_paused")
            .expect("a capture_paused row must be queryable by the system.privacy id");
        assert_eq!(entry.session_id, PRIVACY_AUDIT_ID);
        assert_eq!(entry.status, AuditStatus::Completed);
        let details: serde_json::Value =
            serde_json::from_str(entry.details.as_ref().expect("details")).expect("details json");
        assert_eq!(details["paused"], serde_json::json!(true));
    }

    /// Privacy-safe payloads must carry only booleans/enums — never captured
    /// content. Pin that a focus transition's serialized details contain no
    /// free-text content keys.
    #[test]
    fn focus_transition_details_carry_no_raw_content() {
        let dir = tempfile::tempdir().unwrap();
        let storage = SqliteStorage::open(&dir.path().join("s.db"), 1, None).unwrap();

        audit_privacy_transition(
            &storage,
            focus_mode_action(true),
            serde_json::json!({ "active": true, "auto": false, "duration_minutes": 25 }),
        );

        let rows = storage.entries_by_command_id(PRIVACY_AUDIT_ID, 10);
        let entry = rows
            .iter()
            .find(|e| e.action_type == "focus_mode_entered")
            .expect("a focus_mode_entered row must exist");
        let details = entry.details.as_ref().expect("details");
        for forbidden in ["window_title", "app_name", "ocr", "text", "content"] {
            assert!(
                !details.contains(forbidden),
                "privacy-safe audit details must not contain a `{forbidden}` field: {details}"
            );
        }
    }
}
