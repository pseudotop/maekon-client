use oneshim_core::models::intent::WorkflowPreset;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct QuickstartStepDto {
    pub order: u8,
    pub title: String,
    pub action: String,
    pub expected_outcome: String,
}

#[derive(Debug, Serialize)]
pub struct OnboardingQuickstartDto {
    pub schema_version: String,
    pub generated_at: String,
    pub target_mode: String,
    pub dashboard_url: String,
    pub checklist: Vec<QuickstartStepDto>,
    pub recommended_presets: Vec<WorkflowPreset>,
    pub verification_commands: Vec<String>,
}
