use super::ConfigManager;
use crate::config::AppConfig;
use std::sync::Arc;
use tokio::sync::watch;

impl ConfigManager {
    pub fn get(&self) -> AppConfig {
        AppConfig::clone(&self.inner.sender.borrow())
    }

    /// Subscribe to whole-config change notifications.
    ///
    /// The receiver starts at the current config. `changed().await` resolves
    /// after the next `update` / `update_with` / `reload`. Dropping a receiver
    /// does not affect any other subscriber.
    ///
    /// `watch` has latest-wins semantics: rapid mutations may be coalesced and
    /// a subscriber that wakes late will see only the final value, not every
    /// intermediate transition. Consumers whose correctness depends on
    /// observing every transition (audit-log callers, counters) must either
    /// keep a tick-based poll structure OR run every `update` through their
    /// own side-effect channel. See ADR-016 for the audit-coalescing hazard.
    pub fn subscribe(&self) -> watch::Receiver<Arc<AppConfig>> {
        self.inner.sender.subscribe()
    }

    /// Cheap read-only snapshot of the current config.
    ///
    /// Equivalent to `subscribe().borrow().clone()` without registering a
    /// subscriber. Prefer this over `get()` when the caller is happy with an
    /// `Arc<AppConfig>` (no deep clone).
    pub fn snapshot(&self) -> Arc<AppConfig> {
        self.inner.sender.borrow().clone()
    }
}
