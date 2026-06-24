use maekon_core::models::intent::{AutomationIntent, ElementBounds, IntentResult, WorkflowPreset};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    /// Per-call LLM health for the automation intent path.
    ///
    /// - `None`         — no call yet / health handle not wired (non-LocalModel arms).
    /// - `Some(true)`   — last intent LLM call succeeded.
    /// - `Some(false)`  — last intent LLM call failed; rule-matcher fallback active.
    ///
    /// This field is read at request time from a live `Arc<LlmCallHealth>` shared
    /// with the provider instance, so it always reflects the current per-call state.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub llm_healthy: Option<bool>,
    /// Intent-hint confirmation policy (SCREAMING_SNAKE_CASE serde token).
    ///
    /// Mirrors `ConfirmationRequirement` variants: `"AUTO"` / `"CONFIRM"` / `"BLOCK"`.
    /// Frontend uses this with `sandbox_enabled` / `sandbox_profile` to conditionally
    /// branch the intent-hint caption:
    /// - `"AUTO"`    → immediate-run copy that reflects actual sandbox status
    /// - `"CONFIRM"` → "Requires your confirmation before running"
    /// - `"BLOCK"`   → "Automation execution is disabled by policy"
    ///
    /// Defaults to `"AUTO"` when no config manager is wired (no-config fast path).
    #[serde(default = "default_auto_policy")]
    pub confirmation_policy: String,
}

// Used by `serde(default = ...)`, but rustc misreports it as unused.
#[allow(dead_code)]
fn default_auto_policy() -> String {
    "AUTO".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuditQuery {
    #[serde(default = "default_audit_limit")]
    pub limit: usize,
    pub status: Option<String>,
}

fn default_audit_limit() -> usize {
    50
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PolicyEventQuery {
    #[serde(default = "default_policy_event_limit")]
    pub limit: usize,
}

fn default_policy_event_limit() -> usize {
    100
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PresetListDto {
    // Cross-crate `maekon-core` type — contained as an opaque object array.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object_array")
    )]
    pub presets: Vec<WorkflowPreset>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExecuteIntentHintRequest {
    pub command_id: Option<String>,
    pub session_id: String,
    pub intent_hint: String,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExecuteIntentHintResponse {
    pub command_id: String,
    pub session_id: String,
    // Cross-crate `maekon-core` types — contained as opaque objects.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub planned_intent: AutomationIntent,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub result: IntentResult,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum SceneActionType {
    Click,
    TypeText,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ExecuteSceneActionRequest {
    pub command_id: Option<String>,
    pub session_id: String,
    pub frame_id: Option<i64>,
    pub scene_id: Option<String>,
    pub element_id: String,
    pub action_type: SceneActionType,
    // Cross-crate `maekon-core` type — contained as an opaque object.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub bbox_abs: ElementBounds,
    pub role: Option<String>,
    pub label: Option<String>,
    pub text: Option<String>,
    pub allow_sensitive_input: Option<bool>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
    // Cross-crate `maekon-core` types — contained as opaque objects.
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object_array")
    )]
    pub executed_intents: Vec<AutomationIntent>,
    #[cfg_attr(
        feature = "schema",
        schemars(schema_with = "crate::schema_support::opaque_object")
    )]
    pub result: IntentResult,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AutomationContractsDto {
    pub audit_schema_version: String,
    pub scene_schema_version: String,
    pub scene_action_schema_version: String,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SceneQuery {
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub frame_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct SceneCalibrationQuery {
    pub app_name: Option<String>,
    pub screen_id: Option<String>,
    pub frame_id: Option<i64>,
}

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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

// ── #5734 serde tests for AutomationStatusDto.llm_healthy ────────────────────
#[cfg(test)]
mod tests {
    use super::AutomationStatusDto;

    fn base_dto(llm_healthy: Option<bool>) -> AutomationStatusDto {
        AutomationStatusDto {
            enabled: true,
            sandbox_enabled: false,
            sandbox_profile: "none".to_string(),
            ocr_provider: "local".to_string(),
            llm_provider: "local".to_string(),
            ocr_source: "local".to_string(),
            llm_source: "local".to_string(),
            ocr_fallback_reason: None,
            llm_fallback_reason: None,
            external_data_policy: "off".to_string(),
            pending_audit_entries: 0,
            llm_healthy,
            confirmation_policy: "AUTO".to_string(),
        }
    }

    /// `llm_healthy: None` must be omitted from JSON (`skip_serializing_if`).
    #[test]
    fn automation_status_llm_healthy_none_is_omitted() {
        let dto = base_dto(None);
        let json = serde_json::to_string(&dto).expect("serialize");
        assert!(
            !json.contains("llm_healthy"),
            "llm_healthy=None should be absent from JSON, got: {json}"
        );
    }

    /// `llm_healthy: Some(true)` must serialize as `"llm_healthy":true`.
    #[test]
    fn automation_status_llm_healthy_some_true_serializes() {
        let dto = base_dto(Some(true));
        let json = serde_json::to_string(&dto).expect("serialize");
        assert!(
            json.contains("\"llm_healthy\":true"),
            "expected llm_healthy:true in JSON, got: {json}"
        );
    }

    /// `llm_healthy: Some(false)` must serialize as `"llm_healthy":false`.
    #[test]
    fn automation_status_llm_healthy_some_false_serializes() {
        let dto = base_dto(Some(false));
        let json = serde_json::to_string(&dto).expect("serialize");
        assert!(
            json.contains("\"llm_healthy\":false"),
            "expected llm_healthy:false in JSON, got: {json}"
        );
    }

    /// `confirmation_policy` serializes as a SCREAMING_SNAKE_CASE string.
    #[test]
    fn automation_status_confirmation_policy_serializes_screaming_case() {
        let dto = base_dto(None);
        let json = serde_json::to_string(&dto).expect("serialize");
        assert!(
            json.contains("\"confirmation_policy\":\"AUTO\""),
            "expected confirmation_policy:AUTO in JSON, got: {json}"
        );
    }

    /// All three ConfirmationRequirement variants serialize and pass through the
    /// `default_auto_policy` function correctly.
    #[test]
    fn automation_status_confirmation_policy_variants_round_trip() {
        for policy in ["AUTO", "CONFIRM", "BLOCK"] {
            let mut dto = base_dto(None);
            dto.confirmation_policy = policy.to_string();
            let json = serde_json::to_string(&dto).expect("serialize");
            assert!(
                json.contains(&format!("\"confirmation_policy\":\"{policy}\"")),
                "expected confirmation_policy:{policy} in JSON, got: {json}"
            );
        }
    }

    /// `default_auto_policy()` returns `"AUTO"` — the fail-safe for no-config paths.
    #[test]
    fn automation_status_confirmation_policy_default_is_auto() {
        assert_eq!(
            super::default_auto_policy(),
            "AUTO",
            "default_auto_policy must return AUTO"
        );
    }
}
