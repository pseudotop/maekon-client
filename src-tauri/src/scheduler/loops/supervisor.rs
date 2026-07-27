//! Live loop supervisor: crash-respawn with capped exponential backoff.
//!
//! Every long-lived scheduler loop is wrapped by [`supervise_loop`], which owns
//! the loop for the lifetime of the session and:
//!
//!   * spawns the loop via its factory,
//!   * if the loop exits UNEXPECTEDLY during runtime (returns or panics before
//!     the global shutdown fires) logs it with the loop name and respawns a
//!     fresh instance after a capped exponential backoff (1s, 2s, 4s, … up to
//!     [`RESPAWN_MAX_DELAY`]), incrementing and logging an attempt counter, and
//!   * on shutdown gives the running loop a bounded window ([`GRACEFUL_DRAIN`])
//!     to run its own flush/drain arm before force-aborting it.
//!
//! A loop that stays alive at least [`STABLE_RESET_AFTER`] before dying resets
//! its backoff to the base delay, so a loop that crashes once and then runs
//! healthily does not inherit a long delay from an earlier incident — while a
//! hot-crashing loop is still backed off up to the cap instead of spinning.
//!
//! Prior art: `spawn_suggestion_sse_supervisor` (`loops/suggestions.rs`, #7099)
//! already respawns the SSE consumer after a permanent outage. This module
//! generalizes that pattern to EVERY scheduler loop — most importantly the
//! monitor loop, which owns screen capture: before this, a silent monitor-loop
//! death stopped capture for the whole session with only an `error!` log.

use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};
use tracing::{error, warn};

/// First respawn delay applied after a crash.
const RESPAWN_BASE_DELAY: Duration = Duration::from_secs(1);
/// Upper bound on the respawn backoff delay.
const RESPAWN_MAX_DELAY: Duration = Duration::from_secs(30);
/// A loop that runs at least this long before dying resets its backoff to base.
const STABLE_RESET_AFTER: Duration = Duration::from_secs(60);
/// Bounded window a loop gets to drain its graceful-shutdown arm before abort.
const GRACEFUL_DRAIN: Duration = Duration::from_secs(5);

/// Boxed loop factory: given a fresh shutdown receiver, (re)spawns the loop and
/// returns its `JoinHandle`. Held by the supervisor so a crashed loop can be
/// re-created without a full scheduler restart.
pub(in crate::scheduler) type LoopFactory<'a> =
    Box<dyn FnMut(watch::Receiver<bool>) -> JoinHandle<()> + Send + 'a>;

/// Defensive fallback for an OPTIONAL loop whose backing resource is missing on
/// a respawn attempt: a parked task that never completes on its own (so the
/// supervisor does not hot-respawn a `None`) and is reaped by the shutdown
/// abort. In practice the backing resource (coordinator / receiver / flags) is
/// captured in the factory closure and lives for the scheduler's lifetime, so
/// this path is not expected to be taken.
///
/// Only the analysis-gated optional loops (oauth_refresh / feature_perf_flush)
/// use this fallback today, so it is compiled under the same feature to avoid a
/// dead-code error in minimal builds.
#[cfg(feature = "analysis")]
pub(in crate::scheduler) fn park_task() -> JoinHandle<()> {
    tokio::spawn(std::future::pending::<()>())
}

/// Next backoff delay: double the current delay, capped at `max`.
fn next_backoff(current: Duration, max: Duration) -> Duration {
    current.saturating_mul(2).min(max)
}

/// Supervise ONE loop for the lifetime of the session using the production
/// backoff/drain constants. See the module docs for the full contract.
pub(in crate::scheduler) async fn supervise_loop(
    name: &'static str,
    factory: LoopFactory<'_>,
    shutdown_rx: watch::Receiver<bool>,
) {
    supervise_loop_with(
        name,
        factory,
        RESPAWN_BASE_DELAY,
        RESPAWN_MAX_DELAY,
        STABLE_RESET_AFTER,
        GRACEFUL_DRAIN,
        shutdown_rx,
    )
    .await
}

/// Timing-parameterized supervisor core (see [`supervise_loop`]). Split out so
/// the respawn/backoff/drain behaviour is unit-testable with millisecond delays
/// instead of the multi-second production constants.
#[allow(clippy::too_many_arguments)]
async fn supervise_loop_with(
    name: &'static str,
    mut factory: LoopFactory<'_>,
    base_delay: Duration,
    max_delay: Duration,
    stable_reset: Duration,
    graceful_drain: Duration,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let mut backoff = base_delay;
    let mut attempt: u32 = 0;

    loop {
        let started = Instant::now();
        let mut handle = factory(shutdown_rx.clone());
        let abort = handle.abort_handle();

        tokio::select! {
            biased; // a global shutdown always wins over a simultaneous exit
            _ = shutdown_rx.changed() => {
                // Graceful shutdown: the loop holds its own shutdown receiver and
                // is running its flush/drain arm. Give it a bounded window, then
                // force-abort if it overruns (mirrors the previous supervisor's
                // GRACEFUL_DRAIN-then-abort_all behaviour, now per loop).
                if timeout(graceful_drain, &mut handle).await.is_err() {
                    warn!(
                        loop_name = name,
                        timeout_secs = graceful_drain.as_secs(),
                        "scheduler loop did not drain within graceful window — aborting"
                    );
                    abort.abort();
                    let _ = handle.await;
                }
                return;
            }
            joined = &mut handle => {
                match joined {
                    Ok(()) => error!(
                        loop_name = name,
                        "scheduler loop exited unexpectedly during runtime — respawning"
                    ),
                    // A cancel without a shutdown signal is not produced by this
                    // supervisor (only the drain path aborts). Treat it as a
                    // teardown rather than respawning into a cancelled runtime.
                    Err(e) if e.is_cancelled() => return,
                    Err(e) => error!(
                        loop_name = name,
                        "scheduler loop panicked during runtime: {e} — respawning"
                    ),
                }
            }
        }

        // Reset the backoff if the loop had been stable for a while before dying,
        // so a single crash after a long healthy run respawns promptly.
        if started.elapsed() >= stable_reset {
            backoff = base_delay;
        }
        attempt = attempt.saturating_add(1);
        warn!(
            loop_name = name,
            attempt,
            backoff_secs = backoff.as_secs(),
            "respawning scheduler loop after backoff"
        );

        // Interruptible backoff sleep: a shutdown during the wait aborts respawn.
        tokio::select! {
            _ = tokio::time::sleep(backoff) => {}
            _ = shutdown_rx.changed() => return,
        }
        backoff = next_backoff(backoff, max_delay);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn next_backoff_doubles_and_caps() {
        let max = Duration::from_secs(30);
        assert_eq!(
            next_backoff(Duration::from_secs(1), max),
            Duration::from_secs(2)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(4), max),
            Duration::from_secs(8)
        );
        // Past the cap it clamps rather than growing unbounded.
        assert_eq!(next_backoff(Duration::from_secs(20), max), max);
        assert_eq!(next_backoff(max, max), max);
    }

    /// A loop that exits immediately (an unexpected runtime death) must be
    /// respawned repeatedly until shutdown — the core "capture no longer dies
    /// silently" guarantee.
    #[tokio::test]
    async fn respawns_loop_after_unexpected_exit() {
        let (tx, rx) = watch::channel(false);
        let spawn_count = Arc::new(AtomicU32::new(0));
        let counter = spawn_count.clone();

        let factory: LoopFactory = Box::new(move |_loop_rx| {
            counter.fetch_add(1, Ordering::SeqCst);
            // Returns immediately: looks like a loop that silently died.
            tokio::spawn(async {})
        });

        let supervisor = tokio::spawn(supervise_loop_with(
            "test",
            factory,
            Duration::from_millis(2),
            Duration::from_millis(10),
            Duration::from_secs(60),
            Duration::from_millis(50),
            rx,
        ));

        // Let several respawn cycles run, then request shutdown.
        tokio::time::sleep(Duration::from_millis(40)).await;
        tx.send(true).unwrap();
        supervisor.await.unwrap();

        let spawns = spawn_count.load(Ordering::SeqCst);
        assert!(
            spawns >= 2,
            "supervisor must respawn a crashed loop; it spawned only {spawns} time(s)"
        );
    }

    /// A healthy loop that runs until its own shutdown signal must be spawned
    /// exactly once and NOT respawned when the global shutdown drains it.
    #[tokio::test]
    async fn clean_shutdown_drains_without_respawn() {
        let (tx, rx) = watch::channel(false);
        let spawn_count = Arc::new(AtomicU32::new(0));
        let counter = spawn_count.clone();

        let factory: LoopFactory = Box::new(move |mut loop_rx| {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                // Stay alive until the per-instance shutdown receiver fires.
                let _ = loop_rx.changed().await;
            })
        });

        let supervisor = tokio::spawn(supervise_loop_with(
            "test",
            factory,
            Duration::from_millis(2),
            Duration::from_millis(10),
            Duration::from_secs(60),
            Duration::from_millis(100),
            rx,
        ));

        tokio::time::sleep(Duration::from_millis(20)).await;
        tx.send(true).unwrap();
        supervisor.await.unwrap();

        assert_eq!(
            spawn_count.load(Ordering::SeqCst),
            1,
            "a healthy loop must be spawned once and not respawned on clean shutdown"
        );
    }
}
