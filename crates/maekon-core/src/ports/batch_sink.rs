//! Batch event upload port — server synchronization abstraction.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::event::Event;

/// Port for uploading events to the server in batches.
/// Implemented by `maekon-network::BatchUploader`.
///
/// # Errors
/// - `CoreError::Network` (wire: `network.*`) — connection failure,
///   DNS error, refused connection (from `ApiClient::upload_batch`).
/// - `CoreError::RequestTimeout` (wire: `network.timeout`) — flush
///   exceeding configured upload timeout.
/// - `CoreError::RateLimit` (wire: `network.rate_limit`) — server 429
///   surfaced by the inner HTTP client.
/// - `CoreError::ServiceUnavailable` (wire: `service.unavailable`) —
///   5xx class responses.
/// - `enqueue` / `enqueue_many` are infallible by contract; queue
///   overflow is silently dropped and surfaced via
///   `take_dropped_since_last()` rather than an error variant.
#[async_trait]
pub trait BatchSink: Send + Sync {
    /// Add an event to the upload queue.
    fn enqueue(&self, event: Event);

    /// Add multiple events to the upload queue.
    fn enqueue_many(&self, events: Vec<Event>);

    /// Flush queued events to the server. Returns the number of events sent.
    async fn flush(&self) -> Result<usize, CoreError>;

    /// Return the number of events dropped since the last call and reset the counter.
    fn take_dropped_since_last(&self) -> usize {
        0
    }
}
