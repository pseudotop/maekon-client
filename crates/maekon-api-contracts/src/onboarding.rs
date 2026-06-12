use maekon_core::models::intent::WorkflowPreset;
use serde::Serialize;

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct QuickstartStepDto {
    pub order: u8,
    pub title: String,
    pub action: String,
    pub expected_outcome: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct OnboardingQuickstartDto {
    pub schema_version: String,
    pub generated_at: String,
    pub target_mode: String,
    pub dashboard_url: String,
    pub checklist: Vec<QuickstartStepDto>,
    // Cross-crate `maekon-core` type — contained as an opaque object array.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object_array")
    )]
    pub recommended_presets: Vec<WorkflowPreset>,
    pub verification_commands: Vec<String>,
}
