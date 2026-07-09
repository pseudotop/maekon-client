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

    /// Parse the SQL string representation produced by [`Self::as_sql_str`].
    /// Returns `None` for an unrecognized string so a corrupt persisted row is
    /// skipped rather than mis-mapped (#7913 T2.1c).
    pub fn from_sql_str(s: &str) -> Option<Self> {
        match s {
            Self::LLM_SERVER_STR => Some(SuggestionSource::LlmServer),
            Self::RULE_BASED_STR => Some(SuggestionSource::RuleBased),
            Self::LLM_LOCAL_STR => Some(SuggestionSource::LlmLocal),
            _ => None,
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
    /// proto: SUGGESTION_TYPE_TAKE_BREAK (8) — mirrors the historical `local_suggestions`
    /// row `suggestion_type = 'TakeBreak'` (deprecated `LocalSuggestion` enum writer
    /// removed 2026-07, #7733; rows still readable via `LocalSuggestionRecord`)
    TakeBreak,
    /// proto: SUGGESTION_TYPE_NEED_FOCUS_TIME (9) — mirrors the historical `local_suggestions`
    /// row `suggestion_type = 'NeedFocusTime'` (see `TakeBreak` doc above)
    NeedFocusTime,
    /// proto: SUGGESTION_TYPE_RESTORE_CONTEXT (10) — mirrors the historical `local_suggestions`
    /// row `suggestion_type = 'RestoreContext'` (see `TakeBreak` doc above)
    RestoreContext,
}

impl SuggestionType {
    /// Stable SCREAMING_SNAKE_CASE string for SQL persistence (#7913 T2.1c). Kept
    /// in sync with the `#[serde(rename_all = "SCREAMING_SNAKE_CASE")]` wire form
    /// via the `sql_str_matches_serde` round-trip test.
    pub fn as_sql_str(&self) -> &'static str {
        match self {
            SuggestionType::WorkGuidance => "WORK_GUIDANCE",
            SuggestionType::EmailDraft => "EMAIL_DRAFT",
            SuggestionType::ProductivityTip => "PRODUCTIVITY_TIP",
            SuggestionType::WorkflowOptimization => "WORKFLOW_OPTIMIZATION",
            SuggestionType::ContextBased => "CONTEXT_BASED",
            SuggestionType::BreakReminder => "BREAK_REMINDER",
            SuggestionType::FocusMode => "FOCUS_MODE",
            SuggestionType::TakeBreak => "TAKE_BREAK",
            SuggestionType::NeedFocusTime => "NEED_FOCUS_TIME",
            SuggestionType::RestoreContext => "RESTORE_CONTEXT",
        }
    }

    /// Parse the SQL string produced by [`Self::as_sql_str`]. Returns `None` for
    /// an unrecognized string so a corrupt persisted row is skipped rather than
    /// mis-mapped (#7913 T2.1c).
    pub fn from_sql_str(s: &str) -> Option<Self> {
        match s {
            "WORK_GUIDANCE" => Some(SuggestionType::WorkGuidance),
            "EMAIL_DRAFT" => Some(SuggestionType::EmailDraft),
            "PRODUCTIVITY_TIP" => Some(SuggestionType::ProductivityTip),
            "WORKFLOW_OPTIMIZATION" => Some(SuggestionType::WorkflowOptimization),
            "CONTEXT_BASED" => Some(SuggestionType::ContextBased),
            "BREAK_REMINDER" => Some(SuggestionType::BreakReminder),
            "FOCUS_MODE" => Some(SuggestionType::FocusMode),
            "TAKE_BREAK" => Some(SuggestionType::TakeBreak),
            "NEED_FOCUS_TIME" => Some(SuggestionType::NeedFocusTime),
            "RESTORE_CONTEXT" => Some(SuggestionType::RestoreContext),
            _ => None,
        }
    }
}

/// Persisted per-`(suggestion_type, source)` feedback tally — the restart-
/// surviving form of `FeedbackScorer`'s in-RAM `FeedbackTally` (#7913 T2.1c).
///
/// `last_updated` is persisted so the scorer's 12-hour self-decay is
/// WALL-CLOCK-anchored across restarts: on load the tally keeps its original
/// timestamp, so a tally that has aged past the decay window while the process
/// was down reads as decayed (rather than resetting the clock on every launch).
///
/// This store is keyed by `(suggestion_type, source)` — a reaction to a
/// SUGGESTION card. It is deliberately kept separate from the profile/trigger-
/// keyed coaching effectiveness store (#7600): the two keyings must never join.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackTallyRecord {
    pub suggestion_type: SuggestionType,
    pub source: SuggestionSource,
    pub accepted: u32,
    pub rejected: u32,
    pub deferred: u32,
    pub last_updated: DateTime<Utc>,
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
    /// The regime the user was in when they reacted to the suggestion (#7600).
    /// Populated by the emission site (`FeedbackSender::accept`/`reject`/`defer`)
    /// from the live regime snapshot. `None` when no regime was classified yet,
    /// or for callers that predate this field (retry-queue replays, older
    /// persisted records) — kept `Option` for backward compatibility so old
    /// serialized payloads (and any producer that never learns about regimes)
    /// still deserialize cleanly.
    #[serde(default)]
    pub regime_id: Option<String>,
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
    use super::{SuggestionSource, SuggestionType};

    const ALL_TYPES: [SuggestionType; 10] = [
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

    /// F-RC-11 regression guard: SuggestionType must have exactly 10 variants
    /// matching proto oneshim.v1.user_context.SuggestionType values 1-10.
    #[test]
    fn suggestion_type_has_ten_variants() {
        assert_eq!(ALL_TYPES.len(), 10, "SuggestionType must have 10 variants");
    }

    /// #7913 T2.1c — every SuggestionType round-trips through as_sql_str /
    /// from_sql_str, and the SQL string equals the serde SCREAMING_SNAKE_CASE
    /// wire form (so persistence and the wire never drift).
    #[test]
    fn suggestion_type_sql_str_roundtrips_and_matches_serde() {
        for t in ALL_TYPES {
            let s = t.as_sql_str();
            assert_eq!(
                SuggestionType::from_sql_str(s),
                Some(t.clone()),
                "round-trip failed for {s}"
            );
            let serde = serde_json::to_string(&t).unwrap();
            assert_eq!(
                serde,
                format!("\"{s}\""),
                "as_sql_str must equal the serde wire form"
            );
        }
        assert_eq!(SuggestionType::from_sql_str("NOPE"), None);
    }

    /// #7913 T2.1c — SuggestionSource round-trips through as_sql_str /
    /// from_sql_str for all variants.
    #[test]
    fn suggestion_source_sql_str_roundtrips() {
        for src in [
            SuggestionSource::RuleBased,
            SuggestionSource::LlmLocal,
            SuggestionSource::LlmServer,
        ] {
            assert_eq!(
                SuggestionSource::from_sql_str(src.as_sql_str()),
                Some(src.clone())
            );
        }
        assert_eq!(SuggestionSource::from_sql_str("NOPE"), None);
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
