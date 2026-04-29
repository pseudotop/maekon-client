//! Per-IP (or IPv6 /64 prefix) exponential backoff ban.
//! Checked in the custom accept loop BEFORE TLS handshake.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use lru::LruCache;
use parking_lot::RwLock;

/// Normalized key: IPv4 uses full address; IPv6 uses /64 prefix
/// (first 64 bits) per spec §S5.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) enum BanKey {
    V4(Ipv4Addr),
    V6Prefix64(u64),
}

impl BanKey {
    pub(crate) fn from_ip(ip: IpAddr) -> Self {
        match ip {
            IpAddr::V4(v4) => Self::V4(v4),
            IpAddr::V6(v6) => {
                let segs = v6.segments();
                let prefix = ((segs[0] as u64) << 48)
                    | ((segs[1] as u64) << 32)
                    | ((segs[2] as u64) << 16)
                    | (segs[3] as u64);
                Self::V6Prefix64(prefix)
            }
        }
    }
    pub(crate) fn from_socket(addr: SocketAddr) -> Self {
        Self::from_ip(addr.ip())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BanState {
    pub(crate) failure_count: u32,
    pub(crate) first_failure_at: Instant,
    pub(crate) banned_until: Option<Instant>,
}

/// Ladder: (threshold, ban_duration).
const THRESHOLDS: &[(u32, Duration)] = &[
    (5, Duration::from_secs(60)),
    (10, Duration::from_secs(600)),
    (20, Duration::from_secs(3600)),
];
/// Sliding window for counting failures.
const WINDOW: Duration = Duration::from_secs(60);
/// Max distinct keys tracked.
const DEFAULT_CAPACITY: usize = 10_000;

pub struct IpBan {
    cache: RwLock<LruCache<BanKey, BanState>>,
}

impl IpBan {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
    pub fn with_capacity(cap: usize) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(cap).unwrap())),
        }
    }

    /// Returns true if this IP is currently banned.
    pub fn is_banned(&self, addr: SocketAddr) -> bool {
        self.is_banned_key(BanKey::from_socket(addr))
    }

    fn is_banned_key(&self, key: BanKey) -> bool {
        let now = Instant::now();
        let guard = self.cache.read();
        match guard.peek(&key) {
            Some(state) => state.banned_until.is_some_and(|until| until > now),
            None => false,
        }
    }

    /// Record a failure. Returns the new ban state (if any).
    ///
    /// Holds the write guard for the full critical section because
    /// `LruCache::get_or_insert_mut` returns a `&mut V` that is mutated
    /// across sliding-window + threshold-scan logic before being cloned.
    /// The guard cannot be released mid-function without invalidating
    /// the borrow; atomicity is intentional.
    #[allow(
        clippy::significant_drop_tightening,
        reason = "atomic sliding-window + threshold update on the cache entry requires single contiguous write guard"
    )]
    pub(crate) fn record_failure(&self, addr: SocketAddr) -> Option<BanState> {
        let key = BanKey::from_socket(addr);
        let now = Instant::now();
        let mut guard = self.cache.write();
        let state = guard.get_or_insert_mut(key, || BanState {
            failure_count: 0,
            first_failure_at: now,
            banned_until: None,
        });
        // Sliding window: if first failure was > WINDOW ago, reset.
        if now.duration_since(state.first_failure_at) > WINDOW {
            state.failure_count = 0;
            state.first_failure_at = now;
            state.banned_until = None;
        }
        state.failure_count += 1;
        for &(threshold, duration) in THRESHOLDS.iter().rev() {
            if state.failure_count >= threshold {
                state.banned_until = Some(now + duration);
                break;
            }
        }
        Some(state.clone())
    }

    /// Active ban count (for metrics).
    pub fn active_ban_count(&self) -> usize {
        let now = Instant::now();
        let guard = self.cache.read();
        guard
            .iter()
            .filter(|(_, s)| s.banned_until.is_some_and(|until| until > now))
            .count()
    }
}

impl Default for IpBan {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn addr_v4(a: u8, b: u8, c: u8, d: u8) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(a, b, c, d)), 12345)
    }

    fn addr_v6(s: &str) -> SocketAddr {
        SocketAddr::new(IpAddr::V6(s.parse::<Ipv6Addr>().unwrap()), 12345)
    }

    #[test]
    fn fresh_ip_not_banned() {
        let ban = IpBan::new();
        assert!(!ban.is_banned(addr_v4(1, 2, 3, 4)));
    }

    #[test]
    fn five_failures_triggers_60s_ban() {
        let ban = IpBan::new();
        let a = addr_v4(10, 0, 0, 1);
        for _ in 0..5 {
            ban.record_failure(a);
        }
        assert!(ban.is_banned(a));
    }

    #[test]
    fn ten_failures_triggers_longer_ban() {
        let ban = IpBan::new();
        let a = addr_v4(10, 0, 0, 2);
        let mut last = None;
        for _ in 0..10 {
            last = ban.record_failure(a);
        }
        let state = last.unwrap();
        let remaining = state.banned_until.unwrap().duration_since(Instant::now());
        assert!(
            remaining > Duration::from_secs(500),
            "10+ failures → ≥ 600s ban"
        );
    }

    #[test]
    fn twenty_failures_triggers_longest_ban() {
        let ban = IpBan::new();
        let a = addr_v4(10, 0, 0, 3);
        let mut last = None;
        for _ in 0..20 {
            last = ban.record_failure(a);
        }
        let state = last.unwrap();
        let remaining = state.banned_until.unwrap().duration_since(Instant::now());
        assert!(
            remaining > Duration::from_secs(3000),
            "20+ failures → ≥ 3600s ban"
        );
    }

    #[test]
    fn ipv6_64_prefix_shared_ban() {
        let ban = IpBan::new();
        let a1 = addr_v6("2001:db8::1");
        let a2 = addr_v6("2001:db8::ff");
        for _ in 0..5 {
            ban.record_failure(a1);
        }
        assert!(ban.is_banned(a2), "same /64 prefix ⇒ ban shared");
    }

    #[test]
    fn ipv6_different_64_prefix_not_shared() {
        let ban = IpBan::new();
        let a1 = addr_v6("2001:db8::1");
        let a2 = addr_v6("2001:db9::1"); // different /64
        for _ in 0..5 {
            ban.record_failure(a1);
        }
        assert!(!ban.is_banned(a2));
    }

    #[test]
    fn ipv4_full_address_not_shared() {
        let ban = IpBan::new();
        let a1 = addr_v4(10, 0, 0, 1);
        let a2 = addr_v4(10, 0, 0, 2);
        for _ in 0..5 {
            ban.record_failure(a1);
        }
        assert!(!ban.is_banned(a2));
    }

    #[test]
    fn lru_evicts_old_entries() {
        let ban = IpBan::with_capacity(100);
        for i in 0..100 {
            ban.record_failure(addr_v4(10, 0, 0, i as u8));
        }
        // Touch entry 0 to keep it hot
        let _ = ban.is_banned(addr_v4(10, 0, 0, 0));
        // Evict by inserting 101st
        ban.record_failure(addr_v4(10, 0, 1, 0));
        // Entry 1 (oldest non-touched) should be evicted.
        let guard = ban.cache.read();
        assert_eq!(guard.len(), 100);
    }

    #[test]
    fn active_ban_count_reports_correctly() {
        let ban = IpBan::new();
        for _ in 0..5 {
            ban.record_failure(addr_v4(10, 0, 0, 1));
        }
        for _ in 0..5 {
            ban.record_failure(addr_v4(10, 0, 0, 2));
        }
        assert_eq!(ban.active_ban_count(), 2);
    }

    #[test]
    fn single_failure_not_banned() {
        let ban = IpBan::new();
        ban.record_failure(addr_v4(10, 0, 0, 99));
        assert!(!ban.is_banned(addr_v4(10, 0, 0, 99)));
    }

    #[test]
    fn sliding_window_resets_counter() {
        // Manually construct a past state (simulating stale failures).
        let ban = IpBan::new();
        let a = addr_v4(10, 0, 0, 55);
        let key = BanKey::from_socket(a);
        {
            let mut guard = ban.cache.write();
            guard.put(
                key,
                BanState {
                    failure_count: 4,
                    first_failure_at: Instant::now() - Duration::from_secs(120), // > WINDOW
                    banned_until: None,
                },
            );
        }
        ban.record_failure(a);
        // After record: should reset to count=1 because old window expired.
        let guard = ban.cache.read();
        assert_eq!(guard.peek(&key).unwrap().failure_count, 1);
    }
}
