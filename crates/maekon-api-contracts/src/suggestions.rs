use serde::{Deserialize, Serialize};

/// DTO for a suggestion from the unified V8 `suggestions` table.
///
/// # Wire-format constraint (F-RC-02 / F-RC-06)
/// `suggestion_type`, `source`, and `priority` are stored as plain `String` rather than
/// typed enums because the V8 schema pre-dates the enum definitions and SQLite rows may
/// contain provider-specific values not yet in any enum variant.  Callers should treat
/// these fields as open-coded strings and validate at the application boundary.
/// Migration to typed enums requires a schema migration — tracked in issue #3399.
// F-RC-06: keeping `String` is a deliberate decision — requires a prior schema migration (#3399)
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestionDto {
    pub id: i64,
    pub suggestion_id: String,
    pub suggestion_type: String,
    pub source: String,
    pub content: String,
    pub priority: String,
    pub confidence_score: f64,
    pub relevance_score: f64,
    pub is_actionable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dismissed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acted_at: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SuggestionFeedbackRequest {
    pub action: String,
}
