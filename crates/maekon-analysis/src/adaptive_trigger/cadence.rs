use chrono::{DateTime, Duration, Utc};

// Exposed for the scheduler cadence contract; runtime wiring is handled by the
// capture loop once regime snapshots are available at that boundary.
#[allow(dead_code)]
/// Capture-rate regime used by deterministic scheduler cadence logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRateRegime {
    Active,
    Idle,
}

#[allow(dead_code)]
/// Deterministic capture cadence for regime-aware capture scheduling.
///
/// Active work captures at 500ms by default. Idle work slows to 5s by default,
/// which satisfies the product target of 2-5s while keeping background load low.
#[derive(Debug, Clone)]
pub struct AdaptiveCaptureCadence {
    active_interval: Duration,
    idle_interval: Duration,
    current_regime: Option<CaptureRateRegime>,
    last_capture_at: Option<DateTime<Utc>>,
}

#[allow(dead_code)]
impl AdaptiveCaptureCadence {
    pub fn new(active_interval: Duration, idle_interval: Duration) -> Self {
        Self {
            active_interval,
            idle_interval,
            current_regime: None,
            last_capture_at: None,
        }
    }

    pub fn should_capture(&mut self, regime: CaptureRateRegime, now: DateTime<Utc>) -> bool {
        if self.current_regime != Some(regime) {
            if regime == CaptureRateRegime::Active {
                self.last_capture_at = None;
            }
            self.current_regime = Some(regime);
        }

        let interval = match regime {
            CaptureRateRegime::Active => self.active_interval,
            CaptureRateRegime::Idle => self.idle_interval,
        };

        let due = self
            .last_capture_at
            .map(|last| now.signed_duration_since(last) >= interval)
            .unwrap_or(true);

        if due {
            self.last_capture_at = Some(now);
        }

        due
    }
}

impl Default for AdaptiveCaptureCadence {
    fn default() -> Self {
        Self::new(Duration::milliseconds(500), Duration::seconds(5))
    }
}
