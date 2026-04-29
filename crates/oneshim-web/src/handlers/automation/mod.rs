mod execution;
mod scene;

pub use execution::{
    create_execution_policy, create_preset, delete_execution_policy, delete_preset,
    execute_intent_hint, execute_scene_action, get_audit_logs, get_automation_stats,
    get_automation_status, get_contract_versions, get_policies, get_policy_events,
    list_execution_policies, list_presets, run_preset, update_execution_policy, update_preset,
};
pub use scene::{get_automation_scene, get_automation_scene_calibration};

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use oneshim_api_contracts::automation::{
        AuditEntryDto, AuditQuery, AutomationStatsDto, AutomationStatusDto,
        ExecuteIntentHintRequest, ExecuteIntentHintResponse, PoliciesDto, PolicyEventQuery,
        PresetRunResult, SceneActionType, SceneQuery,
    };
    use oneshim_core::config::{AiAccessMode, SceneActionOverrideConfig, SceneIntelligenceConfig};
    use oneshim_core::models::intent::{AutomationIntent, ElementBounds};
    use oneshim_core::models::ui_scene::{UiScene, UI_SCENE_SCHEMA_VERSION};

    use crate::error::ApiError;
    use crate::services::automation_service::helpers::*;
    use crate::services::automation_service::{
        AUTOMATION_AUDIT_SCHEMA_VERSION, AUTOMATION_SCENE_CALIBRATION_SCHEMA_VERSION,
    };

    use oneshim_api_contracts::automation::ExecuteSceneActionRequest;

    #[test]
    fn automation_status_dto_serializes() {
        let dto = AutomationStatusDto {
            enabled: true,
            sandbox_enabled: true,
            sandbox_profile: "Standard".to_string(),
            ocr_provider: "Local".to_string(),
            llm_provider: "Remote".to_string(),
            ocr_source: "local".to_string(),
            llm_source: "local-fallback".to_string(),
            ocr_fallback_reason: None,
            llm_fallback_reason: Some("llm endpoint timeout".to_string()),
            external_data_policy: "PiiFilterStrict".to_string(),
            pending_audit_entries: 5,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("sandbox_profile"));
        assert!(json.contains("ocr_source"));
        assert!(json.contains("pending_audit_entries"));
    }

    #[test]
    fn audit_entry_dto_serializes() {
        let dto = AuditEntryDto {
            schema_version: AUTOMATION_AUDIT_SCHEMA_VERSION.to_string(),
            entry_id: "e-001".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            session_id: "sess-001".to_string(),
            command_id: "cmd-001".to_string(),
            action_type: "MouseClick".to_string(),
            status: "Completed".to_string(),
            details: Some("OK".to_string()),
            elapsed_ms: Some(150),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("cmd-001"));
        assert!(json.contains("150"));
    }

    #[test]
    fn automation_stats_dto_serializes() {
        let dto = AutomationStatsDto {
            total_executions: 100,
            successful: 80,
            failed: 10,
            denied: 5,
            timeout: 5,
            avg_elapsed_ms: 250.5,
            success_rate: 0.8,
            blocked_rate: 0.05,
            p95_elapsed_ms: 420.0,
            timing_samples: 92,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("total_executions"));
        assert!(json.contains("avg_elapsed_ms"));
        assert!(json.contains("success_rate"));
        assert!(json.contains("p95_elapsed_ms"));
    }

    #[test]
    fn policies_dto_serializes() {
        let dto = PoliciesDto {
            automation_enabled: true,
            sandbox_profile: "Strict".to_string(),
            sandbox_enabled: true,
            allow_network: false,
            external_data_policy: "PiiFilterStrict".to_string(),
            scene_action_override_enabled: true,
            scene_action_override_active: true,
            scene_action_override_reason: Some("calibration".to_string()),
            scene_action_override_approved_by: Some("security-reviewer".to_string()),
            scene_action_override_expires_at: Some("2026-02-24T03:00:00Z".to_string()),
            scene_action_override_issue: None,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("Strict"));
        assert!(json.contains("scene_action_override_active"));
    }

    #[test]
    fn preset_run_result_serializes() {
        let dto = PresetRunResult {
            preset_id: "save-file".to_string(),
            success: true,
            message: "executed".to_string(),
            steps_executed: Some(2),
            total_steps: Some(3),
            total_elapsed_ms: Some(150),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(json.contains("save-file"));
        assert!(json.contains("steps_executed"));
    }

    #[test]
    fn preset_run_result_omits_none_fields() {
        let dto = PresetRunResult {
            preset_id: "test".to_string(),
            success: false,
            message: "failure".to_string(),
            steps_executed: None,
            total_steps: None,
            total_elapsed_ms: None,
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(!json.contains("steps_executed"));
        assert!(!json.contains("total_steps"));
        assert!(!json.contains("total_elapsed_ms"));
    }

    #[test]
    fn audit_query_defaults() {
        let json = "{}";
        let query: AuditQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, 50);
        assert!(query.status.is_none());
    }

    #[test]
    fn policy_event_query_defaults() {
        let json = "{}";
        let query: PolicyEventQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.limit, 100);
    }

    #[test]
    fn infer_runtime_source_respects_access_mode() {
        assert_eq!(
            infer_runtime_source(AiAccessMode::ProviderSubscriptionCli, true),
            "cli-subscription"
        );
        assert_eq!(
            infer_runtime_source(AiAccessMode::LocalModel, true),
            "local"
        );
        assert_eq!(
            infer_runtime_source(AiAccessMode::ProviderApiKey, true),
            "remote"
        );
        assert_eq!(
            infer_runtime_source(AiAccessMode::ProviderApiKey, false),
            "local"
        );
    }

    #[test]
    fn execute_intent_hint_request_deserializes_optional_command_id() {
        let payload = r#"{
            "session_id": "sess-1",
            "intent_hint": "click the save button"
        }"#;
        let request: ExecuteIntentHintRequest = serde_json::from_str(payload).unwrap();
        assert!(request.command_id.is_none());
        assert_eq!(request.session_id, "sess-1");
        assert_eq!(request.intent_hint, "click the save button");
    }

    #[test]
    fn execute_intent_hint_response_serializes() {
        let response = ExecuteIntentHintResponse {
            command_id: "hint-1".to_string(),
            session_id: "sess-1".to_string(),
            planned_intent: oneshim_core::models::intent::AutomationIntent::ExecuteHotkey {
                keys: vec!["Ctrl".to_string(), "S".to_string()],
            },
            result: oneshim_core::models::intent::IntentResult {
                success: true,
                element: None,
                verification: None,
                retry_count: 0,
                elapsed_ms: 10,
                error: None,
            },
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("planned_intent"));
        assert!(json.contains("command_id"));
    }

    #[test]
    fn scene_query_deserializes_frame_id() {
        let json = r#"{"app_name":"Code","screen_id":"main","frame_id":42}"#;
        let query: SceneQuery = serde_json::from_str(json).unwrap();
        assert_eq!(query.app_name.as_deref(), Some("Code"));
        assert_eq!(query.screen_id.as_deref(), Some("main"));
        assert_eq!(query.frame_id, Some(42));
    }

    #[test]
    fn infer_image_format_falls_back_to_webp() {
        let path = std::path::Path::new("frames/2026-02-24/capture");
        assert_eq!(infer_image_format(path), "webp");
    }

    #[test]
    fn build_scene_action_intents_click_returns_raw_click() {
        let req = ExecuteSceneActionRequest {
            command_id: None,
            session_id: "sess-1".to_string(),
            frame_id: Some(1),
            scene_id: Some("scene-1".to_string()),
            element_id: "el-1".to_string(),
            action_type: SceneActionType::Click,
            bbox_abs: ElementBounds {
                x: 10,
                y: 20,
                width: 100,
                height: 40,
            },
            role: Some("button".to_string()),
            label: Some("Save".to_string()),
            text: None,
            allow_sensitive_input: None,
        };

        let intents = build_scene_action_intents(&req).unwrap();
        assert_eq!(intents.len(), 1);
        assert!(matches!(intents[0], AutomationIntent::Raw(_)));
    }

    #[test]
    fn build_scene_action_intents_type_text_requires_text() {
        let req = ExecuteSceneActionRequest {
            command_id: None,
            session_id: "sess-1".to_string(),
            frame_id: Some(1),
            scene_id: Some("scene-1".to_string()),
            element_id: "el-2".to_string(),
            action_type: SceneActionType::TypeText,
            bbox_abs: ElementBounds {
                x: 10,
                y: 20,
                width: 100,
                height: 40,
            },
            role: Some("input".to_string()),
            label: Some("Search".to_string()),
            text: None,
            allow_sensitive_input: None,
        };

        let err = build_scene_action_intents(&req).unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[test]
    fn evaluate_scene_action_override_reports_missing_reason() {
        let cfg = SceneActionOverrideConfig {
            enabled: true,
            reason: None,
            approved_by: Some("reviewer".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::minutes(10)),
        };
        let (active, issue) = evaluate_scene_action_override(&cfg, Utc::now());
        assert!(!active);
        assert!(issue.unwrap_or_default().contains("reason"));
    }

    #[test]
    fn evaluate_scene_action_override_reports_expired_ttl() {
        let cfg = SceneActionOverrideConfig {
            enabled: true,
            reason: Some("incident".to_string()),
            approved_by: Some("reviewer".to_string()),
            expires_at: Some(Utc::now() - chrono::Duration::minutes(1)),
        };
        let (active, issue) = evaluate_scene_action_override(&cfg, Utc::now());
        assert!(!active);
        let issue_text = issue.unwrap_or_default();
        assert!(issue_text.contains("expired"));
    }

    #[test]
    fn evaluate_scene_action_override_active_when_valid() {
        let cfg = SceneActionOverrideConfig {
            enabled: true,
            reason: Some("high-fidelity validation".to_string()),
            approved_by: Some("reviewer".to_string()),
            expires_at: Some(Utc::now() + chrono::Duration::minutes(20)),
        };
        let (active, issue) = evaluate_scene_action_override(&cfg, Utc::now());
        assert!(active);
        assert!(issue.is_none());
    }

    fn sample_scene_with_confidence(values: &[f64]) -> UiScene {
        UiScene {
            schema_version: UI_SCENE_SCHEMA_VERSION.to_string(),
            scene_id: "scene-test".to_string(),
            app_name: Some("TestApp".to_string()),
            screen_id: Some("screen-1".to_string()),
            captured_at: Utc::now(),
            screen_width: 1920,
            screen_height: 1080,
            elements: values
                .iter()
                .enumerate()
                .map(
                    |(idx, confidence)| oneshim_core::models::ui_scene::UiSceneElement {
                        element_id: format!("el-{idx}"),
                        bbox_abs: ElementBounds {
                            x: (idx as i32) * 10,
                            y: 10,
                            width: 100,
                            height: 30,
                        },
                        bbox_norm: oneshim_core::models::ui_scene::NormalizedBounds::new(
                            0.1, 0.1, 0.2, 0.05,
                        ),
                        label: format!("Element {idx}"),
                        role: Some("button".to_string()),
                        intent: None,
                        state: None,
                        confidence: *confidence,
                        text_masked: Some(format!("Element {idx}")),
                        parent_id: None,
                    },
                )
                .collect(),
        }
    }

    #[test]
    fn apply_scene_intelligence_filter_rejects_disabled_config() {
        let scene = sample_scene_with_confidence(&[0.9, 0.7, 0.5]);
        let cfg = SceneIntelligenceConfig {
            enabled: false,
            ..SceneIntelligenceConfig::default()
        };
        let result = apply_scene_intelligence_filter(scene, &cfg);
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
    }

    #[test]
    fn apply_scene_intelligence_filter_applies_threshold_and_limit() {
        let scene = sample_scene_with_confidence(&[0.95, 0.7, 0.61, 0.42, 0.2]);
        let cfg = SceneIntelligenceConfig {
            min_confidence: 0.6,
            max_elements: 2,
            ..SceneIntelligenceConfig::default()
        };
        let filtered = apply_scene_intelligence_filter(scene, &cfg).unwrap();
        assert_eq!(filtered.elements.len(), 2);
        assert!(filtered.elements[0].confidence >= filtered.elements[1].confidence);
        assert!(filtered.elements.iter().all(|e| e.confidence >= 0.6));
    }

    #[test]
    fn build_scene_calibration_reports_failures() {
        let scene = sample_scene_with_confidence(&[0.4, 0.5]);
        let cfg = SceneIntelligenceConfig {
            calibration_enabled: true,
            calibration_min_elements: 4,
            calibration_min_avg_confidence: 0.8,
            ..SceneIntelligenceConfig::default()
        };
        let report = build_scene_calibration(&scene, &cfg);
        assert!(!report.passed);
        assert_eq!(
            report.schema_version,
            AUTOMATION_SCENE_CALIBRATION_SCHEMA_VERSION
        );
        assert!(!report.reasons.is_empty());
    }
}
