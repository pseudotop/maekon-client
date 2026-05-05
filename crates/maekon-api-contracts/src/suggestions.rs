use serde::{Deserialize, Serialize};

/// DTO for a suggestion from the unified V8 `suggestions` table.
#[derive(Debug, Serialize, Deserialize)]
pub struct SuggestionDto {
    pub id: i64,
    pub suggestion_id: String,
    pub suggestion_type: String,
    pub source: String,
    pub content: String,
    pub priority: String,
    pub confidence_score: f64,
    pub relevance_score: f64,
    pub is_actionable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acted_at: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}
