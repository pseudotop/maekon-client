//! gRPC SSE adapter — bridges UnifiedClient suggestion streaming to the SseClient port trait.
//! Converts proto `SuggestionEvent` to core `SseEvent::Suggestion(Suggestion)`.

#[cfg(feature = "grpc")]
use std::sync::Arc;

#[cfg(feature = "grpc")]
use async_trait::async_trait;
#[cfg(feature = "grpc")]
use tracing::{debug, warn};

#[cfg(feature = "grpc")]
use oneshim_core::error::CoreError;
#[cfg(feature = "grpc")]
use oneshim_core::models::suggestion::{Priority, Suggestion, SuggestionSource, SuggestionType};
#[cfg(feature = "grpc")]
use oneshim_core::ports::api_client::{SseClient, SseEvent};

#[cfg(feature = "grpc")]
use super::unified_client::{SuggestionEvent, UnifiedClient};

/// Adapter that implements [`SseClient`] by bridging gRPC server-streaming
/// suggestions from `UnifiedClient` into the mpsc channel that
/// `SuggestionReceiver` consumes.
#[cfg(feature = "grpc")]
pub struct GrpcSseAdapter {
    unified: Arc<UnifiedClient>,
}

#[cfg(feature = "grpc")]
impl GrpcSseAdapter {
    pub fn new(unified: Arc<UnifiedClient>) -> Self {
        Self { unified }
    }
}

#[cfg(feature = "grpc")]
#[async_trait]
impl SseClient for GrpcSseAdapter {
    async fn connect(
        &self,
        session_id: &str,
        tx: tokio::sync::mpsc::Sender<SseEvent>,
    ) -> Result<(), CoreError> {
        debug!(
            session_id,
            "GrpcSseAdapter: starting gRPC suggestion stream"
        );
        let mut stream = self.unified.subscribe_suggestions(session_id).await?;
        let session_id_owned = session_id.to_string();

        tokio::spawn(async move {
            // Emit Connected event for behavioral parity with SseStreamClient.
            let _ = tx
                .send(SseEvent::Connected {
                    session_id: session_id_owned,
                })
                .await;
            loop {
                match stream.message().await {
                    Ok(Some(event)) => {
                        let sse_event = convert_suggestion_event(event);
                        if tx.send(sse_event).await.is_err() {
                            debug!("GrpcSseAdapter: receiver dropped, stopping stream");
                            break;
                        }
                    }
                    Ok(None) => {
                        debug!("GrpcSseAdapter: stream ended by server");
                        let _ = tx.send(SseEvent::Close).await;
                        break;
                    }
                    Err(e) => {
                        warn!("GrpcSseAdapter: stream error: {e}");
                        let _ = tx.send(SseEvent::Error(e.to_string())).await;
                        break;
                    }
                }
            }
        });

        Ok(())
    }
}

/// Convert a proto `SuggestionEvent` to a core `SseEvent::Suggestion`.
#[cfg(feature = "grpc")]
fn convert_suggestion_event(event: SuggestionEvent) -> SseEvent {
    let priority = match event.priority {
        1 => Priority::Low,
        2 => Priority::Medium,
        3 => Priority::High,
        4 => Priority::Critical,
        _ => Priority::Medium, // 0 (Unspecified) and unknown
    };

    let suggestion_type = match event.category.as_str() {
        "WORK_GUIDANCE" => SuggestionType::WorkGuidance,
        "EMAIL_DRAFT" => SuggestionType::EmailDraft,
        "PRODUCTIVITY_TIP" => SuggestionType::ProductivityTip,
        "WORKFLOW_OPTIMIZATION" => SuggestionType::WorkflowOptimization,
        _ => SuggestionType::ContextBased,
    };

    let suggestion = Suggestion {
        suggestion_id: event.suggestion_id,
        suggestion_type,
        content: event.content,
        priority,
        confidence_score: event.confidence_score,
        relevance_score: event.confidence_score, // mirror confidence
        is_actionable: true,
        created_at: chrono::Utc::now(),
        expires_at: None,
        source: SuggestionSource::LlmServer,
        reasoning: None,
    };

    SseEvent::Suggestion(suggestion)
}

#[cfg(all(test, feature = "grpc"))]
mod tests {
    use super::*;

    #[test]
    fn convert_priority_mapping() {
        let make = |p: i32| SuggestionEvent {
            suggestion_id: "s1".into(),
            content: "test".into(),
            priority: p,
            confidence_score: 0.8,
            category: String::new(),
        };

        let check = |p: i32, expected: Priority| {
            if let SseEvent::Suggestion(s) = convert_suggestion_event(make(p)) {
                assert_eq!(s.priority, expected, "proto priority {p}");
            } else {
                panic!("expected Suggestion variant");
            }
        };

        check(0, Priority::Medium); // Unspecified
        check(1, Priority::Low);
        check(2, Priority::Medium);
        check(3, Priority::High);
        check(4, Priority::Critical);
        check(99, Priority::Medium); // unknown
    }

    #[test]
    fn convert_category_mapping() {
        let make = |cat: &str| SuggestionEvent {
            suggestion_id: "s1".into(),
            content: "test".into(),
            priority: 2,
            confidence_score: 0.9,
            category: cat.into(),
        };

        let check = |cat: &str, expected: SuggestionType| {
            if let SseEvent::Suggestion(s) = convert_suggestion_event(make(cat)) {
                assert_eq!(s.suggestion_type, expected, "category '{cat}'");
            }
        };

        check("WORK_GUIDANCE", SuggestionType::WorkGuidance);
        check("EMAIL_DRAFT", SuggestionType::EmailDraft);
        check("PRODUCTIVITY_TIP", SuggestionType::ProductivityTip);
        check(
            "WORKFLOW_OPTIMIZATION",
            SuggestionType::WorkflowOptimization,
        );
        check("CONTEXT_BASED", SuggestionType::ContextBased);
        check("", SuggestionType::ContextBased);
        check("UNKNOWN", SuggestionType::ContextBased);
    }

    #[test]
    fn convert_defaults_for_missing_fields() {
        let event = SuggestionEvent {
            suggestion_id: "test-123".into(),
            content: "Do this thing".into(),
            priority: 3,
            confidence_score: 0.75,
            category: "WORK_GUIDANCE".into(),
        };

        if let SseEvent::Suggestion(s) = convert_suggestion_event(event) {
            assert_eq!(s.relevance_score, 0.75); // mirrors confidence
            assert!(s.is_actionable);
            assert_eq!(s.source, SuggestionSource::LlmServer);
            assert!(s.reasoning.is_none());
            assert!(s.expires_at.is_none());
            assert!(s.created_at <= chrono::Utc::now());
            assert_eq!(s.suggestion_id, "test-123");
            assert_eq!(s.content, "Do this thing");
        } else {
            panic!("expected Suggestion variant");
        }
    }

    #[test]
    fn adapter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<GrpcSseAdapter>();
    }
}
