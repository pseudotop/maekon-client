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

/// Base reconnect delay applied after the first consecutive failure (1s).
///
/// The transport reconnect backoff inside `SseStreamClient::connect` (1s→30s)
/// only exists for the REST/SSE path. The gRPC path (`GrpcSseAdapter`) does NOT
/// reconnect internally — its spawned stream task breaks on the first
/// error/close/idle-timeout and `receiver.run()` returns immediately. So this
/// wrapper must own a backoff itself, otherwise a down gRPC server is hammered
/// with a fixed ~1s retry (#6130, supersedes the #4814 fixed-delay assumption).
#[cfg(feature = "server")]
const RECONNECT_BASE_DELAY: Duration = Duration::from_secs(1);

/// Cap on the reconnect delay (~30s), matching the SSE transport's own ceiling.
#[cfg(feature = "server")]
const RECONNECT_MAX_DELAY: Duration = Duration::from_secs(30);

/// Give-up bound: after this many consecutive failures without the stream ever
/// delivering a suggestion, stop retrying and let the loop exit. The next
/// scheduler restart (re-login / session refresh) will respawn the loop. This
/// prevents an unbounded retry storm against a permanently unreachable server.
#[cfg(feature = "server")]
const RECONNECT_MAX_ATTEMPTS: u32 = 12;

/// Pure helper: compute the reconnect delay for the Nth consecutive failure.
///
/// `consecutive_failures` is 1-based (1 == first failure). The delay escalates
/// `base * 2^(n-1)` capped at `max`, then adds up to 25% jitter (clamped so the
/// jittered result never exceeds `max`) to avoid synchronized retries across
/// many clients. Extracted as a free function so the escalation/cap/reset logic
/// is unit-testable without spinning a tokio task or a live server (#6130).
#[cfg(feature = "server")]
fn next_reconnect_delay(
    consecutive_failures: u32,
    base: Duration,
    max: Duration,
    jitter_ratio: f64,
) -> Duration {
    if consecutive_failures == 0 {
        return Duration::ZERO;
    }
    let base_ms = base.as_millis().min(u128::from(u64::MAX)) as u64;
    let max_ms = max.as_millis().min(u128::from(u64::MAX)) as u64;
    if base_ms == 0 || max_ms == 0 {
        return Duration::ZERO;
    }

    // Exponential growth, saturating: base * 2^(n-1), then clamp to max.
    let shift = consecutive_failures.saturating_sub(1).min(63);
    let exp_ms = base_ms.saturating_mul(1u64 << shift).min(max_ms);

    // Additive jitter in [0, exp_ms * jitter_ratio], clamped so we never exceed max.
    let jitter_span = ((exp_ms as f64) * jitter_ratio.clamp(0.0, 1.0)) as u64;
    let jitter_ms = if jitter_span == 0 {
        0
    } else {
        use rand::RngExt;
        rand::rng().random_range(0..=jitter_span)
    };

    Duration::from_millis(exp_ms.saturating_add(jitter_ms).min(max_ms))
}

/// SSE/gRPC suggestion reception loop with escalating reconnect backoff (#6130).
///
/// `receiver.run()` returns `Ok(true)` when the stream delivered at least one
/// suggestion and `Ok(false)`/`Err(_)` otherwise. On every non-productive
/// return the consecutive-failure counter escalates the reconnect delay
/// (`RECONNECT_BASE_DELAY` → `RECONNECT_MAX_DELAY`, with jitter). The counter is
/// reset to 0 only when the stream actually delivered a suggestion, so a healthy
/// server that briefly closes the stream still reconnects quickly while a down
/// server is backed off. After `RECONNECT_MAX_ATTEMPTS` consecutive failures the
/// loop gives up; a later session refresh respawns it.
#[cfg(feature = "server")]
pub(crate) fn spawn_suggestion_sse_loop(
    receiver: Arc<SuggestionReceiver>,
    session_id: String,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        info!("suggestion SSE loop started");

        // Consecutive failures since the last productive stream (0 == healthy).
        let mut consecutive_failures: u32 = 0;

        loop {
            tokio::select! {
                result = receiver.run(&session_id) => {
                    match result {
                        Ok(true) => {
                            // Stream delivered ≥1 suggestion: it was healthy.
                            // Reset backoff and reconnect promptly.
                            consecutive_failures = 0;
                            info!("SSE stream closed after delivering suggestions, will reconnect");
                        }
                        Ok(false) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            warn!(
                                consecutive_failures,
                                "SSE stream closed without delivering suggestions, will back off and reconnect"
                            );
                        }
                        Err(e) => {
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            warn!(
                                consecutive_failures,
                                "SSE stream error: {e}, will back off and reconnect"
                            );
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("suggestion SSE loop shutdown");
                    // Deterministically signal + abort + await the inner SSE
                    // task instead of relying on background-runtime drop to tear
                    // it down (sse-shutdown-deterministic). `run()` was cancelled
                    // by select!, so its stored stop-sender/JoinHandle are still
                    // live; `shutdown()` settles them before we return.
                    receiver.shutdown().await;
                    return;
                }
            }

            if *shutdown_rx.borrow() {
                break;
            }

            // Give-up bound: stop hammering a permanently-down server.
            if consecutive_failures >= RECONNECT_MAX_ATTEMPTS {
                warn!(
                    consecutive_failures,
                    max_attempts = RECONNECT_MAX_ATTEMPTS,
                    "suggestion SSE loop giving up after repeated failures; \
                     will respawn on next session refresh"
                );
                break;
            }

            let delay = if consecutive_failures == 0 {
                // Productive stream just closed — reconnect promptly (small fixed
                // delay only to avoid a tight loop on rapid open/close churn).
                RECONNECT_BASE_DELAY
            } else {
                next_reconnect_delay(
                    consecutive_failures,
                    RECONNECT_BASE_DELAY,
                    RECONNECT_MAX_DELAY,
                    0.25,
                )
            };

            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = shutdown_rx.changed() => {
                    info!("suggestion SSE loop shutdown during reconnect delay");
                    // During the reconnect delay `run()` is not in-flight, so its
                    // stop-sender/JoinHandle reflect the previous attempt. Still
                    // call shutdown() so any not-yet-finished prior task is aborted
                    // and awaited deterministically (sse-shutdown-deterministic).
                    receiver.shutdown().await;
                    return;
                }
            }
        }

        // Loop exited via `break` (shutdown race after run() returned, or the
        // give-up bound). The last `run()` already returned, so its inner task
        // has finished, but call shutdown() anyway so every loop exit path
        // settles the receiver's abort machinery deterministically rather than
        // leaving teardown to background-runtime drop (sse-shutdown-deterministic).
        receiver.shutdown().await;
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
            //
            // Use `retry_attempt` (not `accept`/`reject`/`defer`) so the
            // FeedbackSignalSink is NOT re-fired on network retries. The sink
            // already fired on the original user action; re-firing would
            // double-count signals in frequency/weight scoring. See #6004.
            let ready = retry_queue.lock().await.collect_ready();
            for pending in ready {
                let result = feedback.retry_attempt(&pending).await;
                match result {
                    Ok(()) => {
                        // Cleanup persisted retry on success.
                        // `delete_pending_feedback` is a blocking SQLite DELETE; offload
                        // it to the blocking pool so it never runs on the async reactor.
                        let storage_del = storage.clone();
                        let sid = pending.suggestion_id.clone();
                        let joined = tokio::task::spawn_blocking(move || {
                            storage_del.delete_pending_feedback(&sid)
                        })
                        .await;
                        match joined {
                            Ok(Ok(())) => {}
                            Ok(Err(e)) => warn!("failed to clean up persisted feedback: {e}"),
                            Err(e) => warn!("feedback cleanup task panicked: {e}"),
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
                            // Also clean up persisted row — offload the blocking DELETE.
                            let storage_del = storage.clone();
                            let sid = pending.suggestion_id.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                storage_del.delete_pending_feedback(&sid)
                            })
                            .await;
                        } else {
                            info!(
                                suggestion_id = %pending.suggestion_id,
                                attempt = pending.attempts + 1,
                                "feedback retry failed: {e}"
                            );
                            // `retry_failed` bumps `attempts` and reschedules
                            // `next_retry_at` in memory only. Re-persist the
                            // updated record (INSERT OR REPLACE keyed on
                            // suggestion_id) so a restart restores the true
                            // attempt count and remaining backoff instead of
                            // resetting the max-attempts bound (#6095).
                            let reschedule = rq.retry_failed(pending);
                            drop(rq);
                            let updated = reschedule.updated;
                            // review4 re-verify: if a concurrent user-feedback enqueue
                            // refilled the queue during the network retry above,
                            // retry_failed may have evicted an entry — delete its
                            // now-orphaned durable row (offloaded blocking DELETE).
                            if let Some(evicted) = reschedule.evicted {
                                let storage_evict = storage.clone();
                                let evicted_sid = evicted.suggestion_id.clone();
                                let _ = tokio::task::spawn_blocking(move || {
                                    storage_evict.delete_pending_feedback(&evicted_sid)
                                })
                                .await;
                            }
                            let record = maekon_core::models::storage_records::PendingFeedbackRecord::new_for_insert(
                                updated.suggestion_id.clone(),
                                &updated.feedback_type,
                                updated.comment.clone(),
                                updated.attempts,
                                updated.next_retry_at,
                            );
                            // `save_pending_feedback` is a blocking SQLite upsert; offload
                            // it to the blocking pool so it never runs on the async reactor.
                            let storage_save = storage.clone();
                            let sid = updated.suggestion_id.clone();
                            let joined = tokio::task::spawn_blocking(move || {
                                storage_save.save_pending_feedback(&record)
                            })
                            .await;
                            match joined {
                                Ok(Ok(())) => {}
                                Ok(Err(e)) => warn!(
                                    suggestion_id = %sid,
                                    "failed to re-persist pending feedback: {e}"
                                ),
                                Err(e) => warn!(
                                    suggestion_id = %sid,
                                    "feedback re-persist task panicked: {e}"
                                ),
                            }
                        }
                    }
                }
            }

            // 3. Cleanup orphaned feedback retries older than 7 days.
            // `cleanup_old_feedback_retries` is a blocking SQLite DELETE; offload it
            // to the blocking pool so it never runs on the async reactor.
            let storage_cleanup = storage.clone();
            let joined = tokio::task::spawn_blocking(move || {
                storage_cleanup.cleanup_old_feedback_retries(7)
            })
            .await;
            match joined {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => warn!("failed to clean up old feedback retries: {e}"),
                Err(e) => warn!("feedback retry cleanup task panicked: {e}"),
            }
        }
    })
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    const BASE: Duration = RECONNECT_BASE_DELAY; // 1s
    const MAX: Duration = RECONNECT_MAX_DELAY; // 30s

    /// #6130: zero consecutive failures means "no delay" — the loop reconnects
    /// immediately (the productive-close path uses BASE separately).
    #[test]
    fn next_delay_zero_failures_is_zero() {
        assert_eq!(
            next_reconnect_delay(0, BASE, MAX, 0.0),
            Duration::ZERO,
            "0 failures must yield no delay"
        );
    }

    /// #6130: without jitter, the delay doubles per consecutive failure
    /// (1s → 2s → 4s → 8s → 16s) until it saturates at the 30s cap.
    #[test]
    fn next_delay_escalates_exponentially_no_jitter() {
        // jitter_ratio = 0.0 makes the result deterministic.
        assert_eq!(
            next_reconnect_delay(1, BASE, MAX, 0.0),
            Duration::from_secs(1)
        );
        assert_eq!(
            next_reconnect_delay(2, BASE, MAX, 0.0),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_reconnect_delay(3, BASE, MAX, 0.0),
            Duration::from_secs(4)
        );
        assert_eq!(
            next_reconnect_delay(4, BASE, MAX, 0.0),
            Duration::from_secs(8)
        );
        assert_eq!(
            next_reconnect_delay(5, BASE, MAX, 0.0),
            Duration::from_secs(16)
        );
    }

    /// #6130: the delay is capped at MAX (~30s) regardless of how high the
    /// consecutive-failure count grows, even with jitter applied.
    #[test]
    fn next_delay_is_capped_at_max() {
        for failures in [6u32, 10, 32, 63, u32::MAX] {
            // No-jitter: exponential alone already exceeds MAX from failure 6 on.
            assert!(
                next_reconnect_delay(failures, BASE, MAX, 0.0) <= MAX,
                "no-jitter delay for {failures} failures must not exceed MAX"
            );
            // With jitter: must STILL be clamped to MAX, never above.
            assert!(
                next_reconnect_delay(failures, BASE, MAX, 0.25) <= MAX,
                "jittered delay for {failures} failures must not exceed MAX"
            );
        }
    }

    /// #6130: jitter widens the delay above the bare exponential but stays within
    /// [exp, exp * (1 + ratio)] and never below the exponential floor.
    #[test]
    fn next_delay_jitter_stays_within_bounds() {
        // Use failure 3 (exp = 4s) where exp + 25% = 5s < MAX, so jitter is
        // observable and not clamped by the cap.
        let exp = Duration::from_secs(4);
        let upper = Duration::from_millis(5000); // 4s + 25%
        for _ in 0..200 {
            let d = next_reconnect_delay(3, BASE, MAX, 0.25);
            assert!(
                d >= exp,
                "jittered delay {d:?} must be >= exponential floor {exp:?}"
            );
            assert!(
                d <= upper,
                "jittered delay {d:?} must be <= exp + 25% ({upper:?})"
            );
        }
    }

    /// #6130: a degenerate zero base or zero max collapses to no delay rather
    /// than panicking or overflowing.
    #[test]
    fn next_delay_degenerate_inputs_yield_zero() {
        assert_eq!(
            next_reconnect_delay(5, Duration::ZERO, MAX, 0.25),
            Duration::ZERO
        );
        assert_eq!(
            next_reconnect_delay(5, BASE, Duration::ZERO, 0.25),
            Duration::ZERO
        );
    }

    /// #6130: the give-up bound is positive so the loop terminates after a
    /// bounded number of consecutive failures.
    #[test]
    fn give_up_bound_is_positive() {
        assert!(RECONNECT_MAX_ATTEMPTS > 0);
    }

    /// sse-shutdown-deterministic: when the SSE loop receives a shutdown signal
    /// while `receiver.run()` is in-flight, it must call `receiver.shutdown()`
    /// so the inner SSE stream task is signalled + aborted + awaited
    /// deterministically — rather than leaving a detached task for the
    /// background runtime to drop. We assert this by spawning the loop with a
    /// receiver whose mock SSE client blocks forever, then signalling shutdown
    /// and confirming the loop task joins promptly.
    #[tokio::test]
    async fn loop_shutdown_deterministically_tears_down_receiver() {
        use maekon_core::error::CoreError;
        use maekon_core::ports::api_client::{SseClient, SseEvent};
        use maekon_suggestion::queue::SuggestionQueue;
        use maekon_suggestion::scorer::FeedbackScorer;
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::watch;
        use tokio::time::{timeout, Duration};

        // Mock SSE client that blocks until its task is aborted/cancelled. Its
        // `connect` future is only dropped when the inner stream task is
        // aborted by `shutdown()`, so the `connected` flag staying observable
        // proves the task actually started before teardown.
        struct BlockingSseClient {
            connected: Arc<AtomicBool>,
        }
        #[async_trait::async_trait]
        impl SseClient for BlockingSseClient {
            async fn connect(
                &self,
                _session_id: &str,
                _tx: tokio::sync::mpsc::Sender<SseEvent>,
            ) -> Result<(), CoreError> {
                self.connected.store(true, Ordering::SeqCst);
                // Block indefinitely; only cancellation/abort ends this future.
                std::future::pending::<()>().await;
                Ok(())
            }
        }

        let connected = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = Arc::new(SuggestionReceiver::new(
            Arc::new(BlockingSseClient {
                connected: connected.clone(),
            }) as Arc<dyn SseClient>,
            None,
            queue,
            scorer,
        ));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let loop_handle =
            spawn_suggestion_sse_loop(receiver, "sess-loop-shutdown".to_string(), shutdown_rx);

        // Let the loop spawn the inner task and start the blocking connect.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            connected.load(Ordering::SeqCst),
            "inner SSE task should have started before shutdown"
        );

        // Signal shutdown. The loop must call receiver.shutdown() (signal +
        // abort + await) before returning, so the loop task joins promptly
        // instead of leaving the blocked inner task to be dropped later.
        shutdown_tx.send(true).expect("shutdown send must succeed");

        timeout(Duration::from_millis(500), loop_handle)
            .await
            .expect("SSE loop did not return within 500 ms after shutdown signal")
            .expect("SSE loop task must not panic on shutdown");
    }
}
