//! CSS-cascade parameter resolution (ADR-012 §4).
//!
//! Resolves final `ResolvedParams` through a 4-level cascade:
//! Default → Regime → Category → Process.

use std::collections::HashMap;

use oneshim_core::models::tiered_memory::{PresetProfile, Regime, ResolvedParams, TriggerParams};
use oneshim_core::models::work_session::AppCategory;

/// Resolves trigger parameters through a 4-level CSS-style cascade:
///
/// 1. **Default** — from the role-based `PresetProfile`
/// 2. **Regime** — per-regime overrides from clustering analysis
/// 3. **Category** — per-app-category overrides (e.g., all Communication apps)
/// 4. **Process** — per-process overrides (e.g., "slack" specifically)
///
/// Each level only overrides `Some` fields; `None` fields pass through.
pub struct ParamResolver {
    global_defaults: ResolvedParams,
    category_overrides: HashMap<AppCategory, TriggerParams>,
    process_overrides: HashMap<String, TriggerParams>,
}

impl ParamResolver {
    /// Create a new resolver from a preset profile.
    pub fn new(preset: PresetProfile) -> Self {
        Self {
            global_defaults: preset.default_params(),
            category_overrides: HashMap::new(),
            process_overrides: HashMap::new(),
        }
    }

    /// Resolve final params given current regime, app category, and process name.
    ///
    /// Cascade: Default → Regime → Category → Process
    pub fn resolve(
        &self,
        regime: Option<&Regime>,
        category: &AppCategory,
        process: &str,
    ) -> ResolvedParams {
        let mut params = self.global_defaults.clone();

        // Level 1: Regime overrides
        if let Some(r) = regime {
            params.apply_overrides(&r.optimal_params);
        }

        // Level 2: Category overrides
        if let Some(cat_override) = self.category_overrides.get(category) {
            params.apply_overrides(cat_override);
        }

        // Level 3: Process overrides (case-insensitive)
        let process_lower = process.to_lowercase();
        if let Some(proc_override) = self.process_overrides.get(&process_lower) {
            params.apply_overrides(proc_override);
        }

        params
    }

    /// Set a category-level override.
    pub fn set_category_override(&mut self, category: AppCategory, params: TriggerParams) {
        self.category_overrides.insert(category, params);
    }

    /// Set a process-level override (stored lowercase).
    pub fn set_process_override(&mut self, process: String, params: TriggerParams) {
        self.process_overrides
            .insert(process.to_lowercase(), params);
    }

    /// Get a reference to the current global defaults.
    pub fn global_defaults(&self) -> &ResolvedParams {
        &self.global_defaults
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::models::tiered_memory::RegimeFeatures;

    fn make_regime_with_params(params: TriggerParams) -> Regime {
        Regime {
            regime_id: "r-test".to_string(),
            name: None,
            auto_label: "Test".to_string(),
            centroid: RegimeFeatures::default(),
            optimal_params: params,
            sample_count: 100,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status: oneshim_core::models::tiered_memory::RegimeStatus::Active,
        }
    }

    #[test]
    fn default_only_uses_preset() {
        let resolver = ParamResolver::new(PresetProfile::Developer);
        let params = resolver.resolve(None, &AppCategory::Other, "unknown");

        let expected = PresetProfile::Developer.default_params();
        assert!((params.t_high - expected.t_high).abs() < 1e-5);
        assert_eq!(params.max_segment_secs, expected.max_segment_secs);
    }

    #[test]
    fn regime_overrides_specific_fields() {
        let resolver = ParamResolver::new(PresetProfile::General);

        let regime_params = TriggerParams {
            t_high: Some(0.90),
            min_segment_secs: Some(300),
            ..Default::default()
        };
        let regime = make_regime_with_params(regime_params);

        let params = resolver.resolve(Some(&regime), &AppCategory::Other, "unknown");

        // t_high should be overridden
        assert!((params.t_high - 0.90).abs() < 1e-5);
        // min_segment_secs should be overridden
        assert_eq!(params.min_segment_secs, 300);
        // Other fields should remain at defaults
        let defaults = PresetProfile::General.default_params();
        assert_eq!(params.buffer_capacity, defaults.buffer_capacity);
    }

    #[test]
    fn category_override_on_top_of_regime() {
        let mut resolver = ParamResolver::new(PresetProfile::General);

        let regime_params = TriggerParams {
            t_high: Some(0.80),
            ..Default::default()
        };
        let regime = make_regime_with_params(regime_params);

        // Category override sets t_high even higher
        resolver.set_category_override(
            AppCategory::Communication,
            TriggerParams {
                t_high: Some(0.50),
                ..Default::default()
            },
        );

        let params = resolver.resolve(Some(&regime), &AppCategory::Communication, "slack");

        // Category override takes precedence over regime
        assert!((params.t_high - 0.50).abs() < 1e-5);
    }

    #[test]
    fn process_override_takes_highest_precedence() {
        let mut resolver = ParamResolver::new(PresetProfile::General);

        let regime_params = TriggerParams {
            t_high: Some(0.80),
            ..Default::default()
        };
        let regime = make_regime_with_params(regime_params);

        resolver.set_category_override(
            AppCategory::Communication,
            TriggerParams {
                t_high: Some(0.50),
                ..Default::default()
            },
        );

        resolver.set_process_override(
            "Slack".to_string(),
            TriggerParams {
                t_high: Some(0.30),
                ..Default::default()
            },
        );

        let params = resolver.resolve(Some(&regime), &AppCategory::Communication, "Slack");

        // Process override wins
        assert!((params.t_high - 0.30).abs() < 1e-5);
    }

    #[test]
    fn weights_normalized_after_cascade() {
        let mut resolver = ParamResolver::new(PresetProfile::General);

        // Override weights to non-normalized values
        resolver.set_process_override(
            "vscode".to_string(),
            TriggerParams {
                w_density: Some(0.8),
                w_importance: Some(0.8),
                ..Default::default()
            },
        );

        let params = resolver.resolve(None, &AppCategory::Development, "vscode");

        let sum = params.w_density + params.w_importance + params.w_context + params.w_buffer;
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "weights should be normalized, got sum={}",
            sum
        );
    }

    #[test]
    fn process_override_case_insensitive() {
        let mut resolver = ParamResolver::new(PresetProfile::General);

        resolver.set_process_override(
            "VSCode".to_string(),
            TriggerParams {
                t_high: Some(0.99),
                ..Default::default()
            },
        );

        // Should match regardless of case
        let params = resolver.resolve(None, &AppCategory::Development, "vscode");
        assert!((params.t_high - 0.99).abs() < 1e-5);

        let params2 = resolver.resolve(None, &AppCategory::Development, "VSCODE");
        assert!((params2.t_high - 0.99).abs() < 1e-5);
    }

    #[test]
    fn no_overrides_returns_defaults() {
        let resolver = ParamResolver::new(PresetProfile::Manager);
        let expected = PresetProfile::Manager.default_params();
        let params = resolver.resolve(None, &AppCategory::Other, "anything");

        assert!((params.w_density - expected.w_density).abs() < 1e-5);
        assert!((params.t_high - expected.t_high).abs() < 1e-5);
        assert_eq!(params.min_segment_secs, expected.min_segment_secs);
    }
}
