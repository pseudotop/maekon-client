//! 배치 이벤트 전송 포트 — 서버 동기화 추상화

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::event::Event;

/// 이벤트를 배치로 서버에 전송하는 포트.
/// `oneshim-network::BatchUploader`가 구현체.
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
    /// 이벤트를 전송 큐에 추가
    fn enqueue(&self, event: Event);

    /// 복수 이벤트를 전송 큐에 추가
    fn enqueue_many(&self, events: Vec<Event>);

    /// 큐에 쌓인 이벤트를 서버로 플러시. 전송된 건수 반환.
    async fn flush(&self) -> Result<usize, CoreError>;

    /// 마지막 호출 이후 드롭된 이벤트 수를 반환하고 카운터 리셋.
    fn take_dropped_since_last(&self) -> usize {
        0
    }
}
