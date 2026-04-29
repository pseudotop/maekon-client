//! Runtime-tunable config slice for the external gRPC server.
//!
//! Single `ArcSwap<LiveSnapshot>` per spec §5.1 / D21 — atomic cross-field
//! reads eliminate the torn-read hazard of rev-1's dual-atomic design.
//!
//! Readers call `snapshot()` once per request-entry; writers (ConfigReloadTask
//! only) construct a new snapshot and `store` it.

use std::sync::Arc;

use arc_swap::ArcSwap;

use crate::grpc::load_policy::LoadPolicy;

/// Atomic snapshot of all runtime-tunable fields.
///
/// Constructed by `ConfigReloadTask` on every config-reload event and
/// atomic-stored into `LiveExternalConfig::current`. Readers always
/// see a consistent cross-field view.
#[derive(Clone)]
pub struct LiveSnapshot {
    pub streaming_enabled: bool,
    pub load_policy: Arc<LoadPolicy>,
}

pub struct LiveExternalConfig {
    current: ArcSwap<LiveSnapshot>,
}

impl LiveExternalConfig {
    pub fn new(initial: LiveSnapshot) -> Self {
        Self {
            current: ArcSwap::new(Arc::new(initial)),
        }
    }

    /// Non-blocking, lock-free read. Called on every request-entry.
    pub fn snapshot(&self) -> Arc<LiveSnapshot> {
        self.current.load_full()
    }

    /// Atomic replace. Only `ConfigReloadTask` calls this (pub(crate) gate).
    pub(crate) fn store(&self, new: LiveSnapshot) {
        self.current.store(Arc::new(new));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oneshim_core::config::LoadThresholds;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    fn fixture_policy() -> Arc<LoadPolicy> {
        Arc::new(LoadPolicy::new(LoadThresholds {
            cpu_low_pct: 30.0,
            cpu_medium_pct: 60.0,
            cpu_high_pct: 85.0,
            min_free_mem_gb: 1.0,
        }))
    }

    #[test]
    fn new_stores_initial_snapshot() {
        let policy = fixture_policy();
        let live = LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: policy.clone(),
        });
        let snap = live.snapshot();
        assert!(snap.streaming_enabled);
        assert!(Arc::ptr_eq(&snap.load_policy, &policy));
    }

    #[test]
    fn store_atomically_replaces_snapshot() {
        let live = LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: fixture_policy(),
        });
        let new_policy = fixture_policy();
        live.store(LiveSnapshot {
            streaming_enabled: false,
            load_policy: new_policy.clone(),
        });
        let snap = live.snapshot();
        assert!(!snap.streaming_enabled);
        assert!(Arc::ptr_eq(&snap.load_policy, &new_policy));
    }

    #[test]
    fn snapshot_observes_consistent_cross_field_view() {
        // Invariant: a reader NEVER sees new streaming_enabled with old load_policy
        // or vice versa. ArcSwap gives a single atomic pointer.
        let policy_a = fixture_policy();
        let policy_b = fixture_policy();
        let live = Arc::new(LiveExternalConfig::new(LiveSnapshot {
            streaming_enabled: true,
            load_policy: policy_a.clone(),
        }));
        let tear_detected = Arc::new(AtomicBool::new(false));

        let live_r = live.clone();
        let tear_r = tear_detected.clone();
        let policy_a_r = policy_a.clone();
        let policy_b_r = policy_b.clone();
        let reader = thread::spawn(move || {
            for _ in 0..10_000 {
                let snap = live_r.snapshot();
                // If streaming changed to false, load_policy MUST be policy_b.
                // If streaming is still true, load_policy MUST be policy_a.
                // Any other combo = torn read.
                if !snap.streaming_enabled && Arc::ptr_eq(&snap.load_policy, &policy_a_r) {
                    tear_r.store(true, Ordering::Relaxed);
                }
                if snap.streaming_enabled
                    && !Arc::ptr_eq(&snap.load_policy, &policy_a_r)
                    && !Arc::ptr_eq(&snap.load_policy, &policy_b_r)
                {
                    tear_r.store(true, Ordering::Relaxed);
                }
            }
        });

        let live_w = live.clone();
        let policy_b_clone = policy_b.clone();
        let writer = thread::spawn(move || {
            for _ in 0..1_000 {
                live_w.store(LiveSnapshot {
                    streaming_enabled: false,
                    load_policy: policy_b_clone.clone(),
                });
            }
        });

        reader.join().unwrap();
        writer.join().unwrap();
        assert!(
            !tear_detected.load(Ordering::Relaxed),
            "torn read observed — D21 invariant violated"
        );
    }

    #[test]
    fn send_sync_bounds() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<LiveExternalConfig>();
        assert_send_sync::<Arc<LiveExternalConfig>>();
    }
}
