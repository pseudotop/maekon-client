mod calibration;
mod content;
mod params;
mod regime;
mod segment;
mod trigger;

pub use calibration::*;
pub use content::*;
pub use params::*;
pub use regime::*;
pub use segment::*;
pub use trigger::*;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use chrono::Utc;

    use crate::models::work_session::AppCategory;

    use super::*;

    #[test]
    fn resolved_params_default_weights_sum_one() {
        let p = ResolvedParams::default();
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn validate_and_normalize_sums_to_one() {
        let mut p = ResolvedParams {
            w_density: 2.0,
            w_importance: 3.0,
            w_context: 4.0,
            w_buffer: 1.0,
            ..ResolvedParams::default()
        };
        p.validate_and_normalize();
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn validate_and_normalize_all_zero_fallback() {
        let mut p = ResolvedParams {
            w_density: 0.0,
            w_importance: 0.0,
            w_context: 0.0,
            w_buffer: 0.0,
            ..ResolvedParams::default()
        };
        p.validate_and_normalize();
        assert!((p.w_density - 0.25).abs() < 1e-5);
        assert!((p.w_importance - 0.25).abs() < 1e-5);
        assert!((p.w_context - 0.25).abs() < 1e-5);
        assert!((p.w_buffer - 0.25).abs() < 1e-5);
    }

    #[test]
    fn validate_and_normalize_negative_weights() {
        let mut p = ResolvedParams {
            w_density: -1.0,
            w_importance: -2.0,
            w_context: -0.5,
            w_buffer: -0.3,
            ..ResolvedParams::default()
        };
        p.validate_and_normalize();
        // All negatives clamped to 0 -> fallback to 0.25 each
        assert!((p.w_density - 0.25).abs() < 1e-5);
    }

    #[test]
    fn validate_and_normalize_swaps_inverted_thresholds() {
        let mut p = ResolvedParams {
            t_high: 0.20,
            t_low: 0.80,
            ..ResolvedParams::default()
        };
        p.validate_and_normalize();
        assert!(p.t_low <= p.t_high);
    }

    #[test]
    fn validate_and_normalize_swaps_inverted_segment_durations() {
        let mut p = ResolvedParams {
            min_segment_secs: 1000,
            max_segment_secs: 100,
            ..ResolvedParams::default()
        };
        p.validate_and_normalize();
        assert!(p.min_segment_secs <= p.max_segment_secs);
    }

    #[test]
    fn apply_overrides_some_fields() {
        let mut p = ResolvedParams::default();
        let overrides = TriggerParams {
            w_density: Some(0.50),
            t_high: Some(0.90),
            ..Default::default()
        };
        p.apply_overrides(&overrides);
        // After apply_overrides, validate_and_normalize is called,
        // so weights are re-normalized: sum = 0.50+0.30+0.25+0.15 = 1.20
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
        // w_density should be largest weight (0.50/1.20 ~ 0.4167)
        assert!(p.w_density > p.w_importance);
        assert!((p.t_high - 0.90).abs() < 1e-5);
    }

    #[test]
    fn apply_overrides_none_fields_keep_defaults() {
        let original = ResolvedParams::default();
        let mut p = original.clone();
        let overrides = TriggerParams::default(); // all None
        p.apply_overrides(&overrides);
        assert!((p.w_density - original.w_density).abs() < 1e-5);
        assert!((p.alpha_short - original.alpha_short).abs() < 1e-5);
        assert_eq!(p.min_segment_secs, original.min_segment_secs);
    }

    #[test]
    fn apply_overrides_merges_importance_map() {
        let mut p = ResolvedParams::default();
        p.importance_overrides.insert("Slack".to_string(), 0.5);
        let overrides = TriggerParams {
            importance_overrides: Some(HashMap::from([("VSCode".to_string(), 0.9)])),
            ..Default::default()
        };
        p.apply_overrides(&overrides);
        assert_eq!(p.importance_overrides.len(), 2);
        assert!((p.importance_overrides["VSCode"] - 0.9).abs() < 1e-5);
        assert!((p.importance_overrides["Slack"] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn preset_developer_valid() {
        let p = PresetProfile::Developer.default_params();
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
        assert!(p.t_low <= p.t_high);
        assert!(p.min_segment_secs <= p.max_segment_secs);
        assert_eq!(p.max_segment_secs, 900);
        assert!(p.importance_overrides.contains_key("VSCode"));
        assert!(p.importance_overrides.contains_key("Terminal"));
    }

    #[test]
    fn preset_manager_valid() {
        let p = PresetProfile::Manager.default_params();
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
        assert!(p.t_low <= p.t_high);
        assert_eq!(p.max_segment_secs, 450);
        assert!(p.importance_overrides.contains_key("Slack"));
        assert!(p.importance_overrides.contains_key("Zoom"));
    }

    #[test]
    fn preset_designer_valid() {
        let p = PresetProfile::Designer.default_params();
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
        assert!(p.importance_overrides.contains_key("Figma"));
        assert!((p.importance_overrides["Figma"] - 0.95).abs() < 1e-5);
    }

    #[test]
    fn preset_researcher_valid() {
        let p = PresetProfile::Researcher.default_params();
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
        assert_eq!(p.max_segment_secs, 1200);
        assert!(p.importance_overrides.contains_key("Google Chrome"));
        assert!(p.importance_overrides.contains_key("Obsidian"));
    }

    #[test]
    fn preset_general_valid() {
        let p = PresetProfile::General.default_params();
        let sum = p.w_density + p.w_importance + p.w_context + p.w_buffer;
        assert!((sum - 1.0).abs() < 1e-5);
        assert!(p.importance_overrides.is_empty());
    }

    #[test]
    fn trigger_input_serde_roundtrip() {
        let input = TriggerInput::AppSwitchNew {
            app_name: "VSCode".to_string(),
            prev_app: "Chrome".to_string(),
            category: AppCategory::Development,
        };
        let json = serde_json::to_string(&input).unwrap();
        let back: TriggerInput = serde_json::from_str(&json).unwrap();
        if let TriggerInput::AppSwitchNew { app_name, .. } = back {
            assert_eq!(app_name, "VSCode");
        } else {
            panic!("unexpected variant");
        }
    }

    #[test]
    fn trigger_reason_score_low_serde_roundtrip() {
        let reason = TriggerReason::ScoreLow;
        let json = serde_json::to_string(&reason).unwrap();
        assert_eq!(json, "\"SCORE_LOW\"");
        let back: TriggerReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TriggerReason::ScoreLow);
    }

    #[test]
    fn segment_summary_serde_roundtrip() {
        let seg = SegmentSummary {
            segment_id: "seg-001".to_string(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            duration_secs: 300,
            regime_id: None,
            trigger_reason: TriggerReason::ScoreHigh,
            event_count: 42,
            app_breakdown: HashMap::from([("VSCode".to_string(), 200)]),
            category_breakdown: HashMap::from([("Development".to_string(), 200)]),
            context_switch_count: 3,
            dominant_category: "Development".to_string(),
            avg_importance: 0.7,
            patterns_detected: vec![],
            content_activities: vec![],
            container: None,
            llm_summary: None,
        };
        let json = serde_json::to_string(&seg).unwrap();
        let back: SegmentSummary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.segment_id, "seg-001");
        assert_eq!(back.event_count, 42);
    }

    // --- RegimeFeatures tests ---

    #[test]
    fn regime_features_to_array_roundtrip() {
        let f = RegimeFeatures {
            category_coding: 1.0,
            category_communication: 0.0,
            category_browser: 0.0,
            avg_event_rate: 0.5,
            avg_importance: 0.8,
            context_activity_signal: 0.1,
            communication_ratio: 0.2,
        };
        let arr = f.to_array();
        let back = RegimeFeatures::from_array(arr);
        assert_eq!(f, back);
    }

    #[test]
    fn regime_features_default_is_zero() {
        let f = RegimeFeatures::default();
        assert!(f.to_array().iter().all(|v| *v == 0.0));
    }

    #[test]
    fn euclidean_distance_same_point_is_zero() {
        let f = RegimeFeatures {
            category_coding: 1.0,
            avg_event_rate: 0.5,
            ..RegimeFeatures::default()
        };
        assert!((euclidean_distance(&f, &f) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn euclidean_distance_known_value() {
        let a = RegimeFeatures::default();
        let b = RegimeFeatures {
            category_coding: 3.0,
            category_communication: 4.0,
            ..RegimeFeatures::default()
        };
        // distance = sqrt(9 + 16) = 5.0
        assert!((euclidean_distance(&a, &b) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn regime_serde_roundtrip() {
        let regime = Regime {
            regime_id: "r-001".to_string(),
            name: Some("My Focus".to_string()),
            auto_label: "Deep Focus (VSCode)".to_string(),
            centroid: RegimeFeatures {
                category_coding: 1.0,
                ..RegimeFeatures::default()
            },
            optimal_params: TriggerParams::default(),
            sample_count: 100,
            first_seen: Utc::now(),
            last_seen: Utc::now(),
            status: RegimeStatus::Active,
        };
        let json = serde_json::to_string(&regime).unwrap();
        let back: Regime = serde_json::from_str(&json).unwrap();
        assert_eq!(back.regime_id, "r-001");
        assert_eq!(back.status, RegimeStatus::Active);
        assert_eq!(back.name, Some("My Focus".to_string()));
    }

    #[test]
    fn regime_status_variants() {
        assert_ne!(RegimeStatus::Active, RegimeStatus::Inactive);
        assert_ne!(RegimeStatus::Inactive, RegimeStatus::Archived);
    }
}
