use std::sync::Arc;

use crate::error::NetworkError;
use maekon_core::config::AiSessionConfig;
use maekon_core::error::CoreError;
use maekon_core::models::ai_session::{
    truncate_chat_history, Attachment, ChatMessage, ChatRole, ContentBlock, MessageContext,
    MessageRole, SessionMessage, SessionState, SessionTransport, TokenUsage, ToolDefinition,
};
use maekon_core::ports::conversation_session::ConversationSession;

use super::helpers::{
    local_content_blocks, ollama_message_payload, parse_ndjson_line, render_local_message_content,
};
use super::types::LocalLlmSession;

// ── NDJSON parsing ──────────────────────────────────────────

#[test]
fn parse_ndjson_content_chunk() {
    let line =
        r#"{"model":"llama3","message":{"role":"assistant","content":"Hello"},"done":false}"#;
    let chunk = parse_ndjson_line(line).unwrap();
    assert!(!chunk.done);
    assert_eq!(chunk.message.as_ref().unwrap().content, "Hello");
    assert!(chunk.eval_count.is_none());
}

#[test]
fn parse_ndjson_final_chunk_with_token_usage() {
    let line = r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":true,"eval_count":50,"prompt_eval_count":20}"#;
    let chunk = parse_ndjson_line(line).unwrap();
    assert!(chunk.done);
    assert_eq!(chunk.eval_count, Some(50));
    assert_eq!(chunk.prompt_eval_count, Some(20));
}

#[test]
fn parse_ndjson_final_chunk_without_usage() {
    let line = r#"{"done":true}"#;
    let chunk = parse_ndjson_line(line).unwrap();
    assert!(chunk.done);
    assert!(chunk.eval_count.is_none());
    assert!(chunk.prompt_eval_count.is_none());
}

#[test]
fn parse_ndjson_invalid_json_returns_error() {
    let line = "not json at all";
    let err = parse_ndjson_line(line).unwrap_err();
    assert!(
        matches!(err, NetworkError::Internal(_)),
        "invalid NDJSON must produce NetworkError::Internal, got: {err:?}"
    );
}

#[test]
fn render_local_message_content_includes_optional_sections() {
    let message = SessionMessage {
        role: MessageRole::User,
        content: "Summarize this".to_string(),
        attachments: vec![Attachment::File {
            path: "/tmp/notes.md".to_string(),
            mime: Some("text/markdown".to_string()),
            data: Some("IyBOb3RlcwoKLSBmaXJzdAo=".to_string()),
        }],
        tools: Some(vec![ToolDefinition {
            name: "get_sessions".to_string(),
            description: "List sessions".to_string(),
            endpoint: "http://localhost/api/sessions".to_string(),
            method: "GET".to_string(),
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            })),
        }]),
        context: Some(MessageContext {
            regime: Some("focus".to_string()),
            active_app: Some("VS Code".to_string()),
        }),
        response_format: Some(serde_json::json!({
            "type": "json_schema",
            "json_schema": {
                "name": "answer",
                "schema": { "type": "object" }
            }
        })),
    };

    let rendered = render_local_message_content(&message);
    assert!(rendered.contains("Additional context JSON"));
    assert!(rendered.contains("Attachments JSON"));
    assert!(rendered.contains("Attachment content previews JSON"));
    assert!(rendered.contains("Available tools JSON"));
    assert!(rendered.contains("Required response schema JSON"));
    assert!(rendered.contains("Notes"));
}

#[test]
fn render_local_message_content_skips_binary_attachment_previews() {
    let message = SessionMessage {
        role: MessageRole::User,
        content: "Summarize this".to_string(),
        attachments: vec![Attachment::File {
            path: "/tmp/photo.png".to_string(),
            mime: Some("image/png".to_string()),
            data: Some("iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB".to_string()),
        }],
        tools: None,
        context: None,
        response_format: None,
    };

    let rendered = render_local_message_content(&message);
    assert!(rendered.contains("Attachments JSON"));
    assert!(!rendered.contains("Attachment content previews JSON"));
}

#[test]
fn render_local_message_content_falls_back_to_response_format_when_schema_missing() {
    let message = SessionMessage {
        role: MessageRole::User,
        content: "Summarize this".to_string(),
        attachments: Vec::new(),
        tools: None,
        context: None,
        response_format: Some(serde_json::json!({
            "type": "json_object"
        })),
    };

    let rendered = render_local_message_content(&message);
    assert!(rendered.contains("Required response format JSON"));
    assert!(!rendered.contains("Required response schema JSON"));
}

#[test]
fn local_content_blocks_include_image_attachments() {
    let blocks = local_content_blocks(
        "Describe this image",
        &[
            Attachment::Image {
                mime: "image/png".to_string(),
                path: None,
                data: Some("iVBORw0KGgo=".to_string()),
            },
            Attachment::File {
                path: "/tmp/chart.jpg".to_string(),
                mime: Some("image/jpeg".to_string()),
                data: Some("/9j/4AAQSkZJRg==".to_string()),
            },
        ],
    )
    .expect("image attachments should produce content blocks");

    assert_eq!(blocks.len(), 3);
    assert!(matches!(blocks[0], ContentBlock::Text { .. }));
    assert!(matches!(blocks[1], ContentBlock::Image { .. }));
    assert!(matches!(blocks[2], ContentBlock::Image { .. }));
}

#[test]
fn ollama_message_payload_emits_images_from_content_blocks() {
    let payload = ollama_message_payload(&ChatMessage {
        role: ChatRole::User,
        content: "fallback".to_string(),
        content_blocks: Some(vec![
            ContentBlock::Text {
                text: "Describe this image".to_string(),
            },
            ContentBlock::Image {
                media_type: "image/png".to_string(),
                data: "iVBORw0KGgo=".to_string(),
            },
        ]),
    });

    assert_eq!(payload["content"], "Describe this image");
    assert_eq!(payload["images"][0], "iVBORw0KGgo=");
}

// ── History truncation ──────────────────────────────────────

#[test]
fn truncate_preserves_system_prompt() {
    let mut history = vec![
        ChatMessage {
            role: ChatRole::System,
            content: "You are helpful.".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "msg 1".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::Assistant,
            content: "resp 1".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "msg 2".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::Assistant,
            content: "resp 2".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "msg 3".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::Assistant,
            content: "resp 3".to_string(),
            content_blocks: None,
        },
    ];

    // Keep max 4 messages: system + last 3
    truncate_chat_history(&mut history, 4);

    assert_eq!(history.len(), 4);
    // First message is always the system prompt.
    assert_eq!(history[0].role, ChatRole::System);
    assert_eq!(history[0].content, "You are helpful.");
    // Last 3 messages are the most recent.
    assert_eq!(history[1].content, "resp 2");
    assert_eq!(history[2].content, "msg 3");
    assert_eq!(history[3].content, "resp 3");
}

#[test]
fn truncate_no_op_when_under_limit() {
    let mut history = vec![
        ChatMessage {
            role: ChatRole::System,
            content: "system".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "hello".to_string(),
            content_blocks: None,
        },
    ];

    truncate_chat_history(&mut history, 10);
    assert_eq!(history.len(), 2);
}

#[test]
fn truncate_exact_boundary() {
    let mut history = vec![
        ChatMessage {
            role: ChatRole::System,
            content: "system".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "a".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::Assistant,
            content: "b".to_string(),
            content_blocks: None,
        },
    ];

    truncate_chat_history(&mut history, 3);
    assert_eq!(history.len(), 3);
}

// ── Session construction and info ───────────────────────────

#[test]
fn session_info_returns_correct_metadata() {
    let config = Arc::new(AiSessionConfig::default());
    let session = LocalLlmSession::new(
        "test-session-1".to_string(),
        "llama3".to_string(),
        "http://localhost:11434".to_string(),
        Some("You are helpful.".to_string()),
        config,
    );

    let info = session.info();
    assert_eq!(info.session_id, "test-session-1");
    assert_eq!(info.provider_name, "ollama");
    assert_eq!(info.model, "llama3");
    assert_eq!(info.state, SessionState::Active);
    assert_eq!(info.transport, SessionTransport::LocalLlm);
    assert_eq!(info.turn_count, 0);
}

#[test]
fn session_id_and_provider_name() {
    let config = Arc::new(AiSessionConfig::default());
    let session = LocalLlmSession::new(
        "sid-42".to_string(),
        "qwen3:8b".to_string(),
        "http://localhost:11434".to_string(),
        None,
        config,
    );

    assert_eq!(session.session_id(), "sid-42");
    assert_eq!(session.provider_name(), "ollama");
}

#[test]
fn session_initializes_with_system_prompt_in_history() {
    let config = Arc::new(AiSessionConfig::default());
    let session = LocalLlmSession::new(
        "s1".to_string(),
        "llama3".to_string(),
        "http://localhost:11434".to_string(),
        Some("Be concise.".to_string()),
        config,
    );

    let history = session.history.blocking_read();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].role, ChatRole::System);
    assert_eq!(history[0].content, "Be concise.");
}

#[test]
fn session_initializes_empty_history_without_system_prompt() {
    let config = Arc::new(AiSessionConfig::default());
    let session = LocalLlmSession::new(
        "s2".to_string(),
        "llama3".to_string(),
        "http://localhost:11434".to_string(),
        None,
        config,
    );

    let history = session.history.blocking_read();
    assert_eq!(history.len(), 0);
}

#[test]
fn base_url_trailing_slash_stripped() {
    let config = Arc::new(AiSessionConfig::default());
    let session = LocalLlmSession::new(
        "s3".to_string(),
        "llama3".to_string(),
        "http://localhost:11434/".to_string(),
        None,
        config,
    );

    assert_eq!(session.base_url, "http://localhost:11434");
}

// ── NDJSON to OutboundMessage normalization ─────────────────

#[test]
fn ndjson_chunk_to_outbound_text() {
    let line =
        r#"{"model":"llama3","message":{"role":"assistant","content":"world"},"done":false}"#;
    let chunk = parse_ndjson_line(line).unwrap();

    assert!(!chunk.done);
    let content = &chunk.message.as_ref().unwrap().content;
    assert_eq!(content, "world");
}

#[test]
fn ndjson_final_to_outbound_result_with_usage() {
    let line = r#"{"model":"llama3","message":{"role":"assistant","content":""},"done":true,"eval_count":123,"prompt_eval_count":45}"#;
    let chunk = parse_ndjson_line(line).unwrap();

    assert!(chunk.done);
    let usage = TokenUsage {
        input_tokens: chunk.prompt_eval_count.unwrap(),
        output_tokens: chunk.eval_count.unwrap(),
    };
    assert_eq!(usage.input_tokens, 45);
    assert_eq!(usage.output_tokens, 123);
}

/// iter-73 regression guard for iter-55c Ollama 404 semantic mapping.
/// Ollama returns 404 when a model isn't pulled; we surface this as
/// CoreError::NotFound with resource_type="ollama_model" so frontend
/// can suggest `ollama pull <model>` rather than "network error".
#[tokio::test]
async fn ollama_404_maps_to_not_found_with_model_hint() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/api/chat")
        .with_status(404)
        .with_body("model 'llama3' not found — try `ollama pull llama3`")
        .create_async()
        .await;

    let session = LocalLlmSession::new(
        "test-session".to_string(),
        "llama3".to_string(),
        server.url(),
        None,
        Arc::new(AiSessionConfig::default()),
    );

    let message = SessionMessage {
        role: MessageRole::User,
        content: "hello".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };

    let result = session.send_message(&message).await;
    match result {
        Err(CoreError::NotFound {
            resource_type, id, ..
        }) => {
            assert_eq!(
                resource_type, "ollama_model",
                "resource_type should be ollama_model"
            );
            assert!(
                id.contains("ollama pull") || id.contains("not found"),
                "id should carry the pull hint, got: {id}"
            );
        }
        Err(other) => panic!("expected CoreError::NotFound, got: {other:?}"),
        Ok(_) => panic!("expected error, got Ok"),
    }
}

/// iter-76 regression guard: non-404 HTTP errors from Ollama fall back
/// to CoreError::Network (not NotFound), so the "model not pulled" UX
/// only triggers for the specific 404 case.
#[tokio::test]
async fn ollama_500_maps_to_network_generic() {
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/api/chat")
        .with_status(500)
        .with_body("Internal Server Error")
        .create_async()
        .await;

    let session = LocalLlmSession::new(
        "test-session".to_string(),
        "llama3".to_string(),
        server.url(),
        None,
        Arc::new(AiSessionConfig::default()),
    );

    let message = SessionMessage {
        role: MessageRole::User,
        content: "hello".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };

    let result = session.send_message(&message).await;
    match result {
        Err(CoreError::Network { .. }) => {
            // Expected: 500 (non-404) falls back to Network/Generic
        }
        Err(other) => {
            panic!("500 should map to CoreError::Network (not domain-specific), got: {other:?}")
        }
        Ok(_) => panic!("expected error, got Ok"),
    }
}
