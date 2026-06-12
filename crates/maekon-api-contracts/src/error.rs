use serde::Serialize;

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ErrorResponse {
    pub error: String,
    pub status: u16,
}
