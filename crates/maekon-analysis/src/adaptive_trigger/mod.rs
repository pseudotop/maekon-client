mod cadence;
mod decision;
mod scoring;
mod signals;

#[cfg(test)]
mod tests;

pub use cadence::{AdaptiveCaptureCadence, CaptureRateRegime};

use chrono::{DateTime, Utc};
use maekon_core::models::tiered_memory::{
    CalibrationEntry, ResolvedParams, TriggerAction, TriggerInput,
};

/// Decision produced by `AdaptiveTrigger::process_event`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerDecision {
    /// No state change — continue accumulating.
    Continue,
    /// Open the first segment, or open after an explicit close.
    OpenSegment,
    /// Close current segment and immediately open a new one
    /// (score > t_high while a segment is already open and min age met).
    RestartSegment,
    /// Close the current segment (score dropped below `t_low`).
    CloseSegment,
    /// Force-close because segment duration exceeded `max_segment_secs`.
    ForceCloseSegment,
}

/// Core adaptive trigger algorithm: Dual-EWMA density estimation with
/// 4-signal weighted fusion and hysteresis-based segment control.
///
/// Pure computation — no I/O, no async.
pub struct AdaptiveTrigger {
    // EWMA state
    pub(super) ewma_short: f32,
    pub(super) ewma_long: f32,
    pub(super) importance_ewma: f32,
    pub(super) context_signal: f32,

    // Segment state
    pub(super) segment_start: Option<DateTime<Utc>>,
    pub(super) segment_event_count: usize,

    // Density timing
    pub(super) last_density_update: Option<DateTime<Utc>>,
}

impl AdaptiveTrigger {
    pub fn new() -> Self {
        Self {
            ewma_short: 0.0,
            ewma_long: 0.0,
            importance_ewma: 0.0,
            context_signal: 0.0,
            segment_start: None,
            segment_event_count: 0,
            last_density_update: None,
        }
    }

    /// Process a single trigger event and return a decision plus a
    /// calibration log entry for offline analysis.
    pub fn process_event(
        &mut self,
        input: &TriggerInput,
        timestamp: DateTime<Utc>,
        params: &ResolvedParams,
    ) -> (TriggerDecision, CalibrationEntry) {
        // 1. Score importance
        let importance = self.score_importance(input, params);

        // 2. Update density signal (Dual-EWMA)
        let density = self.update_density(timestamp, params);

        // 3. Update importance EWMA
        let importance_sig = self.update_importance(importance, params);

        // 4. Update context signal
        let context = self.update_context(input, params);

        // 5. Compute buffer signal
        let buffer = self.compute_buffer_signal(params);

        // 6. Weighted combination
        let score = params.w_density * density
            + params.w_importance * importance_sig
            + params.w_context * context
            + params.w_buffer * buffer;

        // 7. Hysteresis decision
        let decision = self.decide(score, timestamp, params);

        // 8. Build calibration entry
        let (app_name, app_category) = scoring::extract_app_info(input);
        let trigger_action = match decision {
            TriggerDecision::OpenSegment | TriggerDecision::RestartSegment => {
                Some(TriggerAction::Start)
            }
            TriggerDecision::CloseSegment => Some(TriggerAction::Close),
            TriggerDecision::ForceCloseSegment => Some(TriggerAction::ForceClose),
            TriggerDecision::Continue => None,
        };

        let entry = CalibrationEntry {
            timestamp,
            event_type: scoring::input_type_str(input).to_string(),
            app_name,
            app_category,
            event_importance: importance,
            density_signal: density,
            importance_signal: importance_sig,
            context_signal: context,
            buffer_signal: buffer,
            trigger_score: score,
            trigger_action,
            active_regime_id: None,
            params_version_id: String::new(), // caller sets this
            params_json: String::new(),       // caller sets this
            is_noise: false,
        };

        (decision, entry)
    }

    /// Score the raw importance of a single event, applying per-app overrides.
    pub(super) fn score_importance(&self, input: &TriggerInput, params: &ResolvedParams) -> f32 {
        scoring::score_importance(input, params)
    }

    /// Update the Dual-EWMA density signal.
    pub(super) fn update_density(
        &mut self,
        timestamp: DateTime<Utc>,
        params: &ResolvedParams,
    ) -> f32 {
        signals::update_density(
            &mut self.ewma_short,
            &mut self.ewma_long,
            &mut self.last_density_update,
            timestamp,
            params,
        )
    }

    /// Update the importance EWMA and return its current value.
    pub(super) fn update_importance(&mut self, importance: f32, params: &ResolvedParams) -> f32 {
        signals::update_importance(&mut self.importance_ewma, importance, params)
    }

    /// Update the context signal based on context-relevant events.
    pub(super) fn update_context(&mut self, input: &TriggerInput, params: &ResolvedParams) -> f32 {
        signals::update_context(&mut self.context_signal, input, params)
    }

    /// Compute buffer fill signal: ratio of accumulated events to capacity.
    pub(super) fn compute_buffer_signal(&self, params: &ResolvedParams) -> f32 {
        signals::compute_buffer_signal(self.segment_event_count, params)
    }

    /// Hysteresis-based segment decision.
    pub(super) fn decide(
        &mut self,
        score: f32,
        timestamp: DateTime<Utc>,
        params: &ResolvedParams,
    ) -> TriggerDecision {
        decision::decide(
            &self.segment_start,
            &mut self.segment_event_count,
            score,
            timestamp,
            params,
        )
    }

    /// Open a new segment, resetting event counter.
    pub fn start_new_segment(&mut self, timestamp: DateTime<Utc>) {
        self.segment_start = Some(timestamp);
        self.segment_event_count = 0;
    }

    /// Return the current segment start timestamp, if a segment is open.
    pub fn current_segment_start(&self) -> Option<DateTime<Utc>> {
        self.segment_start
    }

    /// Close the current segment.
    pub fn close_segment(&mut self) {
        self.segment_start = None;
        self.segment_event_count = 0;
    }

    /// Current density signal (short-term EWMA).
    pub fn current_density_signal(&self) -> f32 {
        self.ewma_short
    }

    /// Current importance signal (EWMA).
    pub fn current_importance_signal(&self) -> f32 {
        self.importance_ewma
    }

    /// Current context signal (decaying).
    pub fn current_context_signal(&self) -> f32 {
        self.context_signal
    }
}

impl Default for AdaptiveTrigger {
    fn default() -> Self {
        Self::new()
    }
}
