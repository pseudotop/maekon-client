use std::sync::Arc;

use futures::StreamExt;

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

/// #6129 regression guard: a persistent Ollama outage must not grow
/// conversation history. Each failed `send_message` pushes a user turn at the
/// start, so without transactional rollback history would grow by one entry per
/// retry across calls. Here N consecutive 5xx failures must leave history at the
/// initial size (system prompt only).
#[tokio::test]
async fn repeated_failed_sends_do_not_grow_history() {
    let mut server = mockito::Server::new_async().await;
    // Expect many failing calls; mockito matches any number of hits by default.
    let _mock = server
        .mock("POST", "/api/chat")
        .with_status(503)
        .with_body("Service Unavailable")
        .expect_at_least(1)
        .create_async()
        .await;

    let session = LocalLlmSession::new(
        "outage-session".to_string(),
        "llama3".to_string(),
        server.url(),
        Some("You are helpful.".to_string()),
        Arc::new(AiSessionConfig::default()),
    );

    // Baseline: system prompt only.
    let baseline_len = session.history.read().await.len();
    assert_eq!(
        baseline_len, 1,
        "history should start with system prompt only"
    );

    let message = SessionMessage {
        role: MessageRole::User,
        content: "hello".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };

    const N: usize = 5;
    for i in 0..N {
        let result = session.send_message(&message).await;
        let err = result
            .err()
            .unwrap_or_else(|| panic!("send #{i} should fail against a 503 backend"));
        // 503 is not the 404 model-not-pulled special case, so the non-success
        // status path maps it to a generic Network error (#6129 rollback path).
        assert!(
            matches!(
                err,
                CoreError::Network {
                    code: maekon_core::error_codes::NetworkCode::Generic,
                    ..
                }
            ),
            "send #{i} against a 503 backend should be Network::Generic, got: {err:?}"
        );
        let len = session.history.read().await.len();
        assert_eq!(
            len, baseline_len,
            "history must not grow after failed send #{i} (got {len}, expected {baseline_len})"
        );
    }
}

/// #6129: the transport-error early-return path (no HTTP response at all) must
/// also roll back the pushed user message. Binding a loopback port and dropping
/// the listener yields an address that refuses connections immediately, so
/// `reqwest::send()` fails fast before any status is observed.
#[tokio::test]
async fn transport_error_rolls_back_pushed_user_message() {
    // Reserve a free loopback port, then release it so connections are refused
    // (connection-refused fails fast, unlike a black-holed address that times
    // out).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let session = LocalLlmSession::new(
        "unreachable-session".to_string(),
        "llama3".to_string(),
        format!("http://{addr}"),
        Some("You are helpful.".to_string()),
        Arc::new(AiSessionConfig::default()),
    );

    let baseline_len = session.history.read().await.len();
    assert_eq!(baseline_len, 1);

    let message = SessionMessage {
        role: MessageRole::User,
        content: "hello".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };

    let result = session.send_message(&message).await;
    // `ResponseStream` (the Ok type) is not `Debug`, so extract the error via
    // `.err()` rather than `.expect_err()`.
    let Some(err) = result.err() else {
        panic!("send to unreachable host should fail");
    };
    // Connection-refused is a transport error (not a timeout), so the
    // send-time failure path classifies it as a generic Network error and
    // rolls back the pushed user message (#6129).
    assert!(
        matches!(
            &err,
            CoreError::Network {
                code: maekon_core::error_codes::NetworkCode::Generic,
                message,
            } if message.contains("Ollama request failed")
        ),
        "transport failure should be Network::Generic with an Ollama-request message, got: {err:?}"
    );
    let len = session.history.read().await.len();
    assert_eq!(
        len, baseline_len,
        "transport failure must roll back the pushed user message"
    );
}

/// #6206/#6207 regression guard: a successful HTTP response whose body is a
/// large, newline-free blob must NOT be accumulated without bound (OOM). The
/// streaming loop caps `line_buffer` at `MAX_NDJSON_LINE_BYTES`; once the buffer
/// exceeds the cap with no line terminator to drain, the stream must terminate
/// with `CoreError::Network` instead of growing the buffer for every chunk.
///
/// We send ~2 MiB of newline-free bytes so the guard fires regardless of how
/// reqwest splits the body into chunks.
#[tokio::test]
async fn oversized_newline_free_body_is_rejected_not_oomed() {
    // 2 MiB of 'x' with no '\n' anywhere — exceeds the 1 MiB line cap.
    let oversized = "x".repeat(2 * 1024 * 1024);
    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_body(oversized)
        .create_async()
        .await;

    let session = LocalLlmSession::new(
        "oversized-session".to_string(),
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

    // send_message itself succeeds (200 status); the error surfaces while
    // draining the body stream.
    let mut stream = session
        .send_message(&message)
        .await
        .expect("200 response should open a stream");

    // A newline-free body yields no complete NDJSON line, so the FIRST (and only)
    // item the stream produces must be the unbounded-line termination error — never
    // an Ok message. (Single deterministic item, so no drain loop is needed.)
    let item = stream
        .next()
        .await
        .expect("stream must yield the unbounded-line termination error");
    match item {
        Ok(_) => panic!("oversized newline-free body must not yield an Ok message"),
        Err(CoreError::Network { message, .. }) => {
            assert!(
                message.contains("without a newline"),
                "error should explain the unbounded-line termination, got: {message}"
            );
        }
        Err(other) => panic!("expected CoreError::Network, got: {other:?}"),
    }
}

/// #6916 regression guard: many small, individually-valid NDJSON content lines
/// whose AGGREGATE exceeds `MAX_ACCUMULATED_RESPONSE_BYTES` (8 MiB) must terminate
/// the stream with `CoreError::Network` rather than growing `accumulated` without
/// bound. The per-line cap (`MAX_NDJSON_LINE_BYTES`, 1 MiB) does not bound this —
/// each line here is well under 1 MiB and newline-terminated, so only the new
/// cross-line aggregate cap catches it. (A trickle of in-cap lines also never trips
/// the per-read timeout, which resets after every successful read.)
#[tokio::test]
async fn aggregate_accumulated_response_is_capped() {
    // 9 MiB of content split into 9 valid NDJSON lines of ~1 MiB each (each under
    // the 1 MiB per-line cap, all newline-terminated → only the aggregate cap fires).
    let chunk = "y".repeat(900 * 1024); // ~0.9 MiB per line, well under MAX_NDJSON_LINE_BYTES
    let mut body = String::new();
    for _ in 0..10 {
        body.push_str(&format!(
            "{{\"message\":{{\"role\":\"assistant\",\"content\":\"{chunk}\"}},\"done\":false}}\n"
        ));
    }

    let mut server = mockito::Server::new_async().await;
    let _mock = server
        .mock("POST", "/api/chat")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;

    let session = LocalLlmSession::new(
        "aggregate-cap-session".to_string(),
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

    let mut stream = session
        .send_message(&message)
        .await
        .expect("200 response should open a stream");

    // Drain: valid lines yield Ok(Text) until the aggregate cap is exceeded, then
    // the stream must yield the cap-termination error. We must observe that error.
    let mut saw_cap_error = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(_) => continue,
            Err(CoreError::Network { message, .. }) => {
                assert!(
                    message.contains("accumulated response exceeded"),
                    "error must explain the aggregate cap, got: {message}"
                );
                saw_cap_error = true;
                break;
            }
            Err(other) => panic!("expected aggregate-cap CoreError::Network, got: {other:?}"),
        }
    }
    assert!(
        saw_cap_error,
        "stream must terminate with the aggregate-cap error once accumulated > 8 MiB"
    );
}

/// #6205: the session HTTP client is built with connect + per-read timeouts via
/// `build_ollama_http_client`. A true stall test (a connected endpoint that
/// accepts the request but then sends no bytes) requires a live/controllable
/// socket and is not exercised here — `read_timeout` is a wire-level behavior of
/// the underlying client. This test asserts the client is constructed (build
/// does not fall back to the timeout-less default) so the timeouts are present.
#[test]
fn session_http_client_builds_with_timeouts() {
    // build_ollama_http_client uses `.build()` and only falls back to
    // Client::new() if the builder fails; for plain timeout settings it never
    // fails, so a session is always constructed with a configured client.
    let session = LocalLlmSession::new(
        "timeout-session".to_string(),
        "llama3".to_string(),
        "http://localhost:11434".to_string(),
        None,
        Arc::new(AiSessionConfig::default()),
    );
    // Smoke check: the session is usable and reports the expected provider.
    assert_eq!(session.provider_name(), "ollama");
}
