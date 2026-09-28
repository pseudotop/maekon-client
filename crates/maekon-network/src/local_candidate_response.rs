//! Bounded Ollama candidate-response decoding. No transport or execution tools.

use maekon_core::models::candidate_assessment::{LocalAssessmentChoice, LocalModelApproval};
use maekon_core::models::candidate_decision::{
    BackendFailure, DecisionText, DecisionUnavailable, DecisionUsage,
};
use serde::Deserialize;

/// Bound transport reads and parsing to the same response budget.
pub const MAX_LOCAL_CANDIDATE_RESPONSE_BYTES: usize = 32_768;
const MAX_TOKENS: u64 = 65_536;

#[derive(Deserialize)]
struct ChatResponse {
    model: String,
    done: bool,
    message: ChatMessage,
    prompt_eval_count: Option<u64>,
    eval_count: Option<u64>,
    remote_model: Option<String>,
    remote_host: Option<String>,
    error: Option<String>,
}
#[derive(Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
    tool_calls: Option<Vec<serde::de::IgnoredAny>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    selected: String,
}

/// Decode an already-attempted inference response. This function performs no
/// transport or authorization; the caller must retain its original runtime gate.
pub fn decode_response(
    bytes: &[u8],
    text: &DecisionText,
    approval: &LocalModelApproval,
) -> Result<(LocalAssessmentChoice, Option<DecisionUsage>), BackendFailure> {
    if bytes.len() > MAX_LOCAL_CANDIDATE_RESPONSE_BYTES {
        return Err(failure(DecisionUnavailable::ResponseTooLarge, true));
    }
    text.validate().map_err(|reason| failure(reason, true))?;
    let response: ChatResponse = serde_json::from_slice(bytes)
        .map_err(|_| failure(DecisionUnavailable::InvalidResponse, true))?;
    let usage = match (response.prompt_eval_count, response.eval_count) {
        (Some(input_tokens), Some(output_tokens))
            if input_tokens <= MAX_TOKENS && output_tokens <= MAX_TOKENS =>
        {
            Some(DecisionUsage {
                input_tokens,
                output_tokens,
            })
        }
        _ => None,
    };
    let invalid = || BackendFailure {
        usage,
        ..failure(DecisionUnavailable::InvalidResponse, true)
    };
    if !response.done
        || response.model != approval.model
        || response.message.role != "assistant"
        || response.remote_model.is_some()
        || response.remote_host.is_some()
        || response.error.is_some()
        || response
            .message
            .tool_calls
            .as_ref()
            .is_some_and(|calls| !calls.is_empty())
        || response
            .prompt_eval_count
            .is_some_and(|tokens| tokens > MAX_TOKENS)
        || response
            .eval_count
            .is_some_and(|tokens| tokens > MAX_TOKENS)
    {
        return Err(invalid());
    }
    let selected: Selection =
        serde_json::from_str(&response.message.content).map_err(|_| invalid())?;
    let choice = match selected.selected.as_str() {
        "none" => LocalAssessmentChoice::None,
        "delegate" => LocalAssessmentChoice::Delegate,
        id if text.candidates.iter().any(|candidate| candidate.id == id) => {
            LocalAssessmentChoice::Selected(id.into())
        }
        _ => return Err(invalid()),
    };
    Ok((choice, usage))
}

fn failure(reason: DecisionUnavailable, attempted: bool) -> BackendFailure {
    BackendFailure {
        request_hash: None,
        usage: None,
        reason,
        attempted,
    }
}

#[cfg(test)]
#[path = "local_candidate_response_tests.rs"]
mod tests;
