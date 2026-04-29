//! Claude stream-json output -> OutboundMessage normalization.

use oneshim_core::models::ai_session::{OutboundMessage, TokenUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeEventKind {
    StreamChunk,
    AssistantSummary,
    Result,
}

#[derive(Debug)]
pub(crate) struct ClaudeNormalizedEvent {
    pub kind: ClaudeEventKind,
    pub message: OutboundMessage,
}

/// Parse a single line of Claude `--output-format stream-json` output into an
/// [`OutboundMessage`].  Returns `None` for unrecognised event types or
/// malformed JSON so callers can silently skip non-content lines.
pub(crate) fn normalize_claude_stream_event(line: &str) -> Option<ClaudeNormalizedEvent> {
    let event: serde_json::Value = serde_json::from_str(line).ok()?;
    match event.get("type")?.as_str()? {
        "assistant" => normalize_assistant_event(&event).map(|message| ClaudeNormalizedEvent {
            kind: ClaudeEventKind::AssistantSummary,
            message,
        }),
        "result" => Some(ClaudeNormalizedEvent {
            kind: ClaudeEventKind::Result,
            message: OutboundMessage::Result {
                content: event
                    .get("result")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                done: true,
                usage: extract_usage(event.get("usage")),
            },
        }),
        "stream_event" => {
            normalize_stream_event(event.get("event")?).map(|message| ClaudeNormalizedEvent {
                kind: ClaudeEventKind::StreamChunk,
                message,
            })
        }
        _ => None,
    }
}

fn normalize_assistant_event(event: &serde_json::Value) -> Option<OutboundMessage> {
    if let Some(text) = event.get("text").and_then(|value| value.as_str()) {
        if !text.is_empty() {
            return Some(OutboundMessage::Text {
                content: text.to_string(),
                done: false,
            });
        }
    }

    let content = event
        .get("message")
        .and_then(|value| value.get("content"))
        .and_then(|value| value.as_array())?;

    let text = content
        .iter()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some("text"))
        .filter_map(|item| item.get("text").and_then(|value| value.as_str()))
        .collect::<Vec<_>>()
        .join("");

    if !text.is_empty() {
        return Some(OutboundMessage::Text {
            content: text,
            done: false,
        });
    }

    let thinking = content
        .iter()
        .filter(|item| item.get("type").and_then(|value| value.as_str()) == Some("thinking"))
        .filter_map(|item| item.get("thinking").and_then(|value| value.as_str()))
        .collect::<Vec<_>>()
        .join("");

    if thinking.is_empty() {
        None
    } else {
        Some(OutboundMessage::Thinking {
            content: thinking,
            done: false,
        })
    }
}

fn normalize_stream_event(event: &serde_json::Value) -> Option<OutboundMessage> {
    match event.get("type")?.as_str()? {
        "content_block_start" => {
            let block = event.get("content_block")?;
            if block.get("type")?.as_str()? == "tool_use" {
                Some(OutboundMessage::ToolCallDelta {
                    index: event
                        .get("index")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32,
                    id: block
                        .get("id")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string(),
                    name: block
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string(),
                    arguments_chunk: String::new(),
                })
            } else {
                None
            }
        }
        "content_block_delta" => {
            let delta = event.get("delta")?;
            match delta.get("type")?.as_str()? {
                "text_delta" => {
                    let text = delta.get("text")?.as_str()?;
                    if text.is_empty() {
                        None
                    } else {
                        Some(OutboundMessage::Text {
                            content: text.to_string(),
                            done: false,
                        })
                    }
                }
                "thinking_delta" => {
                    let thinking = delta.get("thinking")?.as_str()?;
                    if thinking.is_empty() {
                        None
                    } else {
                        Some(OutboundMessage::Thinking {
                            content: thinking.to_string(),
                            done: false,
                        })
                    }
                }
                "input_json_delta" => Some(OutboundMessage::ToolCallDelta {
                    index: event
                        .get("index")
                        .and_then(|value| value.as_u64())
                        .unwrap_or(0) as u32,
                    id: String::new(),
                    name: String::new(),
                    arguments_chunk: delta
                        .get("partial_json")
                        .and_then(|value| value.as_str())
                        .unwrap_or("")
                        .to_string(),
                }),
                _ => None,
            }
        }
        "message_delta" => extract_usage(event.get("usage")).map(|usage| OutboundMessage::Result {
            content: String::new(),
            done: false,
            usage: Some(usage),
        }),
        _ => None,
    }
}

fn extract_usage(value: Option<&serde_json::Value>) -> Option<TokenUsage> {
    let usage = value?;
    Some(TokenUsage {
        input_tokens: usage.get("input_tokens")?.as_u64()?,
        output_tokens: usage.get("output_tokens")?.as_u64()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unwrap_message(line: &str) -> OutboundMessage {
        normalize_claude_stream_event(line)
            .expect("expected normalized event")
            .message
    }

    #[test]
    fn normalizes_legacy_assistant_text_event() {
        let line = r#"{"type":"assistant","subtype":"text","text":"hello world"}"#;
        let msg = unwrap_message(line);
        match msg {
            OutboundMessage::Text { content, done } => {
                assert_eq!(content, "hello world");
                assert!(!done);
            }
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn normalizes_nested_assistant_message_event() {
        let line =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hello world"}]}}"#;
        let normalized = normalize_claude_stream_event(line).unwrap();
        assert_eq!(normalized.kind, ClaudeEventKind::AssistantSummary);
        match normalized.message {
            OutboundMessage::Text { content, done } => {
                assert_eq!(content, "hello world");
                assert!(!done);
            }
            other => panic!("expected Text variant, got {other:?}"),
        }
    }

    #[test]
    fn normalizes_stream_text_delta_event() {
        let line = r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}}"#;
        let normalized = normalize_claude_stream_event(line).unwrap();
        assert_eq!(normalized.kind, ClaudeEventKind::StreamChunk);
        match normalized.message {
            OutboundMessage::Text { content, done } => {
                assert_eq!(content, "hello");
                assert!(!done);
            }
            other => panic!("expected Text variant, got {other:?}"),
        }
    }

    #[test]
    fn normalizes_result_event() {
        let line = r#"{"type":"result","subtype":"success","result":"final answer","usage":{"input_tokens":100,"output_tokens":50}}"#;
        let normalized = normalize_claude_stream_event(line).unwrap();
        assert_eq!(normalized.kind, ClaudeEventKind::Result);
        match normalized.message {
            OutboundMessage::Result {
                content,
                done,
                usage,
            } => {
                assert_eq!(content, "final answer");
                assert!(done);
                let u = usage.unwrap();
                assert_eq!(u.input_tokens, 100);
                assert_eq!(u.output_tokens, 50);
            }
            _ => panic!("expected Result variant"),
        }
    }

    #[test]
    fn returns_none_for_unknown_event() {
        let line = r#"{"type":"system","message":"loading"}"#;
        assert!(normalize_claude_stream_event(line).is_none());
    }

    #[test]
    fn returns_none_for_invalid_json() {
        assert!(normalize_claude_stream_event("not json").is_none());
    }

    #[test]
    fn handles_result_without_usage() {
        let line = r#"{"type":"result","subtype":"success","result":"answer"}"#;
        let msg = unwrap_message(line);
        match msg {
            OutboundMessage::Result { usage, .. } => assert!(usage.is_none()),
            _ => panic!("expected Result variant"),
        }
    }
}
