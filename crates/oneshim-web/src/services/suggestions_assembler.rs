use oneshim_api_contracts::suggestions::SuggestionDto;
use oneshim_core::models::storage_records::SuggestionRecord;

pub(crate) fn assemble_suggestion(row: SuggestionRecord) -> SuggestionDto {
    SuggestionDto {
        id: row.id,
        suggestion_id: row.suggestion_id,
        suggestion_type: row.suggestion_type,
        source: row.source,
        content: row.content,
        priority: row.priority,
        confidence_score: row.confidence_score,
        relevance_score: row.relevance_score,
        is_actionable: row.is_actionable,
        reasoning: row.reasoning,
        shown_at: row.shown_at,
        dismissed_at: row.dismissed_at,
        acted_at: row.acted_at,
        created_at: row.created_at,
        expires_at: row.expires_at,
    }
}
