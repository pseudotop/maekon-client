use serde::Serialize;

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdlePeriodResponse {
    pub start_time: String,
    pub end_time: Option<String>,
    pub duration_secs: Option<u64>,
}
