use serde::Serialize;

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ErrorResponse {
    pub code: String,
    pub error: String,
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub execution_provenance:
        Option<maekon_core::models::intent_execution::IntentExecutionProvenance>,
}
