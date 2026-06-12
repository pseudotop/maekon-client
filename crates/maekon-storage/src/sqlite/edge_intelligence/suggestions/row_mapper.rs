//! Row-mapping helpers shared across suggestion query functions.

use super::super::super::LocalSuggestionRecord;

/// Map a `local_suggestions` row to a `LocalSuggestionRecord`.
/// Shared by `list_recent_local_suggestions`, `list_local_suggestions_after_id`,
/// and `integration_query_impl`.
pub(crate) fn map_local_suggestion_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<LocalSuggestionRecord> {
    let payload_str: String = row.get(2)?;
    let payload: serde_json::Value =
        serde_json::from_str(&payload_str).unwrap_or(serde_json::json!({}));

    Ok(LocalSuggestionRecord {
        id: row.get(0)?,
        suggestion_type: row.get(1)?,
        payload,
        created_at: row.get(3)?,
        shown_at: row.get(4)?,
        dismissed_at: row.get(5)?,
        acted_at: row.get(6)?,
    })
}
