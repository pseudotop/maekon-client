use maekon_api_contracts::export::{EventExportRecord, FrameExportRecord, MetricExportRecord};
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::storage_records::{
    EventExportRecord as EventExportRow, FrameExportRecord as FrameExportRow,
    MetricExportRecord as MetricExportRow,
};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;
use std::sync::Arc;

use crate::error::ApiError;

/// D5 iter-15: export belt-and-suspenders sanitization. Storage ingest
/// paths (iter-3 OCR, iter-11 FileAccess, capture-time window_title at
/// iter-14 boundary) already sanitize before write. This second pass at
/// export-time catches any row that landed pre-D5 without re-running a
/// full-DB migration.
///
/// Apply `PiiFilterLevel::Standard` at export — matches the most common
/// user-configured level and doesn't upgrade beyond user intent.
///
/// `maekon-web` is an adapter crate — uses injected `PiiSanitizer` port
/// (not direct `maekon-vision::privacy::sanitize_title_with_level`).
const EXPORT_SANITIZE_LEVEL: PiiFilterLevel = PiiFilterLevel::Standard;

fn require_export_sanitizer(
    sanitizer: &Option<Arc<dyn PiiSanitizer>>,
) -> Result<&dyn PiiSanitizer, ApiError> {
    sanitizer.as_deref().ok_or_else(|| {
        ApiError::Internal("PII sanitizer not configured for export assembly".to_string())
    })
}

fn export_sanitize(s: String, sanitizer: &dyn PiiSanitizer) -> String {
    sanitizer.sanitize_text(&s, EXPORT_SANITIZE_LEVEL)
}

fn export_sanitize_opt(s: Option<String>, sanitizer: &dyn PiiSanitizer) -> Option<String> {
    s.map(|v| export_sanitize(v, sanitizer))
}

pub(crate) fn assemble_metric_export_record(row: MetricExportRow) -> MetricExportRecord {
    let memory_percent = if row.memory_total > 0 {
        (row.memory_used as f32 / row.memory_total as f32) * 100.0
    } else {
        0.0
    };

    // Metric records have no user-text fields — sanitization is a no-op
    // but kept here for structural consistency with event/frame assemblers.
    MetricExportRecord {
        timestamp: row.timestamp,
        cpu_usage: row.cpu_usage,
        memory_used: row.memory_used,
        memory_total: row.memory_total,
        memory_percent,
        disk_used: row.disk_used,
        disk_total: row.disk_total,
        network_upload: row.network_upload,
        network_download: row.network_download,
    }
}

pub(crate) fn assemble_event_export_record(
    row: EventExportRow,
    sanitizer: &Option<Arc<dyn PiiSanitizer>>,
) -> Result<EventExportRecord, ApiError> {
    let sanitizer = require_export_sanitizer(sanitizer)?;
    Ok(EventExportRecord {
        event_id: row.event_id,
        event_type: row.event_type,
        timestamp: row.timestamp,
        app_name: export_sanitize_opt(row.app_name, sanitizer),
        window_title: export_sanitize_opt(row.window_title, sanitizer),
    })
}

pub(crate) fn assemble_frame_export_record(
    row: FrameExportRow,
    sanitizer: &Option<Arc<dyn PiiSanitizer>>,
) -> Result<FrameExportRecord, ApiError> {
    let sanitizer = require_export_sanitizer(sanitizer)?;
    Ok(FrameExportRecord {
        id: row.id,
        timestamp: row.timestamp,
        trigger_type: row.trigger_type,
        app_name: export_sanitize(row.app_name, sanitizer),
        window_title: export_sanitize(row.window_title, sanitizer),
        importance: row.importance,
        resolution: format!("{}x{}", row.resolution_w, row.resolution_h),
        ocr_text: export_sanitize_opt(row.ocr_text, sanitizer),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::ports::pii_sanitizer::FakePiiSanitizer;

    fn event_row() -> EventExportRow {
        EventExportRow {
            event_id: "event-1".to_string(),
            event_type: "window_focus".to_string(),
            timestamp: "2026-05-25T15:12:00Z".to_string(),
            app_name: Some("Mail alice@example.com".to_string()),
            window_title: Some("Call 010-1234-5678".to_string()),
        }
    }

    #[test]
    fn event_export_fails_closed_when_sanitizer_missing() {
        let err = assemble_event_export_record(event_row(), &None).unwrap_err();
        assert!(
            matches!(err, ApiError::Internal(_)),
            "missing sanitizer must return ApiError::Internal, got: {err:?}"
        );
        assert!(
            err.to_string().contains("PII sanitizer not configured"),
            "error message mismatch, got: {err:?}"
        );
    }

    #[test]
    fn event_export_sanitizes_user_text_fields() {
        let sanitizer: Arc<dyn PiiSanitizer> = Arc::new(
            FakePiiSanitizer::new()
                .with_email("alice@example.com")
                .with_phone("010-1234-5678"),
        );
        let record = assemble_event_export_record(event_row(), &Some(sanitizer)).unwrap();

        assert_eq!(record.app_name.as_deref(), Some("Mail [EMAIL]"));
        assert_eq!(record.window_title.as_deref(), Some("Call [PHONE]"));
    }
}
