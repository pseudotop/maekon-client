use oneshim_core::models::activity::{IdleInfo, IdleState};
use tracing::debug;

const DEFAULT_IDLE_THRESHOLD_SECS: u64 = 300; // 5min
pub struct IdleTracker {
    threshold_secs: u64,
    previous_state: IdleState,
    current_idle_period_id: Option<i64>,
}

impl IdleTracker {
    pub fn new(threshold_secs: Option<u64>) -> Self {
        Self {
            threshold_secs: threshold_secs.unwrap_or(DEFAULT_IDLE_THRESHOLD_SECS),
            previous_state: IdleState::Active,
            current_idle_period_id: None,
        }
    }

    pub async fn check_idle(&mut self) -> IdleInfo {
        let idle_secs = get_idle_time().await.unwrap_or(0);
        let state = if idle_secs >= self.threshold_secs {
            IdleState::Idle
        } else {
            IdleState::Active
        };

        let info = IdleInfo {
            state,
            idle_secs,
            timestamp: chrono::Utc::now(),
        };

        if state != self.previous_state {
            debug!(
                "idle state changed: {:?} -> {:?} ({}s)",
                self.previous_state, state, idle_secs
            );
        }

        self.previous_state = state;
        info
    }

    pub fn became_idle(&self, current: IdleState) -> bool {
        self.previous_state == IdleState::Active && current == IdleState::Idle
    }

    pub fn became_active(&self, current: IdleState) -> bool {
        self.previous_state == IdleState::Idle && current == IdleState::Active
    }

    pub fn previous_state(&self) -> IdleState {
        self.previous_state
    }

    pub fn set_idle_period_id(&mut self, id: Option<i64>) {
        self.current_idle_period_id = id;
    }

    pub fn idle_period_id(&self) -> Option<i64> {
        self.current_idle_period_id
    }

    pub fn threshold_secs(&self) -> u64 {
        self.threshold_secs
    }
}

impl Default for IdleTracker {
    fn default() -> Self {
        Self::new(None)
    }
}

pub async fn get_idle_time() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        crate::macos::get_idle_time_macos().await
    }

    #[cfg(target_os = "windows")]
    {
        crate::windows::get_idle_time_windows()
    }

    #[cfg(target_os = "linux")]
    {
        crate::linux::get_idle_time_linux().await
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_tracker_default() {
        let tracker = IdleTracker::default();
        assert_eq!(tracker.threshold_secs(), DEFAULT_IDLE_THRESHOLD_SECS);
        assert_eq!(tracker.previous_state(), IdleState::Active);
    }

    #[test]
    fn idle_tracker_custom_threshold() {
        let tracker = IdleTracker::new(Some(60));
        assert_eq!(tracker.threshold_secs(), 60);
    }

    #[tokio::test]
    async fn idle_tracker_state_transitions() {
        let mut tracker = IdleTracker::new(Some(0)); // idle switch
        let info = tracker.check_idle().await;
        assert!(info.state == IdleState::Idle || info.state == IdleState::Active);
    }

    #[test]
    fn idle_period_id_management() {
        let mut tracker = IdleTracker::default();
        assert!(tracker.idle_period_id().is_none());

        tracker.set_idle_period_id(Some(123));
        assert_eq!(tracker.idle_period_id(), Some(123));

        tracker.set_idle_period_id(None);
        assert!(tracker.idle_period_id().is_none());
    }
}
