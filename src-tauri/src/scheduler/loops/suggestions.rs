use oneshim_storage::sqlite::SqliteStorage;
use oneshim_suggestion::deferred::DeferredManager;
use oneshim_suggestion::feedback::FeedbackSender;
use oneshim_suggestion::feedback_retry::FeedbackRetryQueue;
use oneshim_suggestion::queue::SuggestionQueue;
use oneshim_suggestion::receiver::SuggestionReceiver;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{info, warn};

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const CONNECTED_THRESHOLD: Duration = Duration::from_secs(10);

/// SSE suggestion reception loop with automatic reconnection.
/// Exponential backoff (1s -> 30s). Resets after session lasting >10s.
#[cfg(feature = "server")]
pub(crate) fn spawn_suggestion_sse_loop(
    receiver: Arc<SuggestionReceiver>,
    session_id: String,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("suggestion SSE loop started");
        let mut backoff = INITIAL_BACKOFF;

        loop {
            let start = Instant::now();
            tokio::select! {
                result = receiver.run(&session_id) => {
                    let ran_for = start.elapsed();
                    match result {
                        Ok(()) => info!(ran_secs = ran_for.as_secs(), "SSE stream closed, will reconnect"),
                        Err(e) => warn!(ran_secs = ran_for.as_secs(), "SSE stream error: {e}, will reconnect"),
                    }
                    if ran_for > CONNECTED_THRESHOLD {
                        backoff = INITIAL_BACKOFF;
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

            info!(
                delay_ms = backoff.as_millis(),
                "SSE reconnecting after backoff"
            );
            tokio::select! {
                _ = tokio::time::sleep(backoff) => {}
                _ = shutdown_rx.changed() => {
                    info!("suggestion SSE loop shutdown during backoff");
                    return;
                }
            }
            backoff = (backoff * 2).min(MAX_BACKOFF);
        }
    })
}

/// Periodic maintenance: resurface deferred + retry failed feedback.
/// Runs every 30 seconds.
#[cfg(feature = "server")]
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
        let mut interval = tokio::time::interval(Duration::from_secs(30));
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
                    oneshim_core::models::suggestion::FeedbackType::Accepted => {
                        feedback
                            .accept(&pending.suggestion_id, pending.comment.clone())
                            .await
                    }
                    oneshim_core::models::suggestion::FeedbackType::Rejected => {
                        feedback
                            .reject(&pending.suggestion_id, pending.comment.clone())
                            .await
                    }
                    oneshim_core::models::suggestion::FeedbackType::Deferred => {
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
