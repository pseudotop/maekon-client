use maekon_core::config::AutomationConfig;
use maekon_core::models::intent::{builtin_presets, suggested_action_preset, WorkflowPreset};
use maekon_core::models::storage_records::SuggestionRecord;
use maekon_core::models::suggestion::{SuggestionSource, SuggestionType};

use super::types::{SuggestionActionDto, SuggestionContextScopeDto, SuggestionViewDto};

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

/// Parse a raw suggestion source string (from SQLite) into a typed
/// `SuggestionSource`. Recognises the SCREAMING_SNAKE_CASE serde form written by
/// `enum_to_sql_str` plus the legacy PascalCase / snake_case forms already
/// tolerated by `storage_source_label`, so the storage-fallback binding derives
/// the same source the manager path holds in memory (T4.1 #7917).
pub(super) fn parse_suggestion_source(s: &str) -> Option<SuggestionSource> {
    match s {
        "RULE_BASED" | "rule_based" | "rule_engine" | "RuleBased" => {
            Some(SuggestionSource::RuleBased)
        }
        "LLM_LOCAL" | "llm_local" | "LlmLocal" => Some(SuggestionSource::LlmLocal),
        "LLM_SERVER" | "llm_server" | "LlmServer" => Some(SuggestionSource::LlmServer),
        _ => None,
    }
}

/// The single shared enrichment predicate (T4.1 #7917, ADR-027). Returns the
/// live `WorkflowPreset` a bound suggestion should offer, or `None`.
///
/// A preset is offered iff ALL hold: `suggested_action_preset(type, source)`
/// maps (source arm refuses network/LLM origins) ∧ automation is enabled ∧ the
/// derived preset id resolves in the live builtins+custom list. Both DTO build
/// sites and the run command call this one function so they never diverge; the
/// run command additionally re-checks `ensure_enabled` inside `run_workflow`.
pub(super) fn derive_bound_preset(
    suggestion_type: &SuggestionType,
    source: &SuggestionSource,
    automation: &AutomationConfig,
) -> Option<WorkflowPreset> {
    let preset_id = suggested_action_preset(suggestion_type, source)?;
    if !automation.enabled {
        return None;
    }
    resolve_preset(preset_id, automation)
}

/// Resolve a preset id against the live production list — builtins first, then
/// user `custom_presets` — mirroring `AutomationQueryService::run_preset`
/// (the REST path). `PresetStorage` SQLite is dormant and deliberately not
/// consulted (recon Q3/hazard 15).
fn resolve_preset(preset_id: &str, automation: &AutomationConfig) -> Option<WorkflowPreset> {
    builtin_presets()
        .into_iter()
        .find(|p| p.id == preset_id)
        .or_else(|| {
            automation
                .custom_presets
                .iter()
                .find(|p| p.id == preset_id)
                .cloned()
        })
}

/// Build the label-only action DTO for a bound suggestion, from its typed
/// `(type, source)` (manager path). `None` when unbound (T4.1 #7917).
pub(super) fn suggestion_action_dto(
    suggestion_type: &SuggestionType,
    source: &SuggestionSource,
    automation: &AutomationConfig,
) -> Option<SuggestionActionDto> {
    derive_bound_preset(suggestion_type, source, automation)
        .map(|preset| SuggestionActionDto { label: preset.name })
}

/// Build the label-only action DTO for a bound suggestion straight from a
/// `SuggestionRecord`, parsing the persisted type/source strings first
/// (storage-fallback path). `None` when the row is unbound or its type/source
/// cannot be parsed (T4.1 #7917).
pub(super) fn suggestion_action_dto_from_storage(
    row: &SuggestionRecord,
    automation: &AutomationConfig,
) -> Option<SuggestionActionDto> {
    let suggestion_type = parse_suggestion_type(&row.suggestion_type)?;
    let source = parse_suggestion_source(&row.source)?;
    suggestion_action_dto(&suggestion_type, &source, automation)
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
        reasoning: row.reasoning,
        context_scope,
        // Unbound by default. Only the live PENDING build sites enrich `action`
        // (see `pending_suggestions_snapshot`); the shared history/stats readers
        // that reuse this mapper must stay bare — a stale suggestion must never
        // offer execution (ADR-027). (T4.1 #7917)
        action: None,
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

#[cfg(test)]
mod action_binding_tests {
    use super::*;
    use maekon_core::models::intent::PRESET_DEEP_WORK_START;
    use maekon_core::models::storage_records::SuggestionRecord;

    fn enabled() -> AutomationConfig {
        AutomationConfig {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn resolve_preset_finds_builtin_and_rejects_dangling() {
        let cfg = enabled();
        assert!(resolve_preset(PRESET_DEEP_WORK_START, &cfg).is_some());
        // Defensive branch: an id absent from builtins+custom yields None (this is
        // the "dangling preset" backstop the run command relies on).
        assert!(resolve_preset("no-such-preset-id", &cfg).is_none());
    }

    #[test]
    fn derive_bound_preset_gates_on_map_source_and_enabled() {
        // Mapped + rule-based + enabled ⇒ the live preset (label carries its name).
        let preset = derive_bound_preset(
            &SuggestionType::NeedFocusTime,
            &SuggestionSource::RuleBased,
            &enabled(),
        )
        .expect("bound preset");
        assert_eq!(preset.id, PRESET_DEEP_WORK_START);
        // Label flows from the live preset name (#7940 renamed it from "Start Deep Work").
        assert_eq!(preset.name, "Clear Distractions");

        // Automation disabled ⇒ None (default config has enabled=false).
        assert!(derive_bound_preset(
            &SuggestionType::NeedFocusTime,
            &SuggestionSource::RuleBased,
            &AutomationConfig::default(),
        )
        .is_none());

        // Network source ⇒ None even for the mapped type (frozen invariant).
        assert!(derive_bound_preset(
            &SuggestionType::NeedFocusTime,
            &SuggestionSource::LlmServer,
            &enabled(),
        )
        .is_none());

        // Unmapped type ⇒ None.
        assert!(derive_bound_preset(
            &SuggestionType::TakeBreak,
            &SuggestionSource::RuleBased,
            &enabled(),
        )
        .is_none());
    }

    #[test]
    fn parse_suggestion_source_covers_persisted_forms() {
        assert_eq!(
            parse_suggestion_source("RULE_BASED"),
            Some(SuggestionSource::RuleBased)
        );
        assert_eq!(
            parse_suggestion_source("LLM_SERVER"),
            Some(SuggestionSource::LlmServer)
        );
        assert_eq!(
            parse_suggestion_source("LlmLocal"),
            Some(SuggestionSource::LlmLocal)
        );
        assert_eq!(parse_suggestion_source("nonsense"), None);
    }

    fn record(id: &str, suggestion_type: &str, source: &str) -> SuggestionRecord {
        SuggestionRecord {
            id: 0,
            suggestion_id: id.to_string(),
            suggestion_type: suggestion_type.to_string(),
            source: source.to_string(),
            content: "Time to focus.".to_string(),
            priority: "High".to_string(),
            confidence_score: 0.9,
            relevance_score: 0.9,
            is_actionable: true,
            reasoning: None,
            created_at: "2026-07-07T00:00:00Z".to_string(),
            expires_at: None,
            context_app: None,
            context_window: None,
            context_target_id: None,
            acted_at: None,
            dismissed_at: None,
            shown_at: None,
            resurface_at: None,
        }
    }

    #[test]
    fn suggestion_action_dto_from_storage_enriches_bound_row_only() {
        // Persisted (NEED_FOCUS_TIME, RULE_BASED) + enabled ⇒ label present.
        let bound = record("s1", "NEED_FOCUS_TIME", "RULE_BASED");
        assert_eq!(
            suggestion_action_dto_from_storage(&bound, &enabled())
                .map(|dto| dto.label)
                .as_deref(),
            Some("Clear Distractions"),
        );
        // Same row, automation disabled ⇒ None.
        assert!(suggestion_action_dto_from_storage(&bound, &AutomationConfig::default()).is_none());
        // Network-sourced row of the same type ⇒ None.
        let network = record("s2", "NEED_FOCUS_TIME", "LLM_SERVER");
        assert!(suggestion_action_dto_from_storage(&network, &enabled()).is_none());
    }
}
