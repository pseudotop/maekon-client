use oneshim_api_contracts::onboarding::{OnboardingQuickstartDto, QuickstartStepDto};
use oneshim_core::models::intent::builtin_presets;
use oneshim_core::models::intent::WorkflowPreset;

use crate::services::onboarding_assembler::{assemble_quickstart, assemble_quickstart_step};
use crate::services::web_contexts::ConfigWebContext;

#[derive(Clone)]
pub struct OnboardingQueryService {
    ctx: ConfigWebContext,
}

impl OnboardingQueryService {
    pub fn new(ctx: ConfigWebContext) -> Self {
        Self { ctx }
    }

    pub fn get_quickstart(&self) -> OnboardingQuickstartDto {
        assemble_quickstart(
            dashboard_url_from_context(&self.ctx),
            quickstart_checklist(),
            recommended_presets(),
            verification_commands(),
        )
    }
}

fn recommended_presets() -> Vec<WorkflowPreset> {
    let preferred = [
        "daily-priority-sync",
        "deep-work-start",
        "bug-triage-loop",
        "release-readiness",
    ];
    let presets = builtin_presets();
    preferred
        .iter()
        .filter_map(|id| presets.iter().find(|preset| preset.id == *id).cloned())
        .collect()
}

fn dashboard_url_from_context(context: &ConfigWebContext) -> String {
    if let Some(config_manager) = context.config_manager.as_ref() {
        let config = config_manager.get();
        if config.web.allow_external {
            return format!("http://0.0.0.0:{}", config.web.port);
        }
        return format!("http://127.0.0.1:{}", config.web.port);
    }
    format!(
        "http://127.0.0.1:{}",
        oneshim_core::config::DEFAULT_WEB_PORT
    )
}

fn quickstart_checklist() -> Vec<QuickstartStepDto> {
    vec![
        assemble_quickstart_step(
            1,
            "Run Standalone Mode",
            "Launch `cargo run -p oneshim-app -- --offline`.",
            "Agent starts without external server dependency.",
        ),
        assemble_quickstart_step(
            2,
            "Open Dashboard",
            "Open local dashboard URL.",
            "Metrics and timeline panels load without errors.",
        ),
        assemble_quickstart_step(
            3,
            "Check Privacy Baseline",
            "Keep sandbox enabled and `external_data_policy` at least `PiiFilterStandard`.",
            "Sensitive inputs are blocked unless explicit override exists.",
        ),
        assemble_quickstart_step(
            4,
            "Enable One Workflow",
            "Run one recommended workflow preset for daily routine.",
            "Automation audit entries show success/blocked rate baseline.",
        ),
        assemble_quickstart_step(
            5,
            "Validate First Insight",
            "Review focus suggestions and timeline interruption markers.",
            "At least one actionable local insight is produced.",
        ),
    ]
}

fn verification_commands() -> Vec<String> {
    vec![
        "cargo run -p oneshim-app -- --offline".to_string(),
        "cargo test --workspace".to_string(),
        "cargo test -p oneshim-automation perf_budget_ -- --nocapture".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_presets_contains_target_ids() {
        let presets = recommended_presets();
        let ids: Vec<String> = presets.into_iter().map(|preset| preset.id).collect();
        assert!(ids.iter().any(|id| id == "daily-priority-sync"));
        assert!(ids.iter().any(|id| id == "deep-work-start"));
    }
}
