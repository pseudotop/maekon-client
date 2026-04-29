use oneshim_core::config::FocusAutoConfig;

use crate::focus_mode::FocusModeState;

/// Stateless evaluator for focus auto-switch rules.
pub struct FocusAutoEvaluator;

impl FocusAutoEvaluator {
    /// Returns `Some(duration_minutes)` if auto-activation should trigger.
    pub fn evaluate(
        config: &FocusAutoConfig,
        focus_mode: &FocusModeState,
        current_app: &str,
    ) -> Option<u32> {
        if !config.enabled || focus_mode.is_active() {
            return None;
        }
        if focus_mode.in_cooldown(config.cooldown_secs) {
            return None;
        }

        // Rule 1: App detection (exact case-insensitive match)
        if !current_app.is_empty()
            && config
                .trigger_apps
                .iter()
                .any(|app| current_app.eq_ignore_ascii_case(app))
        {
            return Some(config.duration_minutes);
        }

        // Rule 2: Time schedule
        let now = chrono::Local::now();
        for schedule in &config.trigger_schedules {
            if schedule.matches_now(&now) {
                return Some(config.duration_minutes);
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(enabled: bool, apps: Vec<&str>) -> FocusAutoConfig {
        FocusAutoConfig {
            enabled,
            duration_minutes: 25,
            trigger_apps: apps.into_iter().map(String::from).collect(),
            trigger_schedules: vec![],
            cooldown_secs: 300,
        }
    }

    #[test]
    fn disabled_returns_none() {
        let config = make_config(false, vec!["VSCode"]);
        let focus = FocusModeState::new();
        assert!(FocusAutoEvaluator::evaluate(&config, &focus, "VSCode").is_none());
    }

    #[test]
    fn already_active_returns_none() {
        let config = make_config(true, vec!["VSCode"]);
        let focus = FocusModeState::new();
        focus.activate(25, false);
        assert!(FocusAutoEvaluator::evaluate(&config, &focus, "VSCode").is_none());
    }

    #[test]
    fn app_match_triggers() {
        let config = make_config(true, vec!["Visual Studio Code"]);
        let focus = FocusModeState::new();
        assert_eq!(
            FocusAutoEvaluator::evaluate(&config, &focus, "visual studio code"),
            Some(25)
        );
    }

    #[test]
    fn app_no_match_returns_none() {
        let config = make_config(true, vec!["VSCode"]);
        let focus = FocusModeState::new();
        assert!(FocusAutoEvaluator::evaluate(&config, &focus, "Chrome").is_none());
    }

    #[test]
    fn cooldown_blocks_trigger() {
        let config = make_config(true, vec!["VSCode"]);
        let focus = FocusModeState::new();
        focus.activate(25, true);
        focus.deactivate(); // records cooldown
        assert!(FocusAutoEvaluator::evaluate(&config, &focus, "VSCode").is_none());
    }

    #[test]
    fn empty_app_name_skipped() {
        let config = make_config(true, vec!["VSCode"]);
        let focus = FocusModeState::new();
        assert!(FocusAutoEvaluator::evaluate(&config, &focus, "").is_none());
    }
}
