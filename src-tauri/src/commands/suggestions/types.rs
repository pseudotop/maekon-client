use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct SuggestionViewDto {
    pub id: String,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub category: Option<String>,
    pub source: String,
    pub confidence_score: f64,
    pub created_at: String,
    pub reasoning: Option<String>,
    pub context_scope: Option<SuggestionContextScopeDto>,
    /// One-click automation affordance derived for a BOUND, pending suggestion
    /// (T4.1 #7917, ADR-027). `None` on unbound and on all history/stale views:
    /// only the live pending build sites populate it, and only when
    /// `suggested_action_preset` maps ∧ automation is enabled ∧ the preset
    /// resolves in the live builtins+custom list. The run command re-derives the
    /// preset server-side and ignores client data, so this is label-only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<SuggestionActionDto>,
}

/// Label-only automation affordance attached to a bound suggestion view
/// (T4.1 #7917). Carries no preset id: `run_suggestion_action` re-derives the
/// preset from the suggestion's own `(type, source)` and never trusts a
/// client-supplied id.
#[derive(Clone, Debug, Serialize)]
pub struct SuggestionActionDto {
    pub label: String,
}

#[derive(Clone, Serialize)]
pub struct SuggestionContextScopeDto {
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub target_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionReplayEventPayload {
    pub event_name: String,
    pub phase: String,
    pub suggestion_id: Option<String>,
    pub target_id: Option<String>,
    pub surface_placement: String,
    pub app_name: Option<String>,
    pub window_title: Option<String>,
    pub action: Option<String>,
    pub audit_ready: bool,
    pub raw_context_included: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionReplayEventAck {
    pub trace_id: String,
    pub recorded: bool,
}

#[derive(Serialize)]
pub struct SuggestionHistoryDto {
    // Flatten so the wire shape matches the FE flat type (types.ts:172 extends-flat).
    // SuggestionViewDto has no 'feedback' field so there is no key collision (#5699).
    #[serde(flatten)]
    pub suggestion: SuggestionViewDto,
    pub feedback: Option<String>,
}

#[derive(Serialize)]
pub struct TypeCountDto {
    pub suggestion_type: String,
    pub count: u32,
}

#[derive(Serialize)]
pub struct SourceStatsDto {
    pub source: String,
    pub count: u32,
    pub accepted: u32,
    pub rejected: u32,
}

#[derive(Serialize)]
pub struct SuggestionStatsDto {
    pub total_shown: u32,
    pub total_accepted: u32,
    pub total_rejected: u32,
    pub total_deferred: u32,
    pub acceptance_rate: f64,
    pub by_type: Vec<TypeCountDto>,
    pub by_source: Vec<SourceStatsDto>,
    pub latest_local_analysis: Option<crate::local_analysis_status::LocalAnalysisStatus>,
}

#[derive(Serialize)]
pub struct DailyStatDto {
    pub date: String,
    pub shown: u32,
    pub accepted: u32,
    pub rejected: u32,
    pub deferred: u32,
}
