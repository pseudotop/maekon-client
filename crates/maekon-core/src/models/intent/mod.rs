//! Intent and workflow automation types for maekon-core.
//!
//! Split from the original monolithic `intent.rs`.
//! Public API is fully preserved — all types re-exported at the same path.

mod automation;
mod command;
mod elements;
mod workflow;

// Re-export the full public surface of the original intent.rs.
pub use automation::AutomationIntent;
pub use command::{IntentCommand, IntentConfig, IntentResult, VerificationResult};
pub use elements::{ElementBounds, FinderSource, UiElement};
pub use workflow::{
    builtin_presets, platform_alt_modifier, platform_modifier, PresetCategory, WorkflowPreset,
    WorkflowStep,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::automation::AutomationAction;

    #[test]
    fn intent_click_element_serde() {
        let intent = AutomationIntent::ClickElement {
            text: Some("save".to_string()),
            role: Some("button".to_string()),
            app_name: None,
            button: "left".to_string(),
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::ClickElement { text, button, .. } => {
                assert_eq!(text.unwrap(), "save");
                assert_eq!(button, "left");
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_type_into_element_serde() {
        let intent = AutomationIntent::TypeIntoElement {
            element_text: Some("search".to_string()),
            role: None,
            text: "hello world".to_string(),
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::TypeIntoElement { text, .. } => {
                assert_eq!(text, "hello world");
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_execute_hotkey_serde() {
        let intent = AutomationIntent::ExecuteHotkey {
            keys: vec!["Ctrl".to_string(), "S".to_string()],
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::ExecuteHotkey { keys } => {
                assert_eq!(keys.len(), 2);
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_wait_for_text_serde() {
        let intent = AutomationIntent::WaitForText {
            text: "completed".to_string(),
            timeout_ms: 5000,
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::WaitForText { text, timeout_ms } => {
                assert_eq!(text, "completed");
                assert_eq!(timeout_ms, 5000);
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn intent_raw_serde() {
        let intent = AutomationIntent::Raw(AutomationAction::MouseClick {
            button: "left".to_string(),
            x: 100,
            y: 200,
        });
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        assert!(matches!(deser, AutomationIntent::Raw(_)));
    }

    #[test]
    fn intent_activate_app_serde() {
        let intent = AutomationIntent::ActivateApp {
            app_name: "Visual Studio Code".to_string(),
        };
        let json = serde_json::to_string(&intent).unwrap();
        let deser: AutomationIntent = serde_json::from_str(&json).unwrap();
        match deser {
            AutomationIntent::ActivateApp { app_name } => {
                assert_eq!(app_name, "Visual Studio Code");
            }
            other => unreachable!("unexpected variant: {:?}", other),
        }
    }

    #[test]
    fn element_bounds_center() {
        let bounds = ElementBounds {
            x: 100,
            y: 200,
            width: 80,
            height: 40,
        };
        let (cx, cy) = bounds.center();
        assert_eq!(cx, 140);
        assert_eq!(cy, 220);
    }

    #[test]
    fn element_bounds_contains() {
        let bounds = ElementBounds {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        };
        assert!(bounds.contains(10, 20));
        assert!(bounds.contains(50, 40));
        assert!(!bounds.contains(110, 20));
        assert!(!bounds.contains(10, 70));
        assert!(!bounds.contains(9, 20));
    }

    #[test]
    fn ui_element_serde() {
        let elem = UiElement {
            text: "save".to_string(),
            bounds: ElementBounds {
                x: 100,
                y: 200,
                width: 80,
                height: 30,
            },
            role: Some("button".to_string()),
            confidence: 0.95,
            source: FinderSource::Ocr,
        };
        let json = serde_json::to_string(&elem).unwrap();
        let deser: UiElement = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.text, "save");
        assert_eq!(deser.confidence, 0.95);
        assert_eq!(deser.source, FinderSource::Ocr);
    }

    #[test]
    fn intent_config_defaults() {
        let config = IntentConfig::default();
        assert!((config.min_confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.retry_interval_ms, 500);
        assert!(config.verify_after_action);
        assert_eq!(config.verify_delay_ms, 1000);
    }

    #[test]
    fn intent_config_serde_with_defaults() {
        let json = "{}";
        let config: IntentConfig = serde_json::from_str(json).unwrap();
        assert!((config.min_confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn intent_config_serde_override() {
        let json = r#"{"min_confidence": 0.9, "max_retries": 5}"#;
        let config: IntentConfig = serde_json::from_str(json).unwrap();
        assert!((config.min_confidence - 0.9).abs() < f64::EPSILON);
        assert_eq!(config.max_retries, 5);
    }

    #[test]
    fn intent_command_serde() {
        let cmd = IntentCommand {
            command_id: "cmd-1".to_string(),
            session_id: "sess-1".to_string(),
            intent: AutomationIntent::ExecuteHotkey {
                keys: vec!["Ctrl".to_string(), "C".to_string()],
            },
            config: None,
            timeout_ms: Some(10000),
            policy_token: "token-abc".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: IntentCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.command_id, "cmd-1");
        assert_eq!(deser.policy_token, "token-abc");
    }

    #[test]
    fn verification_result_serde() {
        let result = VerificationResult {
            screen_changed: true,
            changed_regions: 3,
            text_found: Some(true),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: VerificationResult = serde_json::from_str(&json).unwrap();
        assert!(deser.screen_changed);
        assert_eq!(deser.changed_regions, 3);
    }

    #[test]
    fn intent_result_serde() {
        let result = IntentResult {
            success: true,
            element: None,
            verification: Some(VerificationResult {
                screen_changed: true,
                changed_regions: 1,
                text_found: None,
            }),
            retry_count: 0,
            elapsed_ms: 250,
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deser: IntentResult = serde_json::from_str(&json).unwrap();
        assert!(deser.success);
        assert_eq!(deser.elapsed_ms, 250);
    }

    #[test]
    fn finder_source_serde() {
        let source = FinderSource::Accessibility;
        let json = serde_json::to_string(&source).unwrap();
        let deser: FinderSource = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, FinderSource::Accessibility);
    }

    #[test]
    fn workflow_preset_serde() {
        let preset = WorkflowPreset {
            id: "save-file".to_string(),
            name: "file save".to_string(),
            description: "Save the current file.".to_string(),
            category: PresetCategory::Productivity,
            steps: vec![WorkflowStep {
                name: "Ctrl+S".to_string(),
                intent: AutomationIntent::ExecuteHotkey {
                    keys: vec!["Ctrl".to_string(), "S".to_string()],
                },
                delay_ms: 0,
                stop_on_failure: true,
            }],
            builtin: true,
            platform: None,
            ai_profile_id: None,
        };
        let json = serde_json::to_string(&preset).unwrap();
        let deser: WorkflowPreset = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.id, "save-file");
        assert_eq!(deser.category, PresetCategory::Productivity);
        assert_eq!(deser.steps.len(), 1);
        assert!(deser.builtin);
    }

    #[test]
    fn workflow_step_defaults() {
        let json = r#"{"name":"step1","intent":{"ExecuteHotkey":{"keys":["Ctrl","Z"]}}}"#;
        let step: WorkflowStep = serde_json::from_str(json).unwrap();
        assert_eq!(step.delay_ms, 0);
        assert!(step.stop_on_failure);
    }

    #[test]
    fn preset_category_serde() {
        for cat in [
            PresetCategory::Productivity,
            PresetCategory::AppManagement,
            PresetCategory::Workflow,
            PresetCategory::Custom,
        ] {
            let json = serde_json::to_string(&cat).unwrap();
            let deser: PresetCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(deser, cat);
        }
    }

    #[test]
    fn builtin_presets_load() {
        let presets = builtin_presets();
        assert_eq!(presets.len(), 15);
        assert!(presets.iter().all(|p| p.builtin));
    }

    #[test]
    fn platform_modifier_keys() {
        let m = platform_modifier();
        if cfg!(target_os = "macos") {
            assert_eq!(m, "Cmd");
        } else {
            assert_eq!(m, "Ctrl");
        }
    }

    #[test]
    fn all_presets_have_steps() {
        let presets = builtin_presets();
        for preset in &presets {
            assert!(
                !preset.steps.is_empty(),
                "프리셋 '{}'에 단계 none",
                preset.id
            );
        }
    }

    #[test]
    fn preset_ids_unique() {
        let presets = builtin_presets();
        let ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        let mut unique_ids = ids.clone();
        unique_ids.sort();
        unique_ids.dedup();
        assert_eq!(ids.len(), unique_ids.len(), "Duplicate preset ID found");
    }

    #[test]
    fn preset_categories_coverage() {
        let presets = builtin_presets();
        let has_productivity = presets
            .iter()
            .any(|p| p.category == PresetCategory::Productivity);
        let has_app = presets
            .iter()
            .any(|p| p.category == PresetCategory::AppManagement);
        let has_workflow = presets
            .iter()
            .any(|p| p.category == PresetCategory::Workflow);
        assert!(has_productivity);
        assert!(has_app);
        assert!(has_workflow);
    }

    /// F-RC-C36-05: PresetCategory serde PascalCase 라운드트립 검증.
    #[test]
    fn preset_category_serde_pascal_case_roundtrip() {
        let cases = [
            (PresetCategory::Productivity, "\"Productivity\""),
            (PresetCategory::AppManagement, "\"AppManagement\""),
            (PresetCategory::Workflow, "\"Workflow\""),
            (PresetCategory::Custom, "\"Custom\""),
        ];
        for (variant, expected_json) in &cases {
            let json = serde_json::to_string(variant).expect("직렬화 실패");
            assert_eq!(
                json, *expected_json,
                "PascalCase JSON 불일치: {:?}",
                variant
            );
            let restored: PresetCategory = serde_json::from_str(&json).expect("역직렬화 실패");
            assert_eq!(restored, *variant, "역직렬화 후 값 불일치: {:?}", variant);
        }
    }
}
