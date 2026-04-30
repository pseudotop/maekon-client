use axum::http::header;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Duration, Utc};
use oneshim_api_contracts::export::{
    EventExportRecord, ExportQuery, FrameExportRecord, MetricExportRecord,
};
use oneshim_core::models::storage_records::FocusWorkSessionRecord;
use serde::Serialize;

use crate::error::ApiError;
use crate::services::export_assembler::{
    assemble_event_export_record, assemble_frame_export_record, assemble_metric_export_record,
};
use crate::services::web_contexts::StorageWebContext;

#[derive(Clone)]
pub struct ExportQueryService {
    ctx: StorageWebContext,
}

impl ExportQueryService {
    pub fn new(ctx: StorageWebContext) -> Self {
        Self { ctx }
    }

    pub fn export_metrics(&self, params: &ExportQuery) -> Result<Response, ApiError> {
        let (from, to) = resolve_export_range(params);
        let records: Vec<MetricExportRecord> = self
            .ctx
            .storage
            .list_metric_exports(&from.to_rfc3339(), &to.to_rfc3339())
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .into_iter()
            .map(assemble_metric_export_record)
            .collect();

        export_response(&records, &params.format, "metrics")
    }

    pub fn export_events(&self, params: &ExportQuery) -> Result<Response, ApiError> {
        let (from, to) = resolve_export_range(params);
        let records: Vec<EventExportRecord> = self
            .ctx
            .storage
            .list_event_exports(&from.to_rfc3339(), &to.to_rfc3339())
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .into_iter()
            .map(|row| assemble_event_export_record(row, &self.ctx.pii_sanitizer))
            .collect();

        export_response(&records, &params.format, "events")
    }

    pub fn export_frames(&self, params: &ExportQuery) -> Result<Response, ApiError> {
        let (from, to) = resolve_export_range(params);
        let records: Vec<FrameExportRecord> = self
            .ctx
            .storage
            .list_frame_exports(&from.to_rfc3339(), &to.to_rfc3339())
            .map_err(|error| ApiError::Internal(error.to_string()))?
            .into_iter()
            .map(|row| assemble_frame_export_record(row, &self.ctx.pii_sanitizer))
            .collect();

        export_response(&records, &params.format, "frames")
    }

    /// Export work sessions as iCalendar (.ics) VEVENT entries.
    pub fn export_ical(&self, params: &ExportQuery) -> Result<Response, ApiError> {
        let (from, to) = resolve_export_range(params);
        let sessions = self
            .ctx
            .storage
            .list_work_sessions(&from.to_rfc3339(), &to.to_rfc3339(), 1000)
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        let ical = sessions_to_ical(&sessions);
        let now = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = format!("sessions_{now}.ics");

        Ok((
            [
                (header::CONTENT_TYPE, "text/calendar; charset=utf-8"),
                (
                    header::CONTENT_DISPOSITION,
                    &format!("attachment; filename=\"{filename}\""),
                ),
            ],
            ical,
        )
            .into_response())
    }

    /// Export work sessions in Toggl-compatible CSV format.
    pub fn export_toggl(&self, params: &ExportQuery) -> Result<Response, ApiError> {
        let (from, to) = resolve_export_range(params);
        let sessions = self
            .ctx
            .storage
            .list_work_sessions(&from.to_rfc3339(), &to.to_rfc3339(), 1000)
            .map_err(|error| ApiError::Internal(error.to_string()))?;

        let csv = sessions_to_toggl_csv(&sessions);
        let now = Utc::now().format("%Y%m%d_%H%M%S");
        let filename = format!("sessions_toggl_{now}.csv");

        Ok((
            [
                (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
                (
                    header::CONTENT_DISPOSITION,
                    &format!("attachment; filename=\"{filename}\""),
                ),
            ],
            csv,
        )
            .into_response())
    }
}

fn resolve_export_range(params: &ExportQuery) -> (DateTime<Utc>, DateTime<Utc>) {
    let from = params
        .from
        .as_ref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|datetime| datetime.with_timezone(&Utc))
        .unwrap_or_else(|| Utc::now() - Duration::days(7));
    let to = params
        .to
        .as_ref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|datetime| datetime.with_timezone(&Utc))
        .unwrap_or_else(Utc::now);

    (from, to)
}

fn export_response<T: Serialize>(
    records: &[T],
    format: &str,
    filename_prefix: &str,
) -> Result<Response, ApiError> {
    let now = Utc::now().format("%Y%m%d_%H%M%S");

    match format.to_lowercase().as_str() {
        "csv" => {
            let csv = records_to_csv(records)?;
            let filename = format!("{filename_prefix}_{now}.csv");
            Ok((
                [
                    (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
                    (
                        header::CONTENT_DISPOSITION,
                        &format!("attachment; filename=\"{filename}\""),
                    ),
                ],
                csv,
            )
                .into_response())
        }
        _ => {
            let json = serde_json::to_string_pretty(records).map_err(|error| {
                ApiError::Internal(format!("JSON serialization failed: {error}"))
            })?;
            let filename = format!("{filename_prefix}_{now}.json");
            Ok((
                [
                    (header::CONTENT_TYPE, "application/json; charset=utf-8"),
                    (
                        header::CONTENT_DISPOSITION,
                        &format!("attachment; filename=\"{filename}\""),
                    ),
                ],
                json,
            )
                .into_response())
        }
    }
}

/// Convert work sessions to iCalendar (RFC 5545) format.
///
/// Each completed session maps to a VEVENT with DTSTART/DTEND/SUMMARY.
/// Sessions without an `ended_at` timestamp are skipped.
pub(crate) fn sessions_to_ical(sessions: &[FocusWorkSessionRecord]) -> String {
    let mut buf = String::with_capacity(sessions.len() * 256);
    buf.push_str("BEGIN:VCALENDAR\r\n");
    buf.push_str("VERSION:2.0\r\n");
    buf.push_str("PRODID:-//ONESHIM//Work Sessions//EN\r\n");
    buf.push_str("CALSCALE:GREGORIAN\r\n");

    for session in sessions {
        let Some(ended_at) = &session.ended_at else {
            continue;
        };

        let dtstart = rfc3339_to_ical(&session.started_at);
        let dtend = rfc3339_to_ical(ended_at);

        buf.push_str("BEGIN:VEVENT\r\n");
        buf.push_str(&format!("UID:session-{}@oneshim\r\n", session.id));
        buf.push_str(&format!("DTSTART:{dtstart}\r\n"));
        buf.push_str(&format!("DTEND:{dtend}\r\n"));
        buf.push_str(&format!(
            "SUMMARY:{} ({})\r\n",
            session.primary_app, session.category
        ));
        buf.push_str(&format!(
            "DESCRIPTION:Duration: {}s | Deep work: {}s | Interruptions: {}\r\n",
            session.duration_secs, session.deep_work_secs, session.interruption_count
        ));
        buf.push_str("END:VEVENT\r\n");
    }

    buf.push_str("END:VCALENDAR\r\n");
    buf
}

/// Convert work sessions to Toggl-compatible CSV format.
///
/// Columns: Description, Start date, Start time, Duration, Tags
pub(crate) fn sessions_to_toggl_csv(sessions: &[FocusWorkSessionRecord]) -> String {
    let mut buf = String::from("Description,Start date,Start time,Duration,Tags\n");

    for session in sessions {
        let description = csv_escape(&format!("{} ({})", session.primary_app, session.category));
        let (start_date, start_time) = split_rfc3339_date_time(&session.started_at);
        let duration = format_toggl_duration(session.duration_secs);
        let tags = &session.category;

        buf.push_str(&format!(
            "{description},{start_date},{start_time},{duration},{tags}\n"
        ));
    }

    buf
}

/// Convert an RFC 3339 timestamp to iCal DATETIME format (yyyyMMddTHHmmssZ).
fn rfc3339_to_ical(ts: &str) -> String {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| dt.with_timezone(&Utc).format("%Y%m%dT%H%M%SZ").to_string())
        .unwrap_or_else(|_| ts.replace(['-', ':'], ""))
}

/// Split an RFC 3339 timestamp into (YYYY-MM-DD, HH:MM:SS) parts for Toggl CSV.
fn split_rfc3339_date_time(ts: &str) -> (String, String) {
    DateTime::parse_from_rfc3339(ts)
        .map(|dt| {
            let utc = dt.with_timezone(&Utc);
            (
                utc.format("%Y-%m-%d").to_string(),
                utc.format("%H:%M:%S").to_string(),
            )
        })
        .unwrap_or_else(|_| (ts.to_string(), "00:00:00".to_string()))
}

/// Format seconds as HH:MM:SS for Toggl duration column.
fn format_toggl_duration(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Escape a CSV field value: quote it if it contains comma, quote, or newline.
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

pub(crate) fn records_to_csv<T: Serialize>(records: &[T]) -> Result<String, ApiError> {
    if records.is_empty() {
        return Ok(String::new());
    }

    let json_values: Vec<serde_json::Value> = records
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ApiError::Internal(format!("JSON conversion failed: {error}")))?;

    let headers: Vec<String> = json_values
        .first()
        .and_then(|value| value.as_object())
        .map(|object| object.keys().cloned().collect())
        .unwrap_or_default();

    let mut csv = headers.join(",") + "\n";

    for value in &json_values {
        if let Some(object) = value.as_object() {
            let row: Vec<String> = headers
                .iter()
                .map(|header_name| {
                    object
                        .get(header_name)
                        .map(|item| match item {
                            serde_json::Value::String(value) => {
                                if value.contains(',')
                                    || value.contains('"')
                                    || value.contains('\n')
                                {
                                    format!("\"{}\"", value.replace('"', "\"\""))
                                } else {
                                    value.clone()
                                }
                            }
                            serde_json::Value::Null => String::new(),
                            other => other.to_string(),
                        })
                        .unwrap_or_default()
                })
                .collect();
            csv.push_str(&row.join(","));
            csv.push('\n');
        }
    }

    Ok(csv)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a sample `FocusWorkSessionRecord` with sensible defaults.
    fn sample_session() -> FocusWorkSessionRecord {
        FocusWorkSessionRecord {
            id: 1,
            started_at: "2026-04-10T09:00:00Z".to_string(),
            ended_at: Some("2026-04-10T10:30:00Z".to_string()),
            primary_app: "VS Code".to_string(),
            category: "Development".to_string(),
            state: "completed".to_string(),
            interruption_count: 2,
            deep_work_secs: 4800,
            duration_secs: 5400,
        }
    }

    // ── sessions_to_ical ──────────────────────────────────────────

    #[test]
    fn ical_empty_sessions() {
        let result = sessions_to_ical(&[]);
        assert!(result.contains("BEGIN:VCALENDAR"));
        assert!(result.contains("END:VCALENDAR"));
        assert!(!result.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn ical_session_with_ended_at() {
        let session = sample_session();
        let result = sessions_to_ical(&[session]);

        assert!(result.contains("BEGIN:VEVENT"));
        assert!(result.contains("UID:session-1@oneshim"));
        assert!(result.contains("DTSTART:20260410T090000Z"));
        assert!(result.contains("DTEND:20260410T103000Z"));
        assert!(result.contains("SUMMARY:VS Code (Development)"));
        assert!(
            result.contains("DESCRIPTION:Duration: 5400s | Deep work: 4800s | Interruptions: 2")
        );
        assert!(result.contains("END:VEVENT"));
    }

    #[test]
    fn ical_session_without_ended_at_is_skipped() {
        let mut session = sample_session();
        session.ended_at = None;
        let result = sessions_to_ical(&[session]);

        assert!(result.contains("BEGIN:VCALENDAR"));
        assert!(!result.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn ical_multiple_sessions() {
        let s1 = sample_session();
        let mut s2 = sample_session();
        s2.id = 2;
        s2.started_at = "2026-04-10T14:00:00Z".to_string();
        s2.ended_at = Some("2026-04-10T15:00:00Z".to_string());
        s2.primary_app = "Terminal".to_string();

        // One session without ended_at — should be skipped.
        let mut s3 = sample_session();
        s3.id = 3;
        s3.ended_at = None;

        let result = sessions_to_ical(&[s1, s2, s3]);

        let vevent_count = result.matches("BEGIN:VEVENT").count();
        assert_eq!(vevent_count, 2);
    }

    // ── sessions_to_toggl_csv ─────────────────────────────────────

    #[test]
    fn toggl_csv_empty_sessions() {
        let result = sessions_to_toggl_csv(&[]);
        assert_eq!(result, "Description,Start date,Start time,Duration,Tags\n");
    }

    #[test]
    fn toggl_csv_single_session() {
        let session = sample_session();
        let result = sessions_to_toggl_csv(&[session]);

        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "Description,Start date,Start time,Duration,Tags");
        assert!(lines[1].contains("VS Code (Development)"));
        assert!(lines[1].contains("2026-04-10"));
        assert!(lines[1].contains("09:00:00"));
        assert!(lines[1].contains("01:30:00"));
        assert!(lines[1].contains("Development"));
    }

    #[test]
    fn toggl_csv_escapes_commas_in_app_name() {
        let mut session = sample_session();
        session.primary_app = "App, with comma".to_string();

        let result = sessions_to_toggl_csv(&[session]);
        let data_line = result.lines().nth(1).unwrap();

        // The description field must be quoted because it contains a comma.
        assert!(data_line.starts_with('"'));
    }

    // ── rfc3339_to_ical ───────────────────────────────────────────

    #[test]
    fn rfc3339_to_ical_valid() {
        let result = rfc3339_to_ical("2026-04-10T12:00:00Z");
        assert_eq!(result, "20260410T120000Z");
    }

    #[test]
    fn rfc3339_to_ical_invalid_fallback() {
        let result = rfc3339_to_ical("not-a-date");
        // Fallback strips dashes and colons.
        assert_eq!(result, "notadate");
    }

    // ── split_rfc3339_date_time ───────────────────────────────────

    #[test]
    fn split_rfc3339_valid() {
        let (date, time) = split_rfc3339_date_time("2026-04-10T12:30:45Z");
        assert_eq!(date, "2026-04-10");
        assert_eq!(time, "12:30:45");
    }

    #[test]
    fn split_rfc3339_invalid_fallback() {
        let (date, time) = split_rfc3339_date_time("garbage");
        assert_eq!(date, "garbage");
        assert_eq!(time, "00:00:00");
    }

    // ── format_toggl_duration ─────────────────────────────────────

    #[test]
    fn toggl_duration_zero() {
        assert_eq!(format_toggl_duration(0), "00:00:00");
    }

    #[test]
    fn toggl_duration_mixed() {
        // 1h 1m 1s = 3661s
        assert_eq!(format_toggl_duration(3661), "01:01:01");
    }

    #[test]
    fn toggl_duration_full_day() {
        assert_eq!(format_toggl_duration(86400), "24:00:00");
    }

    // ── csv_escape ────────────────────────────────────────────────

    #[test]
    fn csv_escape_no_special_chars() {
        assert_eq!(csv_escape("hello"), "hello");
    }

    #[test]
    fn csv_escape_comma() {
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
    }

    #[test]
    fn csv_escape_quote() {
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn csv_escape_newline() {
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    }

    // ── records_to_csv ────────────────────────────────────────────

    #[test]
    fn records_to_csv_empty() {
        let empty: Vec<serde_json::Value> = vec![];
        let result = records_to_csv(&empty).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn records_to_csv_simple_struct() {
        #[derive(Serialize)]
        struct Row {
            name: String,
            value: u32,
        }

        let rows = vec![
            Row {
                name: "alpha".to_string(),
                value: 1,
            },
            Row {
                name: "beta".to_string(),
                value: 2,
            },
        ];
        let csv = records_to_csv(&rows).unwrap();
        let lines: Vec<&str> = csv.lines().collect();

        assert_eq!(lines.len(), 3);
        // Header contains both field names.
        assert!(lines[0].contains("name"));
        assert!(lines[0].contains("value"));
        // Data rows present.
        assert!(lines[1].contains("alpha"));
        assert!(lines[2].contains("beta"));
    }

    #[test]
    fn records_to_csv_escapes_comma_in_field() {
        #[derive(Serialize)]
        struct Row {
            label: String,
        }

        let rows = vec![Row {
            label: "a,b".to_string(),
        }];
        let csv = records_to_csv(&rows).unwrap();
        let data_line = csv.lines().nth(1).unwrap();
        // The value should be quoted.
        assert!(data_line.contains("\"a,b\""));
    }

    // ── resolve_export_range ──────────────────────────────────────

    #[test]
    fn resolve_export_range_with_explicit_values() {
        let params = ExportQuery {
            from: Some("2026-04-01T00:00:00Z".to_string()),
            to: Some("2026-04-10T23:59:59Z".to_string()),
            format: "json".to_string(),
        };
        let (from, to) = resolve_export_range(&params);

        assert!(from.to_rfc3339().starts_with("2026-04-01"));
        assert!(to.to_rfc3339().starts_with("2026-04-10"));
    }

    #[test]
    fn resolve_export_range_defaults() {
        let params = ExportQuery {
            from: None,
            to: None,
            format: "json".to_string(),
        };
        let before = Utc::now();
        let (from, to) = resolve_export_range(&params);
        let after = Utc::now();

        // `to` should be approximately now.
        assert!(to >= before && to <= after);
        // `from` should be roughly 7 days before `to`.
        let gap = to - from;
        assert!(gap.num_days() >= 6 && gap.num_days() <= 7);
    }
}
