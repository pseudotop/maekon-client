//! AI conversation session models — JSONL protocol types, session metadata,
//! and context assembly data structures.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ── Session Metadata ──────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionTransport {
    Subprocess,
    HttpApi,
    LocalLlm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Starting,
    Active,
    Idle,
    Recovering,
    Failed,
    Terminated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSessionInfo {
    pub session_id: String,
    pub provider_name: String,
    pub model: String,
    pub state: SessionState,
    pub transport: SessionTransport,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub turn_count: u32,
    /// User-assigned display title (None = use model/provider fallback).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub transport: SessionTransport,
    pub surface_id: Option<String>,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub tools_enabled: bool,
}

// ── JSONL Inbound Messages ────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InboundMessage {
    Message(SessionMessage),
    Control { action: ControlAction },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: MessageRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<Attachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<MessageContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlAction {
    Cancel,
    Ping,
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_app: Option<String>,
}

// ── Attachments ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Attachment {
    Image {
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<String>,
    },
    Directory {
        path: String,
    },
    Skill {
        skill_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_name: Option<String>,
    },
    AppReference {
        app_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        window_title: Option<String>,
    },
}

// ── Content Blocks ───────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Image {
        media_type: String,
        data: String,
    },
    File {
        media_type: String,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
    },
    Thinking {
        thinking: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub endpoint: String,
    #[serde(default = "default_http_method")]
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
}

fn default_http_method() -> String {
    "GET".to_string()
}

// ── JSONL Outbound Messages ───────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutboundMessage {
    Text {
        content: String,
        done: bool,
    },
    ToolUse {
        tool: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input: Option<serde_json::Value>,
        status: ToolUseStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<String>,
    },
    Result {
        content: String,
        done: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<TokenUsage>,
    },
    Error {
        code: String,
        message: String,
        retryable: bool,
    },
    Control {
        action: ControlAction,
    },
    Thinking {
        content: String,
        done: bool,
    },
    ToolCallDelta {
        index: u32,
        id: String,
        name: String,
        arguments_chunk: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolUseStatus {
    Started,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// ── Context Assembly ──────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemPromptContext {
    pub user_profile: UserProfileSummary,
    pub current_regime: String,
    pub recent_activity: ActivitySummary,
    pub suggestion_history: SuggestionPatterns,
    pub available_skills: Vec<SkillInfo>,
    pub system_info: SystemInfo,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserProfileSummary {
    pub preferred_language: Option<String>,
    pub work_style: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActivitySummary {
    pub top_apps: Vec<String>,
    pub active_minutes: u32,
    pub idle_minutes: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SuggestionPatterns {
    pub total_received: u32,
    pub accepted_count: u32,
    pub rejected_count: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SystemInfo {
    pub os: String,
    pub active_app: Option<String>,
    pub timezone: String,
}

// ── Session Audit Entry ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAuditEntry {
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub category: SessionAuditCategory,
    pub event_type: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionAuditCategory {
    Session,
    Message,
    ToolUse,
    Attachment,
    Error,
    Process,
    Usage,
    Context,
    PullApi,
}

// ── HTTP API conversation history ─────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_blocks: Option<Vec<ContentBlock>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// Truncate conversation history while preserving the system prompt (first message).
/// Keeps at most `max_turns` messages total. If the first message has role `System`,
/// it is always preserved.
pub fn truncate_chat_history(history: &mut Vec<ChatMessage>, max_turns: u32) {
    let max = max_turns as usize;
    if history.len() > max && max > 0 {
        let drain_end = history.len() - max + 1;
        history.drain(1..drain_end);
    }
}

// ── Session Persistence ─────────────────────────────────────

/// Persisted session metadata (SQLite ↔ Rust bridge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub provider_name: String,
    pub model: String,
    pub transport: SessionTransport,
    pub state: SessionState,
    pub system_prompt: Option<String>,
    pub turn_count: u32,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub terminated_at: Option<DateTime<Utc>>,
    /// User-assigned display title.
    #[serde(default)]
    pub title: Option<String>,
}

/// Persisted conversation message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    pub id: Option<i64>,
    pub session_id: String,
    pub role: String,
    pub content: String,
    pub thinking: Option<String>,
    pub tool_use: Option<String>,
    pub usage_input: Option<u64>,
    pub usage_output: Option<u64>,
    pub created_at: DateTime<Utc>,
    pub seq: i64,
}

impl From<&SessionRecord> for ConversationSessionInfo {
    fn from(r: &SessionRecord) -> Self {
        Self {
            session_id: r.session_id.clone(),
            provider_name: r.provider_name.clone(),
            model: r.model.clone(),
            state: r.state,
            transport: r.transport,
            created_at: r.created_at,
            last_active: r.last_active,
            turn_count: r.turn_count,
            title: r.title.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_inbound_message() {
        let msg = InboundMessage::Message(SessionMessage {
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
