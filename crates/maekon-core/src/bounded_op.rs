//! Bounded execution for blocking OS calls that can stall indefinitely (#9588).
//!
//! macOS Security.framework blocks a keychain operation for as long as an ACL
//! consent prompt is pending — e.g. a binary at a new path touching an item
//! created by another build. During boot, before any window exists, that
//! presents as a silent indefinite hang with no log signal. Running the call
//! on a dedicated thread and giving up after a bounded wait converts the hang
//! into an explicit, logged error that existing fallback paths can absorb.
//!
//! The worker thread is detached on timeout: if the user later answers the OS
//! prompt, the call completes in the background and its result is dropped.
//! That is deliberate — the alternative (killing the thread) is unsound, and
//! the caller has already taken its fallback path. To keep detached threads
//! from accumulating without bound while the keychain stays wedged (review
//! #9593 I1: several callers retry on 1–5 minute loops), a process-global
//! [`WedgeLatch`] short-circuits further ops after a timeout and only lets a
//! single probe through per cooldown window; a completed op re-opens the
//! latch. This also collapses multi-key sweeps (e.g. a 7-key namespace probe)
//! from N×timeout to roughly one timeout (review I2).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Default bound for a single OS keychain operation (#9588). Long enough for a
/// slow-but-healthy keychain backend, short enough that a boot blocked on an
/// unanswerable ACL prompt degrades in seconds instead of hanging forever.
pub const DEFAULT_KEYCHAIN_OP_TIMEOUT: Duration = Duration::from_secs(15);

/// While wedged, at most one probe op per this window actually spawns a
/// worker; everything else fails fast without touching the OS.
const WEDGE_PROBE_COOLDOWN: Duration = Duration::from_secs(30);

/// Failure modes of [`run_keychain_bounded`] itself (the inner operation's own errors
/// are returned through its `T`).
#[derive(Debug, thiserror::Error)]
pub enum BoundedOpError {
    #[error("bounded op '{op}': worker thread spawn failed: {reason}")]
    SpawnFailed { op: String, reason: String },
    #[error(
        "bounded op '{op}' timed out after {waited_secs}s \
         (possible pending OS keychain ACL prompt)"
    )]
    TimedOut { op: String, waited_secs: u64 },
    #[error(
        "bounded op '{op}' short-circuited: an earlier keychain op timed out and \
         the cooldown has not elapsed (wedged OS keychain)"
    )]
    Wedged { op: String },
}

/// Process-wide wedge state for one blocking OS resource (here: the single OS
/// keychain). Injectable so tests never share state with the global latch.
pub struct WedgeLatch {
    wedged: AtomicBool,
    last_attempt: parking_lot::Mutex<Option<Instant>>,
    cooldown: Duration,
}

impl WedgeLatch {
    pub const fn new(cooldown: Duration) -> Self {
        Self {
            wedged: AtomicBool::new(false),
            last_attempt: parking_lot::Mutex::new(None),
            cooldown,
        }
    }

    /// Whether an op may run now. While wedged, admits one probe per cooldown
    /// window (recording the attempt); otherwise callers fail fast.
    fn admit(&self, now: Instant) -> bool {
        if !self.wedged.load(Ordering::Acquire) {
            return true;
        }
        let mut last = self.last_attempt.lock();
        let due = wedge_probe_due(last.map(|t| now.duration_since(t)), self.cooldown);
        if due {
            *last = Some(now);
        }
        due
    }

    fn record_timeout(&self, now: Instant) {
        self.wedged.store(true, Ordering::Release);
        *self.last_attempt.lock() = Some(now);
    }

    fn record_success(&self) {
        self.wedged.store(false, Ordering::Release);
    }
}

/// Pure probe-admission rule (unit-testable): with no recorded attempt the
/// probe runs immediately; otherwise only after the cooldown has elapsed.
fn wedge_probe_due(since_last_attempt: Option<Duration>, cooldown: Duration) -> bool {
    match since_last_attempt {
        None => true,
        Some(elapsed) => elapsed >= cooldown,
    }
}

/// The one latch shared by every OS-keychain op in this process.
///
/// Private on purpose: joining it is what [`run_keychain_bounded`] means, and
/// a caller that wants bounded execution for a DIFFERENT resource must bring
/// its own via [`run_bounded_with_latch`] rather than inherit the keychain's
/// wedge state.
static KEYCHAIN_LATCH: WedgeLatch = WedgeLatch::new(WEDGE_PROBE_COOLDOWN);

/// Run an **OS keychain** `op` on a dedicated thread, waiting at most
/// `timeout`, gated by the process-global keychain [`WedgeLatch`].
///
/// On timeout the worker thread keeps running detached (its eventual result is
/// dropped), the latch wedges, and the caller gets
/// [`BoundedOpError::TimedOut`] — it should take its fallback path. While
/// wedged, calls return [`BoundedOpError::Wedged`] without spawning anything,
/// except one probe per cooldown window.
///
/// # Why the name says "keychain" (#9598 item 2)
///
/// This was `run_bounded` — a resource-neutral name on a `pub` function that
/// silently joins a latch scoped to one specific resource. The doc said
/// "the single OS keychain", but nothing stopped the next blocking call from
/// reaching for the obvious name and inheriting a wedge state that has nothing
/// to do with it: one stalled keychain prompt would then fail-fast an unrelated
/// subsystem, and that subsystem's own timeout would wedge the keychain back.
///
/// The name is now the constraint. A caller with a different resource is
/// pushed to [`run_bounded_with_latch`] with its own latch, which is the
/// correct thing to do and is now possible to express.
///
/// (The ambiguity was not hypothetical: `maekon-monitor`'s X11 active-window
/// path already defines its own local `run_bounded` with a separate breaker.)
pub fn run_keychain_bounded<T, F>(
    op_name: &str,
    timeout: Duration,
    op: F,
) -> Result<T, BoundedOpError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    run_bounded_with_latch(&KEYCHAIN_LATCH, op_name, timeout, op)
}

/// Bounded execution against a caller-supplied [`WedgeLatch`].
///
/// Public so a non-keychain blocking resource can get the same
/// spawn-and-give-up behaviour **without** sharing the keychain's wedge state.
/// Give each resource its own `static WedgeLatch`; sharing one across resources
/// is the trap [`run_keychain_bounded`] documents.
pub fn run_bounded_with_latch<T, F>(
    latch: &WedgeLatch,
    op_name: &str,
    timeout: Duration,
    op: F,
) -> Result<T, BoundedOpError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    if !latch.admit(Instant::now()) {
        tracing::warn!(
            "#9588: bounded op '{op_name}' short-circuited — the OS keychain is \
             wedged (an earlier op timed out); the next probe runs after the \
             cooldown"
        );
        return Err(BoundedOpError::Wedged {
            op: op_name.to_owned(),
        });
    }

    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name(format!("bounded-{op_name}"))
        .spawn(move || {
            // The receiver is gone after a timeout; a failed send is expected.
            let _ = tx.send(op());
        });
    if let Err(e) = spawned {
        return Err(BoundedOpError::SpawnFailed {
            op: op_name.to_owned(),
            reason: e.to_string(),
        });
    }

    let started = Instant::now();
    match rx.recv_timeout(timeout) {
        Ok(result) => {
            latch.record_success();
            let elapsed = started.elapsed();
            // A slow-but-completed op is the early signal of the same ACL
            // stall the timeout guards against — surface it before it grows
            // into a timeout on a later run.
            if elapsed > Duration::from_secs(2) {
                tracing::warn!(
                    "#9588: bounded op '{op_name}' completed but took {}ms — \
                     the OS keychain may be waiting on user interaction",
                    elapsed.as_millis()
                );
            }
            Ok(result)
        }
        Err(_) => {
            latch.record_timeout(Instant::now());
            // Deliberately NO advice to delete keychain items here: for the
            // sealed master key, deleting the item while a stale .db_key sits
            // on disk primes the migration path to seal the wrong key
            // (review #9593 B1). Approving the prompt or relaunching from the
            // original install location are the safe remediations.
            tracing::warn!(
                "#9588: bounded op '{op_name}' did not complete within {}s — an OS \
                 keychain ACL prompt is likely pending (a binary at a new path \
                 touching an item created by another build). Continuing on the \
                 fallback path; approve the OS keychain dialog, or relaunch the \
                 app from its original install location.",
                timeout.as_secs()
            );
            Err(BoundedOpError::TimedOut {
                op: op_name.to_owned(),
                waited_secs: timeout.as_secs(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_op_returns_its_result() {
        let latch = WedgeLatch::new(Duration::from_secs(30));
        let result = run_bounded_with_latch(&latch, "fast", Duration::from_secs(5), || 40 + 2);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn fast_op_propagates_inner_errors_through_t() {
        let latch = WedgeLatch::new(Duration::from_secs(30));
        let result: Result<Result<(), String>, _> =
            run_bounded_with_latch(&latch, "inner-err", Duration::from_secs(5), || {
                Err("keychain: denied".to_owned())
            });
        // The bounded layer succeeds; the inner op error is the payload.
        assert_eq!(result.unwrap().unwrap_err(), "keychain: denied");
    }

    #[test]
    fn stalled_op_times_out_promptly_with_guidance_error() {
        let latch = WedgeLatch::new(Duration::from_secs(30));
        let started = Instant::now();
        let result = run_bounded_with_latch(&latch, "stalled", Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(5));
            1
        });
        let elapsed = started.elapsed();

        let err = result.unwrap_err();
        assert!(
            matches!(err, BoundedOpError::TimedOut { .. }),
            "expected TimedOut, got: {err:?}"
        );
        assert!(
            err.to_string().contains("timed out"),
            "error must be self-describing: {err}"
        );
        // The caller must get control back at the timeout, not at op
        // completion (5s). Generous margin for slow CI.
        assert!(
            elapsed < Duration::from_secs(4),
            "timeout must not wait for the stalled op ({elapsed:?})"
        );
    }

    #[test]
    fn wedged_latch_short_circuits_within_cooldown_and_reopens_on_success() {
        let latch = WedgeLatch::new(Duration::from_secs(3600));
        // First op times out → latch wedges.
        let first = run_bounded_with_latch(&latch, "wedge", Duration::from_millis(30), || {
            std::thread::sleep(Duration::from_secs(5));
            1
        });
        assert!(matches!(first, Err(BoundedOpError::TimedOut { .. })));

        // Within the cooldown: fail fast, no spawn, no waiting.
        let started = Instant::now();
        let second = run_bounded_with_latch(&latch, "wedge", Duration::from_secs(5), || 2);
        assert!(
            matches!(second, Err(BoundedOpError::Wedged { .. })),
            "within cooldown the latch must short-circuit: {second:?}"
        );
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "short-circuit must not wait on any timeout"
        );

        // A successful probe re-opens the latch. Simulate cooldown expiry by
        // clearing the last attempt (equivalent to time passing).
        *latch.last_attempt.lock() = None;
        let third = run_bounded_with_latch(&latch, "wedge", Duration::from_secs(5), || 3);
        assert_eq!(third.unwrap(), 3);
        // Latch re-opened: subsequent ops run normally again.
        let fourth = run_bounded_with_latch(&latch, "wedge", Duration::from_secs(5), || 4);
        assert_eq!(fourth.unwrap(), 4);
    }

    /// #9598 item 2: a wedge is scoped to ONE latch, so a second resource that
    /// brings its own is unaffected.
    ///
    /// This is the property the old `run_bounded` name quietly denied: it was
    /// `pub`, resource-neutral, and hardwired to the keychain latch, so the next
    /// blocking subsystem that reached for the obvious name would have inherited
    /// a stall that had nothing to do with it. The fix is only real if a
    /// separate latch is actually reachable from outside this module — which is
    /// what `run_bounded_with_latch` being `pub` buys, and what this pins.
    #[test]
    fn a_wedge_on_one_latch_does_not_short_circuit_another_resource() {
        let keychain_like = WedgeLatch::new(Duration::from_secs(3600));
        let other_resource = WedgeLatch::new(Duration::from_secs(3600));

        let stalled =
            run_bounded_with_latch(&keychain_like, "res-a", Duration::from_millis(30), || {
                std::thread::sleep(Duration::from_secs(5));
                1
            });
        assert!(matches!(stalled, Err(BoundedOpError::TimedOut { .. })));

        // Same process, same cooldown window, different resource: must run.
        let unaffected =
            run_bounded_with_latch(&other_resource, "res-b", Duration::from_secs(5), || 2);
        assert_eq!(
            unaffected.unwrap(),
            2,
            "one resource's wedge must not fail-fast another's ops"
        );

        // …and the wedged one is still wedged (the isolation is not accidental
        // leniency).
        let still_wedged =
            run_bounded_with_latch(&keychain_like, "res-a", Duration::from_secs(5), || 3);
        assert!(
            matches!(still_wedged, Err(BoundedOpError::Wedged { .. })),
            "the originally wedged latch must stay wedged: {still_wedged:?}"
        );
    }

    #[test]
    fn wedge_probe_admission_rule() {
        let cd = Duration::from_secs(30);
        assert!(
            wedge_probe_due(None, cd),
            "no recorded attempt → probe runs"
        );
        assert!(
            !wedge_probe_due(Some(Duration::from_secs(1)), cd),
            "inside cooldown → fail fast"
        );
        assert!(
            wedge_probe_due(Some(Duration::from_secs(30)), cd),
            "cooldown elapsed → probe runs"
        );
    }
}
