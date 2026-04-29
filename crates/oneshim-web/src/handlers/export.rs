use axum::{
    extract::{Query, State},
    response::Response,
};
use oneshim_api_contracts::export::ExportQuery;
#[cfg(test)]
use oneshim_api_contracts::export::{EventExportRecord, MetricExportRecord};

use crate::error::ApiError;
use crate::services::export_service::ExportQueryService;
#[cfg(test)]
use crate::services::export_service::{records_to_csv, sessions_to_ical, sessions_to_toggl_csv};
use crate::services::web_contexts::StorageWebContext;

pub async fn export_metrics(
    State(context): State<StorageWebContext>,
    Query(params): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    ExportQueryService::new(context).export_metrics(&params)
}

pub async fn export_events(
    State(context): State<StorageWebContext>,
    Query(params): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    ExportQueryService::new(context).export_events(&params)
}

pub async fn export_frames(
    State(context): State<StorageWebContext>,
    Query(params): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    ExportQueryService::new(context).export_frames(&params)
}

/// GET /api/export/ical — export work sessions as iCalendar (.ics)
pub async fn export_ical(
    State(context): State<StorageWebContext>,
    Query(params): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    ExportQueryService::new(context).export_ical(&params)
}

/// GET /api/export/toggl — export work sessions in Toggl-compatible CSV
pub async fn export_toggl(
    State(context): State<StorageWebContext>,
    Query(params): Query<ExportQuery>,
) -> Result<Response, ApiError> {
    ExportQueryService::new(context).export_toggl(&params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escapes_special_chars() {
        let records = vec![EventExportRecord {
            event_id: "1".to_string(),
            event_type: "context".to_string(),
            timestamp: "2024-01-30T10:00:00Z".to_string(),
            app_name: Some("VS Code".to_string()),
            window_title: Some("file.rs, modified".to_string()), // includes comma
        }];
        let csv = records_to_csv(&records).unwrap();
        assert!(csv.contains("\"file.rs, modified\"")); // quoted field
    }

    #[test]
    fn empty_records_returns_empty_csv() {
        let records: Vec<MetricExportRecord> = vec![];
        let csv = records_to_csv(&records).unwrap();
        assert!(csv.is_empty());
    }

    #[test]
    fn default_format_is_json() {
        let query: ExportQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(query.format, "json");
    }

    fn make_test_session(
        id: i64,
        app: &str,
        category: &str,
        duration: u64,
    ) -> oneshim_core::models::storage_records::FocusWorkSessionRecord {
        oneshim_core::models::storage_records::FocusWorkSessionRecord {
            id,
            started_at: "2026-03-20T09:00:00+00:00".to_string(),
            ended_at: Some("2026-03-20T10:00:00+00:00".to_string()),
            primary_app: app.to_string(),
            category: category.to_string(),
            state: "completed".to_string(),
            interruption_count: 2,
            deep_work_secs: 3000,
            duration_secs: duration,
        }
    }

    #[test]
    fn ical_export_produces_valid_calendar() {
        let sessions = vec![
            make_test_session(1, "VS Code", "Development", 3600),
            make_test_session(2, "Slack", "Communication", 1800),
        ];
        let ical = sessions_to_ical(&sessions);

        assert!(ical.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ical.ends_with("END:VCALENDAR\r\n"));
        assert!(ical.contains("BEGIN:VEVENT\r\n"));
        assert!(ical.contains("END:VEVENT\r\n"));
        assert!(ical.contains("DTSTART:20260320T090000Z\r\n"));
        assert!(ical.contains("DTEND:20260320T100000Z\r\n"));
        assert!(ical.contains("SUMMARY:VS Code (Development)\r\n"));
        assert!(ical.contains("SUMMARY:Slack (Communication)\r\n"));
        assert!(ical.contains("UID:session-1@oneshim\r\n"));
        assert!(ical.contains("VERSION:2.0\r\n"));
    }

    #[test]
    fn ical_export_skips_active_sessions() {
        let mut session = make_test_session(1, "Code", "Development", 0);
        session.ended_at = None;
        let ical = sessions_to_ical(&[session]);

        assert!(!ical.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn ical_export_empty_sessions() {
        let ical = sessions_to_ical(&[]);
        assert!(ical.starts_with("BEGIN:VCALENDAR\r\n"));
        assert!(ical.ends_with("END:VCALENDAR\r\n"));
        assert!(!ical.contains("VEVENT"));
    }

    #[test]
    fn toggl_csv_produces_correct_format() {
        let sessions = vec![make_test_session(1, "VS Code", "Development", 3661)];
        let csv = sessions_to_toggl_csv(&sessions);

        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "Description,Start date,Start time,Duration,Tags");
        assert!(lines[1].contains("VS Code (Development)"));
        assert!(lines[1].contains("2026-03-20"));
        assert!(lines[1].contains("09:00:00"));
        assert!(lines[1].contains("01:01:01")); // 3661s = 1h 1m 1s
        assert!(lines[1].contains("Development"));
    }

    #[test]
    fn toggl_csv_empty_sessions() {
        let csv = sessions_to_toggl_csv(&[]);
        assert_eq!(csv.lines().count(), 1); // header only
        assert!(csv.starts_with("Description,Start date,Start time,Duration,Tags"));
    }

    #[test]
    fn toggl_csv_escapes_commas_in_description() {
        let session = make_test_session(1, "App, With Comma", "Development", 60);
        let csv = sessions_to_toggl_csv(&[session]);
        // Description should be quoted
        assert!(csv.contains("\"App, With Comma (Development)\""));
    }
}
