use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
    Reconnecting,
}

impl std::fmt::Display for ConnectionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionStatus::Connected => write!(f, "Connected"),
            ConnectionStatus::Disconnected => write!(f, "Disconnected"),
            ConnectionStatus::Reconnecting => write!(f, "Reconnecting"),
        }
    }
}

pub struct ConnectivityManager {
    is_online: AtomicBool,
    last_success: AtomicU64,
    failure_count: AtomicU64,
    status_tx: watch::Sender<ConnectionStatus>,
    status_rx: watch::Receiver<ConnectionStatus>,
    offline_threshold: u64,
    force_offline: AtomicBool,
}

impl ConnectivityManager {
    pub fn new(offline_threshold: u64) -> Self {
        let (status_tx, status_rx) = watch::channel(ConnectionStatus::Connected);
        Self {
            is_online: AtomicBool::new(true),
            last_success: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            status_tx,
            status_rx,
            offline_threshold,
            force_offline: AtomicBool::new(false),
        }
    }

    pub fn default_threshold() -> Self {
        Self::new(3)
    }

    pub fn set_force_offline(&self, force: bool) {
        self.force_offline.store(force, Ordering::Relaxed);
        if force {
            self.is_online.store(false, Ordering::Relaxed);
            if let Err(e) = self.status_tx.send(ConnectionStatus::Disconnected) {
                debug!("channel send failed: {e}");
            }
            info!("force offline mode enabled");
        }
    }

    pub fn is_force_offline(&self) -> bool {
        self.force_offline.load(Ordering::Relaxed)
    }

    pub fn is_online(&self) -> bool {
        !self.is_force_offline() && self.is_online.load(Ordering::Relaxed)
    }

    pub fn status(&self) -> ConnectionStatus {
        *self.status_rx.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<ConnectionStatus> {
        self.status_rx.clone()
    }

    pub fn record_success(&self) {
        if self.is_force_offline() {
            return;
        }

        let was_offline = !self.is_online.load(Ordering::Relaxed);
        let was_reconnecting = *self.status_rx.borrow() == ConnectionStatus::Reconnecting;
        self.is_online.store(true, Ordering::Relaxed);
        self.failure_count.store(0, Ordering::Relaxed);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.last_success.store(now, Ordering::Relaxed);

        if was_offline || was_reconnecting {
            if was_offline {
                info!("server connection recover - mode");
            } else {
                debug!("reconnect success - Connected state restore");
            }
            if let Err(e) = self.status_tx.send(ConnectionStatus::Connected) {
                debug!("channel send failed: {e}");
            }
        }
    }

    pub fn record_failure(&self) {
        if self.is_force_offline() {
            return;
        }

        let count = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        debug!("connection failure record (consecutive {})", count);

        if count >= self.offline_threshold {
            let was_online = self.is_online.swap(false, Ordering::Relaxed);
            if was_online {
                warn!(
                    "Consecutive {} failures - switching to offline mode (queued events are saved locally)",
                    count
                );
                if let Err(e) = self.status_tx.send(ConnectionStatus::Disconnected) {
                    debug!("channel send failed: {e}");
                }
            }
        } else if let Err(e) = self.status_tx.send(ConnectionStatus::Reconnecting) {
            debug!("channel send failed: {e}");
        }
    }

    pub fn failure_count(&self) -> u64 {
        self.failure_count.load(Ordering::Relaxed)
    }

    pub fn time_since_last_success(&self) -> Duration {
        let last = self.last_success.load(Ordering::Relaxed);
        if last == 0 {
            return Duration::ZERO;
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Duration::from_secs(now.saturating_sub(last))
    }

    pub fn stats(&self) -> ConnectivityStats {
        ConnectivityStats {
            is_online: self.is_online(),
            status: self.status(),
            failure_count: self.failure_count(),
            time_since_last_success: self.time_since_last_success(),
            force_offline: self.is_force_offline(),
        }
    }
}

impl Default for ConnectivityManager {
    fn default() -> Self {
        Self::default_threshold()
    }
}

#[derive(Debug, Clone)]
pub struct ConnectivityStats {
    pub is_online: bool,
    pub status: ConnectionStatus,
    pub failure_count: u64,
    pub time_since_last_success: Duration,
    pub force_offline: bool,
}

pub type SharedConnectivityManager = Arc<ConnectivityManager>;

pub fn new_shared_connectivity_manager(offline_threshold: u64) -> SharedConnectivityManager {
    Arc::new(ConnectivityManager::new(offline_threshold))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ProxyFaultHarness {
        latency_pattern_ms: Vec<u64>,
        jitter_pattern_ms: Vec<u64>,
    }

    impl ProxyFaultHarness {
        fn new(latency_pattern_ms: Vec<u64>, jitter_pattern_ms: Vec<u64>) -> Self {
            Self {
                latency_pattern_ms,
                jitter_pattern_ms,
            }
        }

        async fn apply_packet_loss_burst(&self, mgr: &ConnectivityManager, failures: usize) {
            for i in 0..failures {
                mgr.record_failure();
                let sleep_ms = self.latency_pattern_ms[i % self.latency_pattern_ms.len()];
                tokio::time::sleep(Duration::from_millis(sleep_ms)).await;
            }
        }

        async fn apply_jitter_window(&self) {
            for i in 0..self.jitter_pattern_ms.len() {
                tokio::time::sleep(Duration::from_millis(self.jitter_pattern_ms[i])).await;
            }
        }

        async fn apply_transport_reset(&self, mgr: &ConnectivityManager) {
            mgr.record_failure();
            tokio::time::sleep(Duration::from_millis(8)).await;
            mgr.record_failure();
            tokio::time::sleep(Duration::from_millis(8)).await;
            mgr.record_success();
        }
    }

    #[test]
    fn initial_state_is_online() {
        let mgr = ConnectivityManager::default();
        assert!(mgr.is_online());
        assert_eq!(mgr.status(), ConnectionStatus::Connected);
        assert_eq!(mgr.failure_count(), 0);
    }

    #[test]
    fn success_resets_failures() {
        let mgr = ConnectivityManager::new(3);

        mgr.record_failure();
        mgr.record_failure();
        assert_eq!(mgr.failure_count(), 2);

        mgr.record_success();
        assert_eq!(mgr.failure_count(), 0);
        assert!(mgr.is_online());
    }

    #[test]
    fn threshold_triggers_offline() {
        let mgr = ConnectivityManager::new(3);

        mgr.record_failure();
        assert!(mgr.is_online()); // 1 -
        mgr.record_failure();
        assert!(mgr.is_online()); // 2 -
        mgr.record_failure();
        assert!(!mgr.is_online()); // 3 failures -> offline
        assert_eq!(mgr.status(), ConnectionStatus::Disconnected);
    }

    #[test]
    fn recovery_after_offline() {
        let mgr = ConnectivityManager::new(2);

        mgr.record_failure();
        mgr.record_failure();
        assert!(!mgr.is_online()); // offline
        mgr.record_success();
        assert!(mgr.is_online()); // recovered
        assert_eq!(mgr.status(), ConnectionStatus::Connected);
    }

    #[test]
    fn force_offline_overrides() {
        let mgr = ConnectivityManager::default();

        mgr.set_force_offline(true);
        assert!(!mgr.is_online());

        mgr.record_success(); // ignored in force mode
        assert!(!mgr.is_online());

        mgr.set_force_offline(false);
        mgr.record_success();
        assert!(mgr.is_online());
    }

    #[tokio::test]
    async fn subscribe_receives_changes() {
        let mgr = ConnectivityManager::new(1);
        let mut rx = mgr.subscribe();

        assert_eq!(*rx.borrow(), ConnectionStatus::Connected);

        mgr.record_failure();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Disconnected);

        mgr.record_success();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Connected);
    }

    #[tokio::test]
    async fn reconnect_backpressure_coalesces_to_latest_state() {
        let mgr = ConnectivityManager::new(500);
        let mut rx = mgr.subscribe();

        for _ in 0..300 {
            mgr.record_failure();
        }

        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Reconnecting);
        assert_eq!(mgr.failure_count(), 300);

        mgr.record_success();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Connected);
        assert_eq!(mgr.failure_count(), 0);
    }

    #[tokio::test]
    async fn reconnect_conformance_transitions_to_disconnected_after_threshold() {
        let mgr = ConnectivityManager::new(10);
        let mut rx = mgr.subscribe();

        for _ in 0..9 {
            mgr.record_failure();
        }

        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Reconnecting);
        assert!(mgr.is_online());

        mgr.record_failure();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Disconnected);
        assert!(!mgr.is_online());

        mgr.record_success();
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Connected);
        assert!(mgr.is_online());
    }

    #[tokio::test]
    async fn chaos_packet_loss_bursts_stay_online_when_under_threshold() {
        let mgr = ConnectivityManager::new(5);

        for _ in 0..3 {
            for _ in 0..4 {
                mgr.record_failure();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
            mgr.record_success();
            assert!(mgr.is_online());
            assert_eq!(mgr.status(), ConnectionStatus::Connected);
            assert_eq!(mgr.failure_count(), 0);
        }
    }

    #[tokio::test]
    async fn chaos_delayed_ack_recovers_after_disconnection() {
        let mgr = ConnectivityManager::new(3);
        let mut rx = mgr.subscribe();

        for _ in 0..3 {
            mgr.record_failure();
        }
        rx.changed().await.unwrap();
        assert_eq!(*rx.borrow(), ConnectionStatus::Disconnected);
        assert!(!mgr.is_online());

        tokio::time::sleep(Duration::from_millis(25)).await;
        mgr.record_success();
        rx.changed().await.unwrap();

        assert_eq!(*rx.borrow(), ConnectionStatus::Connected);
        assert!(mgr.is_online());
        assert_eq!(mgr.failure_count(), 0);
    }

    #[tokio::test]
    async fn proxy_fault_packet_loss_burst_trips_threshold() {
        let mgr = ConnectivityManager::new(4);
        let harness = ProxyFaultHarness::new(vec![2, 4, 3], vec![1, 2]);
        let mut rx = mgr.subscribe();

        harness.apply_packet_loss_burst(&mgr, 4).await;

        let mut status = *rx.borrow();
        for _ in 0..4 {
            if status == ConnectionStatus::Disconnected {
                break;
            }
            rx.changed().await.unwrap();
            status = *rx.borrow();
        }

        assert_eq!(status, ConnectionStatus::Disconnected);
        assert!(!mgr.is_online());
    }

    #[tokio::test]
    async fn proxy_fault_jitter_only_keeps_connected_state() {
        let mgr = ConnectivityManager::new(3);
        let harness = ProxyFaultHarness::new(vec![1], vec![4, 2, 5, 3, 1]);

        harness.apply_jitter_window().await;
        mgr.record_success();

        assert_eq!(mgr.status(), ConnectionStatus::Connected);
        assert!(mgr.is_online());
    }

    #[tokio::test]
    async fn proxy_fault_transport_reset_recovers_to_connected() {
        let mgr = ConnectivityManager::new(3);
        let harness = ProxyFaultHarness::new(vec![1], vec![1]);
        let mut rx = mgr.subscribe();

        harness.apply_transport_reset(&mgr).await;

        let mut status = *rx.borrow();
        for _ in 0..3 {
            if status == ConnectionStatus::Connected {
                break;
            }
            rx.changed().await.unwrap();
            status = *rx.borrow();
        }

        assert_eq!(status, ConnectionStatus::Connected);

        assert!(mgr.is_online());
        assert_eq!(mgr.failure_count(), 0);
    }
}
