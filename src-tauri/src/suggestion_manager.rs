use maekon_storage::sqlite::SqliteStorage;
use maekon_suggestion::deferred::DeferredManager;
use maekon_suggestion::feedback::FeedbackSender;
use maekon_suggestion::feedback_retry::FeedbackRetryQueue;
use maekon_suggestion::history::SuggestionHistory;
use maekon_suggestion::queue::SuggestionQueue;
use maekon_suggestion::scorer::FeedbackScorer;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

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
    /// Latest meaningful local OCR-analysis outcome. This metadata-only state
    /// lets the review surface distinguish no-candidate/throttle/policy/provider
    /// outcomes without retaining captured text or provider responses (#11737).
    latest_local_analysis: Arc<RwLock<Option<crate::local_analysis_status::LocalAnalysisStatus>>>,
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
            latest_local_analysis: Arc::default(),
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

    /// Crate-visible write seam shared by the periodic and app-switch producers.
    #[cfg(feature = "local-suggestions")]
    pub(crate) async fn record_local_analysis(
        &self,
        status: crate::local_analysis_status::LocalAnalysisStatus,
    ) {
        *self.latest_local_analysis.write().await = Some(status);
    }

    /// Crate-visible read seam used by the suggestion IPC projection.
    pub(crate) async fn latest_local_analysis(
        &self,
    ) -> Option<crate::local_analysis_status::LocalAnalysisStatus> {
        self.latest_local_analysis.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_analysis_status::{
        LocalAnalysisProducer, LocalAnalysisStatus, LocalAnalysisStatusKind,
    };
    use maekon_core::ports::api_client::ApiClient;

    #[tokio::test]
    async fn latest_local_analysis_returns_the_recorded_status() {
        let api: Arc<dyn ApiClient> = Arc::new(crate::local_api_client::LocalApiClient);
        let manager = SuggestionManager::new(
            Arc::new(Mutex::new(SuggestionQueue::new(50))),
            Arc::new(Mutex::new(SuggestionHistory::new(100))),
            Arc::new(FeedbackSender::new_with_sink(api, None)),
            Arc::new(Mutex::new(FeedbackScorer::new())),
            Arc::new(Mutex::new(DeferredManager::new(50))),
            Arc::new(Mutex::new(FeedbackRetryQueue::new(100, 5))),
            Arc::new(SqliteStorage::open_in_memory(30).expect("storage")),
        );
        assert_eq!(manager.latest_local_analysis().await, None);

        let expected = LocalAnalysisStatus::new(
            LocalAnalysisStatusKind::NoCandidate,
            "no_candidate",
            LocalAnalysisProducer::Periodic,
            0,
            7,
        );
        manager.record_local_analysis(expected.clone()).await;

        assert_eq!(manager.latest_local_analysis().await, Some(expected));
    }
}
