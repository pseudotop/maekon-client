use super::*;
use maekon_core::models::ai_session::Attachment;

#[allow(clippy::too_many_arguments)]
fn test_session(
    surface_id: String,
    model: String,
    endpoint: String,
    credential: CredentialSource,
    provider_type: AiProviderType,
    system_prompt: Option<String>,
    config: Arc<AiSessionConfig>,
    default_tools: Option<Vec<ToolDefinition>>,
) -> HttpApiSession {
    HttpApiSession::new(HttpApiSessionInit {
        surface_id,
        model,
        endpoint,
        credential,
        provider_type,
        system_prompt,
        config,
        default_tools,
        breaker_registry: crate::CircuitBreakerRegistry::new(),
    })
}

#[test]
fn anthropic_content_block_delta() {
    let data =
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#;
    let msg = parse_anthropic_sse_event("content_block_delta", data);
    match msg {
        Some(OutboundMessage::Text { content, done }) => {
            assert_eq!(content, "Hello");
            assert!(!done);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn anthropic_message_stop() {
    let data = r#"{"type":"message_stop"}"#;
    let msg = parse_anthropic_sse_event("message_stop", data);
    match msg {
        Some(OutboundMessage::Result { done, .. }) => {
            assert!(done);
        }
        other => panic!("expected Result with done=true, got {other:?}"),
    }
}

#[test]
fn anthropic_message_delta_with_usage() {
    let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":25,"output_tokens":50}}"#;
    let msg = parse_anthropic_sse_event("message_delta", data);
    match msg {
        Some(OutboundMessage::Result { usage, .. }) => {
            let u = usage.unwrap();
            assert_eq!(u.input_tokens, 25);
            assert_eq!(u.output_tokens, 50);
        }
        other => panic!("expected Result with usage, got {other:?}"),
    }
}

#[test]
fn anthropic_message_start_captures_input_only() {
    // #8057 (P2-1): input_tokens ride on message_start; capture them as an
    // input-only usage chunk (output forced to 0 so it does not double-count
    // the output the message_delta chunk adds).
    let data = r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":42,"output_tokens":1}}}"#;
    let msg = parse_anthropic_sse_event("message_start", data);
    match msg {
        Some(OutboundMessage::Result { usage, done, .. }) => {
            let u = usage.unwrap();
            assert_eq!(u.input_tokens, 42);
            assert_eq!(u.output_tokens, 0);
            assert!(!done);
        }
        other => panic!("expected Result with input-only usage, got {other:?}"),
    }
}

#[test]
fn anthropic_message_delta_output_only_updates_usage() {
    // #8057 (P2-1): the real wire omits input_tokens on message_delta. The old
    // parser required both and dropped the whole usage; now output alone
    // suffices (input defaults to 0, already accounted on message_start).
    let data = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":73}}"#;
    let msg = parse_anthropic_sse_event("message_delta", data);
    match msg {
        Some(OutboundMessage::Result { usage, .. }) => {
            let u = usage.unwrap();
            assert_eq!(u.input_tokens, 0);
            assert_eq!(u.output_tokens, 73);
        }
        other => panic!("expected Result with output usage, got {other:?}"),
    }
}

#[test]
fn anthropic_message_start_without_usage_is_ignored() {
    // A message_start carrying no usage yields nothing rather than a zero chunk.
    let msg = parse_anthropic_sse_event(
        "message_start",
        r#"{"type":"message_start","message":{"id":"msg_1"}}"#,
    );
    assert!(msg.is_none());
}

#[test]
fn anthropic_ignores_unknown_event() {
    let msg = parse_anthropic_sse_event("ping", "{}");
    assert!(msg.is_none());
}

#[test]
fn openai_content_delta() {
    let data = r#"{"choices":[{"index":0,"delta":{"content":"world"}}]}"#;
    let msg = parse_openai_chat_sse_event(data);
    match msg {
        Some(OutboundMessage::Text { content, done }) => {
            assert_eq!(content, "world");
            assert!(!done);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn openai_done_event() {
    let msg = parse_openai_chat_sse_event("[DONE]");
    match msg {
        Some(OutboundMessage::Result { done, .. }) => {
            assert!(done);
        }
        other => panic!("expected Result with done=true, got {other:?}"),
    }
}

#[test]
fn openai_with_usage() {
    let data = r#"{"usage":{"prompt_tokens":10,"completion_tokens":20}}"#;
    let msg = parse_openai_chat_sse_event(data);
    match msg {
        Some(OutboundMessage::Result { usage, .. }) => {
            let u = usage.unwrap();
            assert_eq!(u.input_tokens, 10);
            assert_eq!(u.output_tokens, 20);
        }
        other => panic!("expected Result with usage, got {other:?}"),
    }
}

#[test]
fn google_text_chunk() {
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"Hello from Gemini"}],"role":"model"}}],"modelVersion":"gemini-2.5-flash"}"#;
    let msg = parse_google_sse_event(data);
    match msg {
        Some(OutboundMessage::Text { content, done }) => {
            assert_eq!(content, "Hello from Gemini");
            assert!(!done);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn google_final_chunk_with_usage() {
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"!"}],"role":"model"},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":42},"modelVersion":"gemini-2.5-flash"}"#;
    let msg = parse_google_sse_event(data);
    match msg {
        Some(OutboundMessage::Result {
            content,
            done,
            usage,
        }) => {
            assert_eq!(content, "!");
            assert!(done);
            let u = usage.unwrap();
            assert_eq!(u.input_tokens, 10);
            assert_eq!(u.output_tokens, 42);
        }
        other => panic!("expected Result with usage, got {other:?}"),
    }
}

#[test]
fn google_empty_data_ignored() {
    let msg = parse_google_sse_event("");
    assert!(msg.is_none());
}

#[test]
fn openai_empty_content_ignored() {
    let data = r#"{"choices":[{"index":0,"delta":{"content":""}}]}"#;
    let msg = parse_openai_chat_sse_event(data);
    assert!(msg.is_none());
}

#[test]
fn openai_responses_text_delta() {
    let data = r#"{"type":"response.output_text.delta","delta":"hello"}"#;
    let msg = parse_openai_responses_sse_event("response.output_text.delta", data);
    match msg {
        Some(OutboundMessage::Text { content, done }) => {
            assert_eq!(content, "hello");
            assert!(!done);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn openai_responses_function_call_delta() {
    let data = r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_123","call_id":"call_123","name":"get_weather","arguments":""}}"#;
    let msg = parse_openai_responses_sse_event("response.output_item.added", data);
    match msg {
        Some(OutboundMessage::ToolCallDelta {
            index, id, name, ..
        }) => {
            assert_eq!(index, 0);
            assert_eq!(id, "call_123");
            assert_eq!(name, "get_weather");
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn openai_responses_completed_with_usage() {
    let data = r#"{"type":"response.completed","response":{"usage":{"input_tokens":10,"output_tokens":20}}}"#;
    let msg = parse_openai_responses_sse_event("response.completed", data);
    match msg {
        Some(OutboundMessage::Result { done, usage, .. }) => {
            assert!(done);
            let usage = usage.expect("usage should be present");
            assert_eq!(usage.input_tokens, 10);
            assert_eq!(usage.output_tokens, 20);
        }
        other => panic!("expected Result with usage, got {other:?}"),
    }
}

#[test]
fn history_truncation_preserves_system_prompt() {
    let mut history = vec![
        ChatMessage {
            role: ChatRole::System,
            content: "system".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "msg1".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::Assistant,
            content: "reply1".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "msg2".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::Assistant,
            content: "reply2".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "msg3".to_string(),
            content_blocks: None,
        },
    ];

    // max_turns=4: keep system (index 0) + last 3 messages
    // Before: [system, msg1, reply1, msg2, reply2, msg3] (6 items)
    // drain(1..3) removes msg1, reply1
    // After:  [system, msg2, reply2, msg3] (4 items)
    HttpApiSession::truncate_history(&mut history, 4);
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].role, ChatRole::System);
    assert_eq!(history[0].content, "system");
    assert_eq!(history[1].content, "msg2");
    assert_eq!(history[2].content, "reply2");
    assert_eq!(history[3].content, "msg3");
}

#[test]
fn history_truncation_no_op_when_under_limit() {
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

    HttpApiSession::truncate_history(&mut history, 10);
    assert_eq!(history.len(), 2);
}

#[test]
fn chat_message_from_session_message() {
    let session_msg = SessionMessage {
        role: maekon_core::models::ai_session::MessageRole::User,
        content: "test question".to_string(),
        attachments: vec![],
        tools: None,
        context: None,
        response_format: None,
    };

    let chat_msg = ChatMessage {
        role: ChatRole::User,
        content: session_msg.content.clone(),
        content_blocks: None,
    };

    assert_eq!(chat_msg.role, ChatRole::User);
    assert_eq!(chat_msg.content, "test question");

    let json = serde_json::to_string(&chat_msg).unwrap();
    assert!(json.contains("\"role\":\"user\""));
    assert!(json.contains("test question"));
}

#[test]
fn new_session_with_system_prompt_initializes_history() {
    let session = test_session(
        "provider_surface.anthropic.direct_api".to_string(),
        "claude-sonnet-5".to_string(),
        "https://api.anthropic.com/v1/messages".to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        AiProviderType::Anthropic,
        Some("You are helpful.".to_string()),
        Arc::new(AiSessionConfig::default()),
        None,
    );

    assert!(!session.session_id.is_empty());
    assert_eq!(session.provider_name(), "anthropic");
    assert_eq!(session.model, "claude-sonnet-5");

    let info = session.info();
    assert_eq!(info.transport, SessionTransport::HttpApi);
    assert_eq!(info.turn_count, 0);
}

#[test]
fn http_api_session_is_external() {
    // Cloud HTTP API transmits chat content off-device → must be guarded.
    let session = test_session(
        "provider_surface.anthropic.direct_api".to_string(),
        "claude-sonnet-5".to_string(),
        "https://api.anthropic.com/v1/messages".to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        AiProviderType::Anthropic,
        None,
        Arc::new(AiSessionConfig::default()),
        None,
    );
    assert!(session.is_external());
}

#[test]
fn new_session_without_system_prompt_has_empty_history() {
    let session = test_session(
        "provider_surface.openai.direct_api".to_string(),
        "gpt-5.4".to_string(),
        "https://api.openai.com/v1/chat/completions".to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        AiProviderType::OpenAi,
        None,
        Arc::new(AiSessionConfig::default()),
        None,
    );

    assert_eq!(session.provider_name(), "openai");
}

// ── Vision Content Block Tests ──────────────────────────────

/// Helper to create a session and build request body with content blocks.
fn build_body_with_blocks(
    provider: AiProviderType,
    surface: &str,
    endpoint: &str,
    blocks: Vec<ContentBlock>,
) -> serde_json::Value {
    let session = test_session(
        surface.to_string(),
        "test-model".to_string(),
        endpoint.to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        provider,
        Some("system prompt".to_string()),
        Arc::new(AiSessionConfig::default()),
        None,
    );

    let messages = vec![
        ChatMessage {
            role: ChatRole::System,
            content: "system prompt".to_string(),
            content_blocks: None,
        },
        ChatMessage {
            role: ChatRole::User,
            content: "Describe this image".to_string(),
            content_blocks: Some(blocks),
        },
    ];

    session
        .build_request_body(&messages, &RequestOptions::default())
        .expect("build_request_body should succeed")
}

fn sample_image_blocks() -> Vec<ContentBlock> {
    vec![
        ContentBlock::Text {
            text: "Describe this image".to_string(),
        },
        ContentBlock::Image {
            media_type: "image/jpeg".to_string(),
            data: "dGVzdA==".to_string(),
        },
    ]
}

fn sample_file_blocks() -> Vec<ContentBlock> {
    vec![
        ContentBlock::Text {
            text: "Summarize this file".to_string(),
        },
        ContentBlock::File {
            media_type: "application/pdf".to_string(),
            data: "JVBERi0xLjQK".to_string(),
            filename: Some("notes.pdf".to_string()),
        },
    ]
}

#[test]
fn anthropic_vision_content_blocks() {
    let body = build_body_with_blocks(
        AiProviderType::Anthropic,
        "provider_surface.anthropic.direct_api",
        "https://api.anthropic.com/v1/messages",
        sample_image_blocks(),
    );

    let messages = body["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1); // system is excluded
    let content = messages[0]["content"].as_array().expect("content array");
    assert_eq!(content.len(), 2);

    // Text block
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "Describe this image");

    // Image block — Anthropic format
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/jpeg");
    assert_eq!(content[1]["source"]["data"], "dGVzdA==");
}

#[test]
fn openai_vision_content_blocks() {
    let body = build_body_with_blocks(
        AiProviderType::OpenAi,
        "provider_surface.openai.direct_api",
        "https://api.openai.com/v1/responses",
        sample_image_blocks(),
    );

    assert_eq!(body["instructions"], "system prompt");

    let input = body["input"].as_array().expect("input array");
    assert_eq!(input.len(), 1);
    let user_content = input[0]["content"].as_array().expect("input content array");
    assert_eq!(user_content.len(), 2);

    // Text block
    assert_eq!(user_content[0]["type"], "input_text");
    assert_eq!(user_content[0]["text"], "Describe this image");

    // Image block — OpenAI Responses format
    assert_eq!(user_content[1]["type"], "input_image");
    let url = user_content[1]["image_url"].as_str().unwrap();
    assert!(url.starts_with("data:image/jpeg;base64,"));
    assert!(url.ends_with("dGVzdA=="));
}

#[test]
fn google_vision_content_blocks() {
    let body = build_body_with_blocks(
        AiProviderType::Google,
        "provider_surface.google.direct_api",
        "https://generativelanguage.googleapis.com/v1beta/models/test-model:generateContent",
        sample_image_blocks(),
    );

    let contents = body["contents"].as_array().expect("contents array");
    assert_eq!(contents.len(), 1); // system is excluded
    let parts = contents[0]["parts"].as_array().expect("parts array");
    assert_eq!(parts.len(), 2);

    // Text part
    assert_eq!(parts[0]["text"], "Describe this image");

    // Image part — Google format
    assert_eq!(parts[1]["inlineData"]["mimeType"], "image/jpeg");
    assert_eq!(parts[1]["inlineData"]["data"], "dGVzdA==");
}

#[test]
fn anthropic_pdf_file_content_blocks() {
    let body = build_body_with_blocks(
        AiProviderType::Anthropic,
        "provider_surface.anthropic.direct_api",
        "https://api.anthropic.com/v1/messages",
        sample_file_blocks(),
    );

    let messages = body["messages"].as_array().expect("messages array");
    let content = messages[0]["content"].as_array().expect("content array");
    assert_eq!(content[1]["type"], "document");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "application/pdf");
    assert_eq!(content[1]["source"]["data"], "JVBERi0xLjQK");
    assert_eq!(content[1]["title"], "notes.pdf");
}

#[test]
fn openai_file_content_blocks() {
    let body = build_body_with_blocks(
        AiProviderType::OpenAi,
        "provider_surface.openai.direct_api",
        "https://api.openai.com/v1/responses",
        sample_file_blocks(),
    );

    let input = body["input"].as_array().expect("input array");
    let user_content = input[0]["content"].as_array().expect("input content array");
    assert_eq!(user_content[1]["type"], "input_file");
    assert_eq!(user_content[1]["file_data"], "JVBERi0xLjQK");
    assert_eq!(user_content[1]["filename"], "notes.pdf");
}

#[test]
fn google_file_content_blocks() {
    let body = build_body_with_blocks(
        AiProviderType::Google,
        "provider_surface.google.direct_api",
        "https://generativelanguage.googleapis.com/v1beta/models/test-model:generateContent",
        sample_file_blocks(),
    );

    let contents = body["contents"].as_array().expect("contents array");
    let parts = contents[0]["parts"].as_array().expect("parts array");
    assert_eq!(parts[1]["inlineData"]["mimeType"], "application/pdf");
    assert_eq!(parts[1]["inlineData"]["data"], "JVBERi0xLjQK");
}

#[test]
fn render_message_content_omits_native_attachment_manifest_entries() {
    let message = SessionMessage {
        role: maekon_core::models::ai_session::MessageRole::User,
        content: "Summarize these attachments".to_string(),
        attachments: vec![
            Attachment::File {
                path: "/tmp/notes.pdf".to_string(),
                mime: Some("application/pdf".to_string()),
                data: Some("JVBERi0xLjQK".to_string()),
            },
            Attachment::Directory {
                path: "/tmp/workspace".to_string(),
            },
        ],
        tools: None,
        context: None,
        response_format: None,
    };

    let rendered = render_message_content(&message, &ProviderRequestShape::OpenAiResponses);
    assert!(!rendered.contains("/tmp/notes.pdf"));
    assert!(rendered.contains("/tmp/workspace"));
    assert!(rendered.contains("Attachment manifest"));
}

#[test]
fn plain_text_backward_compat() {
    // When content_blocks is None, content should be a plain string
    let session = test_session(
        "provider_surface.anthropic.direct_api".to_string(),
        "test-model".to_string(),
        "https://api.anthropic.com/v1/messages".to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        AiProviderType::Anthropic,
        None,
        Arc::new(AiSessionConfig::default()),
        None,
    );

    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: "Hello world".to_string(),
        content_blocks: None,
    }];

    let body = session
        .build_request_body(&messages, &RequestOptions::default())
        .expect("build_request_body should succeed");

    let api_messages = body["messages"].as_array().expect("messages array");
    assert_eq!(api_messages.len(), 1);

    // Content should be a plain string, not an array
    let content = &api_messages[0]["content"];
    assert!(
        content.is_string(),
        "expected string content, got {content}"
    );
    assert_eq!(content.as_str().unwrap(), "Hello world");
}

// ── Structured Output + Thinking Injection Tests ───────────

/// Helper to build a request body with custom RequestOptions.
fn build_body_with_options(
    provider: AiProviderType,
    surface: &str,
    endpoint: &str,
    options: &RequestOptions<'_>,
) -> serde_json::Value {
    let session = test_session(
        surface.to_string(),
        "test-model".to_string(),
        endpoint.to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        provider,
        None,
        Arc::new(AiSessionConfig::default()),
        None,
    );

    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: "Hello".to_string(),
        content_blocks: None,
    }];

    session
        .build_request_body(&messages, options)
        .expect("build_request_body should succeed")
}

/// Helper to build a request body with thinking config set on the session.
fn build_body_with_thinking(
    provider: AiProviderType,
    surface: &str,
    endpoint: &str,
    thinking: serde_json::Value,
) -> serde_json::Value {
    let config = AiSessionConfig {
        thinking: Some(thinking),
        ..Default::default()
    };

    let session = test_session(
        surface.to_string(),
        "test-model".to_string(),
        endpoint.to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        provider,
        None,
        Arc::new(config),
        None,
    );

    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: "Hello".to_string(),
        content_blocks: None,
    }];

    session
        .build_request_body(&messages, &RequestOptions::default())
        .expect("build_request_body should succeed")
}

#[test]
fn openai_structured_output_injects_response_format() {
    let rf = serde_json::json!({"type": "json_schema", "json_schema": {"name": "result", "schema": {"type": "object"}}});
    let options = RequestOptions {
        response_format: Some(&rf),
        tools: None,
    };
    let body = build_body_with_options(
        AiProviderType::OpenAi,
        "provider_surface.openai.direct_api",
        "https://api.openai.com/v1/responses",
        &options,
    );
    assert_eq!(body["text"]["format"]["type"], "json_schema");
    assert!(body["text"]["format"]["schema"].is_object());
}

#[test]
fn google_structured_output_sets_response_mime_and_schema() {
    let rf = serde_json::json!({"schema": {"type": "object", "properties": {"name": {"type": "string"}}}});
    let options = RequestOptions {
        response_format: Some(&rf),
        tools: None,
    };
    let body = build_body_with_options(
        AiProviderType::Google,
        "provider_surface.google.direct_api",
        "https://generativelanguage.googleapis.com/v1beta/models/test-model:generateContent",
        &options,
    );
    assert_eq!(
        body["generationConfig"]["responseMimeType"],
        "application/json"
    );
    assert_eq!(body["generationConfig"]["responseSchema"]["type"], "object");
}

#[test]
fn anthropic_ignores_response_format() {
    let rf = serde_json::json!({"type": "json_schema"});
    let options = RequestOptions {
        response_format: Some(&rf),
        tools: None,
    };
    let body = build_body_with_options(
        AiProviderType::Anthropic,
        "provider_surface.anthropic.direct_api",
        "https://api.anthropic.com/v1/messages",
        &options,
    );
    assert!(
        body.get("response_format").is_none(),
        "Anthropic body should not contain response_format"
    );
}

#[test]
fn anthropic_thinking_injected() {
    let body = build_body_with_thinking(
        AiProviderType::Anthropic,
        "provider_surface.anthropic.direct_api",
        "https://api.anthropic.com/v1/messages",
        serde_json::json!({"type": "adaptive"}),
    );
    assert_eq!(body["thinking"]["type"], "adaptive");
}

#[test]
fn openai_reasoning_injected() {
    let body = build_body_with_thinking(
        AiProviderType::OpenAi,
        "provider_surface.openai.direct_api",
        "https://api.openai.com/v1/chat/completions",
        serde_json::json!({"effort": "high"}),
    );
    assert_eq!(body["reasoning"]["effort"], "high");
}

#[test]
fn google_thinking_config_injected() {
    let body = build_body_with_thinking(
        AiProviderType::Google,
        "provider_surface.google.direct_api",
        "https://generativelanguage.googleapis.com/v1beta/models/test-model:generateContent",
        serde_json::json!({"thinking_budget": 2048}),
    );
    assert_eq!(
        body["generationConfig"]["thinking_config"]["thinking_budget"],
        2048
    );
}

// ── Thinking SSE Parsing Tests ─────────────────────────────

#[test]
fn anthropic_thinking_delta() {
    let data = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me reason..."}}"#;
    let msg = parse_anthropic_sse_event("content_block_delta", data);
    match msg {
        Some(OutboundMessage::Thinking { content, done }) => {
            assert_eq!(content, "Let me reason...");
            assert!(!done);
        }
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn anthropic_text_delta_still_works() {
    let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"The answer is 42."}}"#;
    let msg = parse_anthropic_sse_event("content_block_delta", data);
    match msg {
        Some(OutboundMessage::Text { content, done }) => {
            assert_eq!(content, "The answer is 42.");
            assert!(!done);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn google_thinking_part() {
    let data = r#"{"candidates":[{"content":{"parts":[{"thinking":"Reasoning step..."}],"role":"model"}}]}"#;
    let msg = parse_google_sse_event(data);
    match msg {
        Some(OutboundMessage::Thinking { content, done }) => {
            assert_eq!(content, "Reasoning step...");
            assert!(!done);
        }
        other => panic!("expected Thinking, got {other:?}"),
    }
}

#[test]
fn google_text_after_thinking() {
    let data = r#"{"candidates":[{"content":{"parts":[{"text":"Final answer"}],"role":"model"}}]}"#;
    let msg = parse_google_sse_event(data);
    match msg {
        Some(OutboundMessage::Text { content, done }) => {
            assert_eq!(content, "Final answer");
            assert!(!done);
        }
        other => panic!("expected Text, got {other:?}"),
    }
}

// ── Tool Calling SSE Parsing Tests ────────────────────────────

#[test]
fn anthropic_tool_use_start() {
    let data = r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_123","name":"get_weather"}}"#;
    let msg = parse_anthropic_sse_event("content_block_start", data);
    match msg {
        Some(OutboundMessage::ToolCallDelta { id, name, .. }) => {
            assert_eq!(id, "toolu_123");
            assert_eq!(name, "get_weather");
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn anthropic_input_json_delta() {
    let data = r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}"#;
    let msg = parse_anthropic_sse_event("content_block_delta", data);
    match msg {
        Some(OutboundMessage::ToolCallDelta {
            arguments_chunk, ..
        }) => {
            assert_eq!(arguments_chunk, "{\"location\":");
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn openai_tool_call_in_delta() {
    let data = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_abc","type":"function","function":{"name":"get_weather","arguments":""}}]}}]}"#;
    let msg = parse_openai_chat_sse_event(data);
    match msg {
        Some(OutboundMessage::ToolCallDelta {
            index, id, name, ..
        }) => {
            assert_eq!(index, 0);
            assert_eq!(id, "call_abc");
            assert_eq!(name, "get_weather");
        }
        other => panic!("expected ToolCallDelta, got {other:?}"),
    }
}

#[test]
fn openai_tool_call_finish() {
    let data = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#;
    let msg = parse_openai_chat_sse_event(data);
    match msg {
        Some(OutboundMessage::Result { done, .. }) => assert!(done),
        other => panic!("expected Result done=true, got {other:?}"),
    }
}

#[test]
fn google_function_call() {
    let data = r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"get_weather","args":{"location":"Tokyo"}}}],"role":"model"}}]}"#;
    let msg = parse_google_sse_event(data);
    match msg {
        Some(OutboundMessage::ToolUse { tool, input, .. }) => {
            assert_eq!(tool, "get_weather");
            assert_eq!(input.unwrap()["location"], "Tokyo");
        }
        other => panic!("expected ToolUse, got {other:?}"),
    }
}

// ── Tool Calling Request Body Tests ───────────────────────────

#[test]
fn anthropic_tools_request_body() {
    let session = test_session(
        "provider_surface.anthropic.direct_api".to_string(),
        "claude-sonnet-5".to_string(),
        "https://api.anthropic.com/v1/messages".to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        AiProviderType::Anthropic,
        None,
        Arc::new(AiSessionConfig::default()),
        None,
    );
    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: "weather?".to_string(),
        content_blocks: None,
    }];
    let tools = vec![ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get weather".to_string(),
        endpoint: String::new(),
        method: "GET".to_string(),
        input_schema: Some(
            serde_json::json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        ),
    }];
    let options = RequestOptions {
        response_format: None,
        tools: Some(&tools),
    };
    let body = session.build_request_body(&messages, &options).unwrap();
    let api_tools = body["tools"].as_array().unwrap();
    assert_eq!(api_tools[0]["name"], "get_weather");
    assert!(api_tools[0]["input_schema"].is_object());
}

#[test]
fn openai_chat_body_requests_streaming_usage() {
    // #8057 (P2-1): the Chat Completions body MUST carry
    // stream_options.include_usage so OpenAI emits the trailing usage chunk;
    // without it every chat-completions surface reported 0 tokens.
    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: "hi".to_string(),
        content_blocks: None,
    }];
    let body = build_openai_chat_request_body("gpt-5.4", 256, None, &messages, None, None);
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[test]
fn openai_tools_request_body() {
    let session = test_session(
        "provider_surface.openai.direct_api".to_string(),
        "gpt-5.4".to_string(),
        "https://api.openai.com/v1/responses".to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        AiProviderType::OpenAi,
        None,
        Arc::new(AiSessionConfig::default()),
        None,
    );
    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: "weather?".to_string(),
        content_blocks: None,
    }];
    let tools = vec![ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get weather".to_string(),
        endpoint: String::new(),
        method: "GET".to_string(),
        input_schema: Some(
            serde_json::json!({"type": "object", "properties": {"location": {"type": "string"}}}),
        ),
    }];
    let options = RequestOptions {
        response_format: None,
        tools: Some(&tools),
    };
    let body = session.build_request_body(&messages, &options).unwrap();
    let api_tools = body["tools"].as_array().unwrap();
    assert_eq!(api_tools[0]["type"], "function");
    assert_eq!(api_tools[0]["name"], "get_weather");
}

#[test]
fn tools_without_schema_receive_default_empty_object_schema() {
    let session = test_session(
        "provider_surface.anthropic.direct_api".to_string(),
        "claude-sonnet-5".to_string(),
        "https://api.anthropic.com/v1/messages".to_string(),
        CredentialSource::ApiKey("sk-test".to_string()),
        AiProviderType::Anthropic,
        None,
        Arc::new(AiSessionConfig::default()),
        None,
    );
    let messages = vec![ChatMessage {
        role: ChatRole::User,
        content: "test".to_string(),
        content_blocks: None,
    }];
    let tools = vec![ToolDefinition {
        name: "ping".to_string(),
        description: "Ping".to_string(),
        endpoint: "http://api/ping".to_string(),
        method: "GET".to_string(),
        input_schema: None,
    }];
    let options = RequestOptions {
        response_format: None,
        tools: Some(&tools),
    };
    let body = session.build_request_body(&messages, &options).unwrap();
    let api_tools = body["tools"].as_array().expect("tools array");
    assert_eq!(api_tools[0]["name"], "ping");
    assert_eq!(api_tools[0]["input_schema"]["type"], "object");
    assert_eq!(api_tools[0]["input_schema"]["additionalProperties"], false);
}

// Regression (#6115): an SSE ToolCallDelta carries a `u32` index read verbatim
// from the wire. A malicious/buggy/MITM endpoint sending a huge index must not
// be able to force unbounded Vec growth (OOM/DoS). The accumulator caps slot
// allocation at MAX_TOOL_CALLS and drops out-of-range deltas.
#[test]
fn tool_call_delta_out_of_range_index_does_not_grow_vec() {
    let mut tool_calls: Vec<PartialToolCall> = Vec::new();

    // Hostile index far beyond the cap must be dropped without panic.
    accumulate_tool_call_delta(&mut tool_calls, 10_000_000, "id", "name", "{}");
    assert_eq!(
        tool_calls.len(),
        0,
        "out-of-range delta must not allocate any slots"
    );
    assert!(
        tool_calls.len() <= MAX_TOOL_CALLS,
        "tool_calls must never exceed the cap"
    );

    // The first index at the cap boundary is also rejected (MAX is exclusive).
    accumulate_tool_call_delta(&mut tool_calls, MAX_TOOL_CALLS as u32, "id", "name", "{}");
    assert_eq!(tool_calls.len(), 0, "index == cap must be rejected");

    // A valid in-range index still accumulates normally.
    accumulate_tool_call_delta(&mut tool_calls, 0, "call_0", "search", "{\"q\":1}");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_calls[0].id, "call_0");
    assert_eq!(tool_calls[0].name, "search");
    assert_eq!(tool_calls[0].arguments, "{\"q\":1}");

    // The highest valid index grows the vec to exactly the cap, no further.
    accumulate_tool_call_delta(
        &mut tool_calls,
        (MAX_TOOL_CALLS - 1) as u32,
        "last",
        "tail",
        "x",
    );
    assert_eq!(tool_calls.len(), MAX_TOOL_CALLS);
    assert!(tool_calls.len() <= MAX_TOOL_CALLS);
}

// iter-82 regression guards for iter-60 semantic HTTP status mapping
// in http_api_session::send_message. Uses the existing test_session
// helper + mockito, plus a minimal SessionMessage construction.
#[cfg(test)]
mod http_status_mapping {
    use super::*;
    use maekon_core::models::ai_session::{MessageRole, SessionMessage};
    use maekon_core::ports::conversation_session::ConversationSession;

    async fn run_http_session_status_test(status: u16) -> maekon_core::error::CoreError {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(status as usize)
            .with_body(format!("http {status}"))
            .create_async()
            .await;

        let session = test_session(
            "provider_surface.anthropic.direct_api".to_string(),
            "claude-sonnet-5".to_string(),
            server.url(),
            CredentialSource::ApiKey("test-key".to_string()),
            AiProviderType::Anthropic,
            None,
            Arc::new(AiSessionConfig::default()),
            None,
        );

        let msg = SessionMessage {
            role: MessageRole::User,
            content: "hi".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        };

        match session.send_message(&msg).await {
            Err(e) => e,
            Ok(_) => panic!("expected error from HTTP {status}"),
        }
    }

    #[tokio::test]
    async fn status_403_maps_to_auth() {
        let err = run_http_session_status_test(403).await;
        assert!(
            matches!(err, maekon_core::error::CoreError::Auth { .. }),
            "403 → Auth, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_429_maps_to_rate_limit() {
        let err = run_http_session_status_test(429).await;
        assert!(
            matches!(err, maekon_core::error::CoreError::RateLimit { .. }),
            "429 → RateLimit, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn status_503_maps_to_service_unavailable() {
        let err = run_http_session_status_test(503).await;
        assert!(
            matches!(
                err,
                maekon_core::error::CoreError::ServiceUnavailable { .. }
            ),
            "503 → ServiceUnavailable, got: {err:?}"
        );
    }

    /// Domain fallback: 500 falls back to Network.
    #[tokio::test]
    async fn status_500_falls_back_to_network() {
        let err = run_http_session_status_test(500).await;
        assert!(
            matches!(err, maekon_core::error::CoreError::Network { .. }),
            "500 should fall back to Network, got: {err:?}"
        );
    }

    // ── D7 Circuit breaker behavior ───────────────────────────────────────

    fn fast_breaker_registry(server_url: &str) -> Arc<crate::CircuitBreakerRegistry> {
        let registry = crate::CircuitBreakerRegistry::new();
        let key = crate::resilience::endpoint_authority(server_url).unwrap();
        let _ = registry.get_with_config(
            &key,
            crate::circuit_breaker::CircuitBreakerConfig {
                failure_threshold: 3,
                initial_cooldown: std::time::Duration::from_millis(50),
                max_cooldown: std::time::Duration::from_millis(200),
                half_open_probes: 1,
            },
        );
        registry
    }

    fn breaker_test_session(
        server_url: String,
        registry: Arc<crate::CircuitBreakerRegistry>,
    ) -> HttpApiSession {
        HttpApiSession::new(HttpApiSessionInit {
            surface_id: "provider_surface.anthropic.direct_api".to_string(),
            model: "claude-sonnet-5".to_string(),
            endpoint: server_url,
            credential: CredentialSource::ApiKey("test-key".to_string()),
            provider_type: AiProviderType::Anthropic,
            system_prompt: None,
            config: Arc::new(AiSessionConfig::default()),
            default_tools: None,
            breaker_registry: registry,
        })
    }

    fn test_user_message() -> SessionMessage {
        SessionMessage {
            role: MessageRole::User,
            content: "hi".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        }
    }

    #[tokio::test]
    async fn breaker_open_fast_fails_http_session() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(503)
            .with_body("down")
            .expect_at_most(3)
            .create_async()
            .await;

        let registry = fast_breaker_registry(&server.url());
        let session = breaker_test_session(server.url(), registry);
        for _ in 0..3 {
            let _ = session.send_message(&test_user_message()).await;
        }
        let result = session.send_message(&test_user_message()).await;
        // ResponseStream doesn't implement Debug, so we discriminate via Result::err().
        let err = result
            .err()
            .expect("expected CircuitOpen error after trip; got Ok stream");
        match err {
            CoreError::ServiceUnavailable { code, .. } => {
                assert_eq!(code, maekon_core::error_codes::ServiceCode::CircuitOpen);
            }
            other => panic!("expected CircuitOpen, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn breaker_half_open_failure_doubles_cooldown_http_session() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(503)
            .with_body("down")
            .create_async()
            .await;

        let registry = fast_breaker_registry(&server.url());
        let session = breaker_test_session(server.url(), registry.clone());
        for _ in 0..3 {
            let _ = session.send_message(&test_user_message()).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(70)).await;
        let _ = session.send_message(&test_user_message()).await;

        let key = crate::resilience::endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert_eq!(
            breaker.stats().current_cooldown,
            std::time::Duration::from_millis(100)
        );
    }

    // Regression (#6125): the user ChatMessage must be committed to the shared
    // history ATOMICALLY with a successful handshake. A failed send (non-2xx,
    // transport error, auth error) must NOT leave the user message in history,
    // otherwise a retry (the session is reused — error_recovery resets
    // transient errors back to Active without clearing history) pushes a SECOND
    // identical copy and egresses two duplicate user messages to the provider.
    #[tokio::test]
    async fn failed_send_does_not_mutate_history() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(500)
            .with_body("boom")
            .expect_at_least(2)
            .create_async()
            .await;

        let session = test_session(
            "provider_surface.anthropic.direct_api".to_string(),
            "claude-sonnet-5".to_string(),
            server.url(),
            CredentialSource::ApiKey("test-key".to_string()),
            AiProviderType::Anthropic,
            None,
            Arc::new(AiSessionConfig::default()),
            None,
        );

        let len_before = session.history.read().await.len();

        // First failed send must not push the user message.
        let r1 = session.send_message(&test_user_message()).await;
        // `ResponseStream` (the Ok type) is not `Debug`, so extract via `.err()`.
        let Some(e1) = r1.err() else {
            panic!("HTTP 500 should produce an Err");
        };
        // 500 is not in the 401/403/408/429/502/503/504 mapped arms, so it
        // falls back to the generic Network variant (see
        // `status_500_falls_back_to_network`).
        assert!(
            matches!(e1, CoreError::Network { .. }),
            "HTTP 500 should map to Network, got: {e1:?}"
        );
        assert_eq!(
            session.history.read().await.len(),
            len_before,
            "failed send must not leave the user message in history"
        );

        // A retry on the reused session must also not accumulate duplicates.
        let r2 = session.send_message(&test_user_message()).await;
        let Some(e2) = r2.err() else {
            panic!("retried HTTP 500 should produce an Err");
        };
        assert!(
            matches!(e2, CoreError::Network { .. }),
            "retried HTTP 500 should map to Network, got: {e2:?}"
        );
        assert_eq!(
            session.history.read().await.len(),
            len_before,
            "retried failed send must not accumulate duplicate user messages"
        );
    }

    // Regression (#6125): a successful send (2xx handshake) commits exactly one
    // user message to history — no more, no fewer.
    #[tokio::test]
    async fn successful_send_commits_exactly_one_user_message() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body("data: [DONE]\n\n")
            .create_async()
            .await;

        let session = test_session(
            "provider_surface.anthropic.direct_api".to_string(),
            "claude-sonnet-5".to_string(),
            server.url(),
            CredentialSource::ApiKey("test-key".to_string()),
            AiProviderType::Anthropic,
            None,
            Arc::new(AiSessionConfig::default()),
            None,
        );

        let len_before = session.history.read().await.len();

        let stream = session
            .send_message(&test_user_message())
            .await
            .expect("HTTP 200 handshake should succeed");
        // Drain the stream so the future runs to completion (history mutation
        // for the user message happens before the stream is built, but draining
        // keeps the test faithful to real usage).
        let _ = stream.collect::<Vec<_>>().await;

        let history = session.history.read().await;
        assert_eq!(
            history.len(),
            len_before + 1,
            "successful send must add exactly one message"
        );
        assert_eq!(
            history.last().map(|m| m.role),
            Some(ChatRole::User),
            "the committed message must be the user message"
        );
        assert_eq!(history.last().map(|m| m.content.as_str()), Some("hi"));
    }

    /// D7 spec O2 three-tier semantics: an initial 2xx response records success
    /// on the breaker even if the stream body is malformed / empty. The breaker
    /// signal is "server acknowledged the request", not "downstream LLM finished".
    #[tokio::test]
    async fn breaker_initial_2xx_records_success_regardless_of_stream_shape() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body("malformed-non-sse-body")
            .create_async()
            .await;

        let registry = fast_breaker_registry(&server.url());
        let session = breaker_test_session(server.url(), registry.clone());
        // Call may fail later (stream parsing), but the initial 200 should
        // record success. Drain the result.
        let _ = session.send_message(&test_user_message()).await;

        let key = crate::resilience::endpoint_authority(&server.url()).unwrap();
        let breaker = registry.get(&key);
        assert!(
            matches!(
                breaker.check(),
                crate::circuit_breaker::CircuitState::Closed
            ),
            "initial 200 should leave breaker Closed even with unreadable stream body"
        );
    }
}

// ── SSE streaming → history regression tests (#6197, #6202, #6203) ─────────
//
// These drive `send_message` end-to-end against a mockito SSE server and inspect
// the resulting conversation history / yielded stream, exercising the
// orchestrator arms (not just the per-event parsers).
#[cfg(test)]
mod streaming_history {
    use std::time::Duration;

    use super::*;
    use maekon_core::models::ai_session::{MessageRole, SessionMessage};
    use maekon_core::ports::conversation_session::ConversationSession;

    fn test_user_message() -> SessionMessage {
        SessionMessage {
            role: MessageRole::User,
            content: "hi".to_string(),
            attachments: vec![],
            tools: None,
            context: None,
            response_format: None,
        }
    }

    fn streaming_session(
        server_url: String,
        provider: AiProviderType,
        surface: &str,
    ) -> HttpApiSession {
        HttpApiSession::new(HttpApiSessionInit {
            surface_id: surface.to_string(),
            model: "test-model".to_string(),
            endpoint: server_url,
            credential: CredentialSource::ApiKey("test-key".to_string()),
            provider_type: provider,
            system_prompt: None,
            config: Arc::new(AiSessionConfig::default()),
            default_tools: None,
            breaker_registry: crate::CircuitBreakerRegistry::new(),
        })
    }

    /// Drain the stream, returning the yielded `OutboundMessage`s (errors dropped).
    async fn collect_messages(stream: ResponseStream) -> Vec<OutboundMessage> {
        stream
            .filter_map(|item| async move { item.ok() })
            .collect()
            .await
    }

    // #6197: a full Anthropic turn ends with TWO Result events — message_delta
    // (usage-only, done=false) followed by message_stop (done=true). Only the
    // terminal one may mutate history; the bug saved on both, pushing two
    // identical assistant messages per turn.
    #[tokio::test]
    async fn anthropic_full_sequence_saves_exactly_one_assistant_message() {
        let mut server = mockito::Server::new_async().await;
        // content_block_delta (text) → message_delta (usage, done=false) →
        // message_stop (done=true). The two Result events are the crux of #6197.
        let body = concat!(
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello world\"}}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":5,\"output_tokens\":2}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let session = streaming_session(
            server.url(),
            AiProviderType::Anthropic,
            "provider_surface.anthropic.direct_api",
        );

        let stream = session
            .send_message(&test_user_message())
            .await
            .expect("2xx handshake should succeed");
        let msgs = collect_messages(stream).await;

        // The intermediate usage-only Result is still yielded to the consumer
        // (live token display): expect at least one done=false Result.
        assert!(
            msgs.iter().any(|m| matches!(
                m,
                OutboundMessage::Result {
                    done: false,
                    usage: Some(_),
                    ..
                }
            )),
            "intermediate usage-only Result must still be yielded, got {msgs:?}"
        );

        let history = session.history.read().await;
        let assistant_count = history
            .iter()
            .filter(|m| m.role == ChatRole::Assistant)
            .count();
        assert_eq!(
            assistant_count,
            1,
            "exactly one assistant message must be saved (was double-saved before #6197): {:?}",
            history
                .iter()
                .map(|m| (m.role, m.content.clone()))
                .collect::<Vec<_>>()
        );
        let assistant = history
            .iter()
            .find(|m| m.role == ChatRole::Assistant)
            .unwrap();
        assert_eq!(assistant.content, "Hello world");
    }

    // #6202: two parallel Anthropic tool_use blocks (index 0 and index 1) must
    // produce two DISTINCT tool calls. Hardcoding index 0 collapsed them into
    // one corrupted call (second name/args overwrote the first).
    #[tokio::test]
    async fn anthropic_parallel_tool_blocks_yield_two_distinct_tool_calls() {
        let mut server = mockito::Server::new_async().await;
        // Two tool_use blocks interleaved by index, each with its own arg delta.
        let body = concat!(
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_a\",\"name\":\"get_weather\"}}\n\n",
            "event: content_block_start\n",
            "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_b\",\"name\":\"get_time\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"city\\\":\\\"Paris\\\"}\"}}\n\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"tz\\\":\\\"UTC\\\"}\"}}\n\n",
            "event: message_stop\n",
            "data: {\"type\":\"message_stop\"}\n\n",
        );
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let session = streaming_session(
            server.url(),
            AiProviderType::Anthropic,
            "provider_surface.anthropic.direct_api",
        );

        let stream = session
            .send_message(&test_user_message())
            .await
            .expect("2xx handshake should succeed");
        let msgs = collect_messages(stream).await;

        let tool_uses: Vec<(String, serde_json::Value)> = msgs
            .iter()
            .filter_map(|m| match m {
                OutboundMessage::ToolUse { tool, input, .. } => {
                    Some((tool.clone(), input.clone().unwrap_or(serde_json::json!({}))))
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            tool_uses.len(),
            2,
            "two parallel tool blocks must yield two tool calls, got {tool_uses:?}"
        );
        // Order follows the slot index. Each call must keep its own name + args
        // (before #6202 both collapsed into slot 0 and the second clobbered it).
        assert_eq!(tool_uses[0].0, "get_weather");
        assert_eq!(tool_uses[0].1["city"], "Paris");
        assert_eq!(tool_uses[1].0, "get_time");
        assert_eq!(tool_uses[1].1["tz"], "UTC");
    }

    // #6203: Gemini may deliver the final text part in the SAME chunk as
    // usageMetadata/finishReason. The parser emits that as
    // Result{content:<final text>, done:true}; the orchestrator previously saved
    // only `accumulated` (built from Text events) and dropped the final text.
    // The fix folds the terminal Result content into `accumulated` before save.
    #[tokio::test]
    async fn google_final_chunk_with_usage_preserves_final_text_in_history() {
        let mut server = mockito::Server::new_async().await;
        // First a streamed text chunk (no usage), then the final chunk carrying
        // BOTH the last text part and usageMetadata.
        let body = concat!(
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"Hello \"}],\"role\":\"model\"}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"world!\"}],\"role\":\"model\"},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":10,\"candidatesTokenCount\":4}}\n\n",
        );
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let session = streaming_session(
            server.url(),
            AiProviderType::Google,
            "provider_surface.google.direct_api",
        );
        // Google rewrites the endpoint to :streamGenerateContent?alt=sse, but
        // mockito matches any path, so the request still hits the mock.

        let stream = session
            .send_message(&test_user_message())
            .await
            .expect("2xx handshake should succeed");
        let _ = collect_messages(stream).await;

        let history = session.history.read().await;
        let assistant = history
            .iter()
            .find(|m| m.role == ChatRole::Assistant)
            .expect("an assistant message must be saved");
        assert_eq!(
            assistant.content, "Hello world!",
            "final text from the usage-bearing chunk must be preserved (was dropped before #6203)"
        );
        // And still exactly one assistant message (no double-save).
        assert_eq!(
            history
                .iter()
                .filter(|m| m.role == ChatRole::Assistant)
                .count(),
            1
        );
    }

    // Accumulate-cap: a hostile/buggy stream can emit an unbounded number of
    // individually in-cap text deltas. The aggregate is bounded by
    // MAX_TURN_RESPONSE_BYTES; once exceeded the turn must terminate with an
    // error (and the over-cap partial must NOT be persisted to history) instead
    // of growing the heap without limit.
    #[tokio::test]
    async fn oversized_turn_terminates_with_error_and_does_not_persist() {
        // Each event carries a 64 KiB text chunk — well under the 1 MiB
        // per-event cap, so individually all are accepted. Enough events to push
        // the running total past MAX_TURN_RESPONSE_BYTES (8 MiB), plus margin.
        let chunk_bytes = 64 * 1024;
        let chunk = "a".repeat(chunk_bytes);
        let events_needed = MAX_TURN_RESPONSE_BYTES / chunk_bytes + 4;

        let mut body = String::new();
        for _ in 0..events_needed {
            body.push_str("event: content_block_delta\n");
            body.push_str(&format!(
                "data: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{chunk}\"}}}}\n\n"
            ));
        }
        // A terminal stop event the turn would normally save on — it must never
        // be reached because the byte cap trips first.
        body.push_str("event: message_stop\n");
        body.push_str("data: {\"type\":\"message_stop\"}\n\n");

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let session = streaming_session(
            server.url(),
            AiProviderType::Anthropic,
            "provider_surface.anthropic.direct_api",
        );

        let stream = session
            .send_message(&test_user_message())
            .await
            .expect("2xx handshake should succeed");

        // Drain, keeping the terminal error (collect_messages drops errors).
        let mut saw_cap_error = false;
        let mut s = stream;
        while let Some(item) = s.next().await {
            if let Err(err) = item {
                let msg = err.to_string();
                assert!(
                    msg.contains("response cap"),
                    "stream must terminate with the byte-cap error, got: {msg}"
                );
                saw_cap_error = true;
                break;
            }
        }
        assert!(
            saw_cap_error,
            "an oversized turn must terminate the stream with an error"
        );

        // The over-cap turn must NOT have been persisted to history — only the
        // user message (and optional system prompt) may remain.
        let history = session.history.read().await;
        assert!(
            history.iter().all(|m| m.role != ChatRole::Assistant),
            "an over-cap turn must not be saved as an assistant message: {:?}",
            history.iter().map(|m| m.role).collect::<Vec<_>>()
        );
    }

    /// #7574 regression: concurrent `send_message` calls on the same
    /// `HttpApiSession` must serialize — the second turn blocks while the
    /// first turn's stream is still alive, and proceeds once that stream is
    /// dropped. Before the turn-guard fix, both calls would race directly on
    /// `history` (interleaved read-snapshot/push of the shared
    /// `Vec<ChatMessage>`), so this test fails before the fix (the second
    /// call returns almost immediately instead of timing out).
    #[tokio::test]
    async fn http_api_send_message_serializes_until_stream_drops() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", mockito::Matcher::Any)
            .with_status(200)
            .with_body("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n")
            .create_async()
            .await;

        let session = Arc::new(streaming_session(
            server.url(),
            AiProviderType::Anthropic,
            "provider_surface.anthropic.direct_api",
        ));

        let first_stream = session
            .send_message(&test_user_message())
            .await
            .expect("first turn should start");

        let mut second = {
            let session = session.clone();
            tokio::spawn(async move { session.send_message(&test_user_message()).await })
        };

        // `ResponseStream` (the Ok type) is not `Debug`, so `.expect_err()` cannot be used
        // here (it would need to format the Ok value); extract the concrete `Elapsed` via
        // `.err()` instead — this still asserts the timeout actually fired, not merely a
        // boolean `is_err()`.
        tokio::time::timeout(Duration::from_millis(100), &mut second)
            .await
            .err()
            .expect("second turn must wait while the first turn stream is still alive");

        drop(first_stream);

        let second_stream = tokio::time::timeout(Duration::from_millis(500), second)
            .await
            .expect("second turn should start once the first stream drops")
            .expect("second task should not panic")
            .expect("second send_message should succeed");
        drop(second_stream);
    }
}
