use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FocusMetricsDto {
    pub date: String,
    pub total_active_secs: u64,
    pub deep_work_secs: u64,
    pub communication_secs: u64,
    pub context_switches: u32,
    pub interruption_count: u32,
    pub avg_focus_duration_secs: u64,
    pub max_focus_duration_secs: u64,
    pub focus_score: f32,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FocusMetricsResponse {
    pub today: FocusMetricsDto,
    pub history: Vec<FocusMetricsDto>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct WorkSessionDto {
    pub id: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub primary_app: String,
    pub category: String,
    pub state: String,
    pub interruption_count: u32,
    pub deep_work_secs: u64,
    pub duration_secs: u64,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct InterruptionDto {
    pub id: i64,
    pub interrupted_at: String,
    pub from_app: String,
    pub from_category: String,
    pub to_app: String,
    pub to_category: String,
    pub resumed_at: Option<String>,
    pub resumed_to_app: Option<String>,
    pub duration_secs: Option<u64>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct LocalSuggestionDto {
    pub id: i64,
    pub suggestion_type: String,
    pub payload: serde_json::Value,
    pub created_at: String,
    pub shown_at: Option<String>,
    pub dismissed_at: Option<String>,
    pub acted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestionFeedbackRequest {
    pub action: String,
}
