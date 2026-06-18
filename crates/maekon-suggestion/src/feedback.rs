use chrono::Utc;
use maekon_core::models::suggestion::{FeedbackType, SuggestionFeedback};
use maekon_core::ports::api_client::ApiClient;
use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::error::SuggestionError;
use crate::feedback_retry::PendingFeedback;

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
        self.send_feedback(suggestion_id, FeedbackType::Accepted, comment, false)
            .await
    }

    pub async fn reject(
        &self,
        suggestion_id: &str,
        comment: Option<String>,
    ) -> Result<(), SuggestionError> {
        self.send_feedback(suggestion_id, FeedbackType::Rejected, comment, false)
            .await
    }

    pub async fn defer(
        &self,
        suggestion_id: &str,
        comment: Option<String>,
    ) -> Result<(), SuggestionError> {
        self.send_feedback(suggestion_id, FeedbackType::Deferred, comment, false)
            .await
    }

    /// Re-attempt a previously failed feedback post from the retry queue.
    ///
    /// Unlike the public `accept`/`reject`/`defer` entry points this method
    /// does **not** fire the `FeedbackSignalSink`. The sink already fired on
    /// the initial user action; suppressing it here prevents double-counting
    /// in frequency/weight scoring. See issue #6004.
    pub async fn retry_attempt(&self, pending: &PendingFeedback) -> Result<(), SuggestionError> {
        self.send_feedback(
            &pending.suggestion_id,
            pending.feedback_type.clone(),
            pending.comment.clone(),
            true,
        )
        .await
    }

    /// Core send implementation.
    ///
    /// `is_retry`: when `true` the `FeedbackSignalSink` is suppressed so
    /// the signal fires exactly once per user action regardless of how many
    /// network retries are needed. See issue #6004.
    async fn send_feedback(
        &self,
        suggestion_id: &str,
        feedback_type: FeedbackType,
        comment: Option<String>,
        is_retry: bool,
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
        // Sink is suppressed on retry attempts so each user action produces
        // exactly one signal regardless of network retries (#6004).
        if !is_retry {
            if let Some(ref sink) = self.sink {
                if let Err(e) = sink.record_user_reaction(&feedback).await {
                    tracing::warn!(
                        error = %e,
                        "feedback sink returned Err — programmer-bug path, not a transient failure"
                    );
                }
            }
        }

        debug!("feedback send: {suggestion_id} -> {feedback_type:?} (is_retry={is_retry})");

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
    use maekon_core::error::CoreError;
    use maekon_core::models::event::EventBatch;

    struct MockApiClient;

    #[async_trait::async_trait]
    impl ApiClient for MockApiClient {
        async fn create_session(
            &self,
            client_id: &str,
        ) -> Result<maekon_core::ports::api_client::SessionCreateResponse, CoreError> {
            Ok(maekon_core::ports::api_client::SessionCreateResponse {
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
            .reject("sug_002", Some("not relevant".to_string()))
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
        use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
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
            ) -> Result<maekon_core::ports::api_client::SessionCreateResponse, CoreError>
            {
                Ok(maekon_core::ports::api_client::SessionCreateResponse {
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

    /// Sink fires exactly once for the initial attempt; a simulated retry via
    /// `retry_attempt` must NOT re-fire the sink. Regression guard for #6004.
    #[tokio::test]
    async fn sink_fires_once_not_on_retry() {
        use async_trait::async_trait;
        use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let sink_count = Arc::new(AtomicUsize::new(0));

        struct CountingSink(Arc<AtomicUsize>);
        #[async_trait]
        impl FeedbackSignalSink for CountingSink {
            async fn record_user_reaction(&self, _: &SuggestionFeedback) -> Result<(), CoreError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        // ApiClient that always fails so the caller would retry.
        struct AlwaysFailApi;
        #[async_trait::async_trait]
        impl ApiClient for AlwaysFailApi {
            async fn create_session(
                &self,
                client_id: &str,
            ) -> Result<maekon_core::ports::api_client::SessionCreateResponse, CoreError>
            {
                Ok(maekon_core::ports::api_client::SessionCreateResponse {
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
            async fn send_feedback(&self, _: &SuggestionFeedback) -> Result<(), CoreError> {
                Err(CoreError::Network {
                    message: "simulated network error".into(),
                    code: maekon_core::error_codes::NetworkCode::Generic,
                })
            }
            async fn send_heartbeat(&self, _: &str) -> Result<(), CoreError> {
                Ok(())
            }
        }

        let sender = FeedbackSender::new_with_sink(
            Arc::new(AlwaysFailApi),
            Some(Arc::new(CountingSink(sink_count.clone()))),
        );

        // Initial attempt: network fails, but sink must have fired once.
        let _ = sender.accept("sug_retry_test", None).await;
        assert_eq!(
            sink_count.load(Ordering::SeqCst),
            1,
            "sink must fire exactly once on the initial attempt"
        );

        // Simulate scheduler retry: sink must NOT fire again.
        let pending = PendingFeedback {
            suggestion_id: "sug_retry_test".to_string(),
            feedback_type: FeedbackType::Accepted,
            comment: None,
            attempts: 1,
            next_retry_at: chrono::Utc::now(),
        };
        let _ = sender.retry_attempt(&pending).await;
        assert_eq!(
            sink_count.load(Ordering::SeqCst),
            1,
            "sink must NOT fire on retry — double-count regression (#6004)"
        );
    }
}
