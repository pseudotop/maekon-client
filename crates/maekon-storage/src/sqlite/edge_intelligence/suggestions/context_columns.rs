//! Helper to extract context scope columns from a `Suggestion`.

/// Extract the three context-scope column values from a `Suggestion`,
/// filtering out empty/whitespace-only values.
pub(super) fn suggestion_context_columns(
    suggestion: &maekon_core::models::suggestion::Suggestion,
) -> (Option<&str>, Option<&str>, Option<&str>) {
    let scope = suggestion.context_scope.as_ref();
    (
        scope
            .and_then(|scope| scope.app_name.as_deref())
            .filter(|value| !value.trim().is_empty()),
        scope
            .and_then(|scope| scope.window_title.as_deref())
            .filter(|value| !value.trim().is_empty()),
        scope
            .and_then(|scope| scope.target_id.as_deref())
            .filter(|value| !value.trim().is_empty()),
    )
}
