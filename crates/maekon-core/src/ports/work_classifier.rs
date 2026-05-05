//! Pure-logic port for classifying user activity into a work type category.

use crate::models::tiered_memory::WorkType;

/// Classify current user activity into a WorkType.
/// Pure logic — no I/O, no async needed.
///
/// # Errors
/// **Infallible.** `classify` returns `WorkType` directly, not
/// `Result<_, _>`. Unclassifiable input falls back to
/// `WorkType::Unknown` rather than surfacing an error — this is a pure
/// classification function with no external dependencies.
pub trait WorkTypeClassifier: Send + Sync {
    fn classify(
        &self,
        app_name: &str,
        window_title: &str,
        focused_role: Option<&str>,
        ocr_text_sample: Option<&str>,
    ) -> WorkType;
}
