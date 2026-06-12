use maekon_api_contracts::onboarding::{OnboardingQuickstartDto, QuickstartStepDto};
use maekon_core::models::intent::builtin_presets;
use maekon_core::models::intent::WorkflowPreset;

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
    let port = context
        .config_manager
        .as_ref()
        .map(|manager| manager.get().web.port)
        .unwrap_or(maekon_core::config::DEFAULT_WEB_PORT);
    dashboard_url(port)
}

/// Display URL for the local dashboard — always loopback (#5593).
///
/// `0.0.0.0` is a bind address, not a connectable URL, so it must never be
/// rendered for users even when `web.allow_external` is set: in that mode the
/// listener may also accept LAN connections, but the URL we show has to work
/// from the local machine, and `http://127.0.0.1:{port}` always does.
fn dashboard_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

fn quickstart_checklist() -> Vec<QuickstartStepDto> {
    vec![
        assemble_quickstart_step(
            1,
            "Run Standalone Mode",
            "Launch `cargo run -p maekon-app -- --offline`.",
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
        "cargo run -p maekon-app -- --offline".to_string(),
        "cargo test --workspace".to_string(),
        "cargo test -p maekon-automation perf_budget_ -- --nocapture".to_string(),
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

    #[test]
    fn dashboard_url_is_always_loopback() {
        assert_eq!(dashboard_url(10090), "http://127.0.0.1:10090");
        // 0.0.0.0 is a bind address, never a connectable display URL (#5593).
        assert!(!dashboard_url(8080).contains("0.0.0.0"));
    }

    #[test]
    fn dashboard_url_without_config_manager_uses_default_port() {
        let ctx = ConfigWebContext {
            config_manager: None,
        };
        assert_eq!(
            dashboard_url_from_context(&ctx),
            format!("http://127.0.0.1:{}", maekon_core::config::DEFAULT_WEB_PORT)
        );
    }
}
