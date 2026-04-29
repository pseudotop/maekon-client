use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use crate::error::NetworkError;
use crossbeam::queue::SegQueue;
use oneshim_core::error::CoreError;
use oneshim_core::models::event::{Event, EventBatch};
use oneshim_core::ports::api_client::ApiClient;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, warn};

/// Maximum number of events allowed in the upload queue.
/// Prevents OOM under backpressure when the server is unreachable or slow.
pub const MAX_UPLOAD_QUEUE_SIZE: usize = 10_000;

/// Threshold ratio (80%) at which a capacity warning is emitted.
const QUEUE_PRESSURE_WARN_RATIO: f64 = 0.80;

/// Threshold ratio (90%) at which a critical error is emitted.
const QUEUE_PRESSURE_CRITICAL_RATIO: f64 = 0.90;

pub struct BatchUploader {
    api_client: Arc<dyn ApiClient>,
    queue: Arc<SegQueue<Event>>,
    queue_size: AtomicUsize,
    session_id: String,
    max_batch_size: usize,
    max_retries: u32,
    dynamic_batch: bool,
    max_queue_size: usize,
    /// Tracks whether we have already emitted a pressure warning for the
    /// current high-water-mark episode, so we don't spam logs every enqueue.
    pressure_warned: AtomicBool,
    /// Total number of batches that exhausted all retries and were requeued.
    failed_batches: AtomicUsize,
    /// Cumulative count of events dropped due to queue capacity overflow.
    total_dropped: AtomicUsize,
    /// Health flag: `true` after a successful upload, `false` after retries exhausted.
    /// Read by the health-check loop (Task 2). `None` when no caller has wired a flag.
    last_upload_ok: Option<Arc<AtomicBool>>,
    /// Circuit breaker for system-level fast-fail when server is persistently unavailable.
    circuit_breaker: CircuitBreaker,
    /// Events dropped during the current flush cycle. Reset by take_dropped_since_last().
    cycle_dropped: AtomicUsize,
    /// Suppression predicate wired by the tracking schedule (A.11).
    /// When it returns `true`, `flush()` early-returns `Ok(0)` without draining the queue.
    upload_suppressed: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl BatchUploader {
    pub fn new(
        api_client: Arc<dyn ApiClient>,
        session_id: String,
        max_batch_size: usize,
        max_retries: u32,
    ) -> Self {
        Self {
            api_client,
            queue: Arc::new(SegQueue::new()),
            queue_size: AtomicUsize::new(0),
            session_id,
            max_batch_size,
            max_retries,
            dynamic_batch: true,
            max_queue_size: MAX_UPLOAD_QUEUE_SIZE,
            pressure_warned: AtomicBool::new(false),
            failed_batches: AtomicUsize::new(0),
            total_dropped: AtomicUsize::new(0),
            last_upload_ok: None,
            circuit_breaker: CircuitBreaker::new(CircuitBreakerConfig::default()),
            cycle_dropped: AtomicUsize::new(0),
            upload_suppressed: Arc::new(|| false),
        }
    }

    /// Attach a shared health flag that is set to `true` on successful upload
    /// and `false` when all retries are exhausted.
    pub fn with_health_flag(mut self, flag: Arc<AtomicBool>) -> Self {
        self.last_upload_ok = Some(flag);
        self
    }

    pub fn with_dynamic_batch(mut self, enabled: bool) -> Self {
        self.dynamic_batch = enabled;
        self
    }

    /// Override the default max queue size. Useful for testing.
    pub fn with_max_queue_size(mut self, max_queue_size: usize) -> Self {
        self.max_queue_size = max_queue_size;
        self
    }

    /// Attach a suppression predicate that gates each `flush()` call.
    ///
    /// When the predicate returns `true`, `flush()` skips draining the queue
    /// and returns `Ok(0)`. When it returns `false`, normal flush logic runs.
    ///
    /// Used by the tracking schedule (A.12) to prevent uploads during inactive windows.
    pub fn with_suppression_predicate(mut self, pred: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.upload_suppressed = pred;
        self
    }

    /// Enqueue an event for batch upload.
    ///
    /// Note: Under heavy concurrent enqueue pressure, the queue may briefly
    /// exceed `max_queue_size` due to a benign TOCTOU race between the size
    /// check and the push. This is acceptable because the SegQueue is unbounded
    /// and the overshoot is limited to the number of concurrent callers.
    pub fn enqueue(&self, event: Event) {
        let size = self.queue_size.load(Ordering::Relaxed);

        // If at capacity, drop the oldest entry to make room (newer data is
        // more valuable for monitoring).
        if size >= self.max_queue_size {
            self.drop_oldest(1);
        }

        self.queue.push(event);
        let new_size = self.queue_size.fetch_add(1, Ordering::Relaxed) + 1;
        self.check_pressure(new_size);
        debug!("event add (lock-free), current size: {new_size}");
    }

    pub fn enqueue_many(&self, events: Vec<Event>) {
        let count = events.len();
        if count == 0 {
            return;
        }

        let size = self.queue_size.load(Ordering::Relaxed);

        // Calculate how many oldest entries we need to evict to stay within
        // capacity after inserting all new events.
        let total_after = size + count;
        if total_after > self.max_queue_size {
            let overflow = total_after - self.max_queue_size;
            self.drop_oldest(overflow);
        }

        for event in events {
            self.queue.push(event);
        }
        let new_size = self.queue_size.fetch_add(count, Ordering::Relaxed) + count;
        self.check_pressure(new_size);
        debug!("event {count}items add (lock-free), current size: {new_size}");
    }

    /// Drop `count` oldest entries from the front of the queue.
    fn drop_oldest(&self, count: usize) {
        let mut dropped = 0;
        for _ in 0..count {
            if self.queue.pop().is_some() {
                dropped += 1;
            } else {
                break;
            }
        }
        if dropped > 0 {
            self.queue_size.fetch_sub(dropped, Ordering::Relaxed);
            self.total_dropped.fetch_add(dropped, Ordering::Relaxed);
            self.cycle_dropped.fetch_add(dropped, Ordering::Relaxed);
            warn!(
                dropped,
                total_dropped = self.total_dropped.load(Ordering::Relaxed),
                max = self.max_queue_size,
                "upload queue at capacity, dropped oldest event(s)",
            );
        }
    }

    /// Emit a warning once when the queue reaches the 80% pressure threshold,
    /// or an error at the 90% critical threshold. The warning flag resets when
    /// the queue drops below the warn threshold.
    fn check_pressure(&self, current_size: usize) {
        let ratio = current_size as f64 / self.max_queue_size as f64;

        if ratio >= QUEUE_PRESSURE_CRITICAL_RATIO {
            error!(
                queue_size = current_size,
                max = self.max_queue_size,
                dropped = self.total_dropped.load(Ordering::Relaxed),
                "upload queue critical — events being dropped"
            );
            self.pressure_warned.store(true, Ordering::Relaxed);
        } else if ratio >= QUEUE_PRESSURE_WARN_RATIO {
            if !self.pressure_warned.swap(true, Ordering::Relaxed) {
                warn!(
                    "upload queue pressure: {current_size}/{max} ({pct:.0}% full)",
                    max = self.max_queue_size,
                    pct = ratio * 100.0,
                );
            }
        } else {
            self.pressure_warned.store(false, Ordering::Relaxed);
        }
    }

    fn compute_batch_size(&self, queue_len: usize) -> usize {
        if !self.dynamic_batch {
            return self.max_batch_size;
        }

        if queue_len < 10 {
            queue_len // send all when queue is small
        } else if queue_len > 50 {
            (self.max_batch_size * 2).min(queue_len) // 2x batch when queue is large
        } else {
            self.max_batch_size
        }
    }

    pub async fn flush(&self) -> Result<usize, NetworkError> {
        if (self.upload_suppressed)() {
            debug!("upload flush suppressed — tracking schedule active");
            return Ok(0);
        }

        let current_size = self.queue_size.load(Ordering::Relaxed);

        if current_size == 0 {
            return Ok(0);
        }

        // Circuit breaker fast-fail
        match self.circuit_breaker.check() {
            CircuitState::Open { .. } => {
                debug!("circuit open — fast-failing flush");
                return Err(NetworkError::CircuitOpen);
            }
            CircuitState::HalfOpen => {
                debug!("circuit half-open — probe flush");
            }
            CircuitState::Closed => {}
        }

        let batch_size = self.compute_batch_size(current_size);
        let drain_count = current_size.min(batch_size);

        let mut events = Vec::with_capacity(drain_count);
        for _ in 0..drain_count {
            if let Some(event) = self.queue.pop() {
                events.push(event);
            } else {
                break;
            }
        }

        let actual_count = events.len();
        if actual_count == 0 {
            return Ok(0);
        }

        self.queue_size.fetch_sub(actual_count, Ordering::Relaxed);

        let batch = EventBatch {
            session_id: self.session_id.clone(),
            events,
            created_at: chrono::Utc::now(),
        };

        let mut retry_delay = Duration::from_secs(1);
        for attempt in 0..=self.max_retries {
            match self.api_client.upload_batch(&batch).await {
                Ok(()) => {
                    if let Some(ref flag) = self.last_upload_ok {
                        flag.store(true, Ordering::Relaxed);
                    }
                    self.circuit_breaker.record_success();
                    debug!("batch upload success: {actual_count}items event");
                    return Ok(actual_count);
                }
                Err(e) => {
                    if attempt < self.max_retries {
                        warn!(
                            "batch upload failure (attempt {}/{}): {e}",
                            attempt + 1,
                            self.max_retries + 1
                        );
                        tokio::time::sleep(retry_delay).await;
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
                    } else {
                        error!("batch upload final failure: {e}");
                        if let Some(ref flag) = self.last_upload_ok {
                            flag.store(false, Ordering::Relaxed);
                        }
                        self.circuit_breaker.record_failure();
                        self.failed_batches.fetch_add(1, Ordering::Relaxed);
                        self.requeue_failed_events(batch.events);
                        return Err(e.into());
                    }
                }
            }
        }

        Ok(0)
    }

    fn requeue_failed_events(&self, events: Vec<Event>) {
        let count = events.len();
        let current_size = self.queue_size.load(Ordering::Relaxed);

        // Respect the queue limit when requeueing failed events.
        // Drop oldest entries if we would exceed capacity.
        let total_after = current_size + count;
        if total_after > self.max_queue_size {
            let overflow = total_after - self.max_queue_size;
            self.drop_oldest(overflow);
        }

        for event in events {
            self.queue.push(event);
        }
        self.queue_size.fetch_add(count, Ordering::Relaxed);
        warn!("failure event {count}items requeued");
    }
}

#[async_trait::async_trait]
impl oneshim_core::ports::batch_sink::BatchSink for BatchUploader {
    fn enqueue(&self, event: Event) {
        BatchUploader::enqueue(self, event);
    }

    fn enqueue_many(&self, events: Vec<Event>) {
        BatchUploader::enqueue_many(self, events);
    }

    async fn flush(&self) -> Result<usize, CoreError> {
        BatchUploader::flush(self).await.map_err(Into::into)
    }

    fn take_dropped_since_last(&self) -> usize {
        BatchUploader::take_dropped_since_last(self)
    }
}

impl BatchUploader {
    pub fn queue_size(&self) -> usize {
        self.queue_size.load(Ordering::Relaxed)
    }

    pub fn max_queue_size(&self) -> usize {
        self.max_queue_size
    }

    pub fn failed_batches(&self) -> usize {
        self.failed_batches.load(Ordering::Relaxed)
    }

    /// Cumulative number of events dropped because the queue was at capacity.
    pub fn total_dropped(&self) -> usize {
        self.total_dropped.load(Ordering::Relaxed)
    }

    /// Returns events dropped since last call and resets the counter.
    pub fn take_dropped_since_last(&self) -> usize {
        self.cycle_dropped.swap(0, Ordering::Relaxed)
    }

    pub fn stats(&self) -> BatchStats {
        let cb = self.circuit_breaker.stats();
        let qs = self.queue_size();
        BatchStats {
            queue_size: qs,
            max_batch_size: self.max_batch_size,
            max_queue_size: self.max_queue_size,
            dynamic_batch_enabled: self.dynamic_batch,
            failed_batches: self.failed_batches(),
            total_dropped: self.total_dropped(),
            circuit_state: cb.state,
            circuit_failures: cb.consecutive_failures,
            queue_pressure: qs as f64 / self.max_queue_size as f64,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BatchStats {
    pub queue_size: usize,
    pub max_batch_size: usize,
    pub max_queue_size: usize,
    pub dynamic_batch_enabled: bool,
    /// Number of batches that exhausted all retries and had their events
    /// requeued. Monotonically increasing; never resets.
    pub failed_batches: usize,
    /// Cumulative number of events dropped due to queue capacity overflow.
    /// Monotonically increasing; never resets.
    pub total_dropped: usize,
    /// Current circuit breaker state: "closed", "open", or "half_open".
    pub circuit_state: &'static str,
    /// Consecutive failure count tracked by the circuit breaker.
    pub circuit_failures: u32,
    /// Queue fill ratio (0.0–1.0).
    pub queue_pressure: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::models::event::{ContextEvent, Event};

    struct MockApiClient {
        should_fail: bool,
    }

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
        async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn upload_batch(&self, _batch: &EventBatch) -> Result<(), CoreError> {
            if self.should_fail {
                Err(CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: "mock failure".to_string(),
                })
            } else {
                Ok(())
            }
        }
        async fn upload_context(
            &self,
            _upload: &oneshim_core::models::frame::ContextUpload,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn send_feedback(
            &self,
            _feedback: &oneshim_core::models::suggestion::SuggestionFeedback,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn send_heartbeat(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn make_test_event() -> Event {
        Event::Context(ContextEvent {
            app_name: "test".to_string(),
            window_title: "Test".to_string(),
            prev_app_name: None,
            timestamp: chrono::Utc::now(),
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn enqueue_and_flush() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = BatchUploader::new(client, "sess_1".to_string(), 100, 3);

        uploader.enqueue(make_test_event());
        uploader.enqueue(make_test_event());
        assert_eq!(uploader.queue_size(), 2);

        let sent = uploader.flush().await.unwrap();
        assert_eq!(sent, 2);
        assert_eq!(uploader.queue_size(), 0);
    }

    #[tokio::test]
    async fn flush_empty_queue() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = BatchUploader::new(client, "sess_1".to_string(), 100, 3);
        let sent = uploader.flush().await.unwrap();
        assert_eq!(sent, 0);
    }

    #[tokio::test]
    async fn max_batch_size_limit() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader =
            BatchUploader::new(client, "sess_1".to_string(), 2, 3).with_dynamic_batch(false); // batch disabled
        for _ in 0..5 {
            uploader.enqueue(make_test_event());
        }
        assert_eq!(uploader.queue_size(), 5);

        let sent = uploader.flush().await.unwrap();
        assert_eq!(sent, 2);
        assert_eq!(uploader.queue_size(), 3);
    }

    #[tokio::test]
    async fn dynamic_batch_small_queue() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = BatchUploader::new(client, "sess_1".to_string(), 100, 3);

        for _ in 0..5 {
            uploader.enqueue(make_test_event());
        }

        let sent = uploader.flush().await.unwrap();
        assert_eq!(sent, 5); // all sent
    }

    #[tokio::test]
    async fn dynamic_batch_large_queue() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = BatchUploader::new(client, "sess_1".to_string(), 20, 3);

        for _ in 0..60 {
            uploader.enqueue(make_test_event());
        }

        let sent = uploader.flush().await.unwrap();
        assert_eq!(sent, 40); // 20 * 2 = 40
    }

    struct FlakeyApiClient {
        call_count: std::sync::atomic::AtomicU32,
        fail_until: u32,
    }

    #[async_trait::async_trait]
    impl ApiClient for FlakeyApiClient {
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
        async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn upload_batch(&self, _batch: &EventBatch) -> Result<(), CoreError> {
            let count = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            if count <= self.fail_until {
                Err(CoreError::Internal {
                    code: oneshim_core::error_codes::InternalCode::Generic,
                    message: "Temporary failure".to_string(),
                })
            } else {
                Ok(())
            }
        }
        async fn upload_context(
            &self,
            _upload: &oneshim_core::models::frame::ContextUpload,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn send_feedback(
            &self,
            _feedback: &oneshim_core::models::suggestion::SuggestionFeedback,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn send_heartbeat(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn retry_on_transient_failure() {
        let client = Arc::new(FlakeyApiClient {
            call_count: std::sync::atomic::AtomicU32::new(0),
            fail_until: 1,
        });
        let uploader = BatchUploader::new(client, "sess_retry".to_string(), 100, 3);

        uploader.enqueue(make_test_event());
        let result = uploader.flush().await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);
    }

    #[tokio::test]
    async fn max_retries_exhaustion() {
        let client = Arc::new(MockApiClient { should_fail: true });
        let uploader = BatchUploader::new(client, "sess_fail".to_string(), 100, 0); // 0 retries

        uploader.enqueue(make_test_event());
        let result = uploader.flush().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn failed_events_requeued() {
        let client = Arc::new(MockApiClient { should_fail: true });
        let uploader = BatchUploader::new(client, "sess_requeue".to_string(), 100, 0);

        uploader.enqueue(make_test_event());
        uploader.enqueue(make_test_event());
        assert_eq!(uploader.queue_size(), 2);

        let result = uploader.flush().await;
        assert!(result.is_err());
        assert_eq!(uploader.queue_size(), 2);
    }

    /// Verifies the complete failure path using MockApiClient { should_fail: true }:
    /// - flush() returns Err
    /// - enqueued events are preserved in the queue after all retries are exhausted
    /// - stats().failed_batches is incremented by one per failed flush call
    #[tokio::test]
    async fn flush_failure_path_increments_failed_batches_and_preserves_events() {
        let client = Arc::new(MockApiClient { should_fail: true });
        // max_retries = 0 so the single attempt fails immediately without sleeping.
        let uploader = BatchUploader::new(client, "sess_fail_path".to_string(), 100, 0);

        uploader.enqueue(make_test_event());
        uploader.enqueue(make_test_event());
        uploader.enqueue(make_test_event());
        assert_eq!(uploader.queue_size(), 3);
        assert_eq!(uploader.failed_batches(), 0);

        // First flush — all 3 events are drained, upload fails, they are requeued.
        let result = uploader.flush().await;
        assert!(
            result.is_err(),
            "flush() must return Err when the API client always fails"
        );
        assert_eq!(
            uploader.queue_size(),
            3,
            "events must be requeued after a failed flush so no data is lost"
        );
        assert_eq!(
            uploader.stats().failed_batches,
            1,
            "failed_batches should be 1 after the first exhausted flush"
        );

        // Second flush — same failure, counter must be 2.
        let result2 = uploader.flush().await;
        assert!(result2.is_err());
        assert_eq!(uploader.queue_size(), 3);
        assert_eq!(
            uploader.stats().failed_batches,
            2,
            "failed_batches must increment monotonically with each exhausted flush"
        );
    }

    #[test]
    fn lock_free_concurrent_enqueue() {
        use std::thread;

        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = Arc::new(BatchUploader::new(
            client,
            "sess_concurrent".to_string(),
            100,
            3,
        ));

        let mut handles = vec![];
        for _ in 0..10 {
            let uploader = Arc::clone(&uploader);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    uploader.enqueue(make_test_event());
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(uploader.queue_size(), 1000);
    }

    #[tokio::test]
    async fn batch_sink_trait_dispatch() {
        use oneshim_core::ports::batch_sink::BatchSink;

        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = BatchUploader::new(client, "sess_trait".to_string(), 100, 3);

        // Use through dyn BatchSink (same as scheduler does in production)
        let sink: Arc<dyn BatchSink> = Arc::new(uploader);

        sink.enqueue(make_test_event());
        sink.enqueue_many(vec![make_test_event(), make_test_event()]);

        let sent = sink.flush().await.unwrap();
        assert_eq!(sent, 3);
    }

    #[test]
    fn batch_stats() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = BatchUploader::new(client, "sess_stats".to_string(), 50, 3);

        for _ in 0..25 {
            uploader.enqueue(make_test_event());
        }

        let stats = uploader.stats();
        assert_eq!(stats.queue_size, 25);
        assert_eq!(stats.max_batch_size, 50);
        assert_eq!(stats.max_queue_size, MAX_UPLOAD_QUEUE_SIZE);
        assert!(stats.dynamic_batch_enabled);
    }

    // --- Backpressure tests ---

    #[test]
    fn enqueue_drops_oldest_when_at_capacity() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader =
            BatchUploader::new(client, "sess_bp".to_string(), 100, 3).with_max_queue_size(5);

        // Fill to capacity
        for _ in 0..5 {
            uploader.enqueue(make_test_event());
        }
        assert_eq!(uploader.queue_size(), 5);

        // Enqueue one more — should drop oldest, size stays at 5
        uploader.enqueue(make_test_event());
        assert_eq!(uploader.queue_size(), 5);
    }

    #[test]
    fn enqueue_many_drops_oldest_when_overflow() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader =
            BatchUploader::new(client, "sess_bp2".to_string(), 100, 3).with_max_queue_size(5);

        // Fill with 3 items
        for _ in 0..3 {
            uploader.enqueue(make_test_event());
        }
        assert_eq!(uploader.queue_size(), 3);

        // Enqueue 4 more (total would be 7, capacity 5) -> drop 2 oldest
        uploader.enqueue_many(vec![
            make_test_event(),
            make_test_event(),
            make_test_event(),
            make_test_event(),
        ]);
        assert_eq!(uploader.queue_size(), 5);
    }

    #[test]
    fn tracks_total_dropped_events() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader =
            BatchUploader::new(client, "sess_drop".to_string(), 100, 3).with_max_queue_size(5);

        for _ in 0..10 {
            uploader.enqueue(make_test_event());
        }
        // Queue capped at 5; the other 5 were dropped one-by-one on each
        // enqueue that found the queue at capacity.
        assert_eq!(uploader.queue_size(), 5);
        assert_eq!(uploader.total_dropped(), 5);
        assert_eq!(uploader.stats().total_dropped, 5);
    }

    #[tokio::test]
    async fn requeue_respects_capacity_limit() {
        let client = Arc::new(MockApiClient { should_fail: true });
        let uploader = BatchUploader::new(client, "sess_bp3".to_string(), 100, 0)
            .with_max_queue_size(5)
            .with_dynamic_batch(false);

        // Fill to capacity
        for _ in 0..5 {
            uploader.enqueue(make_test_event());
        }
        assert_eq!(uploader.queue_size(), 5);

        // Flush will drain up to max_batch_size (100), fail, and requeue all 5.
        // The requeue should still respect the limit.
        let _ = uploader.flush().await;
        assert!(uploader.queue_size() <= 5);
    }

    #[test]
    fn with_max_queue_size_builder() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader =
            BatchUploader::new(client, "sess_builder".to_string(), 100, 3).with_max_queue_size(500);
        assert_eq!(uploader.max_queue_size(), 500);
    }

    #[test]
    fn stats_includes_max_queue_size() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader =
            BatchUploader::new(client, "sess_stats2".to_string(), 50, 3).with_max_queue_size(2000);

        let stats = uploader.stats();
        assert_eq!(stats.max_queue_size, 2000);
    }

    #[test]
    fn concurrent_enqueue_respects_capacity() {
        use std::thread;

        let client = Arc::new(MockApiClient { should_fail: false });
        let uploader = Arc::new(
            BatchUploader::new(client, "sess_concurrent_bp".to_string(), 100, 3)
                .with_max_queue_size(100),
        );

        let mut handles = vec![];
        for _ in 0..10 {
            let uploader = Arc::clone(&uploader);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    uploader.enqueue(make_test_event());
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // With 10 threads x 100 events = 1000, but capacity is 100.
        // Due to concurrent lock-free nature, the exact count may slightly
        // exceed the limit transiently, but it should be close to max.
        assert!(
            uploader.queue_size() <= 150,
            "queue_size {} should be near max_queue_size 100 (some slack for concurrency)",
            uploader.queue_size()
        );
    }
}
