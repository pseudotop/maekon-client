use maekon_core::models::suggestion::Suggestion;
use maekon_core::ports::api_client::{SseClient, SseEvent};
use maekon_core::ports::notifier::DesktopNotifier;
use maekon_core::ports::storage::StorageService;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;
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
    /// #10112: local persistence for server-pushed suggestions.
    ///
    /// Without this the remote producer wrote only to the in-memory `queue`
    /// (cap 50) and every server suggestion vanished on restart, while the
    /// local producer persisted through `save_suggestion`. The local store is
    /// the client's system of record — a suggestion the server produced is
    /// still part of the user's history and must survive a restart and be
    /// readable offline.
    ///
    /// `None` disables persistence (tests, and any future embedding that has
    /// no store); the queue path still works so a missing store degrades
    /// rather than drops suggestions.
    storage: Option<Arc<dyn StorageService>>,
    on_new: Mutex<Option<OnNewSuggestion>>,
    /// Sender half of the shutdown channel. Sending (or dropping) signals the
    /// background SSE task spawned in `run` to stop. Stored so callers can
    /// invoke `shutdown()` explicitly, and so Drop signals implicitly.
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// F-RR-C24-05: JoinHandle for the background SSE task spawned in `run`.
    /// Stored as a struct field so `shutdown()` can abort the task if the
    /// oneshot signal is not enough (e.g. `run()` future was cancelled before
    /// the while-loop started). Absence means `run()` has not been called yet.
    stream_task: Mutex<Option<JoinHandle<()>>>,
}

impl SuggestionReceiver {
    /// #10112: `storage` is a required positional parameter rather than a
    /// `with_storage()` builder on purpose. The defect this fixes was a *wiring
    /// omission* — the store existed and `save_suggestion` documented itself as
    /// "the site that persists server-issued suggestions", but nothing ever
    /// called it from here. A required parameter makes the compiler force every
    /// construction site to make that choice explicitly; a builder would let the
    /// same omission recur silently.
    pub fn new(
        sse_client: Arc<dyn SseClient>,
        notifier: Option<Arc<dyn DesktopNotifier>>,
        queue: Arc<Mutex<SuggestionQueue>>,
        scorer: Arc<Mutex<FeedbackScorer>>,
        storage: Option<Arc<dyn StorageService>>,
    ) -> Self {
        Self {
            sse_client,
            notifier,
            queue,
            scorer,
            storage,
            on_new: Mutex::new(None),
            shutdown_tx: Mutex::new(None),
            stream_task: Mutex::new(None),
        }
    }

    /// Signal the background SSE task to stop and await its completion.
    ///
    /// Calling this is optional — dropping the receiver also triggers shutdown
    /// via the `oneshot` channel being closed.
    ///
    /// F-RR-C24-05: also aborts the stream_task JoinHandle if the oneshot
    /// signal is not sufficient (e.g. `run()` future was dropped before the
    /// event loop started). Logs a warning on abort so operators can observe
    /// unexpected early-cancellation paths.
    pub async fn shutdown(&self) {
        // 1. Signal the spawned task via the oneshot channel.
        let tx = self.shutdown_tx.lock().await.take();
        if let Some(tx) = tx {
            // send() fails only if the receiver is already gone (task finished).
            let _ = tx.send(());
        }

        // 2. F-RR-C24-05: Abort + await the JoinHandle to prevent leaking a
        //    detached task when `run()` was cancelled mid-flight.
        let handle = self.stream_task.lock().await.take();
        if let Some(h) = handle {
            if !h.is_finished() {
                warn!("stream_task still running after shutdown signal — aborting");
                h.abort();
            }
            // await to confirm the task has actually stopped (abort is async).
            let _ = h.await;
        }
    }

    /// Set the on-new callback after construction.
    /// Called when the overlay handle becomes available.
    pub async fn set_on_new(&self, callback: OnNewSuggestion) {
        *self.on_new.lock().await = Some(callback);
    }

    /// Drive the SSE/gRPC suggestion stream until it closes or errors.
    ///
    /// Returns `Ok(true)` when the stream made *meaningful progress* before
    /// terminating, and `Ok(false)` when it ended without any progress (e.g. an
    /// immediate transport failure that never even establishes the stream). The
    /// caller's reconnect loop resets its backoff only on progress; a stream
    /// that fails before making progress must escalate the retry delay so a down
    /// server is not hammered (#6130).
    ///
    /// #7080: progress is NOT limited to delivered suggestions. A successful
    /// connection establishment (`Connected`) or a liveness `Heartbeat` also
    /// counts as progress, because a healthy-but-quiet server (idle user,
    /// nothing to suggest) that connects, heartbeats, then closes the idle
    /// stream is NOT a failure. Counting only suggestions let such a stream
    /// return `Ok(false)` repeatedly until the loop hit its give-up bound and
    /// permanently stopped suggestions for the session. Connection establishment
    /// distinguishes a healthy transport from a genuine outage (where
    /// `connect`/`subscribe` fails before any event is emitted), so the give-up
    /// budget now escalates only on real outages.
    pub async fn run(&self, session_id: &str) -> Result<bool, SuggestionError> {
        let (event_tx, mut rx) = tokio::sync::mpsc::channel::<SseEvent>(64);
        let (stop_tx, mut stop_rx) = oneshot::channel::<()>();

        // Store the stop sender so shutdown() / Drop can cancel the task.
        *self.shutdown_tx.lock().await = Some(stop_tx);

        let sse = self.sse_client.clone();
        let sid = session_id.to_string();
        // F-RR-C24-05: store handle in struct field so shutdown() can abort
        // the task if run() is cancelled before the event loop drains naturally.
        *self.stream_task.lock().await = Some(tokio::spawn(async move {
            tokio::select! {
                result = sse.connect(&sid, event_tx) => {
                    if let Err(e) = result {
                        error!("SSE connection error: {e}");
                    }
                }
                _ = &mut stop_rx => {
                    debug!("SSE task received shutdown signal");
                }
            }
        }));

        // The task is not awaited here: run() processes events until the
        // channel closes (task finished or sender dropped), which is the
        // natural termination path. shutdown() covers the abort path.

        info!("suggestion received waiting started");

        // Tracks whether the stream made meaningful progress. Reported back to
        // the reconnect loop so it can reset its backoff (#6130). #7080: a
        // successful connect or a heartbeat counts as progress too, not just a
        // delivered suggestion — otherwise a healthy idle stream escalates the
        // give-up budget until suggestions stop permanently for the session.
        let mut made_progress = false;

        while let Some(event) = rx.recv().await {
            match event {
                SseEvent::Connected { session_id } => {
                    info!("SSE connection success: {session_id}");
                    // Establishing the stream proves the transport/server is
                    // healthy — count it as progress so an idle, suggestion-less
                    // stream does not look like a transport failure (#7080).
                    made_progress = true;
                }
                SseEvent::Suggestion(suggestion) => {
                    debug!(
                        "suggestion received: {} ({:?})",
                        suggestion.suggestion_id, suggestion.priority
                    );
                    made_progress = true;
                    self.handle_suggestion(suggestion).await;
                }
                SseEvent::Update(data) => {
                    debug!("update received: {data}");
                }
                SseEvent::Heartbeat { timestamp } => {
                    debug!("heartbeat: {timestamp}");
                    // A heartbeat means the stream is alive — also progress
                    // (#7080), covering transports that heartbeat without a
                    // preceding Connected event.
                    made_progress = true;
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

        Ok(made_progress)
    }

    // P2 PR-A: the queue lock is held across an intentional "expiry + dedup
    // + push" atomicity window — these must be one lock window to prevent
    // races where the queue is pushed to twice with stale expiry state.
    #[allow(clippy::significant_drop_tightening)]
    async fn handle_suggestion(&self, mut suggestion: Suggestion) {
        // 0. #10112: persist BEFORE any gating, mirroring the local producer
        //    (`spawn_analysis_*` saves every suggestion it produces and only
        //    then applies relevance gates). Suppression is a *presentation*
        //    decision — a suggestion the server sent is received data either
        //    way, and dropping it here would leave the local store a biased
        //    subset of what the user was actually offered, which also skews the
        //    FeedbackScorer history that reads back from it.
        //
        //    `save_suggestion` is INSERT OR REPLACE keyed on suggestion_id, so
        //    a redelivery after reconnect upserts instead of duplicating.
        //
        //    A storage failure is logged, never fatal: the in-memory queue path
        //    below still runs, so a wedged disk degrades persistence rather than
        //    silently swallowing live suggestions.
        if let Some(storage) = &self.storage {
            if let Err(e) = storage.save_suggestion(&suggestion).await {
                warn!(
                    err.code = %e.code(),
                    id = %suggestion.suggestion_id,
                    "server-pushed suggestion failed to persist locally: {e}"
                );
            }
        }

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
    use maekon_core::error::CoreError;
    use maekon_core::models::suggestion::{Priority, SuggestionSource, SuggestionType};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn suggestion_queue_default_size() {
        let queue = SuggestionQueue::new(50);
        assert!(queue.is_empty());
        assert_eq!(queue.len(), 0);
    }

    // #10112: pub(super) so the sibling `local_persistence_tests` module can
    // reuse the same fixture instead of keeping a second copy that could drift.
    pub(super) struct MockSseClient;
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

    pub(super) fn make_suggestion() -> Suggestion {
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
            context_scope: None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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

    /// F-RR-24: verify that calling shutdown() signals the background SSE task to stop.
    /// The test uses a mock SseClient that blocks indefinitely (simulating a live SSE
    /// stream) and confirms that shutdown() causes run() to return.
    #[tokio::test]
    async fn shutdown_cancels_sse_task() {
        use std::sync::atomic::AtomicBool;
        use tokio::time::{timeout, Duration};

        // A mock SSE client that blocks until the sender side is dropped.
        struct BlockingSseClient {
            unblocked: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl SseClient for BlockingSseClient {
            async fn connect(
                &self,
                _session_id: &str,
                _tx: tokio::sync::mpsc::Sender<SseEvent>,
            ) -> Result<(), CoreError> {
                // Block until cancelled — simulates a live stream that never
                // produces events.  We just sleep long enough for the test to
                // exercise the cancellation path.
                while !self.unblocked.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Ok(())
            }
        }

        let unblocked = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = Arc::new(SuggestionReceiver::new(
            Arc::new(BlockingSseClient {
                unblocked: unblocked.clone(),
            }) as Arc<dyn SseClient>,
            None,
            queue,
            scorer,
            None,
        ));

        let receiver_clone = receiver.clone();
        let run_handle =
            tokio::spawn(async move { receiver_clone.run("sess-shutdown-test").await });

        // Let the task start.
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Call shutdown() — this should unblock run() within the timeout.
        receiver.shutdown().await;

        // timeout returns Ok(join_result) when run_handle completes within the
        // deadline; Err(Elapsed) if it hangs.  The inner JoinHandle result must
        // also be Ok — run() must return cleanly, not panic.
        let join_result = timeout(Duration::from_millis(500), run_handle)
            .await
            .expect("run() did not return within 500 ms after shutdown()");
        join_result
            .expect("run() task must not panic after shutdown()")
            .expect("run() must return Ok after shutdown()");
    }

    /// Mock SSE client that replays a fixed script of events onto the channel
    /// and then returns (closing the stream). Used to exercise `run()`'s
    /// progress-reporting contract deterministically (#7080).
    struct ScriptedSseClient {
        events: Vec<SseEvent>,
    }
    #[async_trait::async_trait]
    impl SseClient for ScriptedSseClient {
        async fn connect(
            &self,
            _session_id: &str,
            tx: tokio::sync::mpsc::Sender<SseEvent>,
        ) -> Result<(), CoreError> {
            for ev in &self.events {
                if tx.send(ev.clone()).await.is_err() {
                    break;
                }
            }
            Ok(())
        }
    }

    fn make_receiver_with_script(events: Vec<SseEvent>) -> SuggestionReceiver {
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        SuggestionReceiver::new(
            Arc::new(ScriptedSseClient { events }) as Arc<dyn SseClient>,
            None,
            queue,
            scorer,
            None,
        )
    }

    /// #7080 (revert-provable): a stream that establishes (`Connected`) and
    /// stays alive on a heartbeat but never delivers a suggestion before a
    /// graceful close must report progress (`Ok(true)`). Pre-fix this returned
    /// `Ok(false)`, which the reconnect loop treats as a failure — eventually
    /// hitting the give-up bound and permanently stopping suggestions for a
    /// healthy-but-quiet server.
    #[tokio::test]
    async fn run_reports_progress_on_connect_and_heartbeat_without_suggestion() {
        let receiver = make_receiver_with_script(vec![
            SseEvent::Connected {
                session_id: "sess-quiet".to_string(),
            },
            SseEvent::Heartbeat {
                timestamp: chrono::Utc::now(),
            },
            SseEvent::Close,
        ]);

        let made_progress = receiver
            .run("sess-quiet")
            .await
            .expect("run() must not error on a clean connect+heartbeat+close");

        assert!(
            made_progress,
            "a connected + heartbeat stream must report progress even without a suggestion (#7080)"
        );
    }

    /// #7080: a genuine outage — a stream that only errors before establishing
    /// (no `Connected`/`Heartbeat`/`Suggestion`) — must still report NO progress
    /// (`Ok(false)`) so the reconnect loop keeps backing off and can eventually
    /// give up on a permanently unreachable server. Guards against the fix
    /// over-counting and defeating the give-up bound.
    #[tokio::test]
    async fn run_reports_no_progress_when_stream_only_errors() {
        let receiver = make_receiver_with_script(vec![
            SseEvent::Error("connect failed".to_string()),
            SseEvent::Close,
        ]);

        let made_progress = receiver
            .run("sess-down")
            .await
            .expect("run() must not error when the stream only emits an error");

        assert!(
            !made_progress,
            "a failed-before-connect stream must report no progress so backoff escalates (#7080)"
        );
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
                    &maekon_core::models::suggestion::FeedbackType::Rejected,
                );
            }
        }

        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            None,
            queue.clone(),
            scorer,
            None,
        );

        let mut suggestion = make_suggestion();
        suggestion.relevance_score = 0.4;

        receiver.handle_suggestion(suggestion).await;

        assert_eq!(queue.lock().await.len(), 0);
    }
}

/// #10112: the remote producer must persist to the local store, not just to the
/// in-memory queue.
///
/// Before this, `SuggestionReceiver` had no storage dependency at all, so every
/// server-pushed suggestion lived only in the queue (cap 50) and vanished on
/// restart — while the local producer persisted via `save_suggestion`. These
/// tests pin the asymmetry closed.
#[cfg(test)]
mod local_persistence_tests {
    use super::tests::*;
    use super::*;
    use chrono::{DateTime, Utc};
    use maekon_core::error::CoreError;
    use maekon_core::models::event::Event;
    use maekon_core::models::tiered_memory::SegmentSummary;
    use std::sync::Mutex as StdMutex;

    /// Records what reached the store. `fail` makes `save_suggestion` return an
    /// error so the degradation path can be asserted.
    struct RecordingStorage {
        saved: StdMutex<Vec<Suggestion>>,
        fail: bool,
    }

    impl RecordingStorage {
        fn new(fail: bool) -> Arc<Self> {
            Arc::new(Self {
                saved: StdMutex::new(Vec::new()),
                fail,
            })
        }

        fn saved_ids(&self) -> Vec<String> {
            self.saved
                .lock()
                .unwrap()
                .iter()
                .map(|s| s.suggestion_id.clone())
                .collect()
        }
    }

    #[async_trait::async_trait]
    impl StorageService for RecordingStorage {
        async fn save_suggestion(&self, suggestion: &Suggestion) -> Result<(), CoreError> {
            if self.fail {
                return Err(CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: "disk wedged".to_string(),
                });
            }
            self.saved.lock().unwrap().push(suggestion.clone());
            Ok(())
        }

        async fn save_event(&self, _event: &Event) -> Result<(), CoreError> {
            Ok(())
        }
        async fn get_events(
            &self,
            _from: DateTime<Utc>,
            _to: DateTime<Utc>,
            _limit: usize,
        ) -> Result<Vec<Event>, CoreError> {
            Ok(Vec::new())
        }
        async fn get_pending_events(&self, _limit: usize) -> Result<Vec<Event>, CoreError> {
            Ok(Vec::new())
        }
        async fn mark_as_sent(&self, _event_ids: &[String]) -> Result<(), CoreError> {
            Ok(())
        }
        async fn enforce_retention(&self) -> Result<usize, CoreError> {
            Ok(0)
        }
        async fn save_activity_segment(&self, _summary: &SegmentSummary) -> Result<(), CoreError> {
            Ok(())
        }
        async fn update_segment_llm_summary(
            &self,
            _segment_id: &str,
            _llm_summary: &str,
        ) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn receiver_with(
        storage: Option<Arc<dyn StorageService>>,
    ) -> (SuggestionReceiver, Arc<Mutex<SuggestionQueue>>) {
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = SuggestionReceiver::new(
            Arc::new(MockSseClient) as Arc<dyn SseClient>,
            None,
            queue.clone(),
            scorer,
            storage,
        );
        (receiver, queue)
    }

    /// The core regression: a server-pushed suggestion reaches the local store.
    #[tokio::test]
    async fn server_pushed_suggestion_is_persisted_locally() {
        let storage = RecordingStorage::new(false);
        let (receiver, queue) = receiver_with(Some(storage.clone() as Arc<dyn StorageService>));

        receiver.handle_suggestion(make_suggestion()).await;

        assert_eq!(
            storage.saved_ids(),
            vec!["test-1".to_string()],
            "the suggestion must reach local storage, not just the in-memory queue"
        );
        assert_eq!(queue.lock().await.len(), 1, "queueing must still happen");
    }

    /// Persistence happens BEFORE the relevance gate, mirroring the local
    /// producer. A suppressed suggestion was still *received*; dropping it would
    /// make the local store a biased subset of what the server actually sent.
    #[tokio::test]
    async fn suppressed_suggestion_is_still_persisted() {
        let storage = RecordingStorage::new(false);
        let (receiver, queue) = receiver_with(Some(storage.clone() as Arc<dyn StorageService>));

        let mut suggestion = make_suggestion();
        suggestion.relevance_score = 0.0;
        receiver.handle_suggestion(suggestion).await;

        assert_eq!(
            queue.lock().await.len(),
            0,
            "precondition: this suggestion is suppressed from the queue"
        );
        assert_eq!(
            storage.saved_ids(),
            vec!["test-1".to_string()],
            "suppression is a presentation decision — the receipt still persists"
        );
    }

    /// A wedged store must degrade, never swallow the suggestion: the queue path
    /// still runs so the user keeps seeing live suggestions.
    #[tokio::test]
    async fn storage_failure_does_not_drop_the_suggestion() {
        let storage = RecordingStorage::new(true);
        let (receiver, queue) = receiver_with(Some(storage.clone() as Arc<dyn StorageService>));

        receiver.handle_suggestion(make_suggestion()).await;

        assert!(
            storage.saved_ids().is_empty(),
            "precondition: the save failed"
        );
        assert_eq!(
            queue.lock().await.len(),
            1,
            "a storage failure must not cost the user a live suggestion"
        );
    }

    /// `None` storage stays functional — the queue path is independent.
    #[tokio::test]
    async fn absent_storage_still_queues() {
        let (receiver, queue) = receiver_with(None);

        receiver.handle_suggestion(make_suggestion()).await;

        assert_eq!(queue.lock().await.len(), 1);
    }
}
