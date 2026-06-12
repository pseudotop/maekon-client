use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AiRuntimeStatus {
    pub ocr_source: String,
    pub llm_source: String,
    pub ocr_fallback_reason: Option<String>,
    pub llm_fallback_reason: Option<String>,
    /// Per-call LLM health observable for the automation intent path.
    ///
    /// - `None` — no LLM call has been made yet since startup, or the health
    ///   handle was not wired (non-LocalModel arms).
    /// - `Some(true)` — the most recent intent LLM call succeeded.
    /// - `Some(false)` — the most recent intent LLM call failed; the
    ///   rule-matcher fallback is currently active.
    ///
    /// **Important**: this field in the SSE `ai_runtime_status` event reflects
    /// the state at the time the SSE snapshot was emitted (build time), not the
    /// live current state.  For the live value use `GET /api/automation/status`
    /// which reads `as_option_bool()` at request time.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub llm_healthy: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "type", content = "data")]
pub enum RealtimeEvent {
    #[serde(rename = "metrics")]
    Metrics(MetricsUpdate),
    #[serde(rename = "frame")]
    Frame(FrameUpdate),
    #[serde(rename = "idle")]
    Idle(IdleUpdate),
    #[serde(rename = "ai_runtime_status")]
    AiRuntimeStatus(AiRuntimeStatus),
    #[serde(rename = "ping")]
    Ping,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct MetricsUpdate {
    pub timestamp: String,
    pub cpu_usage: f32,
    pub memory_percent: f32,
    pub memory_used: u64,
    pub memory_total: u64,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct FrameUpdate {
    pub id: i64,
    pub timestamp: String,
    pub app_name: String,
    pub window_title: String,
    pub importance: f32,
    pub trigger_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct IdleUpdate {
    pub is_idle: bool,
    pub idle_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_update_serializes_trigger_type() {
        let e = RealtimeEvent::Frame(FrameUpdate {
            id: 1,
            timestamp: "t".to_string(),
            app_name: "a".to_string(),
            window_title: "w".to_string(),
            importance: 0.5,
            trigger_type: "timer".to_string(),
        });
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains("\"trigger_type\":\"timer\""));
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["data"]["trigger_type"], "timer");
    }

    // ── #5734 serde tests for AiRuntimeStatus.llm_healthy ────────────────

    fn base_status(llm_healthy: Option<bool>) -> AiRuntimeStatus {
        AiRuntimeStatus {
            ocr_source: "local".to_string(),
            llm_source: "local".to_string(),
            ocr_fallback_reason: None,
            llm_fallback_reason: None,
            llm_healthy,
        }
    }

    /// `llm_healthy: None` must be absent from the serialized JSON.
    #[test]
    fn ai_runtime_status_llm_healthy_none_is_omitted() {
        let status = base_status(None);
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(
            !json.contains("llm_healthy"),
            "llm_healthy=None should be absent, got: {json}"
        );
    }

    /// `llm_healthy: Some(true)` must serialize as `"llm_healthy":true`.
    #[test]
    fn ai_runtime_status_llm_healthy_some_true_serializes() {
        let status = base_status(Some(true));
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(
            json.contains("\"llm_healthy\":true"),
            "expected llm_healthy:true, got: {json}"
        );
    }

    /// `llm_healthy: Some(false)` must serialize as `"llm_healthy":false`.
    #[test]
    fn ai_runtime_status_llm_healthy_some_false_serializes() {
        let status = base_status(Some(false));
        let json = serde_json::to_string(&status).expect("serialize");
        assert!(
            json.contains("\"llm_healthy\":false"),
            "expected llm_healthy:false, got: {json}"
        );
    }
}
