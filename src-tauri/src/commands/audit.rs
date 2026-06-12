//! Tauri IPC: OSS 빌드 로컬 감사 로그 export (#4819, 규제 준수 증거).
//!
//! durable SQLite `audit_log` 테이블에서 최근 감사 항목을 읽어 사용자가 선택한
//! 경로에 JSON 또는 CSV로 저장한다. 휘발성 `AuditLogger` 버퍼(~1000개 cap)가
//! 아닌 durable 스토리지를 source로 삼는다 — 컴플라이언스 증거는 영속 기록에서
//! 나와야 하기 때문이다.
//!
//! `export_bug_report`(bug_report.rs)의 save-dialog 패턴을 그대로 따른다.
//! **반드시 기본 OSS 빌드에 포함**되어야 하므로 `server` feature 게이트 뒤에
//! 두지 않는다.
//!
//! # PII 자세 (#4819 — 요구사항 4)
//!
//! durable 행의 `details`는 두 경로로만 채워진다:
//! 1. `AuditLogger` 버퍼 flush — record 경계에서 `PiiFilterLevel::Strict`로 이미
//!    sanitize됨 (automation `logger.rs::sanitize_details`).
//! 2. `consent.rs::audit_consent` 직접 write — `permissions`/`version`/`consent_id`
//!    구조화 메타데이터만 담으며 secret이 없다.
//!
//! 그럼에도 automation의 `fail_closed_sanitize_details`는 `pub(super)`라 여기서
//! 재사용할 수 없다. 따라서 export 시 미래의 직접 writer가 raw secret을 남기더라도
//! 누출되지 않도록 [`redact_secret_keys`]로 secret-bearing 키 값을 방어적으로
//! 한 번 더 마스킹한다 (no-new-dep, 작은 inline 스캐너).

use std::path::Path;

use maekon_core::models::audit::AuditEntry;

use crate::ipc_error::IpcError;

/// export 직렬화 형식.
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

/// 문자열을 `RFC 4180` CSV 필드로 quote한다 (+ CSV formula injection 완화, #4819).
///
/// 1) formula injection 방어: 셀의 첫 문자가 `=` `+` `-` `@` `\t` `\r` 중 하나면
///    Excel/Sheets가 셀을 수식으로 해석할 수 있으므로 작은따옴표(`'`)를 앞에 붙여
///    중립화한다 (표준 CSV-injection 완화).
/// 2) RFC 4180 quote: 쉼표·큰따옴표·CR/LF가 있으면 큰따옴표로 감싸고 내부 따옴표를
///    이중화한다.
fn csv_quote(field: &str) -> String {
    // 1) formula injection 중립화 — quote 전에 위험한 선두 문자를 ' 로 prefix.
    let neutralized = neutralize_csv_formula(field);
    // 2) RFC 4180 quoting.
    if neutralized.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", neutralized.replace('"', "\"\""))
    } else {
        neutralized
    }
}

/// CSV formula injection 완화: 셀 선두가 수식 트리거 문자면 작은따옴표로 prefix.
///
/// Excel/Google Sheets는 `=` `+` `-` `@`(및 탭/CR) 로 시작하는 셀을 수식으로
/// 평가할 수 있다. 표준 완화책대로 단일 `'`를 앞에 붙여 리터럴 텍스트로 만든다.
fn neutralize_csv_formula(field: &str) -> String {
    const TRIGGERS: &[char] = &['=', '+', '-', '@', '\t', '\r'];
    match field.chars().next() {
        Some(first) if TRIGGERS.contains(&first) => format!("'{field}"),
        _ => field.to_string(),
    }
}

/// secret-bearing 키의 값을 방어적으로 마스킹한다 (#4819 PII 자세).
///
/// automation `fail_closed_sanitize_details`가 `pub(super)`라 재사용 불가하므로
/// 동일 의도의 최소 redactor를 둔다. `"<key>":"<value>"` JSON 패턴과
/// `<key>=<value>` / `<key>: <value>` 평문 패턴에서 값을 `***REDACTED***`로
/// 바꾼다. consent 메타데이터(secret 없음)에는 영향이 없다.
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

/// 단일 키에 대해 JSON 따옴표 값 / 평문 값을 마스킹한다.
fn redact_one_key(input: &str, key: &str, marker: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let key_lower = key.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0usize;

    while let Some(rel) = lower[cursor..].find(&key_lower) {
        let key_start = cursor + rel;
        let after_key = key_start + key.len();

        // 키 직후의 구분자를 찾는다 (": ", ":", "=", "\":\"" 등 공백 허용).
        let rest = &input[after_key..];
        let trimmed = rest.trim_start_matches(['"', ' ', '\t']);
        if let Some(sep) = trimmed
            .strip_prefix(':')
            .or_else(|| trimmed.strip_prefix('='))
        {
            let value_region = sep.trim_start_matches([' ', '\t']);
            // 값 시작 절대 오프셋 계산.
            let consumed = input[after_key..].len() - value_region.len();
            let value_start = after_key + consumed;

            // JSON 따옴표 값이면 닫는 따옴표까지, 아니면 다음 구분자(, } 공백)까지.
            let value_str = &input[value_start..];
            let (value_inner_start, value_end) = if let Some(stripped) = value_str.strip_prefix('"')
            {
                let inner_start = value_start + 1;
                // JSON-escape-aware: `\"`/`\\` 를 건너뛰고 진짜 닫는 따옴표를 찾는다.
                // 단순 find('"')는 escaped quote 에서 조기 종료되어 secret 의 꼬리가
                // 누출된다 (#4819). 바이트 단위로 escape 상태를 추적한다.
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

        // 구분자가 없으면 키만 그대로 통과시키고 진행 (false-positive 회피).
        out.push_str(&input[cursor..after_key]);
        cursor = after_key;
    }

    out.push_str(&input[cursor..]);
    out
}

/// JSON 문자열 본문(여는 따옴표 제거 후)에서 escape-aware 로 닫는 따옴표의
/// 상대 바이트 오프셋을 찾는다 (#4819 escaped-quote 누출 방어).
///
/// `\"` 와 `\\` 를 건너뛰며 진짜 닫는 `"`를 찾는다. 닫는 따옴표가 없으면 `None`.
/// (입력은 JSON 값 본문이라 ASCII 따옴표/백슬래시는 항상 1바이트.)
fn find_json_closing_quote(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2, // escape 시퀀스(`\"`, `\\` 등)는 두 바이트를 건너뛴다.
            b'"' => return Some(i),
            _ => i += 1,
        }
    }
    None
}

/// 감사 항목을 export 형식 문자열로 직렬화한다 (UI 비의존, 테스트 가능).
///
/// JSON: `Vec<AuditEntry>`를 pretty JSON으로. CSV: 고정 헤더 + 행. 두 경로 모두
/// `details`에 [`redact_secret_keys`]를 적용한다.
pub(crate) fn serialize_audit_entries(
    entries: &[AuditEntry],
    format: AuditExportFormat,
) -> Result<String, IpcError> {
    match format {
        AuditExportFormat::Json => {
            // details를 redact한 사본을 직렬화한다.
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
            // 헤더 — AuditEntry 필드 순서와 일치.
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

/// 직렬화 + 파일 쓰기를 명시적 경로로 수행하는 **테스트 가능한 inner fn**.
///
/// UI 다이얼로그 없이 항목·경로·형식만으로 동작하므로 단위 테스트가 직접 호출한다.
/// 성공 시 기록한 절대 경로 문자열을 반환한다.
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

/// 로컬 감사 로그를 사용자가 선택한 파일로 export 한다 (#4819).
///
/// durable `SqliteStorage`(`AppState.storage`)에서 최근 `limit`개(기본 10,000)를
/// 읽어 native save-dialog로 받은 경로에 JSON/CSV로 저장한다. 사용자가 취소하면
/// `Ok(None)`을 반환한다.
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
        // 기본은 JSON (None 또는 "json").
        Some("json") | None => AuditExportFormat::Json,
        Some(other) => {
            return Err(IpcError::new(
                "validation.invalid_arguments",
                format!("unknown audit export format: {other}"),
            ))
        }
    };

    // durable 스토리지에서 최근 항목을 blocking으로 읽는다 (SQLite lock).
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
        None => Ok(None), // 사용자가 다이얼로그를 취소함.
    }
}

/// 로컬 감사 로그 해시 체인의 무결성을 검증한다 (#4834, E20).
///
/// durable `SqliteStorage`(`AppState.storage`)의 `audit_log` SHA-256 해시 체인을
/// 검증하고 [`AuditChainReport`]를 반환한다. tamper-**evident** 검증으로
/// 우발적/부분적 손상·단순 행 편집·삭제·재정렬을 탐지한다(전면 재기록 내부자
/// 위협 방어는 out-of-scope — HMAC/Ed25519 필요).
///
/// **반드시 기본 OSS 빌드에 포함**되어야 하므로 feature 게이트 뒤에 두지 않는다.
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
        // details 필드는 quote되고 내부 따옴표는 이중화된다.
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
        // 선두가 = + - @ 인 셀은 ' 로 prefix 되어 수식 평가가 차단된다 (#4819).
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
        // 직접 csv_quote 단위로도 확인.
        assert_eq!(csv_quote("=danger"), "'=danger");
        // 일반 값은 prefix 되지 않는다.
        assert_eq!(csv_quote("hello"), "hello");
    }

    #[test]
    fn redact_handles_escaped_quote_in_secret_value() {
        // escaped quote 가 포함된 secret 은 꼬리까지 전부 redact 되어야 한다 (#4819).
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
        // consent.rs가 쓰는 형태: secret 없음 → 변형 없어야 함.
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
