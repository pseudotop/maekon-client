use maekon_storage::sqlite::SqliteStorage;
use maekon_suggestion::deferred::DeferredManager;
use maekon_suggestion::feedback::FeedbackSender;
use maekon_suggestion::feedback_retry::FeedbackRetryQueue;
use maekon_suggestion::history::SuggestionHistory;
use maekon_suggestion::queue::SuggestionQueue;
use maekon_suggestion::scorer::FeedbackScorer;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Thin wrapper providing unified access to suggestion pipeline components.
/// CRITICAL: `queue` and `history` must be the SAME Arc instances passed
/// to SuggestionReceiver, so SSE-received suggestions appear in IPC queries.
pub struct SuggestionManager {
    queue: Arc<Mutex<SuggestionQueue>>,
    history: Arc<Mutex<SuggestionHistory>>,
    feedback: Arc<FeedbackSender>,
    scorer: Arc<Mutex<FeedbackScorer>>,
    deferred: Arc<Mutex<DeferredManager>>,
    retry_queue: Arc<Mutex<FeedbackRetryQueue>>,
    storage: Arc<SqliteStorage>,
}

impl SuggestionManager {
    pub fn new(
        queue: Arc<Mutex<SuggestionQueue>>,
        history: Arc<Mutex<SuggestionHistory>>,
        feedback: Arc<FeedbackSender>,
        scorer: Arc<Mutex<FeedbackScorer>>,
        deferred: Arc<Mutex<DeferredManager>>,
        retry_queue: Arc<Mutex<FeedbackRetryQueue>>,
        storage: Arc<SqliteStorage>,
    ) -> Self {
        Self {
            queue,
            history,
            feedback,
            scorer,
            deferred,
            retry_queue,
            storage,
        }
    }

    pub fn queue(&self) -> &Arc<Mutex<SuggestionQueue>> {
        &self.queue
    }

    pub fn history(&self) -> &Arc<Mutex<SuggestionHistory>> {
        &self.history
    }

    pub fn feedback(&self) -> &Arc<FeedbackSender> {
        &self.feedback
    }

    pub fn deferred(&self) -> &Arc<Mutex<DeferredManager>> {
        &self.deferred
    }

    pub fn retry_queue(&self) -> &Arc<Mutex<FeedbackRetryQueue>> {
        &self.retry_queue
    }

    pub fn scorer(&self) -> &Arc<Mutex<FeedbackScorer>> {
        &self.scorer
    }

    pub fn storage(&self) -> &Arc<SqliteStorage> {
        &self.storage
    }
}
