use serde::{Deserialize, Serialize};

use maekon_core::models::ai_session::{Attachment, MessageContext, ToolDefinition};

/// Path parameter for AI conversation session endpoints.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AiSessionPath {
    pub id: String,
}

/// Request body for sending a message to an AI conversation session.
#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AiSendMessageRequest {
    pub content: String,
    // Cross-crate `maekon-core` types — contained as opaque objects/arrays.
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object_array")
    )]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object_array")
    )]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(default)]
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub context: Option<MessageContext>,
    #[serde(default)]
    pub response_format: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SessionResponse {
    pub session_id: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub total_events: u64,
    pub total_frames: u64,
    pub total_idle_secs: u64,
    pub active_duration_secs: Option<u64>,
}
