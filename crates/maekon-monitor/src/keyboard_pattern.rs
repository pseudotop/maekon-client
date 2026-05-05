use std::time::Instant;

/// Tracks keyboard input patterns from existing key_hook data.
/// Consumes aggregate key_count deltas to estimate typing speed
/// and detect idle→active transitions.
pub struct KeyboardPatternTracker {
    last_key_count: u64,
    last_sample_time: Instant,
    wpm_estimate: f32,
    was_active: bool,
}

impl KeyboardPatternTracker {
    pub fn new() -> Self {
        Self {
            last_key_count: 0,
            last_sample_time: Instant::now(),
            wpm_estimate: 0.0,
            was_active: false,
        }
    }

    /// Update with current key_count from InputActivityCollector.
    /// Returns (wpm, became_active) tuple.
    pub fn update(&mut self, current_key_count: u64) -> (f32, bool) {
        let now = Instant::now();
        let elapsed_secs = now.duration_since(self.last_sample_time).as_secs_f32();
        if elapsed_secs < 1.0 {
            return (self.wpm_estimate, false);
        }

        let delta_keys = current_key_count.saturating_sub(self.last_key_count);
        let keys_per_sec = delta_keys as f32 / elapsed_secs;
        // Approximate: 5 keystrokes per word
        self.wpm_estimate = (keys_per_sec * 60.0 / 5.0).min(200.0);

        let is_active = delta_keys > 0;
        let became_active = is_active && !self.was_active;

        self.last_key_count = current_key_count;
        self.last_sample_time = now;
        self.was_active = is_active;

        (self.wpm_estimate, became_active)
    }

    /// Current WPM estimate.
    pub fn wpm(&self) -> f32 {
        self.wpm_estimate
    }
}

impl Default for KeyboardPatternTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn wpm_from_key_delta() {
        let mut tracker = KeyboardPatternTracker::new();
        tracker.last_key_count = 100;
        tracker.last_sample_time = Instant::now() - Duration::from_secs(60);
        let (wpm, _) = tracker.update(200);
        // 100 keys / 60s = 1.67 keys/s → 1.67 * 60 / 5 = 20 WPM
        assert!((wpm - 20.0).abs() < 1.0);
    }

    #[test]
    fn detects_idle_to_active_transition() {
        let mut tracker = KeyboardPatternTracker::new();
        tracker.last_sample_time = Instant::now() - Duration::from_secs(5);
        tracker.was_active = false;
        let (_, became_active) = tracker.update(10);
        assert!(became_active);
    }

    #[test]
    fn no_transition_when_already_active() {
        let mut tracker = KeyboardPatternTracker::new();
        tracker.last_sample_time = Instant::now() - Duration::from_secs(5);
        tracker.was_active = true;
        tracker.last_key_count = 5;
        let (_, became_active) = tracker.update(10);
        assert!(!became_active);
    }

    #[test]
    fn wpm_capped_at_200() {
        let mut tracker = KeyboardPatternTracker::new();
        tracker.last_sample_time = Instant::now() - Duration::from_secs(1);
        let (wpm, _) = tracker.update(10000);
        assert!(wpm <= 200.0);
    }
}
