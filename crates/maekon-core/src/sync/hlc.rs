//! Hybrid Logical Clock (HLC) for causal ordering in cross-device sync.
//!
//! HLC combines wall-clock time with a logical counter and device ID
//! to produce globally unique, causally ordered timestamps without
//! requiring synchronized clocks across devices.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum allowed clock drift between local and remote HLC (1 hour).
/// Remote timestamps beyond this threshold are rejected to prevent
/// a single far-future device from poisoning the causal ordering.
const MAX_CLOCK_DRIFT_MS: u64 = 3_600_000;

/// A Hybrid Logical Clock timestamp.
///
/// Ordering: `wall_ms` → `counter` → `device_id` (lexicographic).
/// Derives `Ord` with fields in this order for correct comparison.
///
/// # Privacy Notice
///
/// HLC values contain precise activity timestamps (`wall_ms`) and device
/// identifiers (`device_id`). When synced to remote storage, this metadata
/// reveals activity patterns (when the user was active, on which device)
/// and should be covered by the cross-device sync consent gate
/// (`ConsentPermissions::cross_device_sync`) and GDPR Article 17 erasure
/// scope. Ensure that any remote persistence of HLC data is included in
/// the user's data export and deletion workflows.
///
/// **Exception — `sync_tombstones` (V38, #5174):** the HLC stored in a tombstone
/// row is the HLC of the *erasure event itself*, not an activity timestamp — it
/// records *when a row was deleted*, equivalent to `deleted_at`. That table is a
/// processing-record retained under GDPR Art. 17(3) (same basis as `egress_ledger`)
/// and is therefore *intentionally excluded* from Art. 17 erasure scope. Do NOT
/// "fix" the apparent conflict by adding `sync_tombstones` to the erase set
/// (`delete_all_data_inner` `ALL_TABLES`) — that would break offline-peer erasure
/// convergence. Its bounded retention is enforced by the tombstone GC (epic #5174).
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hlc {
    /// Wall-clock milliseconds since UNIX epoch.
    pub wall_ms: u64,
    /// Monotonic counter for events within the same millisecond.
    pub counter: u32,
    /// Unique device identifier (tiebreaker for concurrent events).
    pub device_id: String,
}

impl Hlc {
    /// Create a new HLC with the current wall-clock time.
    pub fn now(device_id: &str) -> Self {
        Self {
            wall_ms: current_time_ms(),
            counter: 0,
            device_id: device_id.to_string(),
        }
    }

    /// Advance the clock for a local event.
    ///
    /// Ensures monotonicity: if the wall clock hasn't advanced past
    /// the current HLC, increment the counter instead.
    pub fn tick(&mut self) {
        let now = current_time_ms();
        if now > self.wall_ms {
            self.wall_ms = now;
            self.counter = 0;
        } else {
            self.counter += 1;
        }
    }

    /// Merge with a received remote HLC (on message receive).
    ///
    /// Takes the maximum of local and remote timestamps, then
    /// advances the counter to maintain causal ordering.
    /// Rejects remote timestamps that exceed `MAX_CLOCK_DRIFT_MS` ahead
    /// of the current wall clock to prevent far-future poisoning.
    pub fn merge(&mut self, remote: &Hlc) {
        let now = current_time_ms();

        // Reject remote HLC with excessive clock drift (> 1 hour ahead).
        // Fall back to a local-only tick instead of adopting the far-future timestamp.
        if remote.wall_ms > now + MAX_CLOCK_DRIFT_MS {
            tracing::warn!(
                remote_ms = remote.wall_ms,
                local_ms = now,
                drift_ms = remote.wall_ms - now,
                "rejecting remote HLC: clock drift exceeds 1 hour"
            );
            self.tick();
            return;
        }

        let max_wall = now.max(self.wall_ms).max(remote.wall_ms);

        if max_wall == self.wall_ms && max_wall == remote.wall_ms {
            // All three equal — advance counter past both
            self.counter = self.counter.max(remote.counter) + 1;
        } else if max_wall == self.wall_ms {
            // Local wall is highest — advance local counter
            self.counter += 1;
        } else if max_wall == remote.wall_ms {
            // Remote wall is highest — adopt remote counter + 1
            self.counter = remote.counter + 1;
        } else {
            // Wall clock is highest — reset counter
            self.counter = 0;
        }

        self.wall_ms = max_wall;
    }

    /// Check if this HLC is causally after another.
    pub fn is_after(&self, other: &Hlc) -> bool {
        self > other
    }

    /// Returns `true` when a peer-supplied wall-clock is within the plausible
    /// drift bound (≤ `MAX_CLOCK_DRIFT_MS` ahead of the local clock).
    ///
    /// `merge` applies this same guard when advancing the *local* clock, but the
    /// cross-device sync-merge ingestion path writes peer HLCs straight into
    /// LWW/tombstone tables without ever calling `merge`. Callers on that path use
    /// this to reject implausibly-far-future timestamps that would otherwise
    /// permanently win every LWW conflict and suppress legitimate future writes.
    pub fn wall_ms_within_drift_bound(remote_wall_ms: u64) -> bool {
        remote_wall_ms <= current_time_ms().saturating_add(MAX_CLOCK_DRIFT_MS)
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances_monotonically() {
        let mut hlc = Hlc {
            wall_ms: 1000,
            counter: 0,
            device_id: "dev-a".to_string(),
        };

        hlc.tick();
        // Wall clock should be >= 1000, and either wall advanced or counter incremented
        assert!(hlc.wall_ms >= 1000);
        if hlc.wall_ms == 1000 {
            assert_eq!(hlc.counter, 1);
        }
    }

    #[test]
    fn merge_takes_maximum() {
        // Use future timestamps so current_time_ms() < both
        let far_future = current_time_ms() + 1_000_000;
        let mut local = Hlc {
            wall_ms: far_future,
            counter: 5,
            device_id: "dev-a".to_string(),
        };

        let remote = Hlc {
            wall_ms: far_future,
            counter: 3,
            device_id: "dev-b".to_string(),
        };

        local.merge(&remote);
        // Both walls equal and > now — counter should be max(5,3) + 1 = 6
        assert_eq!(local.wall_ms, far_future);
        assert_eq!(local.counter, 6);
    }

    #[test]
    fn merge_with_higher_remote_wall() {
        let far_future = current_time_ms() + 2_000_000;
        let mut local = Hlc {
            wall_ms: far_future - 1_000_000,
            counter: 10,
            device_id: "dev-a".to_string(),
        };

        let remote = Hlc {
            wall_ms: far_future,
            counter: 7,
            device_id: "dev-b".to_string(),
        };

        local.merge(&remote);
        // Remote wall is highest (> local and > now)
        assert_eq!(local.wall_ms, far_future);
        assert_eq!(local.counter, 8); // remote.counter + 1
    }

    #[test]
    fn ordering_wall_ms_primary() {
        let a = Hlc {
            wall_ms: 100,
            counter: 99,
            device_id: "zzz".to_string(),
        };
        let b = Hlc {
            wall_ms: 200,
            counter: 0,
            device_id: "aaa".to_string(),
        };
        assert!(b > a);
        assert!(b.is_after(&a));
    }

    #[test]
    fn ordering_counter_secondary() {
        let a = Hlc {
            wall_ms: 100,
            counter: 1,
            device_id: "zzz".to_string(),
        };
        let b = Hlc {
            wall_ms: 100,
            counter: 2,
            device_id: "aaa".to_string(),
        };
        assert!(b > a);
    }

    #[test]
    fn ordering_device_id_tiebreaker() {
        let a = Hlc {
            wall_ms: 100,
            counter: 1,
            device_id: "aaa".to_string(),
        };
        let b = Hlc {
            wall_ms: 100,
            counter: 1,
            device_id: "bbb".to_string(),
        };
        assert!(b > a);
    }

    #[test]
    fn serde_roundtrip() {
        let hlc = Hlc::now("test-device");
        let json = serde_json::to_string(&hlc).unwrap();
        let parsed: Hlc = serde_json::from_str(&json).unwrap();
        assert_eq!(hlc, parsed);
    }

    #[test]
    fn merge_rejects_excessive_clock_drift() {
        let now = current_time_ms();
        let mut local = Hlc {
            wall_ms: now,
            counter: 5,
            device_id: "dev-a".to_string(),
        };

        // Remote timestamp 2 hours in the future — exceeds MAX_CLOCK_DRIFT_MS
        let remote = Hlc {
            wall_ms: now + 2 * MAX_CLOCK_DRIFT_MS,
            counter: 10,
            device_id: "dev-b".to_string(),
        };

        local.merge(&remote);

        // Should NOT adopt the far-future timestamp; should tick locally instead
        assert!(
            local.wall_ms < now + MAX_CLOCK_DRIFT_MS,
            "wall_ms should not adopt far-future remote timestamp"
        );
        assert_eq!(local.device_id, "dev-a", "device_id must remain local");
    }

    #[test]
    fn wall_ms_within_drift_bound_rejects_far_future_peer_hlc() {
        let now = current_time_ms();
        // Plausible peer wall-clocks (recent / slightly ahead / in the past) pass.
        assert!(Hlc::wall_ms_within_drift_bound(now));
        assert!(Hlc::wall_ms_within_drift_bound(
            now + MAX_CLOCK_DRIFT_MS / 2
        ));
        assert!(Hlc::wall_ms_within_drift_bound(0));
        // A far-future peer wall-clock (> 1h drift) is rejected — this is the
        // ingestion-path guard the sync merger relies on so a buggy/compromised
        // paired device cannot permanently win every LWW conflict.
        assert!(!Hlc::wall_ms_within_drift_bound(
            now + 2 * MAX_CLOCK_DRIFT_MS
        ));
    }
}
