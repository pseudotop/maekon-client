use axum::response::sse::Event;
use oneshim_api_contracts::stream::{AiRuntimeStatus, RealtimeEvent};
use std::convert::Infallible;

pub(crate) fn build_ai_runtime_status_event(
    status: AiRuntimeStatus,
) -> Option<Result<Event, Infallible>> {
    let event = RealtimeEvent::AiRuntimeStatus(status);
    let json = serde_json::to_string(&event).ok()?;
    Some(Ok(Event::default()
        .event(event_type_name(&event))
        .data(json)))
}

pub(crate) fn build_realtime_event(event: RealtimeEvent) -> Option<Result<Event, Infallible>> {
    let json = serde_json::to_string(&event).ok()?;
    Some(Ok(Event::default()
        .event(event_type_name(&event))
        .data(json)))
}

pub(crate) fn event_type_name(event: &RealtimeEvent) -> &'static str {
    match event {
        RealtimeEvent::Metrics(_) => "metrics",
        RealtimeEvent::Frame(_) => "frame",
        RealtimeEvent::Idle(_) => "idle",
        RealtimeEvent::AiRuntimeStatus(_) => "ai_runtime_status",
        RealtimeEvent::Ping => "ping",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_api_contracts::stream::{FrameUpdate, IdleUpdate, MetricsUpdate};

    #[test]
    fn serialize_metrics_event() {
        let event = RealtimeEvent::Metrics(MetricsUpdate {
            timestamp: "2024-01-30T12:00:00Z".to_string(),
            cpu_usage: 45.5,
            memory_percent: 68.2,
            memory_used: 8_000_000_000,
            memory_total: 16_000_000_000,
        });
        let json = serde_json::to_string(&event).expect("metrics event should serialize");
        assert!(json.contains("\"type\":\"metrics\""));
        assert!(json.contains("\"cpu_usage\":45.5"));
    }

    #[test]
    fn serialize_frame_event() {
        let event = RealtimeEvent::Frame(FrameUpdate {
            id: 123,
            timestamp: "2024-01-30T12:00:00Z".to_string(),
            app_name: "VS Code".to_string(),
            window_title: "main.rs".to_string(),
            importance: 0.85,
            trigger_type: "timer".to_string(),
        });
        let json = serde_json::to_string(&event).expect("frame event should serialize");
        assert!(json.contains("\"type\":\"frame\""));
        assert!(json.contains("\"app_name\":\"VS Code\""));
    }

    #[test]
    fn serialize_idle_event() {
        let event = RealtimeEvent::Idle(IdleUpdate {
            is_idle: true,
            idle_secs: 300,
        });
        let json = serde_json::to_string(&event).expect("idle event should serialize");
        assert!(json.contains("\"type\":\"idle\""));
        assert!(json.contains("\"is_idle\":true"));
    }

    #[test]
    fn serialize_ai_runtime_status_event() {
        let event = RealtimeEvent::AiRuntimeStatus(AiRuntimeStatus {
            ocr_source: "local-fallback".to_string(),
            llm_source: "remote".to_string(),
            ocr_fallback_reason: Some("`ocr_api` config is missing".to_string()),
            llm_fallback_reason: None,
        });
        let json = serde_json::to_string(&event).expect("ai runtime status should serialize");
        assert!(json.contains("\"type\":\"ai_runtime_status\""));
        assert!(json.contains("\"ocr_source\":\"local-fallback\""));
    }

    #[test]
    fn event_type_names() {
        assert_eq!(
            event_type_name(&RealtimeEvent::Metrics(MetricsUpdate {
                timestamp: String::new(),
                cpu_usage: 0.0,
                memory_percent: 0.0,
                memory_used: 0,
                memory_total: 0,
            })),
            "metrics"
        );
        assert_eq!(
            event_type_name(&RealtimeEvent::AiRuntimeStatus(AiRuntimeStatus {
                ocr_source: "remote".to_string(),
                llm_source: "remote".to_string(),
                ocr_fallback_reason: None,
                llm_fallback_reason: None,
            })),
            "ai_runtime_status"
        );
        assert_eq!(event_type_name(&RealtimeEvent::Ping), "ping");
    }
}
