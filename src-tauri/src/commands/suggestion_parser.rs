use chrono::Utc;
use maekon_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
use maekon_suggestion::queue::SuggestionQueue;
use serde::Deserialize;
use std::fmt;
use tracing::debug;

const MAX_CHAT_SUGGESTIONS: usize = 3;

#[derive(Debug, Deserialize)]
struct ParsedSuggestion {
    #[serde(rename = "type")]
    suggestion_type: String,
    content: String,
    priority: String,
    #[serde(default)]
    reasoning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SuggestionResponse {
    suggestions: Vec<ParsedSuggestion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionExtractionError {
    MissingJson,
    InvalidJson,
}

impl fmt::Display for SuggestionExtractionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SuggestionExtractionError::MissingJson => {
                write!(f, "AI response did not include suggestion JSON")
            }
            SuggestionExtractionError::InvalidJson => {
                write!(f, "AI response included malformed suggestion JSON")
            }
        }
    }
}

fn parse_type(s: &str) -> Option<SuggestionType> {
    match s.to_lowercase().replace(' ', "_").as_str() {
        "work_guidance" => Some(SuggestionType::WorkGuidance),
        "email_draft" => Some(SuggestionType::EmailDraft),
        "productivity_tip" => Some(SuggestionType::ProductivityTip),
        "workflow_optimization" => Some(SuggestionType::WorkflowOptimization),
        "context_based" => Some(SuggestionType::ContextBased),
        _ => None,
    }
}

fn parse_priority(s: &str) -> Option<Priority> {
    match s.to_lowercase().as_str() {
        "critical" => Some(Priority::Critical),
        "high" => Some(Priority::High),
        "medium" => Some(Priority::Medium),
        "low" => Some(Priority::Low),
        _ => None,
    }
}

/// Extract suggestion JSON from AI response text.
/// Looks for `{"suggestions": [...]}` pattern — either bare or inside ```json fences.
pub fn extract_suggestions(
    response_text: &str,
) -> Result<Vec<Suggestion>, SuggestionExtractionError> {
    // Try to find JSON block
    let json_str = extract_json_block(response_text)?;

    // Parse as SuggestionResponse
    let parsed: SuggestionResponse = match serde_json::from_str(json_str) {
        Ok(r) => r,
        Err(e) => {
            debug!("suggestion extraction parse error: {e}");
            return Err(SuggestionExtractionError::InvalidJson);
        }
    };

    // Convert to Suggestion structs.
    // Chat-extracted suggestions use lower confidence (0.5) and a 4-hour expiry
    // because they bypass the server-side FeedbackScorer pipeline.
    let now = Utc::now();
    let expires = now + chrono::Duration::hours(4);
    let suggestions: Vec<Suggestion> = parsed
        .suggestions
        .into_iter()
        .filter_map(|p| {
            let stype = parse_type(&p.suggestion_type)?;
            let priority = parse_priority(&p.priority)?;
            let content = p.content.trim();
            if content.is_empty() {
                return None;
            }
            Some(Suggestion {
                suggestion_id: format!("chat-{}", uuid::Uuid::new_v4()),
                suggestion_type: stype,
                content: content.to_string(),
                priority,
                confidence_score: 0.5,
                relevance_score: 0.8,
                is_actionable: true,
                created_at: now,
                expires_at: Some(expires),
                source: SuggestionSource::LlmLocal,
                reasoning: p.reasoning,
                context_scope: None,
            })
        })
        .take(MAX_CHAT_SUGGESTIONS)
        .collect();
    Ok(suggestions)
}

/// Push parsed suggestions through the queue's real admission policy and
/// return only the number actually admitted. Duplicate and full-queue
/// rejections therefore cannot inflate command/event counts.
pub(crate) fn admit_suggestions(queue: &mut SuggestionQueue, suggestions: Vec<Suggestion>) -> u32 {
    suggestions.into_iter().fold(0, |count, suggestion| {
        if queue.push(suggestion) {
            count + 1
        } else {
            count
        }
    })
}

/// Best-effort extraction for passive chat auto-extraction.
///
/// Explicit "Get Suggestions" flows should call [`extract_suggestions`] so
/// malformed AI responses surface as user-visible errors instead of silent
/// zero-count success.
pub fn try_extract_suggestions(response_text: &str) -> Vec<Suggestion> {
    extract_suggestions(response_text).unwrap_or_default()
}

/// Extract the canonical top-level `{"suggestions": [...]}` wrapper from
/// fenced or bare text. Brace matching is string-aware, so braces and escaped
/// quotes inside suggestion content cannot terminate the object early.
fn extract_json_block(text: &str) -> Result<&str, SuggestionExtractionError> {
    for (start, ch) in text.char_indices() {
        if ch != '{' {
            continue;
        }

        let Some(end) = matching_object_end(text, start) else {
            continue;
        };
        let candidate = &text[start..end];
        match serde_json::from_str::<serde_json::Value>(candidate) {
            Ok(serde_json::Value::Object(object)) if object.contains_key("suggestions") => {
                return Ok(candidate);
            }
            Ok(_) => {}
            Err(_) if contains_wrapper_sentinel(candidate) => {
                return Err(SuggestionExtractionError::InvalidJson);
            }
            Err(_) => {}
        }
    }

    if contains_wrapper_sentinel(text) {
        Err(SuggestionExtractionError::InvalidJson)
    } else {
        Err(SuggestionExtractionError::MissingJson)
    }
}

fn contains_wrapper_sentinel(text: &str) -> bool {
    let mut rest = text;
    while let Some(offset) = rest.find("\"suggestions\"") {
        let after_key = &rest[offset + "\"suggestions\"".len()..];
        if after_key.trim_start().starts_with(':') {
            return true;
        }
        rest = after_key;
    }
    false
}

/// Return the exclusive byte offset of the matching outer `}`.
fn matching_object_end(text: &str, start: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(start + offset + ch.len_utf8());
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_fenced_json() {
        let text = r#"Here are some suggestions:

```json
{"suggestions": [{"type": "productivity_tip", "content": "Try batching similar tasks", "priority": "high", "reasoning": "Based on your workflow"}]}
```

Hope that helps!"#;

        let results = try_extract_suggestions(text);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].suggestion_type, SuggestionType::ProductivityTip);
        assert_eq!(results[0].content, "Try batching similar tasks");
        assert_eq!(results[0].priority, Priority::High);
        assert_eq!(
            results[0].reasoning.as_deref(),
            Some("Based on your workflow")
        );
        assert_eq!(results[0].source, SuggestionSource::LlmLocal);
        // Chat-extracted suggestions use reduced confidence and auto-expire
        assert!(
            (results[0].confidence_score - 0.5).abs() < f64::EPSILON,
            "chat-extracted suggestions should have 0.5 confidence"
        );
        assert!(
            results[0].expires_at.is_some(),
            "chat-extracted suggestions should have an expiry"
        );
    }

    #[test]
    fn parse_bare_json() {
        let text = r#"{"suggestions": [{"type": "work_guidance", "content": "Focus on the report", "priority": "medium"}]}"#;
        let results = try_extract_suggestions(text);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].suggestion_type, SuggestionType::WorkGuidance);
        assert!(results[0].reasoning.is_none());
        assert!(results[0].expires_at.is_some());
    }

    #[test]
    fn parse_pretty_wrapper_with_braces_and_escaped_quotes_in_content() {
        let text = r#"Here is the requested wrapper:
{
  "suggestions": [
    {
      "type": "context_based",
      "content": "Keep object {\"key\": \"}\"} intact",
      "priority": "medium"
    }
  ]
}
No additional JSON follows."#;

        let results = extract_suggestions(text).expect("pretty wrapper must parse");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "Keep object {\"key\": \"}\"} intact");
    }

    #[test]
    fn parse_multiple_suggestions() {
        let text = r#"{"suggestions": [
            {"type": "productivity_tip", "content": "Tip 1", "priority": "low"},
            {"type": "email_draft", "content": "Draft email", "priority": "high"},
            {"type": "context_based", "content": "Context suggestion", "priority": "critical"}
        ]}"#;
        let results = try_extract_suggestions(text);
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].priority, Priority::Low);
        assert_eq!(results[1].suggestion_type, SuggestionType::EmailDraft);
        assert_eq!(results[2].priority, Priority::Critical);
    }

    #[test]
    fn invalid_type_filtered_out() {
        let text = r#"{"suggestions": [{"type": "unknown_type", "content": "Test", "priority": "medium"}]}"#;
        let results = try_extract_suggestions(text);
        assert!(results.is_empty());
    }

    #[test]
    fn invalid_priority_and_empty_content_are_filtered_out() {
        let text = r#"{"suggestions": [
            {"type": "work_guidance", "content": "Valid", "priority": "urgent"},
            {"type": "work_guidance", "content": "   ", "priority": "high"}
        ]}"#;
        assert!(try_extract_suggestions(text).is_empty());
    }

    #[test]
    fn output_is_limited_to_three_suggestions() {
        let text = r#"{"suggestions": [
            {"type": "work_guidance", "content": "One", "priority": "low"},
            {"type": "email_draft", "content": "Two", "priority": "medium"},
            {"type": "productivity_tip", "content": "Three", "priority": "high"},
            {"type": "context_based", "content": "Four", "priority": "critical"}
        ]}"#;

        let results = extract_suggestions(text).expect("valid wrapper must parse");
        assert_eq!(results.len(), 3);
        assert_eq!(results[2].content, "Three");
    }

    #[test]
    fn no_json_returns_empty() {
        let results = try_extract_suggestions("Just a normal response with no JSON.");
        assert!(results.is_empty());
    }

    #[test]
    fn wrapperless_json_is_not_treated_as_suggestions() {
        let err = extract_suggestions(r#"{"answer": [{"content": "not a suggestion"}]}"#)
            .expect_err("wrapper sentinel is required");
        assert_eq!(err, SuggestionExtractionError::MissingJson);
    }

    #[test]
    fn explicit_extract_reports_missing_json_without_raw_response() {
        let text = "No JSON here: alice@example.com OTP 123456 payroll \u{ae40}\u{bc94}\u{c900}";
        let err = extract_suggestions(text).unwrap_err().to_string();

        assert!(err.contains("did not include suggestion JSON"));
        assert!(!err.contains("alice@example.com"));
        assert!(!err.contains("123456"));
        assert!(!err.contains("payroll"));
        assert!(!err.contains("\u{ae40}\u{bc94}\u{c900}"));
    }

    #[test]
    fn malformed_json_returns_empty() {
        let text = r#"{"suggestions": [{"type": "work_guidance", "content": broken}]}"#;
        let results = try_extract_suggestions(text);
        assert!(results.is_empty());
    }

    #[test]
    fn explicit_extract_reports_malformed_json_without_raw_response() {
        let text = r#"{"suggestions": [{"type": "work_guidance", "content": "alice@example.com", "priority": broken}]}"#;
        let err = extract_suggestions(text).unwrap_err().to_string();

        assert!(err.contains("malformed suggestion JSON"));
        assert!(!err.contains("alice@example.com"));
        assert!(!err.contains("broken"));
    }

    #[test]
    fn empty_suggestions_array() {
        let text = r#"{"suggestions": []}"#;
        let results = try_extract_suggestions(text);
        assert!(results.is_empty());
    }

    #[test]
    fn admission_count_tracks_duplicate_and_full_queue_rejections() {
        let high = extract_suggestions(
            r#"{"suggestions": [{"type": "work_guidance", "content": "Keep this", "priority": "high"}]}"#,
        )
        .unwrap();
        let duplicate = high.clone();
        let low = extract_suggestions(
            r#"{"suggestions": [{"type": "productivity_tip", "content": "Lower priority", "priority": "low"}]}"#,
        )
        .unwrap();
        let mut queue = SuggestionQueue::new(1);

        assert_eq!(admit_suggestions(&mut queue, high), 1);
        assert_eq!(admit_suggestions(&mut queue, duplicate), 0);
        assert_eq!(admit_suggestions(&mut queue, low), 0);
        assert_eq!(queue.len(), 1);
    }
}
