use reqwest::StatusCode;

/// Privacy-safe marker describing a remote response body without echoing it.
///
/// Remote bodies can carry prompts, OCR text, queries, secrets, or PII. This
/// returns only the *state* of the body (`body=empty`, `body=omitted_for_privacy`,
/// `body=unavailable`), never the body itself, so it is safe to embed in any
/// error field that is logged or persisted.
pub fn provider_error_body_state(body: Option<&str>) -> &'static str {
    match body {
        Some(body) if body.trim().is_empty() => "body=empty",
        Some(_) => "body=omitted_for_privacy",
        None => "body=unavailable",
    }
}

/// Build a privacy-safe provider error message for local propagation.
///
/// Remote provider error bodies can echo prompts, OCR text, embedding queries,
/// or session payloads. Keep status metadata, but never include the body itself
/// in errors, logs, or UI-facing messages.
pub fn provider_error_message(provider: &str, status: StatusCode, body: Option<&str>) -> String {
    let body_state = provider_error_body_state(body);
    format!("{provider} error ({status}): provider response {body_state}")
}

/// Build a privacy-safe provider parse error message.
///
/// Successful HTTP responses can still contain raw model text that fails local
/// parsing. Treat that text like a provider body: preserve enough context for
/// debugging, but never echo the payload.
pub fn provider_parse_error_message(provider: &str, context: &str) -> String {
    format!("{provider} parse error: {context}; provider response body=omitted_for_privacy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_error_message_never_includes_echoed_private_body() {
        let private_body = r#"{
            "error": "prompt echoed: alice@example.com saw OTP 123456 in OCR text",
            "diagnostic": "query=payroll for 김범준"
        }"#;

        for provider in [
            "Analysis API",
            "Summarize API",
            "Embedding API",
            "LLM API",
            "OCR API",
            "HTTP API",
        ] {
            let message = provider_error_message(
                provider,
                StatusCode::INTERNAL_SERVER_ERROR,
                Some(private_body),
            );
            assert!(
                message.contains(provider),
                "provider metadata kept: {message}"
            );
            assert!(
                message.contains("500 Internal Server Error"),
                "status metadata kept: {message}"
            );
            assert!(
                message.contains("body=omitted_for_privacy"),
                "body state kept: {message}"
            );
            assert!(
                !message.contains("alice@example.com"),
                "email leaked: {message}"
            );
            assert!(!message.contains("123456"), "OTP leaked: {message}");
            assert!(!message.contains("payroll"), "query leaked: {message}");
            assert!(!message.contains("김범준"), "name leaked: {message}");
        }
    }

    #[test]
    fn provider_error_message_distinguishes_empty_and_unavailable_body() {
        let empty = provider_error_message("LLM API", StatusCode::BAD_GATEWAY, Some("  "));
        assert!(empty.contains("body=empty"));
        assert!(!empty.contains("omitted_for_privacy"));

        let unavailable = provider_error_message("LLM API", StatusCode::BAD_GATEWAY, None);
        assert!(unavailable.contains("body=unavailable"));
        assert!(!unavailable.contains("omitted_for_privacy"));
    }

    #[test]
    fn provider_parse_error_message_never_includes_raw_model_output() {
        let message = provider_parse_error_message("LLM API", "invalid action JSON");

        assert!(message.contains("LLM API parse error"));
        assert!(message.contains("invalid action JSON"));
        assert!(message.contains("body=omitted_for_privacy"));
        assert!(!message.contains("alice@example.com"));
        assert!(!message.contains("123456"));
    }
}
