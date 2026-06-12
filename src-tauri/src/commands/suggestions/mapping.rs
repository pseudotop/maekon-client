use maekon_core::models::storage_records::SuggestionRecord;
use maekon_core::models::suggestion::SuggestionType;

use super::types::{SuggestionContextScopeDto, SuggestionViewDto};

pub(super) fn source_label(
    source: &maekon_core::models::suggestion::SuggestionSource,
) -> &'static str {
    use maekon_core::models::suggestion::SuggestionSource;
    match source {
        SuggestionSource::LlmLocal => "local",
        SuggestionSource::LlmServer => "server",
        SuggestionSource::RuleBased => "rule",
    }
}

fn storage_source_label(source: &str) -> String {
    match source {
        "llm_local" | "LLM_LOCAL" | "LlmLocal" => "local".to_string(),
        "llm_server" | "LLM_SERVER" | "LlmServer" => "server".to_string(),
        "rule_engine" | "rule_based" | "RULE_BASED" | "RuleBased" => "rule".to_string(),
        _ => source.to_string(),
    }
}

/// Parse a raw suggestion_type string (from SQLite) into a typed `SuggestionType`.
///
/// Recognises both SCREAMING_SNAKE_CASE (serde rename_all) and PascalCase
/// (enum_to_sql_str via serde_json) representations.  Used by the stats/history
/// fallback helpers so that the derived type-key matches the manager path (#5699).
pub(super) fn parse_suggestion_type(s: &str) -> Option<SuggestionType> {
    match s {
        "WORK_GUIDANCE" | "WorkGuidance" => Some(SuggestionType::WorkGuidance),
        "EMAIL_DRAFT" | "EmailDraft" => Some(SuggestionType::EmailDraft),
        "PRODUCTIVITY_TIP" | "ProductivityTip" => Some(SuggestionType::ProductivityTip),
        "WORKFLOW_OPTIMIZATION" | "WorkflowOptimization" => {
            Some(SuggestionType::WorkflowOptimization)
        }
        "CONTEXT_BASED" | "ContextBased" => Some(SuggestionType::ContextBased),
        "BREAK_REMINDER" | "BreakReminder" => Some(SuggestionType::BreakReminder),
        "FOCUS_MODE" | "FocusMode" => Some(SuggestionType::FocusMode),
        "TAKE_BREAK" | "TakeBreak" => Some(SuggestionType::TakeBreak),
        "NEED_FOCUS_TIME" | "NeedFocusTime" => Some(SuggestionType::NeedFocusTime),
        "RESTORE_CONTEXT" | "RestoreContext" => Some(SuggestionType::RestoreContext),
        _ => None,
    }
}

/// Derive the stats map key for a raw suggestion_type string.
///
/// On a successful parse: `format!("{:?}", st).to_lowercase()` — guarantees
/// the SAME key as the manager path (`queries.rs` fold uses `{:?}` on the enum,
/// e.g. `WorkGuidance` → `"workguidance"`).  On failure: raw lowercase fallback.
/// (#5699 storage fallback)
pub(super) fn storage_type_key(suggestion_type: &str) -> String {
    match parse_suggestion_type(suggestion_type) {
        Some(st) => format!("{st:?}").to_lowercase(),
        None => suggestion_type.to_lowercase(),
    }
}

/// Derive a feedback label string from a `SuggestionRecord`.
///
/// `SuggestionRecord` has no `state` column — the lifecycle columns are used:
/// - `acted_at IS NOT NULL`  → `"accepted"`
/// - `dismissed_at IS NOT NULL` → `"rejected"`
/// - `resurface_at IS NOT NULL` → `"deferred"` (proxy for deferred state)
/// - otherwise                 → `None` (pending / no feedback yet)
///
/// (#5699 storage fallback)
pub(super) fn storage_feedback_label(row: &SuggestionRecord) -> Option<String> {
    if row.acted_at.is_some() {
        Some("accepted".to_string())
    } else if row.dismissed_at.is_some() {
        Some("rejected".to_string())
    } else if row.resurface_at.is_some() {
        Some("deferred".to_string())
    } else {
        None
    }
}

/// Derive a source label string from the raw storage `source` field.
///
/// Exposed as `pub(super)` so the fallback paths in `queries.rs` can access it
/// without re-implementing the match.  Identical logic to `storage_source_label`
/// below, but with pub visibility. (#5699)
pub(super) fn storage_source_label_pub(source: &str) -> String {
    storage_source_label(source)
}

fn storage_suggestion_title(suggestion_type: &str) -> String {
    // Reuse parse_suggestion_type so the two tables stay in sync (#5699).
    parse_suggestion_type(suggestion_type)
        .map(|st| maekon_suggestion::presenter::type_to_title(&st).to_string())
        .unwrap_or_else(|| suggestion_type.to_string())
}

/// `pub(super)` wrapper so the deferred fallback in `queries.rs` can obtain a
/// human-readable title without duplicating the match table. (#5699)
pub(super) fn storage_suggestion_title_pub(suggestion_type: &str) -> String {
    storage_suggestion_title(suggestion_type)
}

pub(super) fn storage_record_to_view(row: SuggestionRecord) -> SuggestionViewDto {
    let context_scope = suggestion_context_scope_dto(
        row.context_app.clone(),
        row.context_window.clone(),
        row.context_target_id.clone(),
    );
    SuggestionViewDto {
        title: storage_suggestion_title(&row.suggestion_type),
        source: storage_source_label(&row.source),
        id: row.suggestion_id,
        body: row.content,
        priority: row.priority,
        category: None,
        confidence_score: row.confidence_score,
        created_at: row.created_at,
        is_read: false,
        reasoning: row.reasoning,
        context_scope,
    }
}

fn suggestion_context_scope_dto(
    app_name: Option<String>,
    window_title: Option<String>,
    target_id: Option<String>,
) -> Option<SuggestionContextScopeDto> {
    let dto = SuggestionContextScopeDto {
        app_name: normalized_context_value(app_name),
        window_title: normalized_context_value(window_title),
        target_id: normalized_context_value(target_id),
    };
    if dto.app_name.is_none() && dto.window_title.is_none() && dto.target_id.is_none() {
        None
    } else {
        Some(dto)
    }
}

pub(super) fn suggestion_context_scope_dto_from_domain(
    scope: Option<maekon_core::models::suggestion::SuggestionContextScope>,
) -> Option<SuggestionContextScopeDto> {
    scope.and_then(|scope| {
        suggestion_context_scope_dto(scope.app_name, scope.window_title, scope.target_id)
    })
}

fn normalized_context_value(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}
