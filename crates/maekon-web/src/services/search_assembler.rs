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

    // F-QA-C34-04: sibling-miss fix — frame window_title/matched_text も event と同様に PII マスキング必須.
    // cycle 33 PR #4012 は assemble_event_search_result のみ修正 — frame 側が漏れていた.
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
    // #3603: window_title 과 data(matched_text) 는 비정형 사용자 입력 — PII 마스킹 필수.
    // sanitize_title 은 Standard 레벨(이메일/전화/주민번호/경로 등) 로 동작한다.
    // Option<String> 에 대해 as_deref() 로 &str 를 얻은 뒤 마스킹 후 재포장.
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
    use std::sync::Arc;

    struct MockSanitizer;

    impl PiiSanitizer for MockSanitizer {
        fn sanitize_text(&self, text: &str, _level: PiiFilterLevel) -> String {
            text.replace("user@example.com", "[EMAIL]")
                .replace("admin@company.org", "[EMAIL]")
                .replace("admin@corp.io", "[EMAIL]")
                .replace("홍길동@회사.com", "[EMAIL]")
                .replace("010-1234-5678", "[PHONE]")
        }
    }

    fn sanitizer() -> Option<Arc<dyn PiiSanitizer>> {
        Some(Arc::new(MockSanitizer))
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

    /// #3603: assemble_event_search_result 가 window_title 의 이메일 주소를 마스킹하는지 검증.
    #[test]
    fn test_assemble_event_search_result_masks_email_in_window_title() {
        let row = make_event_row(Some("Login — user@example.com"), Some("normal data"));
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        let title = result
            .window_title
            .expect("window_title 은 Some 이어야 한다");
        assert!(
            !title.contains("user@example.com"),
            "이메일이 마스킹되지 않았다: {title}"
        );
        assert!(title.contains("[EMAIL]"), "이메일 마스커가 없다: {title}");
    }

    /// #3603: assemble_event_search_result 가 data(matched_text) 의 이메일을 마스킹하는지 검증.
    #[test]
    fn test_assemble_event_search_result_masks_email_in_data() {
        let row = make_event_row(Some("일반 제목"), Some("연락처: admin@company.org 로 문의"));
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        let text = result
            .matched_text
            .expect("matched_text 는 Some 이어야 한다");
        assert!(
            !text.contains("admin@company.org"),
            "data 이메일이 마스킹되지 않았다: {text}"
        );
        assert!(
            text.contains("[EMAIL]"),
            "data 이메일 마스커가 없다: {text}"
        );
    }

    /// #3603: window_title 과 data 모두 None 이면 None 을 그대로 반환해야 한다.
    #[test]
    fn test_assemble_event_search_result_none_fields_passthrough() {
        let row = make_event_row(None, None);
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        assert!(
            result.window_title.is_none(),
            "None window_title 이 변환됐다"
        );
        assert!(
            result.matched_text.is_none(),
            "None matched_text 가 변환됐다"
        );
    }

    /// #3603: PII 없는 일반 텍스트는 원문 보존 — 과도한 마스킹 방지 회귀.
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
            "PII 없는 window_title 이 변경됐다"
        );
        assert_eq!(
            result.matched_text.as_deref(),
            Some("일반적인 이벤트 데이터"),
            "PII 없는 data 가 변경됐다"
        );
    }

    /// #3603: 한국어 텍스트 + 이메일 혼합 — UTF-8 안전성 검증.
    #[test]
    fn test_assemble_event_search_result_utf8_korean_with_pii() {
        let row = make_event_row(
            Some("사용자 로그인 — 계정: 홍길동@회사.com"),
            Some("데이터: 010-1234-5678 전화번호"),
        );
        let result = assemble_event_search_result(row, &sanitizer()).unwrap();
        let title = result.window_title.expect("window_title 은 Some");
        // 이메일 마스킹 (sanitize_title 이 @ 패턴 탐지)
        assert!(
            !title.contains("홍길동@회사.com"),
            "한국어 이메일이 마스킹되지 않았다: {title}"
        );
        // 한국어 문자 자체는 보존
        assert!(
            title.contains("사용자 로그인"),
            "한국어 컨텍스트 텍스트가 소실됐다: {title}"
        );
    }

    // -----------------------------------------------------------------------
    // F-QA-C34-04: assemble_frame_search_result PII 마스킹 — sibling-miss fix
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

    /// F-QA-C34-04: assemble_frame_search_result が window_title のメールアドレスをマスクするか検証.
    /// cycle 33 PR #4012 のシブリングミス修正 — frame 側も event と同様に sanitize_title 適用が必須.
    #[test]
    fn frame_search_result_masks_email_in_window_title() {
        let row = make_frame_row(Some("Frame — user@example.com"), Some("data"));
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        let title = result
            .window_title
            .expect("window_title は Some であるべき");
        assert!(
            !title.contains("user@example.com"),
            "frame window_title のメールがマスクされていない: {title}"
        );
        assert!(
            title.contains("[EMAIL]"),
            "frame window_title にマスカーがない: {title}"
        );
    }

    /// F-QA-C34-04: assemble_frame_search_result が matched_text のメールアドレスをマスクするか検証.
    #[test]
    fn frame_search_result_masks_email_in_matched_text() {
        let row = make_frame_row(Some("일반 제목"), Some("contact: admin@corp.io"));
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        let text = result
            .matched_text
            .expect("matched_text は Some であるべき");
        assert!(
            !text.contains("admin@corp.io"),
            "frame matched_text のメールがマスクされていない: {text}"
        );
        assert!(
            text.contains("[EMAIL]"),
            "frame matched_text にマスカーがない: {text}"
        );
    }

    /// F-QA-C34-04: PII 없는 frame title は원문 보존 — 과도한 마스킹 회귀 방지.
    #[test]
    fn frame_search_result_no_pii_unchanged() {
        let row = make_frame_row(Some("Finder — Documents"), Some("regular content"));
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        assert_eq!(
            result.window_title.as_deref(),
            Some("Finder — Documents"),
            "PII 없는 frame window_title 이 변경됐다"
        );
        assert_eq!(
            result.matched_text.as_deref(),
            Some("regular content"),
            "PII 없는 frame matched_text 가 변경됐다"
        );
    }

    /// F-QA-C34-04: window_title / matched_text 모두 None 이면 None 반환 — passthrough.
    #[test]
    fn frame_search_result_none_fields_passthrough() {
        let row = make_frame_row(None, None);
        let result = assemble_frame_search_result(row, vec![], &sanitizer()).unwrap();
        assert!(
            result.window_title.is_none(),
            "None frame window_title 이 변환됐다"
        );
        assert!(
            result.matched_text.is_none(),
            "None frame matched_text 가 변환됐다"
        );
    }
}
