//! D13-v2b dashboard gRPC — RAII guard enforcing concurrent-stream cap.
//!
//! CRIT-3 + CRIT-4: plain `fetch_add` TOCTOU would let N+1 subscribers pass
//! the cap under contention. Instead we use a CAS-style increment with
//! revert-on-over: `fetch_add(1)` → if `prev >= cap`, `fetch_sub(1)` revert +
//! return `Status::resource_exhausted`. The guard's `Drop` impl always
//! decrements, surviving every generator exit path (abrupt disconnect,
//! `yield Err → return`, `spawn_blocking` JoinError panic). Capture the
//! guard into `async_stream!` closure so Drop runs when the stream drops.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tonic::Status;

#[derive(Debug)]
pub struct StreamCounterGuard {
    counter: Arc<AtomicUsize>,
}

impl StreamCounterGuard {
    /// Attempt to reserve one of `cap` concurrent-stream slots. On success
    /// returns a guard whose Drop decrements. On failure (cap exceeded)
    /// reverts the increment and returns `Status::resource_exhausted`.
    ///
    /// Over-admission is IMPOSSIBLE under concurrent access:
    /// at most `cap` threads can observe `prev < cap` and hold a guard.
    /// False-positive rejection (under tight contention where A,B both CAS
    /// to cap-1 then prev observed concurrently) is benign — caller retries.
    pub fn try_acquire(counter: Arc<AtomicUsize>, cap: usize) -> Result<Self, Status> {
        let prev = counter.fetch_add(1, Ordering::AcqRel);
        if prev >= cap {
            counter.fetch_sub(1, Ordering::AcqRel);
            return Err(Status::resource_exhausted("concurrent stream cap reached"));
        }
        Ok(Self { counter })
    }

    /// Current counter value (for introspection).
    #[cfg(any(test, feature = "test-support"))]
    pub fn count(counter: &Arc<AtomicUsize>) -> usize {
        counter.load(Ordering::Relaxed)
    }
}

impl Drop for StreamCounterGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn acquire_below_cap_succeeds() {
        let counter = Arc::new(AtomicUsize::new(0));
        let g1 = StreamCounterGuard::try_acquire(counter.clone(), 3).expect("slot 1");
        let g2 = StreamCounterGuard::try_acquire(counter.clone(), 3).expect("slot 2");
        assert_eq!(counter.load(Ordering::Relaxed), 2);
        drop(g1);
        drop(g2);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn acquire_at_cap_rejects_and_does_not_hold() {
        let counter = Arc::new(AtomicUsize::new(0));
        let _g1 = StreamCounterGuard::try_acquire(counter.clone(), 2).expect("slot 1");
        let _g2 = StreamCounterGuard::try_acquire(counter.clone(), 2).expect("slot 2");
        assert_eq!(counter.load(Ordering::Relaxed), 2);
        let result = StreamCounterGuard::try_acquire(counter.clone(), 2);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::ResourceExhausted,);
        // Counter should NOT reflect the rejected acquire — revert succeeded.
        assert_eq!(
            counter.load(Ordering::Relaxed),
            2,
            "rejected acquire must revert counter"
        );
    }

    #[test]
    fn drop_decrements() {
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let _g = StreamCounterGuard::try_acquire(counter.clone(), 5).expect("slot");
            assert_eq!(counter.load(Ordering::Relaxed), 1);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn concurrent_acquires_respect_cap() {
        // Spawn N threads racing to acquire; exactly `cap` should win.
        use std::sync::Barrier;
        use std::thread;

        let counter = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(10));
        let cap = 3;
        let mut handles = Vec::new();
        for _ in 0..10 {
            let counter = counter.clone();
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                StreamCounterGuard::try_acquire(counter, cap).ok()
            }));
        }
        let guards: Vec<Option<StreamCounterGuard>> = handles
            .into_iter()
            .map(|h| h.join().expect("thread panic"))
            .collect();
        let held = guards.iter().filter(|g| g.is_some()).count();
        assert_eq!(held, cap, "exactly {cap} threads should hold guards");
        assert_eq!(counter.load(Ordering::Relaxed), cap);
        drop(guards);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
    }
}
