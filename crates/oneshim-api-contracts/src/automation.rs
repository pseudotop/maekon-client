use oneshim_core::models::intent::{AutomationIntent, ElementBounds, IntentResult, WorkflowPreset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
pub struct AutomationStatusDto {
    pub enabled: bool,
    pub sandbox_enabled: bool,
    pub sandbox_profile: String,
    pub ocr_provider: String,
    pub llm_provider: String,
    pub ocr_source: String,
    pub llm_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ocr_fallback_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_fallback_reason: Option<String>,
    pub external_data_policy: String,
    pub pending_audit_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntryDto {
    pub schema_version: String,
    pub entry_id: String,
    pub timestamp: String,
    pub session_id: String,
    pub command_id: String,
    pub action_type: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_audit_limit")]
    pub limit: usize,
    pub status: Option<String>,
}

fn default_audit_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
pub struct PolicyEventQuery {
    #[serde(default = "default_policy_event_limit")]
    pub limit: usize,
}

fn default_policy_event_limit() -> usize {
    100
}

#[derive(Debug, Serialize)]
pub struct AutomationStatsDto {
    pub total_executions: usize,
    pub successful: usize,
    pub failed: usize,
    pub denied: usize,
    pub timeout: usize,
    pub avg_elapsed_ms: f64,
    pub success_rate: f64,
    pub blocked_rate: f64,
    pub p95_elapsed_ms: f64,
    pub timing_samples: usize,
}

#[derive(Debug, Serialize)]
pub struct PoliciesDto {
    pub automation_enabled: bool,
    pub sandbox_profile: String,
    pub sandbox_enabled: bool,
    pub allow_network: bool,
    pub external_data_policy: String,
    pub scene_action_override_enabled: bool,
    pub scene_action_override_active: bool,
    pub scene_action_override_reason: Option<String>,
    pub scene_action_override_approved_by: Option<String>,
    pub scene_action_override_expires_at: Option<String>,
    pub scene_action_override_issue: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PresetListDto {
    pub presets: Vec<WorkflowPreset>,
}

#[derive(Debug, Serialize)]
pub struct PresetRunResult {
    pub preset_id: String,
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steps_executed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_steps: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_elapsed_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteIntentHintRequest {
    pub command_id: Option<String>,
    pub session_id: String,
    pub intent_hint: String,
}

#[derive(Debug, Serialize)]
pub struct ExecuteIntentHintResponse {
    pub command_id: String,
    pub session_id: String,
    pub planned_intent: AutomationIntent,
    pub result: IntentResult,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneActionType {
    Click,
    TypeText,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteSceneActionRequest {
    pub command_id: Option<String>,
    pub session_id: String,
    pub frame_id: Option<i64>,
    pub scene_id: Option<String>,
    pub element_id: String,
    pub action_type: SceneActionType,
    pub bbox_abs: ElementBounds,
    pub role: Option<String>,
    pub label: Option<String>,
    pub text: Option<String>,
    pub allow_sensitive_input: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ExecuteSceneActionResponse {
    pub schema_version: String,
    pub command_id: String,
    pub session_id: String,
    pub frame_id: Option<i64>,
    pub scene_id: Option<String>,
    pub element_id: String,
    pub applied_privacy_policy: String,
    pub scene_action_override_active: bool,
    pub scene_action_override_expires_at: Option<String>,
    pub executed_intents: Vec<AutomationIntent>,
    pub result: IntentResult,
}

#[derive(Debug, Serialize)]
pub struct AutomationContractsDto {
    pub audit_schema_version: String,
    pub scene_schema_version: String,
    pub scene_action_schema_version: String,
}

#[derive(Debug, Deserialize)]
pub struct SceneQuery {
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub frame_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SceneCalibrationQuery {
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub frame_id: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct SceneCalibrationDto {
    pub schema_version: String,
    pub scene_id: String,
    pub total_elements: usize,
    pub considered_elements: usize,
    pub avg_confidence: f64,
    pub min_confidence: f64,
    pub min_required_elements: usize,
    pub min_required_avg_confidence: f64,
    pub passed: bool,
    pub reasons: Vec<String>,
}
