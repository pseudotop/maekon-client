use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SuggestionSource {
    #[default]
    RuleBased,
    LlmLocal,
    LlmServer,
}

impl SuggestionSource {
    /// SQL string representation for LlmServer source.
    pub const LLM_SERVER_STR: &'static str = "LLM_SERVER";
    /// SQL string representation for RuleBased source.
    pub const RULE_BASED_STR: &'static str = "RULE_BASED";
    /// SQL string representation for LlmLocal source.
    pub const LLM_LOCAL_STR: &'static str = "LLM_LOCAL";

    /// Convert to the SQL string representation.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            SuggestionSource::LlmServer => Self::LLM_SERVER_STR,
            SuggestionSource::RuleBased => Self::RULE_BASED_STR,
            SuggestionSource::LlmLocal => Self::LLM_LOCAL_STR,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub suggestion_id: String,
    pub suggestion_type: SuggestionType,
    pub content: String,
    pub priority: Priority,
    pub confidence_score: f64,
    pub relevance_score: f64,
    pub is_actionable: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub source: SuggestionSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SuggestionType {
    WorkGuidance,
    EmailDraft,
    ProductivityTip,
    WorkflowOptimization,
    ContextBased,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestionFeedback {
    pub suggestion_id: String,
    pub feedback_type: FeedbackType,
    pub timestamp: DateTime<Utc>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FeedbackType {
    Accepted,
    Rejected,
    Deferred,
}

/// Suggestion with feedback data, used for few-shot prompt construction.
/// Distinct from `RelevantHistoryEntry` (RAG-based activity history in oneshim-analysis).
#[derive(Debug, Clone)]
pub struct SuggestionHistoryEntry {
    pub suggestion_id: String,
    pub suggestion_type: String,
    pub content: String,
    pub confidence: f64,
    pub feedback_type: String,
    pub regime_label: Option<String>,
    pub context_app: String,
    pub context_window: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
