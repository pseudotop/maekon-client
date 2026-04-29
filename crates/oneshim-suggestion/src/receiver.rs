use oneshim_core::models::suggestion::Suggestion;
use oneshim_core::ports::api_client::{SseClient, SseEvent};
use oneshim_core::ports::notifier::DesktopNotifier;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::error::SuggestionError;
use crate::queue::SuggestionQueue;
use crate::scorer::FeedbackScorer;

/// Callback type invoked when a new suggestion is accepted into the queue.
/// Parameter is the current queue size after the push.
pub type OnNewSuggestion = Arc<dyn Fn(usize) + Send + Sync>;

pub struct SuggestionReceiver {
    sse_client: Arc<dyn SseClient>,
    notifier: Option<Arc<dyn DesktopNotifier>>,
    queue: Arc<Mutex<SuggestionQueue>>,
    scorer: Arc<Mutex<FeedbackScorer>>,
    on_new: Mutex<Option<OnNewSuggestion>>,
}

impl SuggestionReceiver {
    pub fn new(
        sse_client: Arc<dyn SseClient>,
        notifier: Option<Arc<dyn DesktopNotifier>>,
        queue: Arc<Mutex<SuggestionQueue>>,
        scorer: Arc<Mutex<FeedbackScorer>>,
    ) -> Self {
        Self {
            sse_client,
            notifier,
            queue,
            scorer,
            on_new: Mutex::new(None),
        }
    }

    /// Set the on-new callback after construction.
    /// Called when the overlay handle becomes available.
    pub async fn set_on_new(&self, callback: OnNewSuggestion) {
        *self.on_new.lock().await = Some(callback);
    }

    pub async fn run(&self, session_id: &str) -> Result<(), SuggestionError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<SseEvent>(64);

        let sse = self.sse_client.clone();
        let sid = session_id.to_string();
        tokio::spawn(async move {
            if let Err(e) = sse.connect(&sid, tx).await {
                error!("SSE connection error: {e}");
            }
        });

        info!("suggestion received waiting started");

        while let Some(event) = rx.recv().await {
            match event {
                SseEvent::Connected { session_id } => {
                    info!("SSE connection success: {session_id}");
                }
                SseEvent::Suggestion(suggestion) => {
                    debug!(
                        "suggestion received: {} ({:?})",
                        suggestion.suggestion_id, suggestion.priority
                    );
                    self.handle_suggestion(suggestion).await;
                }
                SseEvent::Update(data) => {
                    debug!("update received: {data}");
                }
                SseEvent::Heartbeat { timestamp } => {
                    debug!("heartbeat: {timestamp}");
                }
                SseEvent::Error(msg) => {
                    warn!("SSE error: {msg}");
                }
                SseEvent::Close => {
                    info!("SSE connection ended");
                    break;
                }
            }
        }

        Ok(())
    }

    // P2 PR-A: the queue lock is held across an intentional "expiry + dedup
    // + push" atomicity window — these must be one lock window to prevent
    // races where the queue is pushed to twice with stale expiry state.
    #[allow(clippy::significant_drop_tightening)]
    async fn handle_suggestion(&self, mut suggestion: Suggestion) {
        // 1. Feedback-based relevance adjustment
        let should_queue = {
            let scorer = self.scorer.lock().await;
            scorer.adjust(
                &suggestion.suggestion_type,
                &suggestion.source,
                &mut suggestion.relevance_score,
            )
        };
        if !should_queue {
            debug!(
                id = %suggestion.suggestion_id,
                relevance = suggestion.relevance_score,
                "suggestion suppressed — relevance below threshold"
            );
            return;
        }

        // 2. Opportunistic expiry + dedup + push (single queue lock)
        let (accepted, queue_count) = {
            let mut queue = self.queue.lock().await;
            let expired_count = queue.remove_expired();
            if expired_count > 0 {
                debug!(expired_count, "expired suggestions removed from queue");
            }
            let accepted = queue.push(suggestion.clone());
            let count = queue.len();
            (accepted, count)
        };

        if !accepted {
            return;
        }

        if let Some(notifier) = &self.notifier {
            if let Err(e) = notifier.show_suggestion(&suggestion).await {
                warn!("notification display failure: {e}");
            }
        }

        // Notify overlay of new suggestion (badge count update)
        if let Some(on_new) = self.on_new.lock().await.as_ref() {
            on_new(queue_count);
        }
    }

    pub async fn queue_size(&self) -> usize {
        self.queue.lock().await.len()
    }

    pub async fn peek_top(&self) -> Option<Suggestion> {
        self.queue.lock().await.peek().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::error::CoreError;
    use oneshim_core::models::suggestion::{Priority, SuggestionSource, SuggestionType};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn suggestion_queue_default_size() {
        let queue = SuggestionQueue::new(50);
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }

    struct MockSseClient;
    #[async_trait::async_trait]
    impl SseClient for MockSseClient {
        async fn connect(
            &self,
            _session_id: &str,
            _tx: tokio::sync::mpsc::Sender<SseEvent>,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    struct CountingNotifier {
        count: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl DesktopNotifier for CountingNotifier {
        async fn show_suggestion(&self, _suggestion: &Suggestion) -> Result<(), CoreError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn show_notification(&self, _title: &str, _body: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn show_error(&self, _message: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn make_suggestion() -> Suggestion {
        Suggestion {
            suggestion_id: "test-1".to_string(),
            suggestion_type: SuggestionType::WorkGuidance,
            content: "Test suggestion content".to_string(),
            priority: Priority::Medium,
            confidence_score: 0.8,
            relevance_score: 0.9,
            is_actionable: true,
            created_at: chrono::Utc::now(),
            expires_at: None,
            source: SuggestionSource::RuleBased,
            reasoning: None,
        }
    }

    #[tokio::test]
    async fn handle_suggestion_calls_notifier() {
        let notifier = Arc::new(CountingNotifier {
            count: AtomicUsize::new(0),
        });
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            Some(notifier.clone() as Arc<dyn DesktopNotifier>),
            queue.clone(),
            scorer,
        );

        receiver.handle_suggestion(make_suggestion()).await;

        assert_eq!(notifier.count.load(Ordering::SeqCst), 1);
        assert_eq!(queue.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn handle_suggestion_works_without_notifier() {
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            None,
            queue.clone(),
            scorer,
        );

        receiver.handle_suggestion(make_suggestion()).await;

        assert_eq!(queue.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn handle_suggestion_runs_expiry_before_push() {
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            None,
            queue.clone(),
            scorer,
        );

        {
            let mut q = queue.lock().await;
            let mut expired = make_suggestion();
            expired.suggestion_id = "expired-1".to_string();
            expired.content = "expired content".to_string();
            expired.expires_at = Some(chrono::Utc::now() - chrono::Duration::hours(1));
            q.push(expired);
            assert_eq!(q.len(), 1);
        }

        receiver.handle_suggestion(make_suggestion()).await;

        let q = queue.lock().await;
        assert_eq!(q.len(), 1);
        assert_eq!(q.peek().unwrap().suggestion_id, "test-1");
    }

    #[tokio::test]
    async fn handle_suggestion_skips_duplicate() {
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            None,
            queue.clone(),
            scorer,
        );

        receiver.handle_suggestion(make_suggestion()).await;
        receiver.handle_suggestion(make_suggestion()).await;

        assert_eq!(queue.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn handle_suggestion_fires_on_new_callback() {
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let notified_count = Arc::new(AtomicUsize::new(0));
        let count_clone = notified_count.clone();

        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            None,
            queue.clone(),
            scorer,
        );

        // Wire callback after construction (matches production pattern)
        receiver
            .set_on_new(Arc::new(move |count| {
                count_clone.store(count, Ordering::SeqCst);
            }))
            .await;

        receiver.handle_suggestion(make_suggestion()).await;

        assert_eq!(notified_count.load(Ordering::SeqCst), 1);
        assert_eq!(queue.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn handle_suggestion_suppresses_low_relevance() {
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));

        {
            let mut s = scorer.lock().await;
            for _ in 0..10 {
                s.record(
                    SuggestionType::WorkGuidance,
                    SuggestionSource::RuleBased,
                    &oneshim_core::models::suggestion::FeedbackType::Rejected,
                );
            }
        }

        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            None,
            queue.clone(),
            scorer,
        );

        let mut suggestion = make_suggestion();
        suggestion.relevance_score = 0.4;

        receiver.handle_suggestion(suggestion).await;

        assert_eq!(queue.lock().await.len(), 0);
    }
}
