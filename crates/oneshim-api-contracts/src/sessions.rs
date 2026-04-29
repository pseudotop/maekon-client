use serde::{Deserialize, Serialize};

use oneshim_core::models::ai_session::{Attachment, MessageContext, ToolDefinition};

/// Path parameter for AI conversation session endpoints.
#[derive(Debug, Deserialize)]
pub struct AiSessionPath {
    pub id: String,
}

/// Request body for sending a message to an AI conversation session.
#[derive(Debug, Deserialize)]
pub struct AiSendMessageRequest {
    pub content: String,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(default)]
    pub context: Option<MessageContext>,
    #[serde(default)]
    pub response_format: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    pub session_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub total_events: u64,
    pub total_frames: u64,
    pub total_idle_secs: u64,
    pub active_duration_secs: Option<u64>,
}
