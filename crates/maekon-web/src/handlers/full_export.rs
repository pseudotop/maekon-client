//! `GET /api/export/full` — GDPR Article 20 (data portability) full export (#8056 P2-3).
//!
//! The scoped `/export/{metrics,events,frames,ical,toggl}` routes and `/backup`
//! cover only events/frames/tags/metrics/settings. This route completes the
//! Art.20 obligation by dumping every OTHER personal-data table the device holds
//! (activity_segments incl. LLM summaries, suggestions, coaching_events,
//! habit_streaks, memory-graph claims/edges, frame_annotations,
//! ai_sessions/messages, focus_metrics, work_sessions, digests, …) into a single
//! structured JSON archive.
//!
//! Free-text PII columns are masked at Standard, FAIL-CLOSED (a missing
//! sanitizer errors the request rather than emitting unmasked content) — the
//! same policy as `/backup` and `/export/*`.

use axum::{
    extract::State,
    http::header,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use std::path::PathBuf;

use maekon_core::config::PiiFilterLevel;
use maekon_core::consent::{UserDataExport, CURRENT_POLICY_VERSION};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use maekon_core::ports::web_storage::PersonalDataTableExport;

use crate::error::ApiError;
use crate::services::web_contexts::BackupWebContext;

/// Standard, fail-closed — mirrors the `/backup` + `/export/*` masking floor.
const EXPORT_SANITIZE_LEVEL: PiiFilterLevel = PiiFilterLevel::Standard;

/// Column names (across the exported tables) that carry free-text personal
/// content and MUST be masked before the archive leaves the device. A generous
/// allowlist: over-masking an export is safe, under-masking leaks. Non-string
/// values under these keys are left untouched.
const MASKED_COLUMNS: &[&str] = &[
    "window_title",
    "ocr_text",
    "searchable_text",
    "content_activities_json",
    "llm_summary",
    "app_breakdown",
    "app_breakdown_json",
    "personalized_message",
    "message_template",
    "message",
    "content",
    "text",
    "memo",
    "note",
    "summary",
    "title",
    "description",
    "subject",
    "predicate",
    "object",
    "claim",
    "original_text",
    "payload",
    "app_name",
    // V49 (#8577, ADR-028): durable task free-text columns. Task content is
    // already PiiFilterLevel::Standard-sanitized at write time; masking these on
    // export is a defense-in-depth second layer (ADR-028 Amendment P4).
    "body",
    "reason",
    // V51 (#8587, ADR-030 §12 + Amendment M4): work-context projection free-text.
    // The projection plane's sanitized title/summary carry timeline/search
    // content; exporting the projections table WITHOUT masking these would leak
    // it. The existing `title`/`summary` entries above do NOT cover the
    // `sanitized_*` column names, so they are listed explicitly.
    "sanitized_title",
    "sanitized_summary",
];

fn mask_row(mut row: serde_json::Value, sanitizer: &dyn PiiSanitizer) -> serde_json::Value {
    if let serde_json::Value::Object(map) = &mut row {
        for key in MASKED_COLUMNS {
            if let Some(serde_json::Value::String(text)) = map.get_mut(*key) {
                *text = sanitizer.sanitize_text(text, EXPORT_SANITIZE_LEVEL);
            }
        }
    }
    row
}

fn mask_tables(
    tables: Vec<PersonalDataTableExport>,
    sanitizer: &dyn PiiSanitizer,
) -> Vec<PersonalDataTableExport> {
    tables
        .into_iter()
        .map(|table| PersonalDataTableExport {
            name: table.name,
            rows: table
                .rows
                .into_iter()
                .map(|row| mask_row(row, sanitizer))
                .collect(),
        })
        .collect()
}

/// `GET /api/export/full` — download the full Art.20 personal-data archive.
pub async fn export_full(State(context): State<BackupWebContext>) -> Result<Response, ApiError> {
    // Fail-closed: never emit an unmasked archive.
    let sanitizer = context.pii_sanitizer.as_deref().ok_or_else(|| {
        ApiError::Internal("PII sanitizer not configured for full export".to_string())
    })?;

    let raw_tables = context
        .storage
        .export_personal_data_tables()
        .await
        .map_err(|error| ApiError::Internal(error.to_string()))?;
    let tables = mask_tables(raw_tables, sanitizer);

    // The user's own settings (no raw secrets — those live in the OS keychain).
    let settings = context
        .config_manager
        .as_ref()
        .and_then(|cm| serde_json::to_value(cm.get()).ok())
        .unwrap_or(serde_json::Value::Null);

    let filename = format!(
        "maekon_data_export_{}.json",
        Utc::now().format("%Y%m%d_%H%M%S")
    );
    let export = UserDataExport {
        exported_at: Utc::now(),
        policy_version: CURRENT_POLICY_VERSION.to_string(),
        // Consent is exportable separately via /consent; the portability archive
        // focuses on the activity data the scoped exports omit.
        consent: None,
        settings,
        event_count: 0,
        frame_count: 0,
        export_path: PathBuf::from(&filename),
        tables,
    };

    let body = serde_json::to_string_pretty(&export)
        .map_err(|error| ApiError::Internal(format!("serialize export: {error}")))?;

    Ok((
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        body,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RedactAllSanitizer;
    impl PiiSanitizer for RedactAllSanitizer {
        fn sanitize_text(&self, _text: &str, _level: PiiFilterLevel) -> String {
            "[REDACTED]".to_string()
        }
    }

    #[test]
    fn mask_row_redacts_only_known_text_columns() {
        let row = serde_json::json!({
            "id": 7,
            "window_title": "Secret Q2 headcount.xlsx",
            "duration_secs": 42,
            "content": "personal notes",
        });
        let masked = mask_row(row, &RedactAllSanitizer);
        assert_eq!(masked["window_title"], "[REDACTED]");
        assert_eq!(masked["content"], "[REDACTED]");
        // Non-listed / non-string columns are untouched.
        assert_eq!(masked["id"], 7);
        assert_eq!(masked["duration_secs"], 42);
    }

    #[test]
    fn mask_row_redacts_work_context_projection_text() {
        // ADR-030 §12/M4: exporting the projection plane must mask its sanitized
        // title/summary — otherwise timeline/search content leaks in the archive.
        let row = serde_json::json!({
            "projection_id": "proj-1",
            "sanitized_title": "Q3 layoffs planning",
            "sanitized_summary": "confidential meeting notes",
            "access_epoch_id": 3,
        });
        let masked = mask_row(row, &RedactAllSanitizer);
        assert_eq!(masked["sanitized_title"], "[REDACTED]");
        assert_eq!(masked["sanitized_summary"], "[REDACTED]");
        // Non-text metadata is untouched.
        assert_eq!(masked["projection_id"], "proj-1");
        assert_eq!(masked["access_epoch_id"], 3);
    }

    #[test]
    fn mask_tables_masks_every_row() {
        let tables = vec![PersonalDataTableExport {
            name: "activity_segments".to_string(),
            rows: vec![
                serde_json::json!({"llm_summary": "reviewed the budget"}),
                serde_json::json!({"llm_summary": "wrote an email"}),
            ],
        }];
        let masked = mask_tables(tables, &RedactAllSanitizer);
        assert_eq!(masked[0].rows.len(), 2);
        assert_eq!(masked[0].rows[0]["llm_summary"], "[REDACTED]");
        assert_eq!(masked[0].rows[1]["llm_summary"], "[REDACTED]");
    }
}
