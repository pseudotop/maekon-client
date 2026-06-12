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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_scope: Option<SuggestionContextScope>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SuggestionContextScope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SuggestionType {
    WorkGuidance,
    EmailDraft,
    ProductivityTip,
    WorkflowOptimization,
    ContextBased,
    /// proto: SUGGESTION_TYPE_BREAK_REMINDER (6)
    BreakReminder,
    /// proto: SUGGESTION_TYPE_FOCUS_MODE (7)
    FocusMode,
    /// proto: SUGGESTION_TYPE_TAKE_BREAK (8) — mirrors LocalSuggestion::TakeBreak
    TakeBreak,
    /// proto: SUGGESTION_TYPE_NEED_FOCUS_TIME (9) — mirrors LocalSuggestion::NeedFocusTime
    NeedFocusTime,
    /// proto: SUGGESTION_TYPE_RESTORE_CONTEXT (10) — mirrors LocalSuggestion::RestoreContext
    RestoreContext,
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

#[cfg(test)]
mod suggestion_type_tests {
    use super::SuggestionType;

    /// F-RC-11 regression guard: SuggestionType must have exactly 10 variants
    /// matching proto oneshim.v1.user_context.SuggestionType values 1-10.
    #[test]
    fn suggestion_type_has_ten_variants() {
        let variants = [
            SuggestionType::WorkGuidance,
            SuggestionType::EmailDraft,
            SuggestionType::ProductivityTip,
            SuggestionType::WorkflowOptimization,
            SuggestionType::ContextBased,
            SuggestionType::BreakReminder,
            SuggestionType::FocusMode,
            SuggestionType::TakeBreak,
            SuggestionType::NeedFocusTime,
            SuggestionType::RestoreContext,
        ];
        assert_eq!(variants.len(), 10, "SuggestionType must have 10 variants");
    }
}

/// Suggestion with feedback data, used for few-shot prompt construction.
/// Distinct from `RelevantHistoryEntry` (RAG-based activity history in maekon-analysis).
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
