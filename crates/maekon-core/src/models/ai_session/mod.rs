//! AI conversation session models — JSONL protocol types, session metadata,
//! and context assembly data structures.
//!
//! Split from a single 618-line file per ADR-013.
//! Public API unchanged — all types re-exported at the same paths.

mod context;
mod protocol;
mod session;

// Re-export everything at identical public paths.
pub use context::{
    ActivitySummary, SkillInfo, SuggestionPatterns, SystemInfo, SystemPromptContext,
    UserProfileSummary,
};
pub use protocol::{
    validate_session_input_size, Attachment, ContentBlock, ControlAction, InboundMessage,
    MessageContext, MessageRole, OutboundMessage, SessionInputLimitError, SessionMessage,
    SessionState, SessionTransport, TokenUsage, ToolDefinition, ToolUseStatus,
    MAX_SESSION_ATTACHMENTS, MAX_SESSION_INPUT_BYTES, SESSION_INPUT_TOO_LARGE_CODE,
};
pub use session::{
    truncate_chat_history, ChatMessage, ChatRole, ConversationSessionInfo, MessageRecord,
    SessionAuditCategory, SessionAuditEntry, SessionConfig, SessionRecord,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_inbound_message() {
        let msg = InboundMessage::Message(SessionMessage {
            screen_derived: false,
            role: MessageRole::User,
            content: "hello".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"message\""));
        assert!(json.contains("\"role\":\"user\""));
    }

    #[test]
    fn serializes_outbound_text() {
        let msg = OutboundMessage::Text {
            content: "hi".to_string(),
            done: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"done\":false"));
    }

    #[test]
    fn serializes_attachment_image() {
        let att = Attachment::Image {
            mime: "image/png".to_string(),
            path: Some("/tmp/test.png".to_string()),
            data: None,
        };
        let json = serde_json::to_string(&att).unwrap();
        assert!(json.contains("\"kind\":\"image\""));
    }

    #[test]
    fn session_input_size_counts_inline_attachment_data() {
        let attachment = Attachment::File {
            path: "huge.txt".to_string(),
            mime: Some("text/plain".to_string()),
            data: Some("a".repeat(MAX_SESSION_INPUT_BYTES)),
        };

        let err = validate_session_input_size("message", "hi", &[attachment])
            .expect_err("oversized inline attachment data must be rejected");
        assert_eq!(err.code, SESSION_INPUT_TOO_LARGE_CODE);
    }

    #[test]
    fn session_input_size_limits_attachment_count() {
        let attachments = (0..=MAX_SESSION_ATTACHMENTS)
            .map(|idx| Attachment::Directory {
                path: format!("dir-{idx}"),
            })
            .collect::<Vec<_>>();

        let err = validate_session_input_size("message", "hi", &attachments)
            .expect_err("too many attachments must be rejected");
        assert_eq!(err.code, SESSION_INPUT_TOO_LARGE_CODE);
    }

    #[test]
    fn deserializes_outbound_error() {
        let json = r#"{"type":"error","code":"rate_limit","message":"exceeded","retryable":true}"#;
        let msg: OutboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            OutboundMessage::Error {
                code, retryable, ..
            } => {
                assert_eq!(code, "rate_limit");
                assert!(retryable);
            }
            _ => panic!("expected Error variant"),
        }
    }

    #[test]
    fn chat_message_roundtrip() {
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: "hi".to_string(),
            content_blocks: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"role\":\"assistant\""));
        let parsed: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.role, ChatRole::Assistant);
    }

    #[test]
    fn session_state_roundtrip() {
        let state = SessionState::Active;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"active\"");
        let parsed: SessionState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, SessionState::Active);
    }

    #[test]
    fn content_block_text_roundtrip() {
        let block = ContentBlock::Text {
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"hello world\""));
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::Text { text } => assert_eq!(text, "hello world"),
            _ => panic!("expected Text variant"),
        }
    }

    #[test]
    fn content_block_image_roundtrip() {
        let block = ContentBlock::Image {
            media_type: "image/png".to_string(),
            data: "base64data==".to_string(),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"image\""));
        assert!(json.contains("\"media_type\":\"image/png\""));
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::Image { media_type, data } => {
                assert_eq!(media_type, "image/png");
                assert_eq!(data, "base64data==");
            }
            _ => panic!("expected Image variant"),
        }
    }

    #[test]
    fn content_block_file_roundtrip() {
        let block = ContentBlock::File {
            media_type: "application/pdf".to_string(),
            data: "JVBERi0xLjQK".to_string(),
            filename: Some("notes.pdf".to_string()),
        };
        let json = serde_json::to_string(&block).unwrap();
        assert!(json.contains("\"type\":\"file\""));
        assert!(json.contains("\"media_type\":\"application/pdf\""));
        let parsed: ContentBlock = serde_json::from_str(&json).unwrap();
        match parsed {
            ContentBlock::File {
                media_type,
                data,
                filename,
            } => {
                assert_eq!(media_type, "application/pdf");
                assert_eq!(data, "JVBERi0xLjQK");
                assert_eq!(filename.as_deref(), Some("notes.pdf"));
            }
            _ => panic!("expected File variant"),
        }
    }

    #[test]
    fn chat_message_backward_compat_no_content_blocks() {
        // Old JSON without content_blocks should deserialize successfully
        let json = r#"{"role":"user","content":"hello"}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, ChatRole::User);
        assert_eq!(msg.content, "hello");
        assert!(msg.content_blocks.is_none());
    }

    #[test]
    fn chat_message_with_content_blocks() {
        let msg = ChatMessage {
            role: ChatRole::Assistant,
            content: "summary".to_string(),
            content_blocks: Some(vec![
                ContentBlock::Text {
                    text: "part 1".to_string(),
                },
                ContentBlock::Thinking {
                    thinking: "let me think".to_string(),
                },
            ]),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"content_blocks\""));
        let parsed: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.content_blocks.as_ref().unwrap().len(), 2);
    }

    #[test]
    fn outbound_thinking_serialization() {
        let msg = OutboundMessage::Thinking {
            content: "reasoning step".to_string(),
            done: false,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"thinking\""));
        assert!(json.contains("\"content\":\"reasoning step\""));
        assert!(json.contains("\"done\":false"));
        let parsed: OutboundMessage = serde_json::from_str(&json).unwrap();
        match parsed {
            OutboundMessage::Thinking { content, done } => {
                assert_eq!(content, "reasoning step");
                assert!(!done);
            }
            _ => panic!("expected Thinking variant"),
        }
    }

    #[test]
    fn tool_definition_with_schema() {
        let tool = ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather info".to_string(),
            endpoint: "/weather".to_string(),
            method: "GET".to_string(),
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "city": {"type": "string"}
                }
            })),
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("\"input_schema\""));
        assert!(json.contains("\"properties\""));
        let parsed: ToolDefinition = serde_json::from_str(&json).unwrap();
        assert!(parsed.input_schema.is_some());
    }

    #[test]
    fn tool_definition_without_schema_omits_field() {
        let tool = ToolDefinition {
            name: "ping".to_string(),
            description: "Ping".to_string(),
            endpoint: "/ping".to_string(),
            method: "GET".to_string(),
            input_schema: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(!json.contains("input_schema"));
    }
}
