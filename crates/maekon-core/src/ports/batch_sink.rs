//! Batch event upload port — server synchronization abstraction.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::event::Event;

/// A queued upload item: the (possibly egress-filtered) event payload paired
/// with the STORAGE id of the original persisted event (#7946).
///
/// The two must travel together because the storage id is derived from the
/// original event's content at persist time, while the uploaded payload may
/// have been transformed by the egress policy (PII masking / title redaction)
/// — deriving an id from the payload would stamp the wrong row as sent.
/// Producers compute `storage_id` from the ORIGINAL event (the same value the
/// persistence layer derives) before handing the event to the egress policy.
#[derive(Debug, Clone)]
pub struct QueuedUpload {
    /// Storage primary key of the persisted original event.
    pub storage_id: String,
    /// The upload payload (post egress-policy transformation).
    pub event: Event,
}

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
    fn enqueue(&self, item: QueuedUpload);

    /// Add multiple events to the upload queue.
    fn enqueue_many(&self, items: Vec<QueuedUpload>);

    /// Flush queued events to the server. Returns the storage ids of the
    /// events that were confirmed uploaded in this flush (#7946) — the caller
    /// marks exactly these rows as sent, never a time-based bulk stamp.
    async fn flush(&self) -> Result<Vec<String>, CoreError>;

    /// Return the number of events dropped since the last call and reset the counter.
    fn take_dropped_since_last(&self) -> usize {
        0
    }
}
