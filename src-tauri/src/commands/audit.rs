//! Tauri IPC: OSS-build local audit log export (#4819, regulatory-compliance evidence).
//!
//! Reads recent audit entries from the durable SQLite `audit_log` table and saves them as
//! JSON or CSV to a user-selected path. The source is durable storage, NOT the volatile
//! `AuditLogger` buffer (~1000-entry cap) — because compliance evidence must come from the
//! persistent record.
//!
//! Follows the save-dialog pattern of `export_bug_report` (bug_report.rs) verbatim.
//! It **MUST be included in the default OSS build**, so it is not placed behind the `server`
//! feature gate.
//!
//! # PII posture (#4819 — requirement 4)
//!
//! The `details` of a durable row is populated by only two paths:
//! 1. `AuditLogger` buffer flush — already sanitized at the record boundary with
//!    `PiiFilterLevel::Strict` (automation `logger.rs::sanitize_details`).
//! 2. `consent.rs::audit_consent` direct write — carries only `permissions`/`version`/`consent_id`
//!    structured metadata, with no secrets.
//!
//! Even so, automation's `fail_closed_sanitize_details` is `pub(super)` and cannot be reused
//! here. Therefore, on export, we defensively mask secret-bearing key values once more via
//! [`redact_secret_keys`] so that even if a future direct writer leaves a raw secret behind,
//! it does not leak (no-new-dep, a small inline scanner).

use std::path::Path;

use maekon_core::models::audit::AuditEntry;

use crate::ipc_error::IpcError;

/// Export serialization format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuditExportFormat {
    Json,
    Csv,
}

impl AuditExportFormat {
    fn extension(self) -> &'static str {
        match self {
            AuditExportFormat::Json => "json",
            AuditExportFormat::Csv => "csv",
        }
    }
}

/// Quote a string as an `RFC 4180` CSV field (+ CSV formula-injection mitigation, #4819).
///
/// 1) Formula-injection defense: if the cell's first character is one of `=` `+` `-` `@`
///    `\t` `\r`, Excel/Sheets may interpret the cell as a formula, so we neutralize it by
///    prefixing a single quote (`'`) (standard CSV-injection mitigation).
/// 2) RFC 4180 quoting: if there is a comma, double-quote, or CR/LF, wrap in double-quotes
///    and double any interior quotes.
fn csv_quote(field: &str) -> String {
    // 1) Formula-injection neutralization — prefix a dangerous leading char with ' before quoting.
    let neutralized = neutralize_csv_formula(field);
    // 2) RFC 4180 quoting.
    if neutralized.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", neutralized.replace('"', "\"\""))
    } else {
        neutralized
    }
}

/// CSV formula-injection mitigation: if a cell starts with a formula-trigger char, prefix
/// it with a single quote.
///
/// Excel/Google Sheets may evaluate a cell starting with `=` `+` `-` `@` (and tab/CR) as a
/// formula. Per the standard mitigation, prepend a single `'` to turn it into literal text.
fn neutralize_csv_formula(field: &str) -> String {
    const TRIGGERS: &[char] = &['=', '+', '-', '@', '\t', '\r'];
    match field.chars().next() {
        Some(first) if TRIGGERS.contains(&first) => format!("'{field}"),
        _ => field.to_string(),
    }
}

/// Defensively mask the values of secret-bearing keys (#4819 PII posture).
///
/// Because automation's `fail_closed_sanitize_details` is `pub(super)` and cannot be reused,
/// this provides a minimal redactor with the same intent. It replaces the value with
/// `***REDACTED***` in the `"<key>":"<value>"` JSON pattern and the
/// `<key>=<value>` / `<key>: <value>` plaintext patterns. Consent metadata (no secrets)
/// is unaffected.
pub(crate) fn redact_secret_keys(details: &str) -> String {
    const SECRET_KEYS: &[&str] = &[
        "password",
        "secret",
        "api_key",
        "apikey",
        "access_token",
        "refresh_token",
        "authorization",
        "token",
    ];
    const MARKER: &str = "***REDACTED***";

    let mut out = details.to_string();
    for key in SECRET_KEYS {
        out = redact_one_key(&out, key, MARKER);
    }
    out
}

/// Mask the JSON quoted value / plaintext value for a single key.
fn redact_one_key(input: &str, key: &str, marker: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let key_lower = key.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while let Some(rel) = lower[cursor..].find(&key_lower) {
        let key_start = cursor + rel;
        let after_key = key_start + key.len();

        // Find the separator right after the key (": ", ":", "=", "\":\"" etc., whitespace allowed).
        let rest = &input[after_key..];
        let trimmed = rest.trim_start_matches(['"', ' ', '\t']);
        if let Some(sep) = trimmed
            .strip_prefix(':')
            .or_else(|| trimmed.strip_prefix('='))
        {
            let value_region = sep.trim_start_matches([' ', '\t']);
            // Compute the absolute offset of the value start.
            let consumed = input[after_key..].len() - value_region.len();
            let value_start = after_key + consumed;

            // For a JSON quoted value, go up to the closing quote; otherwise up to the next
            // separator (, } or whitespace).
            let value_str = &input[value_start..];
            let (value_inner_start, value_end) = if let Some(stripped) = value_str.strip_prefix('"')
            {
                let inner_start = value_start + 1;
                // JSON-escape-aware: skip `\"`/`\\` and find the real closing quote.
                // A naive find('"') would stop early at an escaped quote, leaking the tail of
                // the secret (#4819). Track the escape state byte by byte.
                let end_rel = find_json_closing_quote(stripped).unwrap_or(stripped.len());
                (inner_start, inner_start + end_rel)
            } else {
                let end_rel = value_str
                    .find([',', '}', ']', ' ', '\n', '\r', '\t'])
                    .unwrap_or(value_str.len());
                (value_start, value_start + end_rel)
            };

            out.push_str(&input[cursor..value_inner_start]);
            out.push_str(marker);
            cursor = value_end;
            continue;
        }

        // If there is no separator, pass the key through unchanged and advance (avoid false positives).
        out.push_str(&input[cursor..after_key]);
        cursor = after_key;
    }

    out.push_str(&input[cursor..]);
    out
}

/// Find, escape-aware, the relative byte offset of the closing quote within a JSON string
/// body (after the opening quote has been removed) (#4819 escaped-quote leak defense).
///
/// Skips `\"` and `\\` to find the real closing `"`. Returns `None` if there is no closing
/// quote. (The input is a JSON value body, so ASCII quotes/backslashes are always 1 byte.)
fn find_json_closing_quote(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // an escape sequence (`\"`, `\\`, etc.) skips two bytes.
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// Serialize audit entries into an export-format string (UI-independent, testable).
///
/// JSON: `Vec<AuditEntry>` as pretty JSON. CSV: a fixed header + rows. Both paths apply
/// [`redact_secret_keys`] to `details`.
pub(crate) fn serialize_audit_entries(
    entries: &[AuditEntry],
    format: AuditExportFormat,
) -> Result<String, IpcError> {
    match format {
        AuditExportFormat::Json => {
            // Serialize a copy with details redacted.
            let redacted: Vec<AuditEntry> = entries
                .iter()
                .map(|e| {
                    let mut e = e.clone();
                    e.details = e.details.as_deref().map(redact_secret_keys);
                    e
                })
                .collect();
            serde_json::to_string_pretty(&redacted).map_err(|e| {
                IpcError::new(
                    "internal.generic",
                    format!("audit JSON serialize failed: {e}"),
                )
            })
        }
        AuditExportFormat::Csv => {
            let mut out = String::new();
            // Header — matches the AuditEntry field order.
            out.push_str(
                "entry_id,timestamp,session_id,command_id,action_type,status,details,execution_time_ms\n",
            );
            for e in entries {
                let status = format!("{:?}", e.status);
                let details = e
                    .details
                    .as_deref()
                    .map(redact_secret_keys)
                    .unwrap_or_default();
                let etime = e
                    .execution_time_ms
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                let row = [
                    csv_quote(&e.entry_id),
                    csv_quote(&e.timestamp.to_rfc3339()),
                    csv_quote(&e.session_id),
                    csv_quote(&e.command_id),
                    csv_quote(&e.action_type),
                    csv_quote(&status),
                    csv_quote(&details),
                    csv_quote(&etime),
                ]
                .join(",");
                out.push_str(&row);
                out.push('\n');
            }
            Ok(out)
        }
    }
}

/// A **testable inner fn** that serializes and writes to a file at an explicit path.
///
/// It operates from just the entries, path, and format (no UI dialog), so unit tests call it
/// directly. On success it returns the absolute path string it wrote to.
pub(crate) async fn write_audit_export(
    path: &Path,
    entries: &[AuditEntry],
    format: AuditExportFormat,
) -> Result<String, IpcError> {
    let contents = serialize_audit_entries(entries, format)?;
    tokio::fs::write(path, contents)
        .await
        .map_err(IpcError::from)?;
    Ok(path.display().to_string())
}

/// Export the local audit log to a user-selected file (#4819).
///
/// Reads the most recent `limit` entries (default 10,000) from durable `SqliteStorage`
/// (`AppState.storage`) and saves them as JSON/CSV to the path obtained from the native
/// save-dialog. Returns `Ok(None)` if the user cancels.
#[tauri::command]
pub async fn export_audit_log(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::runtime_state::AppState>,
    format: Option<String>,
    limit: Option<usize>,
) -> Result<Option<String>, IpcError> {
    use tauri_plugin_dialog::DialogExt;

    let export_format = match format.as_deref() {
        Some("csv") => AuditExportFormat::Csv,
        // Default is JSON (None or "json").
        Some("json") | None => AuditExportFormat::Json,
        Some(other) => {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("unknown audit export format: {other}"),
            ))
        }
    };

    // Read the recent entries from durable storage, blocking (SQLite lock).
    let storage = state.storage.clone();
    let max = limit.unwrap_or(10_000);
    let entries = tokio::task::spawn_blocking(move || storage.recent_audit_entries(max))
        .await
        .map_err(|e| IpcError::new("internal.generic", format!("audit read task failed: {e}")))?;

    let ext = export_format.extension();
    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let file_name = format!("maekon-audit-{ts}.{ext}");

    let dialog = app.dialog().clone();
    let filter_ext = ext;
    let path = tokio::task::spawn_blocking(move || {
        dialog
            .file()
            .set_file_name(file_name)
            .add_filter(filter_ext.to_uppercase(), &[filter_ext])
            .blocking_save_file()
    })
    .await
    .map_err(|e| IpcError::new("internal.generic", format!("dialog task failed: {e}")))?;

    match path {
        Some(file_path) => {
            let p = file_path.as_path().ok_or_else(|| {
                IpcError::new("validation.invalid_arguments", "invalid file path")
            })?;
            let written = write_audit_export(p, &entries, export_format).await?;
            Ok(Some(written))
        }
        None => Ok(None), // user canceled the dialog.
    }
}

/// Verify the integrity of the local audit log's hash chain (#4834, E20).
///
/// Verifies the `audit_log` SHA-256 hash chain in durable `SqliteStorage`
/// (`AppState.storage`) and returns an [`AuditChainReport`]. This is tamper-**evident**
/// verification: it detects accidental/partial corruption, simple row edits, deletions, and
/// reordering (defense against a full-rewrite insider threat is out-of-scope — that needs
/// HMAC/Ed25519).
///
/// It **MUST be included in the default OSS build**, so it is not placed behind a feature gate.
#[tauri::command]
pub async fn verify_audit_log(
    state: tauri::State<'_, crate::runtime_state::AppState>,
) -> Result<maekon_core::models::audit::AuditChainReport, IpcError> {
    let storage = state.storage.clone();
    let report = tokio::task::spawn_blocking(move || storage.verify_audit_chain())
        .await
        .map_err(|e| {
            IpcError::new(
                "internal.generic",
                format!("audit chain verify task failed: {e}"),
            )
        })?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::audit::AuditStatus;

    fn sample_entry(id: &str, details: Option<&str>) -> AuditEntry {
        AuditEntry {
            entry_id: id.to_string(),
            timestamp: Utc::now(),
            session_id: "sess-1".to_string(),
            command_id: "cmd-1".to_string(),
            action_type: "MouseClick".to_string(),
            status: AuditStatus::Completed,
            details: details.map(|s| s.to_string()),
            execution_time_ms: Some(42),
        }
    }

    #[test]
    fn json_serialize_roundtrips() {
        let entries = vec![sample_entry("e-1", Some("ok")), sample_entry("e-2", None)];
        let json = serialize_audit_entries(&entries, AuditExportFormat::Json).expect("json");
        let parsed: Vec<AuditEntry> = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].entry_id, "e-1");
        assert_eq!(parsed[1].details, None);
    }

    #[test]
    fn csv_header_and_columns_match_entry_fields() {
        let entries = vec![sample_entry("e-1", Some("hello"))];
        let csv = serialize_audit_entries(&entries, AuditExportFormat::Csv).expect("csv");
        let mut lines = csv.lines();
        assert_eq!(
            lines.next().unwrap(),
            "entry_id,timestamp,session_id,command_id,action_type,status,details,execution_time_ms"
        );
        let row = lines.next().unwrap();
        let cols: Vec<&str> = row.split(',').collect();
        assert_eq!(cols.len(), 8, "8 columns per row");
        assert_eq!(cols[0], "e-1");
        assert_eq!(cols[3], "cmd-1");
        assert_eq!(cols[5], "Completed");
        assert_eq!(cols[6], "hello");
        assert_eq!(cols[7], "42");
    }

    #[test]
    fn csv_quotes_fields_with_commas_and_quotes() {
        let entry = sample_entry("e-1", Some("a,b\"c"));
        let csv = serialize_audit_entries(&[entry], AuditExportFormat::Csv).expect("csv");
        // The details field is quoted and its interior quotes are doubled.
        assert!(csv.contains("\"a,b\"\"c\""), "csv = {csv}");
    }

    #[test]
    fn redact_secret_keys_masks_json_and_plain_values() {
        let raw = r#"{"api_key":"sk-12345","note":"safe","password=hunter2"}"#;
        let red = redact_secret_keys(raw);
        assert!(!red.contains("sk-12345"), "api_key value leaked: {red}");
        assert!(!red.contains("hunter2"), "password value leaked: {red}");
        assert!(red.contains("safe"), "non-secret field must survive: {red}");
    }

    #[test]
    fn csv_neutralizes_formula_injection_triggers() {
        // A cell starting with = + - @ gets prefixed with ' so formula evaluation is blocked (#4819).
        for (raw, expected_prefix) in [
            ("=cmd|'/c calc'!A1", "'=cmd"),
            ("+1+1", "'+1+1"),
            ("-2+3", "'-2+3"),
            ("@SUM(A1)", "'@SUM(A1)"),
        ] {
            let entry = sample_entry("e-1", Some(raw));
            let csv = serialize_audit_entries(&[entry], AuditExportFormat::Csv).expect("csv");
            let details_col = csv.lines().nth(1).unwrap();
            assert!(
                details_col.contains(expected_prefix),
                "formula trigger not neutralized for {raw:?}: {csv}"
            );
        }
        // Also check at the csv_quote unit level directly.
        assert_eq!(csv_quote("=danger"), "'=danger");
        // An ordinary value is not prefixed.
        assert_eq!(csv_quote("hello"), "hello");
    }

    #[test]
    fn redact_handles_escaped_quote_in_secret_value() {
        // A secret containing an escaped quote must be fully redacted, including its tail (#4819).
        let raw = r#"{"api_key":"sk-ab\"cd","note":"safe"}"#;
        let red = redact_secret_keys(raw);
        assert!(!red.contains("sk-ab"), "secret head leaked: {red}");
        assert!(
            !red.contains("cd"),
            "secret tail leaked past escaped quote: {red}"
        );
        assert!(red.contains("safe"), "non-secret field must survive: {red}");
        assert!(
            red.contains(MARKER_FOR_TEST),
            "redaction marker missing: {red}"
        );
    }

    const MARKER_FOR_TEST: &str = "***REDACTED***";

    #[test]
    fn redact_leaves_consent_metadata_untouched() {
        // The shape consent.rs writes: no secrets → must be left unchanged.
        let raw = r#"{"permissions":{"screen":true},"version":3,"consent_id":"c-1"}"#;
        assert_eq!(redact_secret_keys(raw), raw);
    }

    #[tokio::test]
    async fn write_audit_export_produces_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("audit.json");
        let entries = vec![sample_entry("e-1", Some("ok"))];
        let written = write_audit_export(&path, &entries, AuditExportFormat::Json)
            .await
            .expect("write");
        assert_eq!(written, path.display().to_string());
        let contents = std::fs::read_to_string(&path).expect("read back");
        let parsed: Vec<AuditEntry> = serde_json::from_str(&contents).expect("parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].entry_id, "e-1");
    }
}
