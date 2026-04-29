use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::context::WindowBounds;
use crate::models::intent::ElementBounds;
use crate::models::ui_scene::{UiScene, UiSceneElement};

pub const GUI_INTERACTION_SCHEMA_VERSION: &str = "automation.gui.v2";
pub const GUI_SESSION_EVENT_SCHEMA_VERSION: &str = "automation.gui.event.v1";
pub const GUI_TICKET_SCHEMA_VERSION: &str = "automation.gui.ticket.v1";

fn default_gui_interaction_schema_version() -> String {
    GUI_INTERACTION_SCHEMA_VERSION.to_string()
}

fn default_gui_event_schema_version() -> String {
    GUI_SESSION_EVENT_SCHEMA_VERSION.to_string()
}

fn default_gui_ticket_schema_version() -> String {
    GUI_TICKET_SCHEMA_VERSION.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusSnapshot {
    pub app_name: String,
    pub window_title: String,
    pub pid: u32,
    pub bounds: Option<WindowBounds>,
    pub captured_at: DateTime<Utc>,
    pub focus_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionBinding {
    pub focus_hash: String,
    pub app_name: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FocusValidation {
    pub valid: bool,
    pub reason: Option<String>,
    pub current_focus: Option<FocusSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightTarget {
    pub candidate_id: String,
    pub bbox_abs: ElementBounds,
    pub color: String,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightRequest {
    pub session_id: String,
    pub scene_id: String,
    pub targets: Vec<HighlightTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighlightHandle {
    pub handle_id: String,
    pub rendered_at: DateTime<Utc>,
    pub target_count: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiActionType {
    Click,
    DoubleClick,
    RightClick,
    TypeText,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiActionRequest {
    pub action_type: GuiActionType,
    pub text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiCandidate {
    pub element: UiSceneElement,
    pub ranking_reason: Option<String>,
    pub eligible: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GuiSessionState {
    Proposed,
    Highlighted,
    Confirmed,
    Executing,
    Executed,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiExecutionTicket {
    #[serde(default = "default_gui_ticket_schema_version")]
    pub schema_version: String,
    pub ticket_id: String,
    pub session_id: String,
    pub scene_id: String,
    pub element_id: String,
    pub action_hash: String,
    pub focus_hash: String,
    pub issued_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiSessionEvent {
    #[serde(default = "default_gui_event_schema_version")]
    pub schema_version: String,
    pub event_type: String,
    pub session_id: String,
    pub state: GuiSessionState,
    pub emitted_at: DateTime<Utc>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiInteractionSession {
    #[serde(default = "default_gui_interaction_schema_version")]
    pub schema_version: String,
    pub session_id: String,
    pub state: GuiSessionState,
    pub scene: UiScene,
    pub focus: FocusSnapshot,
    pub candidates: Vec<GuiCandidate>,
    pub selected_element_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

// ── GUI Interaction request/response types ──
// 이전에는 oneshim-automation::gui_interaction에 있었으나, AutomationPort 추상화를 위해
// oneshim-core로 이동 (ADR-001 §7)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiCreateSessionRequest {
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub min_confidence: Option<f64>,
    pub max_candidates: Option<usize>,
    pub session_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiCreateSessionResponse {
    pub session: GuiInteractionSession,
    pub capability_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiHighlightRequest {
    pub candidate_ids: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiConfirmRequest {
    pub candidate_id: String,
    pub action: GuiActionRequest,
    pub ticket_ttl_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiExecutionRequest {
    pub ticket: GuiExecutionTicket,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiExecutionOutcome {
    pub session: GuiInteractionSession,
    pub succeeded: bool,
    pub detail: Option<String>,
    pub steps_completed: usize,
    pub total_steps: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::intent::ElementBounds;
    use crate::models::ui_scene::{NormalizedBounds, UiSceneElement};

    #[test]
    fn gui_ticket_serde_roundtrip() {
        let ticket = GuiExecutionTicket {
            schema_version: GUI_TICKET_SCHEMA_VERSION.to_string(),
            ticket_id: "ticket-1".to_string(),
            session_id: "session-1".to_string(),
            scene_id: "scene-1".to_string(),
            element_id: "el-1".to_string(),
            action_hash: "abc".to_string(),
            focus_hash: "focus".to_string(),
            issued_at: Utc::now(),
            expires_at: Utc::now(),
            nonce: "nonce".to_string(),
            signature: "sig".to_string(),
        };

        let json = serde_json::to_string(&ticket).unwrap();
        let parsed: GuiExecutionTicket = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.ticket_id, "ticket-1");
        assert_eq!(parsed.schema_version, GUI_TICKET_SCHEMA_VERSION);
    }

    #[test]
    fn gui_candidate_wraps_scene_element() {
        let candidate = GuiCandidate {
            element: UiSceneElement {
                element_id: "el-1".to_string(),
                bbox_abs: ElementBounds {
                    x: 10,
                    y: 20,
                    width: 100,
                    height: 30,
                },
                bbox_norm: NormalizedBounds::new(0.0, 0.0, 1.0, 1.0),
                label: "Save".to_string(),
                role: Some("button".to_string()),
                intent: None,
                state: None,
                confidence: 0.9,
                text_masked: Some("Save".to_string()),
                parent_id: None,
            },
            ranking_reason: Some("confidence=0.90".to_string()),
            eligible: true,
        };

        assert_eq!(candidate.element.label, "Save");
        assert!(candidate.eligible);
    }
}
