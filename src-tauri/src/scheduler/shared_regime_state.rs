use chrono::{DateTime, Utc};
use parking_lot::RwLock;

/// Snapshot of current regime state, shared between monitor and coaching loops.
/// Monitor loop writes (~1/sec), coaching loop reads (~1/30s).
/// parking_lot::RwLock chosen over tokio::sync::RwLock: no .await needed
/// for <1μs read/write operations. Already a workspace dependency.
#[derive(Debug, Clone)]
pub struct RegimeSnapshot {
    pub regime_id: Option<String>,
    pub regime_label: Option<String>,
    pub current_app: String,
    pub updated_at: DateTime<Utc>,
}

pub struct SharedRegimeState {
    inner: RwLock<RegimeSnapshot>,
}

impl SharedRegimeState {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(RegimeSnapshot {
                regime_id: None,
                regime_label: None,
                current_app: String::new(),
                updated_at: Utc::now(),
            }),
        }
    }

    /// Called by monitor loop after regime classification each tick.
    pub fn update(&self, regime_id: Option<&str>, label: Option<&str>, app: &str) {
        let mut guard = self.inner.write();
        guard.regime_id = regime_id.map(|s| s.to_string());
        guard.regime_label = label.map(|s| s.to_string());
        guard.current_app = app.to_string();
        guard.updated_at = Utc::now();
    }

    /// Called by coaching loop to get current regime context.
    pub fn snapshot(&self) -> RegimeSnapshot {
        self.inner.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_snapshot_has_no_regime() {
        let state = SharedRegimeState::new();
        let snap = state.snapshot();
        assert!(snap.regime_id.is_none());
        assert!(snap.regime_label.is_none());
        assert!(snap.current_app.is_empty());
    }

    #[test]
    fn update_then_snapshot_returns_latest() {
        let state = SharedRegimeState::new();
        state.update(Some("regime-1"), Some("Deep Work"), "VSCode");
        let snap = state.snapshot();
        assert_eq!(snap.regime_id.as_deref(), Some("regime-1"));
        assert_eq!(snap.regime_label.as_deref(), Some("Deep Work"));
        assert_eq!(snap.current_app, "VSCode");
    }

    #[test]
    fn update_clears_previous_values_when_none() {
        let state = SharedRegimeState::new();
        state.update(Some("r1"), Some("label"), "App");
        state.update(None, None, "");
        let snap = state.snapshot();
        assert!(snap.regime_id.is_none());
        assert!(snap.regime_label.is_none());
    }
}
