use chrono::Utc;
use oneshim_api_contracts::onboarding::{OnboardingQuickstartDto, QuickstartStepDto};
use oneshim_core::models::intent::WorkflowPreset;

const ONBOARDING_QUICKSTART_SCHEMA_VERSION: &str = "onboarding.quickstart.v1";

pub(crate) fn assemble_quickstart_step(
    order: u8,
    title: &str,
    action: &str,
    expected_outcome: &str,
) -> QuickstartStepDto {
    QuickstartStepDto {
        order,
        title: title.to_string(),
        action: action.to_string(),
        expected_outcome: expected_outcome.to_string(),
    }
}

pub(crate) fn assemble_quickstart(
    dashboard_url: String,
    checklist: Vec<QuickstartStepDto>,
    recommended_presets: Vec<WorkflowPreset>,
    verification_commands: Vec<String>,
) -> OnboardingQuickstartDto {
    OnboardingQuickstartDto {
        schema_version: ONBOARDING_QUICKSTART_SCHEMA_VERSION.to_string(),
        generated_at: Utc::now().to_rfc3339(),
        target_mode: "standalone".to_string(),
        dashboard_url,
        checklist,
        recommended_presets,
        verification_commands,
    }
}
