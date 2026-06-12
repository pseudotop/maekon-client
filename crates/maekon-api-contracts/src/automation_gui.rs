use maekon_core::models::gui::{
    GuiActionRequest, GuiBenchmarkDecision, GuiExecutionTicket, GuiInteractionSession,
    GuiReadinessSnapshot,
};
use maekon_core::models::intent::IntentResult;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiSessionPath {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiCreateSessionRequest {
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub min_confidence: Option<f64>,
    pub max_candidates: Option<usize>,
    pub session_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiHighlightRequest {
    pub candidate_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiConfirmRequest {
    pub candidate_id: String,
    // Cross-crate `maekon-core` type — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub action: GuiActionRequest,
    pub ticket_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiExecutionRequest {
    // Cross-crate `maekon-core` type — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub ticket: GuiExecutionTicket,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiCreateSessionResponse {
    pub schema_version: String,
    // Cross-crate `maekon-core` type — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub session: GuiInteractionSession,
    pub capability_token: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiSessionResponse {
    pub schema_version: String,
    // Cross-crate `maekon-core` type — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub session: GuiInteractionSession,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiConfirmResponse {
    pub schema_version: String,
    // Cross-crate `maekon-core` type — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub ticket: GuiExecutionTicket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiExecutionOutcome {
    // Cross-crate `maekon-core` type — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub session: GuiInteractionSession,
    pub succeeded: bool,
    pub detail: Option<String>,
    pub steps_completed: usize,
    pub total_steps: usize,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiExecuteResponse {
    pub schema_version: String,
    pub command_id: String,
    // Cross-crate `maekon-core` types — contained as opaque objects.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub ticket: GuiExecutionTicket,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub result: IntentResult,
    pub outcome: GuiExecutionOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct GuiReadinessResponse {
    pub schema_version: String,
    // Cross-crate `maekon-core` types — contained as opaque objects.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub benchmark_decision: GuiBenchmarkDecision,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub snapshot: GuiReadinessSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::gui::{
        GuiActionType, GuiBenchmarkDecision, GuiCapabilityMatrix, GuiCapabilityState,
        GuiExecutionVerificationMode, GuiInputExecutionMode, GuiInputExecutionModeReason,
        GuiReadinessPlatform, GuiReadinessSnapshot, GUI_READINESS_SCHEMA_VERSION,
    };

    #[test]
    fn create_session_request_deserializes_minimal() {
        let json = r#"{}"#;
        let req: GuiCreateSessionRequest = serde_json::from_str(json).unwrap();
        assert!(req.app_name.is_none());
        assert!(req.min_confidence.is_none());
        assert!(req.max_candidates.is_none());
        assert!(req.session_ttl_secs.is_none());
    }

    #[test]
    fn create_session_request_deserializes_full() {
        let json = r#"{
            "app_name": "VSCode",
            "screen_id": "main",
            "min_confidence": 0.7,
            "max_candidates": 5,
            "session_ttl_secs": 120
        }"#;
        let req: GuiCreateSessionRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.app_name.as_deref(), Some("VSCode"));
        assert_eq!(req.screen_id.as_deref(), Some("main"));
        assert_eq!(req.min_confidence, Some(0.7));
        assert_eq!(req.max_candidates, Some(5));
        assert_eq!(req.session_ttl_secs, Some(120));
    }

    #[test]
    fn highlight_request_deserializes_without_ids() {
        let json = r#"{}"#;
        let req: GuiHighlightRequest = serde_json::from_str(json).unwrap();
        assert!(req.candidate_ids.is_none());
    }

    #[test]
    fn highlight_request_deserializes_with_ids() {
        let json = r#"{"candidate_ids": ["el-1", "el-2"]}"#;
        let req: GuiHighlightRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.candidate_ids.unwrap().len(), 2);
    }

    #[test]
    fn confirm_request_deserializes() {
        let json = r#"{
            "candidate_id": "el-1",
            "action": {"action_type": "click"},
            "ticket_ttl_secs": 30
        }"#;
        let req: GuiConfirmRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.candidate_id, "el-1");
        assert_eq!(req.action.action_type, GuiActionType::Click);
        assert_eq!(req.ticket_ttl_secs, Some(30));
    }

    #[test]
    fn confirm_request_type_text_with_text() {
        let json = r#"{
            "candidate_id": "el-1",
            "action": {"action_type": "type_text", "text": "hello"}
        }"#;
        let req: GuiConfirmRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.action.action_type, GuiActionType::TypeText);
        assert_eq!(req.action.text.as_deref(), Some("hello"));
    }

    #[test]
    fn session_path_deserializes() {
        let json = r#"{"id": "session-abc"}"#;
        let path: GuiSessionPath = serde_json::from_str(json).unwrap();
        assert_eq!(path.id, "session-abc");
    }

    #[test]
    fn execution_outcome_serde_roundtrip() {
        let json = r#"{
            "session": {
                "session_id": "s1",
                "state": "executed",
                "scene": {
                    "scene_id": "sc1",
                    "captured_at": "2026-01-01T00:00:00Z",
                    "screen_width": 1920,
                    "screen_height": 1080,
                    "elements": []
                },
                "focus": {
                    "app_name": "App",
                    "window_title": "Win",
                    "pid": 100,
                    "captured_at": "2026-01-01T00:00:00Z",
                    "focus_hash": "hash"
                },
                "candidates": [],
                "created_at": "2026-01-01T00:00:00Z",
                "updated_at": "2026-01-01T00:00:00Z",
                "expires_at": "2026-01-01T01:00:00Z"
            },
            "succeeded": true,
            "detail": null,
            "steps_completed": 1,
            "total_steps": 1
        }"#;
        let outcome: GuiExecutionOutcome = serde_json::from_str(json).unwrap();
        assert!(outcome.succeeded);
        assert!(outcome.detail.is_none());
        assert_eq!(outcome.session.session_id, "s1");
        assert_eq!(outcome.steps_completed, 1);
        assert_eq!(outcome.total_steps, 1);
    }

    #[test]
    fn readiness_response_serializes_snapshot_and_decision() {
        let snapshot = GuiReadinessSnapshot {
            schema_version: GUI_READINESS_SCHEMA_VERSION.to_string(),
            platform: GuiReadinessPlatform::Windows,
            captured_at: Utc::now(),
            automation_enabled: true,
            controller_built: true,
            gui_service_configured: true,
            hmac_secret_present: true,
            input_execution_mode: GuiInputExecutionMode::DirectRealInput,
            input_execution_reason: GuiInputExecutionModeReason::DirectNativeInput,
            execution_verification_mode: GuiExecutionVerificationMode::ObservableStateChange,
            session_constraints: vec![],
            capabilities: GuiCapabilityMatrix {
                screen_visibility: GuiCapabilityState::Available,
                accessibility_extraction: GuiCapabilityState::Available,
                ocr_fallback: GuiCapabilityState::Available,
                overlay: GuiCapabilityState::Available,
                input_execution: GuiCapabilityState::Available,
                permissions: GuiCapabilityState::Available,
                sandbox_support: GuiCapabilityState::Unsupported,
                audit: GuiCapabilityState::Available,
                privacy_policy: GuiCapabilityState::Available,
            },
            diagnostics: vec![],
        };
        let response = GuiReadinessResponse {
            schema_version: GUI_READINESS_SCHEMA_VERSION.to_string(),
            benchmark_decision: snapshot.benchmark_decision(),
            snapshot,
        };

        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["schema_version"], GUI_READINESS_SCHEMA_VERSION);
        assert_eq!(value["benchmark_decision"], "run");
        assert_eq!(value["snapshot"]["platform"], "windows");
        assert_eq!(response.benchmark_decision, GuiBenchmarkDecision::Run);
    }
}
