use std::sync::Arc;

use maekon_api_contracts::search::{SearchResponse, SearchResult, TagInfo};
use maekon_core::config::PiiFilterLevel;
use maekon_core::models::storage_records::{SearchEventRow, SearchFrameRow, TagRecord};
use maekon_core::ports::pii_sanitizer::PiiSanitizer;

use crate::error::ApiError;

const SEARCH_SANITIZE_LEVEL: PiiFilterLevel = PiiFilterLevel::Standard;

fn require_search_sanitizer(
    sanitizer: &Option<Arc<dyn PiiSanitizer>>,
) -> Result<&dyn PiiSanitizer, ApiError> {
    sanitizer.as_deref().ok_or_else(|| {
        ApiError::Internal("PII sanitizer not configured for search assembly".to_string())
    })
}

fn search_sanitize_opt(s: Option<String>, sanitizer: &dyn PiiSanitizer) -> Option<String> {
    s.map(|value| sanitizer.sanitize_text(&value, SEARCH_SANITIZE_LEVEL))
}

pub(crate) fn assemble_tag_info(tag: TagRecord) -> TagInfo {
    TagInfo {
        id: tag.id,
        name: tag.name,
        color: tag.color,
    }
}

pub(crate) fn assemble_frame_search_result(
    row: SearchFrameRow,
    tags: Vec<TagInfo>,
    sanitizer: &Option<Arc<dyn PiiSanitizer>>,
) -> Result<SearchResult, ApiError> {
    let sanitizer = require_search_sanitizer(sanitizer)?;
    let image_url = row
        .file_path
        .as_ref()
        .map(|_| format!("/api/frames/{}/image", row.id));

    // F-QA-C34-04: sibling-miss fix — like event, frame window_title/matched_text also require PII masking.
    // cycle 33 PR #4012 only fixed assemble_event_search_result — the frame side was missed.
    let window_title = search_sanitize_opt(row.window_title, sanitizer);
    let matched_text = search_sanitize_opt(row.matched_text, sanitizer);

    Ok(SearchResult {
        result_type: "frame".to_string(),
        id: row.id.to_string(),
        timestamp: row.timestamp,
        app_name: row.app_name,
        window_title,
        matched_text,
        image_url,
        importance: row.importance,
        tags: Some(tags),
    })
}

pub(crate) fn assemble_event_search_result(
    row: SearchEventRow,
    sanitizer: &Option<Arc<dyn PiiSanitizer>>,
) -> Result<SearchResult, ApiError> {
    let sanitizer = require_search_sanitizer(sanitizer)?;
    // #3603: window_title and data(matched_text) are unstructured user input — PII masking is required.
    // sanitize_title operates at the Standard level (email/phone/SSN/path, etc.).
    // For Option<String>, obtain a &str via as_deref(), mask, then re-wrap.
    let window_title = search_sanitize_opt(row.window_title, sanitizer);
    let matched_text = search_sanitize_opt(row.data, sanitizer);

    Ok(SearchResult {
        result_type: "event".to_string(),
        id: row.event_id,
        timestamp: row.timestamp,
        app_name: row.app_name,
        window_title,
        matched_text,
        image_url: None,
        importance: None,
        tags: None,
    })
}

pub(crate) fn assemble_search_response(
    query: String,
    total: u64,
    offset: usize,
    limit: usize,
    results: Vec<SearchResult>,
) -> SearchResponse {
    SearchResponse {
        query,
        total,
        offset,
        limit,
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::models::storage_records::{SearchEventRow, SearchFrameRow};
    use maekon_core::ports::pii_sanitizer::FakePiiSanitizer;
    use std::sync::Arc;

    fn sanitizer() -> Option<Arc<dyn PiiSanitizer>> {
        Some(Arc::new(
            FakePiiSanitizer::new()
                .with_email("user@example.com")
                .with_email("admin@company.org")
                .with_email("admin@corp.io")
                .with_email("홍길동@회사.com")
                .with_phone("010-1234-5678"),
        ))
    }

    fn make_event_row(window_title: Option<&str>, data: Option<&str>) -> SearchEventRow {
        SearchEventRow {
            event_id: "evt-001".to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            app_name: Some("TestApp".to_string()),
            window_title: window_title.map(str::to_string),
            data: data.map(str::to_string),
        }
    }

    /// #3603: verify assemble_event_search_result masks the email address in window_title.
    #[test]
    fn test_assemble_event_search_result_masks_email_in_window_title() {
        let row = make_event_row(Some("Login — user@example.com"), Some("normal data"));
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        let title = result.window_title.expect("window_title should be Some");
        assert!(
            !title.contains("user@example.com"),
            "email was not masked: {title}"
        );
        assert!(
            title.contains("[EMAIL]"),
            "email masker is missing: {title}"
        );
    }

    /// #3603: verify assemble_event_search_result masks the email in data(matched_text).
    #[test]
    fn test_assemble_event_search_result_masks_email_in_data() {
        let row = make_event_row(Some("일반 제목"), Some("연락처: admin@company.org 로 문의"));
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        let text = result.matched_text.expect("matched_text should be Some");
        assert!(
            !text.contains("admin@company.org"),
            "data email was not masked: {text}"
        );
        assert!(
            text.contains("[EMAIL]"),
            "data email masker is missing: {text}"
        );
    }

    /// #3603: when both window_title and data are None, None must be returned as-is.
    #[test]
    fn test_assemble_event_search_result_none_fields_passthrough() {
        let row = make_event_row(None, None);
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        assert!(
            result.window_title.is_none(),
            "None window_title was transformed"
        );
        assert!(
            result.matched_text.is_none(),
            "None matched_text was transformed"
        );
    }

    /// #3603: PII-free plain text is preserved verbatim — regression guard against over-masking.
    #[test]
    fn test_assemble_event_search_result_no_pii_unchanged() {
        let row = make_event_row(
            Some("Visual Studio Code - main.rs"),
            Some("일반적인 이벤트 데이터"),
        );
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        assert_eq!(
            result.window_title.as_deref(),
            Some("Visual Studio Code - main.rs"),
            "PII-free window_title was changed"
        );
        assert_eq!(
            result.matched_text.as_deref(),
            Some("일반적인 이벤트 데이터"),
            "PII-free data was changed"
        );
    }

    /// #3603: Korean text mixed with an email — UTF-8 safety verification.
    #[test]
    fn test_assemble_event_search_result_utf8_korean_with_pii() {
        let row = make_event_row(
            Some("사용자 로그인 — 계정: 홍길동@회사.com"),
            Some("데이터: 010-1234-5678 전화번호"),
        );
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        let title = result.window_title.expect("window_title should be Some");
        // Email masking (sanitize_title detects the @ pattern)
        assert!(
            !title.contains("홍길동@회사.com"),
            "Korean email was not masked: {title}"
        );
        // The Korean characters themselves are preserved
        assert!(
            title.contains("사용자 로그인"),
            "Korean context text was lost: {title}"
        );
    }

    // -----------------------------------------------------------------------
    // F-QA-C34-04: assemble_frame_search_result PII masking — sibling-miss fix
    // -----------------------------------------------------------------------

    fn make_frame_row(window_title: Option<&str>, matched_text: Option<&str>) -> SearchFrameRow {
        SearchFrameRow {
            id: 1,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            app_name: Some("TestApp".to_string()),
            window_title: window_title.map(str::to_string),
            matched_text: matched_text.map(str::to_string),
            importance: None,
            file_path: None,
        }
    }

    /// F-QA-C34-04: verify assemble_frame_search_result masks the email address in window_title.
    /// cycle 33 PR #4012 sibling-miss fix — like event, the frame side must also apply sanitize_title.
    #[test]
    fn frame_search_result_masks_email_in_window_title() {
        let row = make_frame_row(Some("Frame — user@example.com"), Some("data"));
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        let title = result.window_title.expect("window_title should be Some");
        assert!(
            !title.contains("user@example.com"),
            "frame window_title email is not masked: {title}"
        );
        assert!(
            title.contains("[EMAIL]"),
            "frame window_title has no masker: {title}"
        );
    }

    /// F-QA-C34-04: verify assemble_frame_search_result masks the email address in matched_text.
    #[test]
    fn frame_search_result_masks_email_in_matched_text() {
        let row = make_frame_row(Some("일반 제목"), Some("contact: admin@corp.io"));
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        let text = result.matched_text.expect("matched_text should be Some");
        assert!(
            !text.contains("admin@corp.io"),
            "frame matched_text email is not masked: {text}"
        );
        assert!(
            text.contains("[EMAIL]"),
            "frame matched_text has no masker: {text}"
        );
    }

    /// F-QA-C34-04: PII-free frame titles are preserved verbatim — guard against over-masking regression.
    #[test]
    fn frame_search_result_no_pii_unchanged() {
        let row = make_frame_row(Some("Finder — Documents"), Some("regular content"));
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        assert_eq!(
            result.window_title.as_deref(),
            Some("Finder — Documents"),
            "PII-free frame window_title was changed"
        );
        assert_eq!(
            result.matched_text.as_deref(),
            Some("regular content"),
            "PII-free frame matched_text was changed"
        );
    }

    /// F-QA-C34-04: when both window_title and matched_text are None, return None — passthrough.
    #[test]
    fn frame_search_result_none_fields_passthrough() {
        let row = make_frame_row(None, None);
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        assert!(
            result.window_title.is_none(),
            "None frame window_title was transformed"
        );
        assert!(
            result.matched_text.is_none(),
            "None frame matched_text was transformed"
        );
    }
}
