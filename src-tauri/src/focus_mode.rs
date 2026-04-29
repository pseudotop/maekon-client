use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

/// Transient focus mode state. Not persisted across app restarts.
pub struct FocusModeState {
    active: AtomicBool,
    activated_at: RwLock<Option<DateTime<Utc>>>,
    duration_minutes: AtomicU32, // 0 = indefinite
    auto_activated: AtomicBool,
    last_deactivation: RwLock<Option<Instant>>,
}

impl FocusModeState {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            activated_at: RwLock::new(None),
            duration_minutes: AtomicU32::new(0),
            auto_activated: AtomicBool::new(false),
            last_deactivation: RwLock::new(None),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn is_auto_activated(&self) -> bool {
        self.auto_activated.load(Ordering::Relaxed)
    }

    pub fn activate(&self, duration_minutes: u32, auto: bool) {
        self.duration_minutes
            .store(duration_minutes, Ordering::Relaxed);
        self.auto_activated.store(auto, Ordering::Relaxed);
        *self.activated_at.write() = Some(Utc::now());
        self.active.store(true, Ordering::Release);
    }

    pub fn deactivate(&self) {
        self.active.store(false, Ordering::Release);
        *self.activated_at.write() = None;
        self.duration_minutes.store(0, Ordering::Relaxed);
        self.auto_activated.store(false, Ordering::Relaxed);
        *self.last_deactivation.write() = Some(Instant::now());
    }

    pub fn remaining_minutes(&self) -> Option<u32> {
        if !self.is_active() {
            return None;
        }
        let dur = self.duration_minutes.load(Ordering::Relaxed);
        if dur == 0 {
            return None; // indefinite
        }
        let activated = self.activated_at.read();
        let at = (*activated)?;
        let elapsed = (Utc::now() - at).num_minutes().max(0) as u32;
        Some(dur.saturating_sub(elapsed))
    }

    /// Check if timed focus mode has expired. Returns true if it was active
    /// and just expired (auto-deactivates).
    pub fn check_expiry(&self) -> bool {
        if !self.is_active() {
            return false;
        }
        let dur = self.duration_minutes.load(Ordering::Relaxed);
        if dur == 0 {
            return false; // indefinite never expires
        }
        let activated = self.activated_at.read();
        let Some(at) = *activated else { return false };
        let elapsed = (Utc::now() - at).num_minutes().max(0) as u32;
        if elapsed >= dur {
            drop(activated); // release read lock before write
            self.deactivate();
            true
        } else {
            false
        }
    }

    pub fn activated_at(&self) -> Option<DateTime<Utc>> {
        *self.activated_at.read()
    }

    /// Check if we are within the cooldown window after a deactivation.
    pub fn in_cooldown(&self, cooldown_secs: u64) -> bool {
        if cooldown_secs == 0 {
            return false;
        }
        let guard = self.last_deactivation.read();
        match *guard {
            Some(ts) => ts.elapsed().as_secs() < cooldown_secs,
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_state_is_inactive() {
        let state = FocusModeState::new();
        assert!(!state.is_active());
        assert!(state.remaining_minutes().is_none());
    }

    #[test]
    fn activate_then_check() {
        let state = FocusModeState::new();
        state.activate(30, false);
        assert!(state.is_active());
        assert!(!state.is_auto_activated());
        assert!(state.remaining_minutes().is_some());
    }

    #[test]
    fn auto_activate() {
        let state = FocusModeState::new();
        state.activate(25, true);
        assert!(state.is_active());
        assert!(state.is_auto_activated());
    }

    #[test]
    fn indefinite_has_no_remaining() {
        let state = FocusModeState::new();
        state.activate(0, false);
        assert!(state.is_active());
        assert!(state.remaining_minutes().is_none());
    }

    #[test]
    fn deactivate_clears_state() {
        let state = FocusModeState::new();
        state.activate(30, true);
        state.deactivate();
        assert!(!state.is_active());
        assert!(!state.is_auto_activated());
        assert!(state.activated_at().is_none());
    }

    #[test]
    fn deactivate_records_cooldown() {
        let state = FocusModeState::new();
        state.activate(30, false);
        state.deactivate();
        assert!(state.in_cooldown(300));
        assert!(!state.in_cooldown(0));
    }

    #[test]
    fn indefinite_does_not_expire() {
        let state = FocusModeState::new();
        state.activate(0, false);
        assert!(!state.check_expiry());
        assert!(state.is_active());
    }

    #[test]
    fn no_cooldown_before_deactivation() {
        let state = FocusModeState::new();
        assert!(!state.in_cooldown(300));
    }
}
