// OOS-TBD: ADR-013 file split (cycle 35+) — LOC: 847
use crate::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig, CircuitState};
use crate::error::NetworkError;
use crossbeam::queue::SegQueue;
use maekon_core::error::CoreError;
use maekon_core::models::event::{Event, EventBatch};
use maekon_core::ports::api_client::ApiClient;
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

/// Classify an upload failure as transient (worth retrying / requeueing) or
/// permanent (must be dropped). Mirrors the classification used by
/// `HttpApiClient::execute_with_retry` (`is_retryable`) and
/// `RemoteSyncTransport::is_retryable`: only transport/timeout/5xx/429 errors
/// are retryable. Permanent 4xx failures — `Auth` (401/403), `Validation`
/// (400/422), `NotFound` (404) — are NOT retryable. Retrying them would loop
/// forever and, because failed batches are requeued, keep the queue full and
/// drop the newest events (a poison-pill #6078).
fn is_retryable(error: &CoreError) -> bool {
    matches!(
        error,
        CoreError::Network { .. }
            | CoreError::RequestTimeout {
                code: maekon_core::error_codes::NetworkCode::Timeout,
                ..
            }
            | CoreError::ServiceUnavailable { .. }
            | CoreError::RateLimit {
                code: maekon_core::error_codes::NetworkCode::RateLimit,
                ..
            }
            // Internal/Generic failures are conservatively retryable: they
            // typically wrap an unclassified transport-layer fault rather than
            // a deterministic 4xx rejection.
            | CoreError::Internal { .. }
    )
}

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

        // Circuit breaker fast-fail. When HalfOpen, `check()` admits this caller
        // as a probe (incrementing the breaker's in-flight probe budget, #31),
        // which we are then obligated to resolve via record_success/record_failure
        // before returning — otherwise the probe slot leaks until the self-heal
        // window elapses.
        let admitted_as_probe = match self.circuit_breaker.check() {
            CircuitState::Open { .. } => {
                debug!("circuit open — fast-failing flush");
                return Err(NetworkError::CircuitOpen);
            }
            CircuitState::HalfOpen => {
                debug!("circuit half-open — probe flush");
                true
            }
            CircuitState::Closed => false,
        };

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
            // A concurrent flush drained the queue between the size snapshot and
            // the pop loop. Treat the empty drain as a benign no-op success: if
            // we were admitted as a HalfOpen probe, release the probe slot now
            // (record_success) so the breaker budget is freed immediately rather
            // than waiting for the self-heal window. We send nothing, so there is
            // no real outcome to record — closing the breaker on an empty drain
            // is harmless because the next real flush re-evaluates connectivity.
            if admitted_as_probe {
                self.circuit_breaker.record_success();
            }
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
                    // Permanent 4xx failures (Auth / Validation / NotFound) will
                    // never succeed on retry. Retrying + requeueing them is a
                    // poison pill (#6078): the same batch fails forever, keeping
                    // the queue full and forcing newer events to be dropped.
                    // Drop the batch immediately instead of requeueing it.
                    if !is_retryable(&e) {
                        error!(
                            err.code = %e.code(),
                            "batch upload permanent failure — dropping {actual_count} non-retryable event(s): {e}"
                        );
                        if let Some(ref flag) = self.last_upload_ok {
                            flag.store(false, Ordering::Relaxed);
                        }
                        self.circuit_breaker.record_failure();
                        self.failed_batches.fetch_add(1, Ordering::Relaxed);
                        self.total_dropped
                            .fetch_add(actual_count, Ordering::Relaxed);
                        self.cycle_dropped
                            .fetch_add(actual_count, Ordering::Relaxed);
                        return Err(e.into());
                    }

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
impl maekon_core::ports::batch_sink::BatchSink for BatchUploader {
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
    use maekon_core::models::event::{ContextEvent, Event};

    struct MockApiClient {
        should_fail: bool,
    }

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
        async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn upload_batch(&self, _batch: &EventBatch) -> Result<(), CoreError> {
            if self.should_fail {
                Err(CoreError::Internal {
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "mock failure".to_string(),
                })
            } else {
                Ok(())
            }
        }
        async fn send_feedback(
            &self,
            _feedback: &maekon_core::models::suggestion::SuggestionFeedback,
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

    /// Regression (batch-probe-leak): when `flush()` is admitted as a HalfOpen
    /// probe but a concurrent flush drains the queue first (`actual_count == 0`),
    /// the empty-drain early-return must RELEASE the admitted probe slot via
    /// `record_success` rather than leak it. A leaked probe pins the breaker's
    /// budget and bricks subsequent flushes until the self-heal window elapses.
    #[tokio::test]
    async fn flush_empty_drain_releases_half_open_probe() {
        let client = Arc::new(MockApiClient { should_fail: false });
        let mut uploader = BatchUploader::new(client, "sess_probe_leak".to_string(), 100, 3);

        // Replace the breaker with a fast-cooldown one so we can reach HalfOpen
        // deterministically within the test.
        let fast_breaker = CircuitBreaker::new(CircuitBreakerConfig {
            failure_threshold: 1,
            initial_cooldown: Duration::from_millis(40),
            max_cooldown: Duration::from_millis(200),
            half_open_probes: 1,
        });
        // Trip → Open, then wait the cooldown so the next check() yields HalfOpen.
        fast_breaker.record_failure();
        assert!(matches!(fast_breaker.check(), CircuitState::Open { .. }));
        uploader.circuit_breaker = fast_breaker;
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Simulate the concurrent-drain race: the size counter reports a pending
        // event (so flush passes the `current_size == 0` guard and is admitted as
        // a probe), but the underlying queue is empty so the pop loop yields zero
        // events. This is exactly the window the fix targets.
        uploader.queue_size.store(1, Ordering::Relaxed);
        assert!(uploader.queue.pop().is_none(), "queue must be empty");

        let sent = uploader.flush().await.unwrap();
        assert_eq!(sent, 0, "empty drain returns Ok(0)");

        // The probe slot must have been released. Because record_success() closes
        // the breaker, a follow-up check() observes Closed — not Open from a
        // budget that is still pinned by the leaked in-flight probe.
        assert!(
            matches!(uploader.circuit_breaker.check(), CircuitState::Closed),
            "empty-drain probe must be released (breaker closed), not leaked"
        );
        assert_eq!(uploader.circuit_breaker.stats().state, "closed");
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
        ) -> Result<maekon_core::ports::api_client::SessionCreateResponse, CoreError> {
            Ok(maekon_core::ports::api_client::SessionCreateResponse {
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
                    code: maekon_core::error_codes::InternalCode::Generic,
                    message: "Temporary failure".to_string(),
                })
            } else {
                Ok(())
            }
        }
        async fn send_feedback(
            &self,
            _feedback: &maekon_core::models::suggestion::SuggestionFeedback,
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
        let flushed = uploader
            .flush()
            .await
            .expect("flush must succeed after transient failure clears on attempt 2");
        assert_eq!(flushed, 1, "exactly 1 event must be confirmed uploaded");
    }

    #[tokio::test]
    async fn max_retries_exhaustion() {
        let client = Arc::new(MockApiClient { should_fail: true });
        let uploader = BatchUploader::new(client, "sess_fail".to_string(), 100, 0); // 0 retries

        uploader.enqueue(make_test_event());
        let err = uploader.flush().await.unwrap_err();
        assert!(
            matches!(err, NetworkError::Core(CoreError::Internal { .. })),
            "mock failure must be NetworkError::Core(CoreError::Internal), got: {err:?}"
        );
    }

    #[tokio::test]
    async fn failed_events_requeued() {
        let client = Arc::new(MockApiClient { should_fail: true });
        let uploader = BatchUploader::new(client, "sess_requeue".to_string(), 100, 0);

        uploader.enqueue(make_test_event());
        uploader.enqueue(make_test_event());
        assert_eq!(uploader.queue_size(), 2);

        let err = uploader.flush().await.unwrap_err();
        assert!(
            matches!(err, NetworkError::Core(CoreError::Internal { .. })),
            "mock failure must be NetworkError::Core(CoreError::Internal), got: {err:?}"
        );
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
        let flush_err = uploader.flush().await.unwrap_err();
        assert!(
            matches!(
                flush_err,
                crate::NetworkError::Core(maekon_core::error::CoreError::Internal { .. })
            ),
            "flush() failure must be NetworkError::Core(CoreError::Internal); got: {flush_err:?}"
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
        let err2 = uploader.flush().await.unwrap_err();
        assert!(
            matches!(err2, NetworkError::Core(CoreError::Internal { .. })),
            "second flush mock failure must be NetworkError::Core(CoreError::Internal), got: {err2:?}"
        );
        assert_eq!(uploader.queue_size(), 3);
        assert_eq!(
            uploader.stats().failed_batches,
            2,
            "failed_batches must increment monotonically with each exhausted flush"
        );
    }

    /// Counts `upload_batch` calls and always returns a permanent `Auth`
    /// failure (e.g. a 401). Used to prove non-retryable errors short-circuit
    /// the retry loop and are dropped rather than requeued (#6078).
    struct AuthFailApiClient {
        call_count: std::sync::atomic::AtomicU32,
    }

    #[async_trait::async_trait]
    impl ApiClient for AuthFailApiClient {
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
        async fn end_session(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
        async fn upload_batch(&self, _batch: &EventBatch) -> Result<(), CoreError> {
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(CoreError::Auth {
                code: maekon_core::error_codes::AuthCode::Failed,
                message: "permanent 401".to_string(),
            })
        }
        async fn send_feedback(
            &self,
            _feedback: &maekon_core::models::suggestion::SuggestionFeedback,
        ) -> Result<(), CoreError> {
            Ok(())
        }
        async fn send_heartbeat(&self, _session_id: &str) -> Result<(), CoreError> {
            Ok(())
        }
    }

    /// #6078: a permanent Auth (401) failure is a poison pill — it must NOT be
    /// retried and must NOT be requeued. Verifies:
    /// - `upload_batch` is called exactly once (no retry attempts), even though
    ///   `max_retries = 3`
    /// - the queue is left empty (batch dropped, not requeued)
    /// - the dropped events are accounted for in the drop counters
    #[tokio::test]
    async fn permanent_auth_failure_is_not_retried_or_requeued() {
        let client = Arc::new(AuthFailApiClient {
            call_count: std::sync::atomic::AtomicU32::new(0),
        });
        // max_retries = 3: a retryable error would attempt 4 uploads. A
        // non-retryable Auth error must short-circuit after the first.
        let uploader = BatchUploader::new(client.clone(), "sess_auth_fail".to_string(), 100, 3);

        uploader.enqueue(make_test_event());
        uploader.enqueue(make_test_event());
        assert_eq!(uploader.queue_size(), 2);

        let err = uploader.flush().await.unwrap_err();
        assert!(
            matches!(err, NetworkError::Core(CoreError::Auth { .. })),
            "permanent failure must surface as NetworkError::Core(CoreError::Auth), got: {err:?}"
        );

        assert_eq!(
            client.call_count.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a non-retryable Auth error must not be retried (exactly 1 upload attempt)"
        );
        assert_eq!(
            uploader.queue_size(),
            0,
            "a poison-pill batch must be dropped, not requeued"
        );
        assert_eq!(
            uploader.total_dropped(),
            2,
            "dropped events from a permanent failure must be counted"
        );
        // A failed flush still counts toward failed_batches for observability.
        assert_eq!(uploader.failed_batches(), 1);
    }

    #[test]
    fn is_retryable_classification() {
        // Transient / 5xx / 429 / timeout → retryable
        assert!(is_retryable(&CoreError::Network {
            code: maekon_core::error_codes::NetworkCode::Generic,
            message: "boom".into(),
        }));
        assert!(is_retryable(&CoreError::RequestTimeout {
            code: maekon_core::error_codes::NetworkCode::Timeout,
            timeout_ms: 1000,
        }));
        assert!(is_retryable(&CoreError::ServiceUnavailable {
            code: maekon_core::error_codes::ServiceCode::Unavailable,
            message: "503".into(),
        }));
        assert!(is_retryable(&CoreError::RateLimit {
            code: maekon_core::error_codes::NetworkCode::RateLimit,
            retry_after_secs: 5,
        }));

        // Permanent 4xx → NOT retryable
        assert!(!is_retryable(&CoreError::Auth {
            code: maekon_core::error_codes::AuthCode::Failed,
            message: "401".into(),
        }));
        assert!(!is_retryable(&CoreError::Validation {
            code: maekon_core::error_codes::ValidationCode::InvalidField,
            field: "events".into(),
            message: "422".into(),
        }));
        assert!(!is_retryable(&CoreError::NotFound {
            code: maekon_core::error_codes::NotFoundCode::ResourceMissing,
            resource_type: "session".into(),
            id: "x".into(),
        }));
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
        use maekon_core::ports::batch_sink::BatchSink;

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
