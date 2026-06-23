//! JSONL protocol types — session transport/state enums, inbound and outbound
//! messages, content blocks, attachments, and tool definitions.

use serde::{Deserialize, Serialize};

// ── Session Transport / State ─────────────────────────────────

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

pub const MAX_SESSION_INPUT_BYTES: usize = 256 * 1024; // 256 KiB
pub const MAX_SESSION_ATTACHMENTS: usize = 16;
pub const SESSION_INPUT_TOO_LARGE_CODE: &str = "input.too_large";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct SessionInputLimitError {
    pub code: &'static str,
    pub message: String,
}

fn attachment_wire_bytes(attachment: &Attachment) -> usize {
    match attachment {
        Attachment::Image { mime, path, data } => {
            mime.len() + path.as_ref().map_or(0, String::len) + data.as_ref().map_or(0, String::len)
        }
        Attachment::File { path, mime, data } => {
            path.len() + mime.as_ref().map_or(0, String::len) + data.as_ref().map_or(0, String::len)
        }
        Attachment::Directory { path } => path.len(),
        Attachment::Skill {
            skill_id,
            display_name,
        } => skill_id.len() + display_name.as_ref().map_or(0, String::len),
        Attachment::AppReference {
            app_name,
            window_title,
        } => app_name.len() + window_title.as_ref().map_or(0, String::len),
    }
}

pub fn validate_session_input_size(
    label: &str,
    message: &str,
    attachments: &[Attachment],
) -> Result<(), SessionInputLimitError> {
    if attachments.len() > MAX_SESSION_ATTACHMENTS {
        return Err(SessionInputLimitError {
            code: SESSION_INPUT_TOO_LARGE_CODE,
            message: format!(
                "{label} has too many attachments ({} > {})",
                attachments.len(),
                MAX_SESSION_ATTACHMENTS
            ),
        });
    }

    let attachment_bytes = attachments.iter().map(attachment_wire_bytes).sum::<usize>();
    let total_bytes = message.len().saturating_add(attachment_bytes);
    if total_bytes > MAX_SESSION_INPUT_BYTES {
        return Err(SessionInputLimitError {
            code: SESSION_INPUT_TOO_LARGE_CODE,
            message: format!(
                "{label} exceeds maximum allowed size ({} bytes > {} bytes)",
                total_bytes, MAX_SESSION_INPUT_BYTES
            ),
        });
    }

    Ok(())
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
