use chrono::Utc;
use maekon_core::models::storage_records::EgressLedgerRecord;
use maekon_core::models::suggestion::{FeedbackType, SuggestionFeedback};
use maekon_core::ports::api_client::ApiClient;
use maekon_core::ports::egress_ledger::EgressLedgerSink;
use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
use std::sync::Arc;
use tracing::{debug, warn};

use crate::error::SuggestionError;
use crate::feedback_retry::PendingFeedback;

pub struct FeedbackSender {
    api_client: Arc<dyn ApiClient>,
    sink: Option<Arc<dyn FeedbackSignalSink>>,
    /// #6442 (F9): audit-ledger sink for recording feedback egress. `None` → not wired
    /// (tests / unconfigured builds); production wiring injects the SqliteStorage.
    egress_ledger: Option<Arc<dyn EgressLedgerSink>>,
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
        Self {
            api_client,
            sink,
            egress_ledger: None,
        }
    }

    /// #6442 (F9): inject the egress audit-ledger sink (chainable). When set, every
    /// successful feedback send records one egress-ledger row.
    pub fn with_egress_ledger(mut self, ledger: Arc<dyn EgressLedgerSink>) -> Self {
        self.egress_ledger = Some(ledger);
        self
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
                self.record_feedback_egress(suggestion_id, &feedback);
                Ok(())
            }
            Err(e) => {
                warn!("feedback sent failure: {e}");
                Err(SuggestionError::Core(e))
            }
        }
    }

    /// #6442 (F9): record the feedback egress in the audit ledger. Feedback is
    /// user-initiated — its own lawful basis, NOT telemetry-consent-gated — so we record
    /// it rather than gate the send. `record_id` is deterministic so the initial send and
    /// any retry re-send (both routed through `send_feedback`) collapse to a single audit
    /// row (the ledger de-dups on `record_id`). Best-effort: a ledger failure must never
    /// fail the feedback flow it audits.
    fn record_feedback_egress(&self, suggestion_id: &str, feedback: &SuggestionFeedback) {
        let Some(ref ledger) = self.egress_ledger else {
            return;
        };
        let byte_count = serde_json::to_vec(feedback)
            .map(|v| v.len() as i64)
            .unwrap_or(0);
        let record = EgressLedgerRecord {
            record_id: format!(
                "suggestion_feedback|{}|{:?}",
                suggestion_id, feedback.feedback_type
            ),
            event_type: "SuggestionFeedback".to_string(),
            event_id: Some(suggestion_id.to_string()),
            byte_count,
            recipient_count: 1,
            destination: "server.suggestion_feedback".to_string(),
            disposition: "uploaded".to_string(),
            consent_state: "user_initiated".to_string(),
            occurred_at: Utc::now().to_rfc3339(),
        };
        if let Err(e) = ledger.record_egress(&record) {
            warn!("feedback egress-ledger write failed (non-fatal): {e}");
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

    struct MockEgressLedger {
        records: std::sync::Mutex<Vec<EgressLedgerRecord>>,
    }

    impl EgressLedgerSink for MockEgressLedger {
        fn record_egress(&self, record: &EgressLedgerRecord) -> Result<(), CoreError> {
            self.records.lock().unwrap().push(record.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn feedback_records_egress_ledger_on_success() {
        let ledger = Arc::new(MockEgressLedger {
            records: std::sync::Mutex::new(Vec::new()),
        });
        let sender =
            FeedbackSender::new(Arc::new(MockApiClient)).with_egress_ledger(ledger.clone());
        sender
            .accept("sug_777", Some("useful".to_string()))
            .await
            .unwrap();

        let records = ledger.records.lock().unwrap();
        assert_eq!(
            records.len(),
            1,
            "a successful feedback send must record exactly one egress-ledger row"
        );
        let r = &records[0];
        // #6442 (F9): feedback egress is audited under a user-initiated lawful basis (not
        // telemetry-gated), with a deterministic record_id so retries de-dup in the store.
        assert_eq!(r.destination, "server.suggestion_feedback");
        assert_eq!(r.consent_state, "user_initiated");
        assert_eq!(r.disposition, "uploaded");
        assert_eq!(r.event_type, "SuggestionFeedback");
        assert_eq!(r.event_id.as_deref(), Some("sug_777"));
        assert_eq!(r.record_id, "suggestion_feedback|sug_777|Accepted");
        assert!(
            r.byte_count > 0,
            "byte_count should reflect the egressed payload"
        );
    }

    #[tokio::test]
    async fn feedback_without_ledger_is_noop() {
        // No ledger wired (the None branch) — the send still succeeds, no panic.
        let sender = FeedbackSender::new(Arc::new(MockApiClient));
        sender.accept("sug_888", None).await.unwrap();
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
