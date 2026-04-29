use chrono::Utc;
use oneshim_core::models::suggestion::{FeedbackType, SuggestionFeedback};
use oneshim_core::ports::api_client::ApiClient;
use oneshim_core::ports::feedback_signal_sink::FeedbackSignalSink;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::error::SuggestionError;

pub struct FeedbackSender {
    api_client: Arc<dyn ApiClient>,
    sink: Option<Arc<dyn FeedbackSignalSink>>,
}

impl FeedbackSender {
    /// Preserve the pre-Phase-3 signature. New call sites should prefer
    /// `new_with_sink` and pass a real sink when available.
    pub fn new(api_client: Arc<dyn ApiClient>) -> Self {
        Self::new_with_sink(api_client, None)
    }

    pub fn new_with_sink(
        api_client: Arc<dyn ApiClient>,
        sink: Option<Arc<dyn FeedbackSignalSink>>,
    ) -> Self {
        Self { api_client, sink }
    }

    pub async fn accept(
        &self,
        suggestion_id: &str,
        comment: Option<String>,
    ) -> Result<(), SuggestionError> {
        self.send_feedback(suggestion_id, FeedbackType::Accepted, comment)
            .await
    }

    pub async fn reject(
        &self,
        suggestion_id: &str,
        comment: Option<String>,
    ) -> Result<(), SuggestionError> {
        self.send_feedback(suggestion_id, FeedbackType::Rejected, comment)
            .await
    }

    pub async fn defer(
        &self,
        suggestion_id: &str,
        comment: Option<String>,
    ) -> Result<(), SuggestionError> {
        self.send_feedback(suggestion_id, FeedbackType::Deferred, comment)
            .await
    }

    async fn send_feedback(
        &self,
        suggestion_id: &str,
        feedback_type: FeedbackType,
        comment: Option<String>,
    ) -> Result<(), SuggestionError> {
        let feedback = SuggestionFeedback {
            suggestion_id: suggestion_id.to_string(),
            feedback_type: feedback_type.clone(),
            timestamp: Utc::now(),
            comment,
        };

        // Fire-and-forget into the local sink BEFORE the server call.
        // See ADR-017 for failure + latency rules.
        //
        // TODO(Phase 4/5 learning-impl, tracked in ADR-017 addendum):
        // on server failure the scheduler enqueues into
        // `FeedbackRetryQueue` and re-invokes `send_feedback` per retry
        // tick. Current stubs are `debug!`-only and idempotent. Future
        // learning impls (Bayesian trigger priors, per-regime
        // acceptance counters) MUST either dedupe by `suggestion_id`
        // or move the sink call up into the IPC command layer so the
        // signal fires exactly once per user action.
        if let Some(ref sink) = self.sink {
            if let Err(e) = sink.record_user_reaction(&feedback).await {
                tracing::warn!(
                    error = %e,
                    "feedback sink returned Err — programmer-bug path, not a transient failure"
                );
            }
        }

        debug!("feedback sent: {suggestion_id} -> {feedback_type:?}");

        match self.api_client.send_feedback(&feedback).await {
            Ok(()) => {
                debug!("feedback sent success");
                Ok(())
            }
            Err(e) => {
                warn!("feedback sent failure: {e}");
                Err(SuggestionError::Core(e))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::event::EventBatch;
    use oneshim_core::models::frame::ContextUpload;

    struct MockApiClient;

    #[async_trait::async_trait]
    impl ApiClient for MockApiClient {
        async fn create_session(
            &self,
            client_id: &str,
        ) -> Result<oneshim_core::ports::api_client::SessionCreateResponse, CoreError> {
            Ok(oneshim_core::ports::api_client::SessionCreateResponse {
                session_id: format!("sess_{client_id}"),
                user_id: "user_1".to_string(),
                client_id: client_id.to_string(),
                capabilities: vec![],
            })
        }
        async fn end_session(&self, _: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn upload_batch(&self, _: &EventBatch) -> Result<(), CoreError> {
            Ok(())
        }
        async fn upload_context(&self, _: &ContextUpload) -> Result<(), CoreError> {
            Ok(())
        }
        async fn send_feedback(&self, feedback: &SuggestionFeedback) -> Result<(), CoreError> {
            assert!(!feedback.suggestion_id.is_empty());
            Ok(())
        }
        async fn send_heartbeat(&self, _: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn accept_feedback() {
        let sender = FeedbackSender::new(Arc::new(MockApiClient));
        sender.accept("sug_001", None).await.unwrap();
    }

    #[tokio::test]
    async fn reject_feedback_with_comment() {
        let sender = FeedbackSender::new(Arc::new(MockApiClient));
        sender
            .reject("sug_002", Some("관련 none".to_string()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn defer_feedback() {
        let sender = FeedbackSender::new(Arc::new(MockApiClient));
        sender.defer("sug_003", None).await.unwrap();
    }

    #[tokio::test]
    async fn sink_fires_before_api_client() {
        use async_trait::async_trait;
        use oneshim_core::ports::feedback_signal_sink::FeedbackSignalSink;
        use std::sync::Mutex;

        let timeline: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));

        // Sink records "sink" into the timeline.
        struct OrderingSink(Arc<Mutex<Vec<&'static str>>>);
        #[async_trait]
        impl FeedbackSignalSink for OrderingSink {
            async fn record_user_reaction(&self, _: &SuggestionFeedback) -> Result<(), CoreError> {
                self.0.lock().unwrap().push("sink");
                Ok(())
            }
        }

        // ApiClient records "api" into the same timeline.
        struct OrderingApi(Arc<Mutex<Vec<&'static str>>>);
        #[async_trait]
        impl ApiClient for OrderingApi {
            async fn create_session(
                &self,
                client_id: &str,
            ) -> Result<oneshim_core::ports::api_client::SessionCreateResponse, CoreError>
            {
                Ok(oneshim_core::ports::api_client::SessionCreateResponse {
                    session_id: format!("sess_{client_id}"),
                    user_id: "u".into(),
                    client_id: client_id.into(),
                    capabilities: vec![],
                })
            }
            async fn end_session(&self, _: &str) -> Result<(), CoreError> {
                Ok(())
            }
            async fn upload_batch(&self, _: &EventBatch) -> Result<(), CoreError> {
                Ok(())
            }
            async fn upload_context(&self, _: &ContextUpload) -> Result<(), CoreError> {
                Ok(())
            }
            async fn send_feedback(&self, _: &SuggestionFeedback) -> Result<(), CoreError> {
                self.0.lock().unwrap().push("api");
                Ok(())
            }
            async fn send_heartbeat(&self, _: &str) -> Result<(), CoreError> {
                Ok(())
            }
        }

        let sender = FeedbackSender::new_with_sink(
            Arc::new(OrderingApi(timeline.clone())),
            Some(Arc::new(OrderingSink(timeline.clone()))),
        );
        sender.accept("sug_ord", None).await.unwrap();

        let observed = timeline.lock().unwrap().clone();
        assert_eq!(observed, vec!["sink", "api"]);
    }
}
