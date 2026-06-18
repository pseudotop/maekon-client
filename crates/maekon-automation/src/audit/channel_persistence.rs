//! Off-reactor audit persistence (#6123).
//!
//! Audit persistence callbacks bridge `AuditLogger` to SQLite, which is a
//! *blocking* synchronous operation. The callback fires from inside
//! [`super::logger::AuditLogger::push_entry`], which itself runs on the tokio
//! reactor (the automation command path holds the logger behind a
//! `tokio::sync::RwLock` and calls it from async handlers). Running blocking
//! SQLite directly there stalls the reactor.
//!
//! [`ChannelAuditPersistence`] decouples the two: the reactor-side `persist`
//! call only performs a non-blocking `try_send` onto a *bounded* channel, and a
//! dedicated `spawn_blocking` drain task invokes the inner (SQLite) callback off
//! the reactor. The bound smooths bursts and caps memory — when the channel is
//! full, the oldest-strategy is to drop the *new* entry with a `warn!` rather
//! than block the reactor or grow unbounded (audit durability is best-effort
//! under sustained overload; reactor liveness is not negotiable).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use maekon_core::models::audit::AuditEntry;
use tokio::runtime::Handle;

use super::traits::AuditPersistence;

/// Default bound for the persistence channel. Large enough to absorb normal
/// audit bursts (the in-memory buffer default is 500–1000 entries) while still
/// capping memory under a stalled/slow disk.
const DEFAULT_CHANNEL_CAPACITY: usize = 1024;

/// Bounded-channel wrapper that runs a blocking [`AuditPersistence`] callback on
/// a dedicated `spawn_blocking` drain task instead of on the tokio reactor.
///
/// Construct with [`ChannelAuditPersistence::new`] and hand the result to
/// [`super::logger::AuditLogger::with_persistence`]. The inner callback is the
/// real (blocking, SQLite) persistence closure.
///
/// # Runtime requirement (#6123)
///
/// Construction spawns the drain task onto an **explicitly provided**
/// [`tokio::runtime::Handle`]. The original design relied on
/// [`tokio::runtime::Handle::try_current`], but every production wiring site
/// constructs this type on the *synchronous* Tauri main thread, where
/// `try_current()` is guaranteed `Err` — so the off-reactor drain never engaged
/// and `persist` always fell back to blocking on the reactor (the bug this type
/// exists to prevent). Passing the runtime handle explicitly makes the drain
/// task engage in production.
///
/// For unit tests (and any caller genuinely without a runtime), use
/// [`ChannelAuditPersistence::new_sync_fallback`], which invokes the inner
/// callback synchronously on `persist` — still correct, just not off-reactor
/// (there is no reactor to protect in that case).
pub struct ChannelAuditPersistence {
    sender: Option<tokio::sync::mpsc::Sender<AuditEntry>>,
    /// Synchronous fallback used only when no tokio runtime was available at
    /// construction time (drain task could not be spawned).
    fallback: Option<Arc<dyn AuditPersistence>>,
    /// Latches the first "channel closed" warning so a dead drain task does not
    /// spam the log on every subsequent entry.
    closed_logged: AtomicBool,
}

impl ChannelAuditPersistence {
    /// Wrap `inner` with the default channel capacity, draining on `handle`.
    ///
    /// `handle` is the tokio runtime handle the off-reactor drain task runs on.
    /// Wiring sites pass the background/agent runtime handle they already hold.
    pub fn new(inner: Arc<dyn AuditPersistence>, handle: Handle) -> Self {
        Self::with_capacity(inner, handle, DEFAULT_CHANNEL_CAPACITY)
    }

    /// Wrap `inner` with an explicit channel capacity, draining on `handle`.
    pub fn with_capacity(
        inner: Arc<dyn AuditPersistence>,
        handle: Handle,
        capacity: usize,
    ) -> Self {
        let capacity = capacity.max(1);
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<AuditEntry>(capacity);
        // Drain on a blocking thread (spawned via the provided handle, so it does
        // not depend on an ambient runtime context) so the inner (SQLite)
        // callback never touches the reactor. `blocking_recv` parks the blocking
        // thread, not a reactor worker, and returns `None` once all senders drop
        // (i.e. the logger is torn down), letting the task exit cleanly.
        handle.spawn_blocking(move || {
            while let Some(entry) = receiver.blocking_recv() {
                inner.persist(&entry);
            }
            tracing::debug!("audit persistence drain task exiting (channel closed)");
        });
        Self {
            sender: Some(sender),
            fallback: None,
            closed_logged: AtomicBool::new(false),
        }
    }

    /// Construct a synchronous-fallback wrapper with no off-reactor drain.
    ///
    /// Intended for callers genuinely without a runtime handle (unit tests, or a
    /// degraded wiring path). `persist` invokes the inner callback synchronously.
    /// Production wiring should prefer [`ChannelAuditPersistence::new`] with a
    /// real runtime handle so the SQLite write stays off the reactor.
    pub fn new_sync_fallback(inner: Arc<dyn AuditPersistence>) -> Self {
        Self {
            sender: None,
            fallback: Some(inner),
            closed_logged: AtomicBool::new(false),
        }
    }
}

impl AuditPersistence for ChannelAuditPersistence {
    fn persist(&self, entry: &AuditEntry) {
        let Some(sender) = self.sender.as_ref() else {
            // No-runtime fallback: run inner synchronously.
            if let Some(fallback) = self.fallback.as_ref() {
                fallback.persist(entry);
            }
            return;
        };

        // `try_send` is non-blocking and reactor-safe. On a full channel we drop
        // the entry (back-pressure) rather than block the reactor; on a closed
        // channel the drain task has exited, so we warn once and drop.
        match sender.try_send(entry.clone()) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("audit persistence channel full; dropping entry to protect reactor");
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                if !self.closed_logged.swap(true, Ordering::Relaxed) {
                    tracing::warn!("audit persistence channel closed; entries no longer persisted");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::audit::AuditStatus;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use std::time::Duration;

    fn make_entry(command_id: &str) -> AuditEntry {
        AuditEntry {
            entry_id: format!("aud-{command_id}"),
            timestamp: Utc::now(),
            session_id: "sess".to_string(),
            command_id: command_id.to_string(),
            action_type: "test".to_string(),
            status: AuditStatus::Completed,
            details: None,
            execution_time_ms: None,
        }
    }

    /// Regression (#6123): entries pushed on the reactor must be forwarded to the
    /// inner blocking callback via the off-reactor drain task.
    #[tokio::test]
    async fn drains_entries_to_inner_callback() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_clone = seen.clone();
        let inner: Arc<dyn AuditPersistence> = Arc::new(move |entry: &AuditEntry| {
            seen_clone.lock().unwrap().push(entry.command_id.clone());
        });

        let wrapper = ChannelAuditPersistence::new(inner, Handle::current());
        wrapper.persist(&make_entry("c-1"));
        wrapper.persist(&make_entry("c-2"));

        // Drop the wrapper (and its sender) so the drain task observes channel
        // close and exits after flushing — then poll for the result.
        drop(wrapper);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if seen.lock().unwrap().len() == 2 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "drain task did not flush entries in time"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let mut got = seen.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, vec!["c-1".to_string(), "c-2".to_string()]);
    }

    /// A full channel must drop entries rather than block; `persist` must always
    /// return promptly (reactor safety). Uses a multi-thread runtime so the
    /// blocking drain task and the (simulated) reactor caller run concurrently.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_channel_does_not_block() {
        // Count how many entries the drain task has begun handling, and let the
        // inner callback self-release after a bounded park so the test can never
        // hang the process even if assertions fail.
        let entered = Arc::new(AtomicUsize::new(0));
        let entered_clone = entered.clone();
        let inner: Arc<dyn AuditPersistence> = Arc::new(move |_: &AuditEntry| {
            entered_clone.fetch_add(1, Ordering::SeqCst);
            // Park the drain task long enough for the bounded channel to fill,
            // but bounded so the spawn_blocking thread always exits.
            std::thread::sleep(Duration::from_millis(300));
        });

        let wrapper = ChannelAuditPersistence::with_capacity(inner, Handle::current(), 1);

        // First send drains immediately; wait until the drain task is parked
        // inside the inner callback so the next sends find the channel full.
        wrapper.persist(&make_entry("c-0"));
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while entered.load(Ordering::SeqCst) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "drain task never started handling the first entry"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }

        // Saturate: drain task parked, 1 buffered, rest must be dropped without
        // blocking this (reactor) caller. The whole burst must finish well
        // inside the inner callback's 300ms park.
        let start = std::time::Instant::now();
        for i in 1..512 {
            wrapper.persist(&make_entry(&format!("c-{i}")));
        }
        assert!(
            start.elapsed() < Duration::from_millis(200),
            "persist blocked the caller on a full channel: {:?}",
            start.elapsed()
        );
    }

    /// Explicit synchronous-fallback constructor persists synchronously (still
    /// correct for callers genuinely without a runtime handle).
    #[test]
    fn sync_fallback_persists_synchronously() {
        let seen = Arc::new(Mutex::new(0usize));
        let seen_clone = seen.clone();
        let inner: Arc<dyn AuditPersistence> = Arc::new(move |_: &AuditEntry| {
            *seen_clone.lock().unwrap() += 1;
        });

        let wrapper = ChannelAuditPersistence::new_sync_fallback(inner);
        wrapper.persist(&make_entry("c-1"));
        assert_eq!(
            *seen.lock().unwrap(),
            1,
            "fallback should persist synchronously"
        );
    }

    /// Regression (#6123): the production wiring path constructs
    /// `ChannelAuditPersistence` on the *synchronous* Tauri main thread (no
    /// ambient tokio runtime). The off-reactor drain must still engage when an
    /// explicit runtime handle is supplied — the old `Handle::try_current()`
    /// gate silently degraded to synchronous persistence here, defeating the fix.
    #[test]
    fn drain_engages_when_handle_supplied_off_reactor() {
        // A dedicated multi-thread runtime; we hold only its handle and never
        // enter its context on this thread — exactly the production scenario.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("build runtime");
        let handle = runtime.handle().clone();

        // Sanity: this thread has no ambient runtime, so the old gate would fail.
        // Assert the *specific* reason (no runtime in the Tokio context), not just
        // any error — a `ThreadLocalDestroyed` error would be an unrelated teardown
        // condition and must not satisfy this precondition.
        let try_current_err = Handle::try_current()
            .expect_err("test must run off any tokio runtime to reproduce the production path");
        assert!(
            try_current_err.is_missing_context(),
            "expected a missing-runtime-context error, got: {try_current_err:?}"
        );

        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let seen_clone = seen.clone();
        let inner: Arc<dyn AuditPersistence> = Arc::new(move |entry: &AuditEntry| {
            seen_clone.lock().unwrap().push(entry.command_id.clone());
        });

        let wrapper = ChannelAuditPersistence::new(inner, handle);
        // The off-reactor path was taken iff a sender exists (no sync fallback).
        assert!(
            wrapper.sender.is_some(),
            "explicit handle must engage the off-reactor drain, not the fallback"
        );
        wrapper.persist(&make_entry("c-1"));
        wrapper.persist(&make_entry("c-2"));

        // Drop the sender so the drain task observes channel close and exits
        // after flushing, then poll for the result.
        drop(wrapper);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            if seen.lock().unwrap().len() == 2 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "drain task did not flush entries in time"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let mut got = seen.lock().unwrap().clone();
        got.sort();
        assert_eq!(got, vec!["c-1".to_string(), "c-2".to_string()]);
    }
}
