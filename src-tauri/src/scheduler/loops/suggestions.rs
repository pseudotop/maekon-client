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

// #7725: the escalation/cap/jitter delay math itself is NOT re-implemented
// here. It used to be (a ~85% re-implementation of
// `maekon_network::resilience::jittered_backoff_delay`); it is now computed
// directly via that shared helper at the `spawn_suggestion_sse_loop` call
// site below. `maekon-network` is already a dependency whenever the `server`
// feature (which gates this whole module) is enabled, via
// `server = ["analysis", ...]` -> `analysis = ["dep:maekon-network"]` in
// Cargo.toml.

/// Give-up bound: after this many consecutive failures without the stream ever
/// delivering a suggestion, stop retrying and let the loop exit. The
/// supervisor (`spawn_suggestion_sse_supervisor`, #7099) then respawns a fresh
/// consumer after a bounded cooldown — or sooner, the moment the server
/// connection is re-established — so the session recovers without a full
/// scheduler restart. This bound prevents an unbounded retry storm against a
/// permanently unreachable server within a single consumer instance.
#[cfg(feature = "server")]
const RECONNECT_MAX_ATTEMPTS: u32 = 12;

/// Minimum wall-clock lifetime a stream must stay open before an `Ok(true)`
/// (progress reported) is trusted enough to reset `consecutive_failures`
/// (#7617 / SUG-1).
///
/// `GrpcSseAdapter::connect` emits `SseEvent::Connected` the instant HTTP/2
/// headers arrive — well before the RPC's real status is known (a rejected or
/// immediately-broken stream can still complete the handshake). `receiver.run`
/// counts that bare `Connected` event as progress (#7080, correctly — a
/// healthy-but-idle server should not look like an outage). Without this
/// lifetime floor, a stream that connects and then immediately
/// closes/errors (empty/idle-close, a post-auth rejection, a Caddy h2c
/// upstream RST) would report `Ok(true)` on every attempt and reset the
/// backoff every time, bypassing the escalating backoff (#6130), the give-up
/// bound (`RECONNECT_MAX_ATTEMPTS`), and the respawn supervisor (#7099) —
/// producing a ~1 reconnect/sec storm against a reachable-but-broken-stream
/// server.
#[cfg(feature = "server")]
const MIN_PRODUCTIVE_STREAM_LIFETIME: Duration = Duration::from_secs(2);

/// Bounded cooldown the supervisor waits after a consumer gives up (permanent
/// outage) before respawning a fresh one (#7099). Long enough that a
/// permanently-down server is not hammered with back-to-back full retry cycles,
/// short enough that recovery is automatic once the server returns.
#[cfg(feature = "server")]
const RESPAWN_COOLDOWN: Duration = Duration::from_secs(60);

/// Polling granularity inside the respawn cooldown (#7099). Short enough that a
/// recovered server connection (`server_connected` false -> true) cuts the
/// cooldown short promptly, long enough that the idle wait stays cheap.
#[cfg(feature = "server")]
const RESPAWN_COOLDOWN_POLL: Duration = Duration::from_secs(2);

/// SSE/gRPC suggestion reception loop with escalating reconnect backoff (#6130).
///
/// `receiver.run()` returns `Ok(true)` when the stream delivered at least one
/// suggestion and `Ok(false)`/`Err(_)` otherwise. On every non-productive
/// return the consecutive-failure counter escalates the reconnect delay
/// (`RECONNECT_BASE_DELAY` → `RECONNECT_MAX_DELAY`, with jitter). The counter is
/// reset to 0 only when the stream actually delivered a suggestion, so a healthy
/// server that briefly closes the stream still reconnects quickly while a down
/// server is backed off. After `RECONNECT_MAX_ATTEMPTS` consecutive failures the
/// loop gives up and returns; the supervisor
/// ([`spawn_suggestion_sse_supervisor`], #7099) then respawns a fresh consumer.
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
            let attempt_started = tokio::time::Instant::now();
            tokio::select! {
                result = receiver.run(&session_id) => {
                    let lived = attempt_started.elapsed();
                    match result {
                        Ok(true) if lived >= MIN_PRODUCTIVE_STREAM_LIFETIME => {
                            // Stream delivered ≥1 suggestion (or connected/
                            // heartbeated) and stayed open long enough to trust
                            // that as real progress: it was healthy. Reset
                            // backoff and reconnect promptly.
                            consecutive_failures = 0;
                            info!("SSE stream closed after delivering suggestions, will reconnect");
                        }
                        Ok(true) => {
                            // #7617 (SUG-1): progress was reported (e.g. a bare
                            // `Connected` event) but the stream closed again
                            // almost immediately -- too quickly to trust as a
                            // genuinely healthy connection. Treat this the same
                            // as a failed attempt so an immediately-closing/
                            // erroring stream cannot bypass the escalating
                            // backoff by resetting the counter every cycle.
                            consecutive_failures = consecutive_failures.saturating_add(1);
                            warn!(
                                consecutive_failures,
                                lived_ms = lived.as_millis() as u64,
                                "SSE stream reported progress but closed too quickly to trust \
                                 (< {MIN_PRODUCTIVE_STREAM_LIFETIME:?}); backing off and reconnecting"
                            );
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
                     supervisor will respawn after cooldown or on server reconnect"
                );
                break;
            }

            let delay = if consecutive_failures == 0 {
                // Productive stream just closed — reconnect promptly (small fixed
                // delay only to avoid a tight loop on rapid open/close churn).
                RECONNECT_BASE_DELAY
            } else {
                // #7725: `consecutive_failures` here is 1-based (>= 1, guarded
                // by the branch above), while `jittered_backoff_delay` takes a
                // 0-based `attempt`.
                maekon_network::resilience::jittered_backoff_delay(
                    consecutive_failures - 1,
                    RECONNECT_BASE_DELAY,
                    RECONNECT_MAX_DELAY,
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

/// Supervise the suggestion SSE consumer and **respawn** it after a permanent
/// outage instead of leaving the session without suggestions until the next
/// full scheduler restart (#7099).
///
/// `spawn_suggestion_sse_loop` already owns an internal reconnect backoff and
/// only returns when it hits the give-up bound (`RECONNECT_MAX_ATTEMPTS`
/// consecutive real outages) or on shutdown. Before #7099 the generic scheduler
/// supervisor merely logged that exit ("scheduler loop exited unexpectedly
/// during runtime") and the session then received no further suggestions until
/// the user re-logged in. This supervisor instead:
///
///   • spawns the consumer on a per-instance shutdown channel so it can be
///     stopped CLEANLY (its own loop runs `receiver.shutdown()`), rather than
///     hard-aborting it and detaching the inner SSE stream task;
///   • when the consumer gives up (permanent outage), waits a BOUNDED cooldown
///     (`respawn_cooldown`) — so a permanently-down server is not hammered with
///     back-to-back full retry cycles — then respawns a fresh consumer, which
///     resets the consecutive-failure counter and re-establishes the session
///     (refreshing the auth token inside `connect`);
///   • cuts the cooldown short the moment the server connection is
///     re-established (`server_connected` transitions false -> true) — the
///     "session refresh" the old give-up log alluded to. The transition (not
///     the level) is used so an SSE-specific failure while the server is
///     otherwise reachable waits the full cooldown instead of hot-looping;
///   • on global shutdown, stops the current consumer cleanly and returns.
///
/// Generic over the consumer-spawn closure so the respawn / cancel logic is
/// unit-testable with a fake consumer (no live server or SSE transport).
#[cfg(feature = "server")]
async fn run_suggestion_sse_supervisor<F>(
    mut spawn_consumer: F,
    server_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    respawn_cooldown: Duration,
    cooldown_poll: Duration,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) where
    F: FnMut(tokio::sync::watch::Receiver<bool>) -> tokio::task::JoinHandle<()> + Send,
{
    use std::sync::atomic::Ordering;

    loop {
        // Per-instance shutdown channel: lets the supervisor stop exactly ONE
        // consumer cleanly (its loop observes the change and runs
        // `receiver.shutdown()`), rather than aborting it and detaching the
        // inner SSE stream task.
        let (inst_shutdown_tx, inst_shutdown_rx) = tokio::sync::watch::channel(false);
        let mut consumer = spawn_consumer(inst_shutdown_rx);

        // Phase 1: run until the consumer exits on its own or shutdown fires.
        tokio::select! {
            biased; // honour a global shutdown over a simultaneous give-up
            _ = shutdown_rx.changed() => {
                // Global shutdown: stop the consumer cleanly, then exit.
                let _ = inst_shutdown_tx.send(true);
                let _ = consumer.await;
                return;
            }
            joined = &mut consumer => {
                match joined {
                    Ok(()) => warn!(
                        "suggestion SSE consumer gave up after a permanent outage; \
                         supervisor will respawn after cooldown or on server reconnect"
                    ),
                    Err(e) if e.is_cancelled() => warn!(
                        "suggestion SSE consumer was cancelled unexpectedly; \
                         supervisor will respawn after cooldown or on server reconnect"
                    ),
                    Err(e) => warn!(
                        "suggestion SSE consumer panicked: {e}; \
                         supervisor will respawn after cooldown or on server reconnect"
                    ),
                }
            }
        }

        // Phase 2: bounded respawn cooldown. Snapshot the connection state at
        // give-up so we respawn PROMPTLY only on a genuine reconnect transition
        // (false -> true). A connection that was already up at give-up means the
        // failure was SSE-specific, so we wait the full cooldown rather than
        // hot-looping on the unchanged "connected" level.
        let connected_at_giveup = server_connected
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed));

        let mut waited = Duration::ZERO;
        loop {
            if !connected_at_giveup
                && server_connected
                    .as_ref()
                    .is_some_and(|f| f.load(Ordering::Relaxed))
            {
                info!("server connection restored — respawning suggestion SSE consumer");
                break;
            }
            if waited >= respawn_cooldown {
                info!("respawn cooldown elapsed — respawning suggestion SSE consumer");
                break;
            }
            let step = cooldown_poll
                .min(respawn_cooldown - waited)
                .max(Duration::from_millis(1));
            tokio::select! {
                _ = tokio::time::sleep(step) => waited += step,
                _ = shutdown_rx.changed() => return,
            }
        }
    }
}

/// Spawn the suggestion SSE supervisor (#7099): owns the SSE consumer task and
/// respawns it on permanent outage / server reconnect. Production wrapper around
/// [`run_suggestion_sse_supervisor`] that builds the consumer-spawn closure from
/// the real `SuggestionReceiver` and uses the default cooldown constants.
#[cfg(feature = "server")]
pub(crate) fn spawn_suggestion_sse_supervisor(
    receiver: Arc<SuggestionReceiver>,
    session_id: String,
    server_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let spawn_consumer = move |inst_shutdown_rx: tokio::sync::watch::Receiver<bool>| {
            spawn_suggestion_sse_loop(receiver.clone(), session_id.clone(), inst_shutdown_rx)
        };
        run_suggestion_sse_supervisor(
            spawn_consumer,
            server_connected,
            RESPAWN_COOLDOWN,
            RESPAWN_COOLDOWN_POLL,
            shutdown_rx,
        )
        .await;
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

    // #7725: these tests used to exercise a hand-rolled `next_reconnect_delay`
    // free function local to this module. That function has been removed —
    // the call site now delegates directly to
    // `maekon_network::resilience::jittered_backoff_delay` — so the tests
    // below exercise the shared helper with this module's own tuning
    // (`RECONNECT_BASE_DELAY` / `RECONNECT_MAX_DELAY`), pinning this call
    // site's envelope rather than the (now-deleted) private function.
    //
    // `consecutive_failures` at the call site is 1-based; `attempt` below is
    // the shared helper's 0-based equivalent (`attempt == consecutive_failures - 1`).

    /// #6130/#7725: without jitter, the delay doubles per attempt
    /// (1s → 2s → 4s → 8s → 16s) until it saturates at the 30s cap.
    #[test]
    fn next_delay_escalates_exponentially_no_jitter() {
        use maekon_network::resilience::exponential_delay;
        assert_eq!(exponential_delay(0, BASE, MAX), Duration::from_secs(1));
        assert_eq!(exponential_delay(1, BASE, MAX), Duration::from_secs(2));
        assert_eq!(exponential_delay(2, BASE, MAX), Duration::from_secs(4));
        assert_eq!(exponential_delay(3, BASE, MAX), Duration::from_secs(8));
        assert_eq!(exponential_delay(4, BASE, MAX), Duration::from_secs(16));
    }

    /// #6130/#7725: the delay is capped at MAX (~30s) regardless of how high
    /// the attempt count grows, with or without jitter applied.
    #[test]
    fn next_delay_is_capped_at_max() {
        use maekon_network::resilience::{exponential_delay, jittered_backoff_delay};
        for failures in [6u32, 10, 32, 63, u32::MAX] {
            let attempt = failures.saturating_sub(1);
            // No-jitter: exponential alone already exceeds MAX from failure 6 on.
            assert!(
                exponential_delay(attempt, BASE, MAX) <= MAX,
                "no-jitter delay for {failures} failures must not exceed MAX"
            );
            // With jitter: must STILL be clamped to MAX, never above.
            assert!(
                jittered_backoff_delay(attempt, BASE, MAX) <= MAX,
                "jittered delay for {failures} failures must not exceed MAX"
            );
        }
    }

    /// #6130/#7725: jitter widens the delay above the bare exponential but
    /// stays within [exp, exp * 1.25] and never below the exponential floor.
    #[test]
    fn next_delay_jitter_stays_within_bounds() {
        use maekon_network::resilience::{exponential_delay, jittered_backoff_delay};
        // Failure 3 -> attempt 2 (exp = 4s), where exp + 25% = 5s < MAX, so
        // jitter is observable and not clamped by the cap.
        let attempt = 2;
        let exp = exponential_delay(attempt, BASE, MAX);
        assert_eq!(exp, Duration::from_secs(4));
        let upper = Duration::from_millis(5000); // 4s + 25%
        for _ in 0..200 {
            let d = jittered_backoff_delay(attempt, BASE, MAX);
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

    /// #6130/#7725: a degenerate zero base or zero max collapses to no delay
    /// rather than panicking or overflowing.
    #[test]
    fn next_delay_degenerate_inputs_yield_zero() {
        use maekon_network::resilience::jittered_backoff_delay;
        assert_eq!(
            jittered_backoff_delay(4, Duration::ZERO, MAX),
            Duration::ZERO
        );
        assert_eq!(
            jittered_backoff_delay(4, BASE, Duration::ZERO),
            Duration::ZERO
        );
    }

    /// #6130: the give-up bound is positive so the loop terminates after a
    /// bounded number of consecutive failures.
    #[test]
    fn give_up_bound_is_positive() {
        const _: () = assert!(RECONNECT_MAX_ATTEMPTS > 0);
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

    /// #7099: when the suggestion SSE consumer gives up (permanent outage), the
    /// supervisor must RESPAWN a fresh consumer after the bounded cooldown —
    /// not merely log and leave the session suggestion-less. A fake consumer
    /// that exits immediately on its first spawn and then blocks proves exactly
    /// one respawn occurs and that the live consumer is not re-spawned again.
    #[tokio::test]
    async fn supervisor_respawns_consumer_after_giveup() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtOrdering};
        use tokio::sync::watch;
        use tokio::time::{timeout, Duration as TDuration};

        let spawn_count = Arc::new(AtomicUsize::new(0));
        let sc = spawn_count.clone();
        // 1st consumer "gives up" immediately; the 2nd blocks until cleanly
        // cancelled — bounding the total spawn count to 2.
        let spawn_consumer = move |mut inst_rx: watch::Receiver<bool>| {
            let n = sc.fetch_add(1, AtOrdering::SeqCst);
            tokio::spawn(async move {
                if n == 0 {
                    // Give up: return immediately (simulates the give-up bound).
                } else {
                    let _ = inst_rx.changed().await;
                }
            })
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = tokio::spawn(run_suggestion_sse_supervisor(
            spawn_consumer,
            None,
            TDuration::from_millis(30),
            TDuration::from_millis(5),
            shutdown_rx,
        ));

        // Give-up + cooldown (30 ms) + respawn should have happened well within
        // this window; the respawned consumer then blocks, so the count stays 2.
        tokio::time::sleep(TDuration::from_millis(200)).await;
        assert_eq!(
            spawn_count.load(AtOrdering::SeqCst),
            2,
            "supervisor must respawn the consumer exactly once after a give-up"
        );

        // The respawned consumer is alive; shutdown must stop it cleanly.
        shutdown_tx.send(true).expect("shutdown send must succeed");
        timeout(TDuration::from_millis(500), sup)
            .await
            .expect("supervisor must return promptly after shutdown")
            .expect("supervisor task must not panic");
        assert_eq!(
            spawn_count.load(AtOrdering::SeqCst),
            2,
            "no further respawn after a clean shutdown"
        );
    }

    /// #7099: a server-connection recovery (the "session refresh") must cut the
    /// respawn cooldown short and respawn the consumer PROMPTLY, rather than
    /// waiting out the full (here: 30 s) cooldown.
    #[tokio::test]
    async fn supervisor_respawns_promptly_on_server_reconnect() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtOrdering};
        use tokio::sync::watch;
        use tokio::time::{timeout, Duration as TDuration};

        let spawn_count = Arc::new(AtomicUsize::new(0));
        let sc = spawn_count.clone();
        let spawn_consumer = move |mut inst_rx: watch::Receiver<bool>| {
            let n = sc.fetch_add(1, AtOrdering::SeqCst);
            tokio::spawn(async move {
                if n == 0 {
                    // Give up immediately while the server is "down".
                } else {
                    let _ = inst_rx.changed().await;
                }
            })
        };

        // Server starts disconnected, so the give-up snapshot is false and a
        // later false -> true transition is treated as the reconnect signal.
        let server_connected = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = tokio::spawn(run_suggestion_sse_supervisor(
            spawn_consumer,
            Some(server_connected.clone()),
            TDuration::from_secs(30), // long cooldown — only a reconnect cuts it short
            TDuration::from_millis(5),
            shutdown_rx,
        ));

        // Let the 1st consumer give up and enter the long cooldown.
        tokio::time::sleep(TDuration::from_millis(60)).await;
        assert_eq!(
            spawn_count.load(AtOrdering::SeqCst),
            1,
            "consumer must still be in cooldown (no respawn before reconnect)"
        );

        // Server connection restored — respawn must happen well under 30 s.
        server_connected.store(true, AtOrdering::SeqCst);
        let respawned = async {
            while spawn_count.load(AtOrdering::SeqCst) < 2 {
                tokio::time::sleep(TDuration::from_millis(5)).await;
            }
        };
        timeout(TDuration::from_millis(500), respawned)
            .await
            .expect("server reconnect must trigger a prompt respawn, not wait the full cooldown");

        shutdown_tx.send(true).expect("shutdown send must succeed");
        timeout(TDuration::from_millis(500), sup)
            .await
            .expect("supervisor must return promptly after shutdown")
            .expect("supervisor task must not panic");
    }

    /// #7099: a clean shutdown must stop the live consumer via its per-instance
    /// shutdown channel (clean cancel, not a hard abort) and must NOT respawn.
    #[tokio::test]
    async fn supervisor_clean_shutdown_cancels_consumer_without_respawn() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtOrdering};
        use tokio::sync::watch;
        use tokio::time::{timeout, Duration as TDuration};

        let spawn_count = Arc::new(AtomicUsize::new(0));
        let cancelled_cleanly = Arc::new(AtomicBool::new(false));
        let sc = spawn_count.clone();
        let cc = cancelled_cleanly.clone();
        let spawn_consumer = move |mut inst_rx: watch::Receiver<bool>| {
            sc.fetch_add(1, AtOrdering::SeqCst);
            let cc = cc.clone();
            tokio::spawn(async move {
                // Live consumer: stop only on a clean per-instance cancel signal,
                // then record that the cancel was observed (not aborted).
                let _ = inst_rx.changed().await;
                cc.store(true, AtOrdering::SeqCst);
            })
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = tokio::spawn(run_suggestion_sse_supervisor(
            spawn_consumer,
            None,
            TDuration::from_millis(30),
            TDuration::from_millis(5),
            shutdown_rx,
        ));

        tokio::time::sleep(TDuration::from_millis(40)).await;
        assert_eq!(
            spawn_count.load(AtOrdering::SeqCst),
            1,
            "a live consumer must not be respawned"
        );

        shutdown_tx.send(true).expect("shutdown send must succeed");
        timeout(TDuration::from_millis(500), sup)
            .await
            .expect("supervisor must return promptly after shutdown")
            .expect("supervisor task must not panic");

        assert!(
            cancelled_cleanly.load(AtOrdering::SeqCst),
            "supervisor must stop the consumer via its clean per-instance shutdown"
        );
        assert_eq!(
            spawn_count.load(AtOrdering::SeqCst),
            1,
            "no respawn on a clean shutdown"
        );
    }

    /// #7617 (MED finding #5 / SUG-1): a stream that emits `Connected` (or a
    /// `Heartbeat`) and then closes again almost immediately -- mirroring
    /// `GrpcSseAdapter` sending `Connected` the instant HTTP/2 headers arrive,
    /// well before the RPC's real status is known -- must NOT reset the
    /// backoff on every attempt. It must escalate through every attempt like
    /// any other failed connection and eventually hit the give-up bound.
    ///
    /// Uses a paused tokio clock, driven forward with explicit
    /// `tokio::time::advance` + `yield_now` steps so the (up to ~30s-capped,
    /// jittered) reconnect delays resolve without any real wall-clock
    /// sleeping.
    #[tokio::test(start_paused = true)]
    async fn immediate_close_stream_does_not_bypass_backoff_and_reaches_giveup() {
        use maekon_core::error::CoreError;
        use maekon_core::ports::api_client::{SseClient, SseEvent};
        use maekon_suggestion::queue::SuggestionQueue;
        use maekon_suggestion::scorer::FeedbackScorer;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::watch;
        use tokio::time::timeout;

        /// Mimics `GrpcSseAdapter::connect`'s spawned task: emits `Connected`
        /// immediately, then returns (closing the event channel) without ever
        /// delivering a suggestion or living for any meaningful duration.
        struct ImmediateCloseSseClient {
            connect_calls: Arc<AtomicUsize>,
        }
        #[async_trait::async_trait]
        impl SseClient for ImmediateCloseSseClient {
            async fn connect(
                &self,
                _session_id: &str,
                tx: tokio::sync::mpsc::Sender<SseEvent>,
            ) -> Result<(), CoreError> {
                self.connect_calls.fetch_add(1, Ordering::SeqCst);
                let _ = tx
                    .send(SseEvent::Connected {
                        session_id: "sess-immediate-close".to_string(),
                    })
                    .await;
                // Returning here drops `tx`, closing the channel -- the
                // stream "connects" and immediately ends, exactly like a
                // Caddy h2c upstream RST or an immediate post-auth rejection.
                Ok(())
            }
        }

        let connect_calls = Arc::new(AtomicUsize::new(0));
        let queue = Arc::new(Mutex::new(SuggestionQueue::new(50)));
        let scorer = Arc::new(Mutex::new(FeedbackScorer::new()));
        let receiver = Arc::new(SuggestionReceiver::new(
            Arc::new(ImmediateCloseSseClient {
                connect_calls: connect_calls.clone(),
            }) as Arc<dyn SseClient>,
            None,
            queue,
            scorer,
        ));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let loop_handle =
            spawn_suggestion_sse_loop(receiver, "sess-immediate-close".to_string(), shutdown_rx);

        // Drive the paused clock forward in small steps, yielding so the
        // background loop task can react to each timer advance. Worst-case
        // total delay to exhaust RECONNECT_MAX_ATTEMPTS (12) is bounded well
        // under this budget even with jitter.
        for _ in 0..3000 {
            tokio::time::advance(Duration::from_millis(200)).await;
            tokio::task::yield_now().await;
        }

        let total_attempts = connect_calls.load(Ordering::SeqCst);
        assert_eq!(
            total_attempts, RECONNECT_MAX_ATTEMPTS as usize,
            "an immediately-closing stream must escalate through every attempt up to \
             the give-up bound instead of resetting the backoff and retrying forever"
        );

        // The loop already self-terminated after hitting the give-up bound
        // (dropping `shutdown_rx`), so `shutdown_tx.send` would race an
        // already-gone receiver -- just await the handle, which must resolve
        // immediately since the task has already finished.
        drop(shutdown_tx);
        timeout(Duration::from_secs(5), loop_handle)
            .await
            .expect("SSE loop did not return after hitting the give-up bound")
            .expect("SSE loop task must not panic");
    }
}
