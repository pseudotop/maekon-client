use maekon_core::config::AiProviderType;
use maekon_core::models::memory_graph::{ClaimStatusChange, RelationEdgeProposal};
use maekon_core::models::suggestion::Suggestion;

use crate::error::NetworkError;
use maekon_http_core::provider_error_body::provider_parse_error_message;

/// Extract the first top-level JSON array substring from an LLM text response
/// (tolerant of ```` ```json ```` fences and surrounding prose).
fn extract_json_array(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let json_str = if trimmed.starts_with("```") {
        let inner = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        inner.strip_suffix("```").unwrap_or(inner).trim()
    } else {
        trimmed
    };
    let start = json_str.find('[')?;
    let end = json_str.rfind(']')?;
    if end < start {
        return None;
    }
    Some(json_str[start..=end].to_string())
}

/// Parse ADR-023 Phase-2 D1 relation proposals from an LLM text response.
/// Tolerant: returns an empty `Vec` on any extraction/parse failure (degrade,
/// never error). An unknown `edge_type` fails the whole parse → empty.
pub(crate) fn parse_relation_proposals(text: &str) -> Vec<RelationEdgeProposal> {
    extract_json_array(text)
        .and_then(|arr| serde_json::from_str::<Vec<RelationEdgeProposal>>(&arr).ok())
        .unwrap_or_default()
}

/// Parse ADR-023 Phase-2 D2 contradiction resolutions from an LLM text response.
/// Tolerant (same degrade contract as [`parse_relation_proposals`]).
pub(crate) fn parse_status_changes(text: &str) -> Vec<ClaimStatusChange> {
    extract_json_array(text)
        .and_then(|arr| serde_json::from_str::<Vec<ClaimStatusChange>>(&arr).ok())
        .unwrap_or_default()
}

/// LLM suggestion candidate wire shape.
///
/// #10050 AC3: this is an alias onto the shared `maekon-core` type rather than a
/// local copy, so the HTTP adapter here and the subprocess-CLI analysis provider
/// in `src-tauri` cannot drift into different wire shapes.
pub(crate) type SuggestionCandidate = maekon_core::models::suggestion::LlmSuggestionCandidate;

/// Extract text content from provider-specific response format.
pub(crate) fn extract_text(
    provider_type: AiProviderType,
    body: &serde_json::Value,
) -> Result<String, NetworkError> {
    match provider_type {
        AiProviderType::Anthropic => body
            .get("content")
            .and_then(|c| c.as_array())
            .and_then(|arr| arr.first())
            .and_then(|block| block.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| NetworkError::Analysis("No text in Anthropic response".to_string())),
        _ => {
            // OpenAI / Generic / Ollama: choices[0].message.content
            body.get("choices")
                .and_then(|c| c.as_array())
                .and_then(|arr| arr.first())
                .and_then(|choice| choice.get("message"))
                .and_then(|msg| msg.get("content"))
                .and_then(|t| t.as_str())
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    NetworkError::Analysis("No text in OpenAI/Generic response".to_string())
                })
        }
    }
}

/// Parse candidates from JSON text extracted from LLM response.
pub(crate) fn parse_candidates(text: &str) -> Result<Vec<SuggestionCandidate>, NetworkError> {
    // Strip markdown fences if present
    let trimmed = text.trim();
    let json_str = if trimmed.starts_with("```") {
        let inner = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        inner.strip_suffix("```").unwrap_or(inner).trim()
    } else {
        trimmed
    };

    // Find the JSON array in the text
    let start = json_str.find('[').ok_or_else(|| {
        NetworkError::Analysis(provider_parse_error_message(
            "Analysis API",
            "no JSON array found in LLM response",
        ))
    })?;
    let end = json_str.rfind(']').ok_or_else(|| {
        NetworkError::Analysis(provider_parse_error_message(
            "Analysis API",
            "no closing bracket in LLM response",
        ))
    })?;

    // Guard reversed bounds (e.g. adversarial `] ... [`): a closing bracket
    // before the first opening bracket would make `json_str[start..=end]` a
    // reversed inclusive-range slice, which panics. Mirror the sibling
    // `extract_json_array` guard and degrade to a privacy-safe error instead.
    if end < start {
        return Err(NetworkError::Analysis(provider_parse_error_message(
            "Analysis API",
            "malformed JSON array bounds in LLM response",
        )));
    }

    let array_str = &json_str[start..=end];
    serde_json::from_str(array_str).map_err(|e| {
        NetworkError::Analysis(provider_parse_error_message(
            "Analysis API",
            &format!("failed to parse suggestion candidates ({e})"),
        ))
    })
}

/// Convert a parsed candidate into a domain `Suggestion`.
///
/// Stateless — kept as a free function so existing tests can call it without
/// constructing an `AnalysisClient` instance.
/// D5 iter-5: sanitization happens in the caller
/// (`candidate_to_suggestion_sanitized`) because the sanitizer is instance state.
pub(crate) fn candidate_to_suggestion(candidate: SuggestionCandidate) -> Suggestion {
    // #10050 AC3: delegate to the shared mapping in `maekon-core` so this HTTP
    // adapter and the subprocess-CLI analysis provider apply identical type and
    // priority rules. Do NOT re-inline the match/threshold logic here.
    Suggestion::from_llm_candidate(candidate)
}
