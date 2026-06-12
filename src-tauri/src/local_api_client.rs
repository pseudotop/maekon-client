//! E20-24 (#4816): no-op `ApiClient` for OSS local-suggestion builds.
//!
//! The on-device suggestion pipeline (queue → score → accept/reject/defer →
//! history) learns ENTIRELY locally: `FeedbackSender::send_feedback` fires the
//! `FeedbackSignalSink` (CoachingEngine + RegimeClassifier) BEFORE any network
//! call (see `maekon-suggestion/src/feedback.rs`). The only reason a server is
//! involved at all is the optional cross-user feedback POST.
//!
//! `FeedbackSender` nonetheless REQUIRES an `Arc<dyn ApiClient>`. Rather than
//! change that crate's port surface (a breaking change), an OSS build injects
//! this no-op: every method returns `Ok(())`, so accept/reject succeed
//! synchronously with no network and never enqueue a feedback retry. It depends
//! only on `maekon-core` (the port + models), so it compiles even under
//! `--no-default-features` where `maekon-network` is absent.
//!
//! IMPORTANT: this must NEVER grow real egress. Server transport (auth, gRPC,
//! HTTP feedback POST, SSE receive) lives behind the `server` feature; keeping
//! this a pure no-op is what preserves the OSS / Enterprise boundary
//! (no cross-user/fleet aggregation in OSS).

use async_trait::async_trait;

use maekon_core::error::CoreError;
use maekon_core::models::event::EventBatch;
use maekon_core::models::suggestion::SuggestionFeedback;
use maekon_core::ports::api_client::{ApiClient, SessionCreateResponse};

/// No-op server client used when the suggestion pipeline runs without a
/// connected server (OSS `local-suggestions` builds without `server`).
// Constructed only on the non-`server` wiring path — dead under
// `--features server` by design.
#[cfg_attr(feature = "server", allow(dead_code))]
pub struct LocalApiClient;

#[async_trait]
impl ApiClient for LocalApiClient {
    /// No server session in a local-only build. Returns a synthetic, clearly
    /// local session so any caller that does construct one keeps working
    /// offline; the suggestion pipeline itself never calls this.
    async fn create_session(&self, client_id: &str) -> Result<SessionCreateResponse, CoreError> {
        Ok(SessionCreateResponse {
            session_id: format!("local-{client_id}"),
            user_id: "local".to_string(),
            client_id: client_id.to_string(),
            capabilities: Vec::new(),
        })
    }

    async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
        Ok(())
    }

    async fn upload_batch(&self, _batch: &EventBatch) -> Result<(), CoreError> {
        Ok(())
    }

    /// The learning signal was already recorded locally by the
    /// `FeedbackSignalSink` before this call; there is no server to POST to, so
    /// this is a successful no-op. Returning `Ok(())` is load-bearing: an `Err`
    /// would make the scheduler enqueue a never-drainable feedback retry.
    async fn send_feedback(&self, _feedback: &SuggestionFeedback) -> Result<(), CoreError> {
        Ok(())
    }

    async fn send_heartbeat(&self, _session_id: &str) -> Result<(), CoreError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::suggestion::FeedbackType;

    #[tokio::test]
    async fn send_feedback_is_ok_noop() {
        // Load-bearing: accept/reject must succeed locally so no retry is ever
        // enqueued (an Err would grow the SQLite feedback-retry table unbounded).
        let client = LocalApiClient;
        let feedback = SuggestionFeedback {
            suggestion_id: "sug_local_1".to_string(),
            feedback_type: FeedbackType::Accepted,
            timestamp: Utc::now(),
            comment: None,
        };
        client
            .send_feedback(&feedback)
            .await
            .expect("local feedback is a no-op success");
    }

    #[tokio::test]
    async fn create_session_is_local() {
        let client = LocalApiClient;
        let session = client.create_session("device_a").await.unwrap();
        assert_eq!(session.session_id, "local-device_a");
        assert_eq!(session.user_id, "local");
    }

    #[tokio::test]
    async fn accept_via_feedback_sender_learns_locally_without_network() {
        // End-to-end of the OSS local-learning path (#4816): the real production
        // `FeedbackSender` wired with the no-op `LocalApiClient` + a local sink.
        // accept() must SUCCEED (no server) AND fire the sink (the on-device
        // learning signal) — proving "accept/reject learns entirely on-device".
        use maekon_core::ports::feedback_signal_sink::FeedbackSignalSink;
        use maekon_suggestion::feedback::FeedbackSender;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct CountingSink(Arc<AtomicUsize>);
        #[async_trait]
        impl FeedbackSignalSink for CountingSink {
            async fn record_user_reaction(&self, _f: &SuggestionFeedback) -> Result<(), CoreError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }

        let fired = Arc::new(AtomicUsize::new(0));
        let sender = FeedbackSender::new_with_sink(
            Arc::new(LocalApiClient),
            Some(Arc::new(CountingSink(fired.clone()))),
        );
        sender
            .accept("sug_local_e2e", None)
            .await
            .expect("local accept succeeds without a server");
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "local learning sink must fire on accept (on-device learning)"
        );
    }
}
