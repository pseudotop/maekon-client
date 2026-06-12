//! Session metadata, state machine types, config, and persistence records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::ai_session::protocol::{ContentBlock, SessionState, SessionTransport};

// ── Session Metadata ──────────────────────────────────────────

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
    /// Working directory for the conversation thread (app-server `thread/start`,
    /// E21 #4866 I3). `None` → adapter default (a tempdir).
    #[serde(default)]
    pub cwd: Option<String>,
    /// Requested sandbox policy. The app-server adapter never weakens it below
    /// the exec path's `read-only` containment (#4866 R6).
    #[serde(default)]
    pub sandbox_policy: Option<String>,
    /// Requested command-approval policy for the conversation thread.
    #[serde(default)]
    pub approval_policy: Option<String>,
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
