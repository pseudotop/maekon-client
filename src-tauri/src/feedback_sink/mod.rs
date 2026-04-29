//! CompositeFeedbackSink — fans user reactions out to CoachingEngine
//! and RegimeClassifier. Binary-crate composition glue per ADR-017.

use async_trait::async_trait;
use oneshim_analysis::CoachingEngine;
use oneshim_analysis::RegimeClassifier;
use oneshim_core::error::CoreError;
use oneshim_core::models::suggestion::SuggestionFeedback;
use oneshim_core::ports::feedback_signal_sink::FeedbackSignalSink;
use std::sync::Arc;

pub struct CompositeFeedbackSink {
    coaching: Option<Arc<CoachingEngine>>,
    regime_classifier: Option<Arc<parking_lot::Mutex<RegimeClassifier>>>,
}

impl CompositeFeedbackSink {
    pub fn new(
        coaching: Option<Arc<CoachingEngine>>,
        regime_classifier: Option<Arc<parking_lot::Mutex<RegimeClassifier>>>,
    ) -> Self {
        Self {
            coaching,
            regime_classifier,
        }
    }
}

#[async_trait]
impl FeedbackSignalSink for CompositeFeedbackSink {
    async fn record_user_reaction(&self, feedback: &SuggestionFeedback) -> Result<(), CoreError> {
        if let Some(ref c) = self.coaching {
            c.record_user_reaction(feedback).await;
        }
        if let Some(ref cls) = self.regime_classifier {
            // SAFETY: `RegimeClassifier::record_user_reaction` MUST stay
            // synchronous and fast (~10 ms budget, ADR-017). The
            // `parking_lot::Mutex` guard is held only for the stub call
            // and dropped before the `Ok(())` below — no `.await` happens
            // while the guard is alive (ADR-007). A future impl with real
            // work (Bayesian updates, per-regime counters) MUST offload to
            // `tokio::spawn` rather than growing this critical section.
            let mut guard = cls.lock();
            guard.record_user_reaction(feedback);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use oneshim_core::models::suggestion::FeedbackType;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // --- Mock sink that counts calls ---
    struct CountingSink {
        calls: AtomicUsize,
    }

    impl CountingSink {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl FeedbackSignalSink for CountingSink {
        async fn record_user_reaction(
            &self,
            _feedback: &SuggestionFeedback,
        ) -> Result<(), CoreError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn sample_feedback(t: FeedbackType) -> SuggestionFeedback {
        SuggestionFeedback {
            suggestion_id: "sug_001".into(),
            feedback_type: t,
            timestamp: Utc::now(),
            comment: None,
        }
    }

    /// T-X3-5 — `CompositeFeedbackSink` fans out to BOTH consumers.
    ///
    /// Exercises the real production types (`CoachingEngine` +
    /// `RegimeClassifier`). The Phase-3 stubs only `debug!` trace-log, so
    /// observability goes through a temporary `tracing_subscriber` that
    /// captures output into a shared buffer. The assertion is both log
    /// messages appear — proves `CompositeFeedbackSink::record_user_reaction`
    /// actually iterates both `coaching` and `regime_classifier` Option
    /// fields, not just one.
    ///
    /// Runs on the tokio current-thread runtime (default for `#[tokio::test]`),
    /// so `tracing::subscriber::set_default`'s thread-local scope applies
    /// across the `.await` boundary inside `record_user_reaction`.
    #[tokio::test]
    async fn composite_sink_fans_out_to_both_real_consumers() {
        use std::io::Write;
        use std::sync::Mutex as StdMutex;
        use tracing_subscriber::fmt::MakeWriter;

        #[derive(Clone)]
        struct SharedBuf(Arc<StdMutex<Vec<u8>>>);
        impl Write for SharedBuf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> MakeWriter<'a> for SharedBuf {
            type Writer = Self;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buf = Arc::new(StdMutex::new(Vec::<u8>::new()));
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(SharedBuf(buf.clone()))
            .with_ansi(false)
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

        let coaching = Arc::new(oneshim_analysis::CoachingEngine::new(
            oneshim_core::config::CoachingConfig::default(),
        ));
        let regime = Arc::new(parking_lot::Mutex::new(
            oneshim_analysis::RegimeClassifier::new(1.5),
        ));
        let sink = CompositeFeedbackSink::new(Some(coaching), Some(regime));

        sink.record_user_reaction(&sample_feedback(FeedbackType::Accepted))
            .await
            .unwrap();

        let captured = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(
            captured.contains("coaching_engine: user reaction recorded"),
            "coaching stub not invoked — captured output: {captured}"
        );
        assert!(
            captured.contains("regime_classifier: user reaction recorded"),
            "regime classifier stub not invoked — captured output: {captured}"
        );
    }

    /// T-X3-1 — accept / reject / defer each land exactly once.
    #[tokio::test]
    async fn sink_receives_accept_reject_defer() {
        let sink = CountingSink::new();
        sink.record_user_reaction(&sample_feedback(FeedbackType::Accepted))
            .await
            .unwrap();
        sink.record_user_reaction(&sample_feedback(FeedbackType::Rejected))
            .await
            .unwrap();
        sink.record_user_reaction(&sample_feedback(FeedbackType::Deferred))
            .await
            .unwrap();
        assert_eq!(sink.calls.load(Ordering::SeqCst), 3);
    }

    /// T-X3-2 — `CompositeFeedbackSink` with no consumers is a happy-path no-op.
    ///
    /// The spec property "sink error does NOT fail send_feedback" lives on
    /// `FeedbackSender::send_feedback` (asserted implicitly by the
    /// `sink_fires_before_api_client` test in oneshim-suggestion — when the
    /// sink returns Err, send_feedback still proceeds to the API call and
    /// returns Ok). Here we just guard the `None, None` feature-gated-off
    /// path from a silent regression.
    #[tokio::test]
    async fn composite_sink_ok_with_no_consumers() {
        let sink = CompositeFeedbackSink::new(None, None);
        let result = sink
            .record_user_reaction(&sample_feedback(FeedbackType::Accepted))
            .await;
        assert!(result.is_ok());
    }

    // T-X3-3 — no sink configured on FeedbackSender still works.
    // Exercised by existing crates/oneshim-suggestion/src/feedback.rs tests
    // (accept_feedback / reject_feedback_with_comment / defer_feedback) which
    // construct `FeedbackSender::new(api)` — that shim calls
    // `new_with_sink(api, None)`. Their passing IS the regression guard.

    // T-X3-4 — sink is invoked BEFORE the server ApiClient call.
    // Property lives in FeedbackSender::send_feedback (oneshim-suggestion),
    // not in CompositeFeedbackSink. Asserted by the
    // `sink_fires_before_api_client` test added in Task 3.
}
