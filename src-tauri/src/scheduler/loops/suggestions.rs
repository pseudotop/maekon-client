use maekon_storage::sqlite::SqliteStorage;
use maekon_suggestion::deferred::DeferredManager;
use maekon_suggestion::feedback::FeedbackSender;
use maekon_suggestion::feedback_retry::FeedbackRetryQueue;
use maekon_suggestion::queue::SuggestionQueue;
#[cfg(feature = "server")]
use maekon_suggestion::receiver::SuggestionReceiver;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// `receiver.run()` 이 정상 종료(서버측 `close` 이벤트 등)했을 때, 즉시 재진입하며
/// hot-loop 을 방지하기 위한 짧은 고정 지연. 전송 계층(transport) 재연결의 지수 백오프는
/// `SseStreamClient::connect` 의 내부 루프(1s→30s)가 단독으로 담당하므로 여기서 중복 백오프를
/// 두지 않는다 (#4814 SSE 이중 reconnect 정리).
#[cfg(feature = "server")]
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// SSE suggestion reception 루프.
///
/// 실제 자동 재연결(지수 백오프 1s→30s)은 `SseStreamClient::connect` 내부 루프가
/// authoritative 하게 수행한다. `receiver.run()` 은 전송 오류 시 반환하지 않고 내부에서
/// 재연결하므로, 이 래퍼는 서버가 `close` 이벤트로 스트림을 정상 종료해 `run()` 이
/// 반환한 경우에만 짧은 지연 후 재진입한다. 따라서 외부 지수 백오프는 중복이며 제거됐다.
#[cfg(feature = "server")]
pub(crate) fn spawn_suggestion_sse_loop(
    receiver: Arc<SuggestionReceiver>,
    session_id: String,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("suggestion SSE loop started");

        loop {
            tokio::select! {
                result = receiver.run(&session_id) => {
                    match result {
                        Ok(()) => info!("SSE stream closed, will reconnect"),
                        Err(e) => warn!("SSE stream error: {e}, will reconnect"),
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("suggestion SSE loop shutdown");
                    return;
                }
            }

            if *shutdown_rx.borrow() {
                break;
            }

            // 내부 client 가 transport 재연결을 담당하므로, 여기서는 hot-loop 방지용
            // 짧은 고정 지연만 둔다 (지수 백오프 중복 제거).
            tokio::select! {
                _ = tokio::time::sleep(RECONNECT_DELAY) => {}
                _ = shutdown_rx.changed() => {
                    info!("suggestion SSE loop shutdown during reconnect delay");
                    return;
                }
            }
        }
    })
}

/// Periodic maintenance: resurface deferred + retry failed feedback.
/// Runs every 30 seconds.
///
/// E20-24 (#4816): runs in OSS `local-suggestions` builds too — the resurface-deferred
/// step is the local pipeline's snooze mechanism. The feedback-retry step is a no-op
/// in local-only builds: `LocalApiClient.send_feedback` returns `Ok(())`, so accept/
/// reject never enqueue a retry and `collect_ready()` stays empty (no unbounded growth).
#[cfg(feature = "local-suggestions")]
pub(crate) fn spawn_suggestion_maintenance_loop(
    queue: Arc<Mutex<SuggestionQueue>>,
    deferred: Arc<Mutex<DeferredManager>>,
    retry_queue: Arc<Mutex<FeedbackRetryQueue>>,
    feedback: Arc<FeedbackSender>,
    storage: Arc<SqliteStorage>,
    on_change: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("suggestion maintenance loop started");
        let mut interval = super::intervals::coalescing_interval(Duration::from_secs(30));
        interval.tick().await; // skip immediate first tick

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown_rx.changed() => {
                    info!("suggestion maintenance loop shutdown");
                    return;
                }
            }

            // 1. Resurface deferred suggestions
            let resurfaced = deferred.lock().await.collect_resurfaced();
            if !resurfaced.is_empty() {
                let mut q = queue.lock().await;
                for suggestion in resurfaced {
                    let id = suggestion.suggestion_id.clone();
                    if q.push(suggestion) {
                        info!(suggestion_id = %id, "deferred suggestion resurfaced");
                    }
                }
                let count = q.len();
                drop(q);
                if let Some(ref cb) = on_change {
                    cb(count);
                }
            }

            // 2. Process feedback retry queue
            let ready = retry_queue.lock().await.collect_ready();
            for pending in ready {
                let result = match pending.feedback_type {
                    maekon_core::models::suggestion::FeedbackType::Accepted => {
                        feedback
                            .accept(&pending.suggestion_id, pending.comment.clone())
                            .await
                    }
                    maekon_core::models::suggestion::FeedbackType::Rejected => {
                        feedback
                            .reject(&pending.suggestion_id, pending.comment.clone())
                            .await
                    }
                    maekon_core::models::suggestion::FeedbackType::Deferred => {
                        feedback
                            .defer(&pending.suggestion_id, pending.comment.clone())
                            .await
                    }
                };
                match result {
                    Ok(()) => {
                        // Cleanup persisted retry on success
                        if let Err(e) = storage.delete_pending_feedback(&pending.suggestion_id) {
                            warn!("failed to clean up persisted feedback: {e}");
                        }
                    }
                    Err(e) => {
                        let mut rq = retry_queue.lock().await;
                        if rq.is_exhausted(&pending) {
                            warn!(
                                suggestion_id = %pending.suggestion_id,
                                attempts = pending.attempts,
                                "feedback retry exhausted"
                            );
                            rq.drop_exhausted(&pending.suggestion_id);
                            // Also clean up persisted row
                            let _ = storage.delete_pending_feedback(&pending.suggestion_id);
                        } else {
                            info!(
                                suggestion_id = %pending.suggestion_id,
                                attempt = pending.attempts + 1,
                                "feedback retry failed: {e}"
                            );
                            rq.retry_failed(pending);
                        }
                    }
                }
            }

            // 3. Cleanup orphaned feedback retries older than 7 days
            if let Err(e) = storage.cleanup_old_feedback_retries(7) {
                warn!("failed to clean up old feedback retries: {e}");
            }
        }
    })
}
