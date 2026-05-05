use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AiRuntimeStatus {
    pub ocr_source: String,
    pub llm_source: String,
    pub ocr_fallback_reason: Option<String>,
    pub llm_fallback_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
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
pub struct MetricsUpdate {
    pub timestamp: String,
    pub cpu_usage: f32,
    pub memory_percent: f32,
    pub memory_used: u64,
    pub memory_total: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameUpdate {
    pub id: i64,
    pub timestamp: String,
    pub app_name: String,
    pub window_title: String,
    pub importance: f32,
    pub trigger_type: String,
}

#[derive(Debug, Clone, Serialize)]
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
}
