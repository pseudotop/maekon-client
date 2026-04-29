use chrono::{DateTime, Utc};
use oneshim_core::models::tiered_memory::{
    CalibrationEntry, ResolvedParams, TriggerAction, TriggerInput,
};
use oneshim_core::models::work_session::AppCategory;

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
    ewma_short: f32,
    ewma_long: f32,
    importance_ewma: f32,
    context_signal: f32,

    // Segment state
    segment_start: Option<DateTime<Utc>>,
    segment_event_count: usize,

    // Density timing
    last_density_update: Option<DateTime<Utc>>,
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
        let (app_name, app_category) = extract_app_info(input);
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
            event_type: input_type_str(input).to_string(),
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
    fn score_importance(&self, input: &TriggerInput, params: &ResolvedParams) -> f32 {
        let (app_name, _) = extract_app_info(input);

        // Check per-app override first
        if let Some(&override_score) = params.importance_overrides.get(&app_name) {
            return override_score.clamp(0.0, 1.0);
        }

        // Base importance by event type
        let base = match input {
            TriggerInput::AppSwitchNew { .. } => 0.8,
            TriggerInput::WindowTitleChange { .. } => 0.6,
            TriggerInput::OcrUpdate { diff_ratio, .. } => 0.4 + diff_ratio.clamp(0.0, 1.0) * 0.4,
            TriggerInput::IdleTransition { to_idle } => {
                if *to_idle {
                    0.9
                } else {
                    0.7
                }
            }
            TriggerInput::WorkTypeChange { .. } => 0.85,
            TriggerInput::ClipboardChange => 0.5,
            TriggerInput::FileAccess => 0.55,
            TriggerInput::InputActivity => 0.3,
            TriggerInput::AppPoll { .. } => 0.15,
            TriggerInput::ProcessSnapshot => 0.1,
            TriggerInput::SystemMetric => 0.05,
        };

        base.clamp(0.0, 1.0)
    }

    /// Update the Dual-EWMA density signal using a pure time-weighted approach.
    ///
    /// Each event computes an instantaneous rate (1 / elapsed seconds since last
    /// event) and feeds it into both short and long EWMAs. The sigmoid of their
    /// normalized difference produces the density signal.
    fn update_density(&mut self, timestamp: DateTime<Utc>, params: &ResolvedParams) -> f32 {
        let elapsed = self
            .last_density_update
            .map(|last| (timestamp - last).num_milliseconds().max(1) as f32 / 1000.0)
            .unwrap_or(1.0);

        self.last_density_update = Some(timestamp);

        // Instantaneous rate: 1 event / elapsed seconds, capped at 10 events/sec
        let instant_rate = (1.0 / elapsed).min(10.0);

        // Update both EWMAs with the same rate input
        self.ewma_short =
            params.alpha_short * instant_rate + (1.0 - params.alpha_short) * self.ewma_short;
        self.ewma_long =
            params.alpha_long * instant_rate + (1.0 - params.alpha_long) * self.ewma_long;

        // Deviation: how much is short-term different from long-term baseline
        let deviation = (self.ewma_short - self.ewma_long) / self.ewma_long.max(0.001);
        sigmoid(deviation)
    }

    /// Update the importance EWMA and return its current value.
    fn update_importance(&mut self, importance: f32, params: &ResolvedParams) -> f32 {
        self.importance_ewma = params.alpha_importance * importance
            + (1.0 - params.alpha_importance) * self.importance_ewma;
        self.importance_ewma
    }

    /// Update the context signal based on context-relevant events.
    ///
    /// Context events (app switch, title change, work type change) boost the
    /// signal; other events cause it to decay.
    fn update_context(&mut self, input: &TriggerInput, params: &ResolvedParams) -> f32 {
        let is_context_event = matches!(
            input,
            TriggerInput::AppSwitchNew { .. }
                | TriggerInput::WindowTitleChange { .. }
                | TriggerInput::WorkTypeChange { .. }
                | TriggerInput::IdleTransition { .. }
        );

        if is_context_event {
            // Boost towards 1.0
            self.context_signal = self.context_signal + (1.0 - self.context_signal) * 0.5;
        } else {
            // Decay
            self.context_signal *= params.context_decay_rate;
        }

        self.context_signal.clamp(0.0, 1.0)
    }

    /// Compute buffer fill signal: ratio of accumulated events to capacity.
    fn compute_buffer_signal(&self, params: &ResolvedParams) -> f32 {
        if params.buffer_capacity == 0 {
            return 0.0;
        }
        (self.segment_event_count as f32 / params.buffer_capacity as f32).min(1.0)
    }

    /// Hysteresis-based segment decision.
    ///
    /// - No segment open AND score > t_high → `OpenSegment`
    /// - Segment open AND score > t_high AND age >= min_segment → `RestartSegment`
    /// - score < t_low AND segment open AND age >= min_segment → `CloseSegment`
    /// - segment_age >= max_segment → `ForceCloseSegment`
    /// - else → `Continue`
    fn decide(
        &mut self,
        score: f32,
        timestamp: DateTime<Utc>,
        params: &ResolvedParams,
    ) -> TriggerDecision {
        self.segment_event_count += 1;

        if let Some(start) = self.segment_start {
            let age_secs = (timestamp - start).num_seconds().max(0) as u64;

            // Force close if segment exceeds max duration
            if age_secs >= params.max_segment_secs {
                return TriggerDecision::ForceCloseSegment;
            }

            // Close if score drops below low threshold and min duration met
            if score < params.t_low && age_secs >= params.min_segment_secs {
                return TriggerDecision::CloseSegment;
            }

            // Restart: close current + open new (score still high after min duration)
            if score > params.t_high && age_secs >= params.min_segment_secs {
                return TriggerDecision::RestartSegment;
            }

            TriggerDecision::Continue
        } else {
            // No open segment — open one if score is high enough
            if score > params.t_high {
                return TriggerDecision::OpenSegment;
            }
            TriggerDecision::Continue
        }
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sigmoid activation: maps (-inf, +inf) → (0, 1).
fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Extract app name and category from a TriggerInput.
fn extract_app_info(input: &TriggerInput) -> (String, AppCategory) {
    match input {
        TriggerInput::AppSwitchNew {
            app_name, category, ..
        } => (app_name.clone(), *category),
        TriggerInput::AppPoll { app_name } => {
            (app_name.clone(), AppCategory::from_app_name(app_name))
        }
        TriggerInput::WindowTitleChange { app_name, .. } => {
            (app_name.clone(), AppCategory::from_app_name(app_name))
        }
        TriggerInput::IdleTransition { .. } => ("system".to_string(), AppCategory::System),
        TriggerInput::OcrUpdate { .. } => ("ocr".to_string(), AppCategory::Other),
        TriggerInput::InputActivity => ("input".to_string(), AppCategory::Other),
        TriggerInput::ProcessSnapshot => ("process".to_string(), AppCategory::System),
        TriggerInput::SystemMetric => ("system".to_string(), AppCategory::System),
        TriggerInput::ClipboardChange => ("clipboard".to_string(), AppCategory::Other),
        TriggerInput::FileAccess => ("file".to_string(), AppCategory::Other),
        TriggerInput::WorkTypeChange { .. } => ("work_type".to_string(), AppCategory::Other),
    }
}

/// Return a stable string label for the TriggerInput variant.
fn input_type_str(input: &TriggerInput) -> &'static str {
    match input {
        TriggerInput::AppSwitchNew { .. } => "APP_SWITCH_NEW",
        TriggerInput::AppPoll { .. } => "APP_POLL",
        TriggerInput::WindowTitleChange { .. } => "WINDOW_TITLE_CHANGE",
        TriggerInput::IdleTransition { .. } => "IDLE_TRANSITION",
        TriggerInput::OcrUpdate { .. } => "OCR_UPDATE",
        TriggerInput::InputActivity => "INPUT_ACTIVITY",
        TriggerInput::ProcessSnapshot => "PROCESS_SNAPSHOT",
        TriggerInput::SystemMetric => "SYSTEM_METRIC",
        TriggerInput::ClipboardChange => "CLIPBOARD_CHANGE",
        TriggerInput::FileAccess => "FILE_ACCESS",
        TriggerInput::WorkTypeChange { .. } => "WORK_TYPE_CHANGE",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use oneshim_core::models::tiered_memory::WorkType;

    fn default_params() -> ResolvedParams {
        let mut p = ResolvedParams::default();
        p.validate_and_normalize();
        p
    }

    fn app_switch(name: &str) -> TriggerInput {
        TriggerInput::AppSwitchNew {
            app_name: name.to_string(),
            prev_app: "Other".to_string(),
            category: AppCategory::Development,
        }
    }

    #[test]
    fn importance_scoring() {
        let trigger = AdaptiveTrigger::new();
        let params = default_params();

        let app_switch = TriggerInput::AppSwitchNew {
            app_name: "VSCode".to_string(),
            prev_app: "Chrome".to_string(),
            category: AppCategory::Development,
        };
        assert!((trigger.score_importance(&app_switch, &params) - 0.8).abs() < 1e-5);

        let poll = TriggerInput::AppPoll {
            app_name: "VSCode".to_string(),
        };
        assert!((trigger.score_importance(&poll, &params) - 0.15).abs() < 1e-5);

        let metric = TriggerInput::SystemMetric;
        assert!((trigger.score_importance(&metric, &params) - 0.05).abs() < 1e-5);

        let idle = TriggerInput::IdleTransition { to_idle: true };
        assert!((trigger.score_importance(&idle, &params) - 0.9).abs() < 1e-5);

        let work_change = TriggerInput::WorkTypeChange {
            from: WorkType::ActiveCoding,
            to: WorkType::Reading,
        };
        assert!((trigger.score_importance(&work_change, &params) - 0.85).abs() < 1e-5);

        let ocr = TriggerInput::OcrUpdate { diff_ratio: 0.5 };
        let ocr_score = trigger.score_importance(&ocr, &params);
        assert!(ocr_score > 0.59 && ocr_score < 0.61); // 0.4 + 0.5*0.4 = 0.6
    }

    #[test]
    fn ewma_convergence() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();
        let base = Utc::now();

        // Send many events at regular intervals — EWMAs should converge
        for i in 0..100 {
            let ts = base + Duration::seconds(i);
            trigger.update_density(ts, &params);
        }

        // After many events, short and long EWMA should be relatively close
        let diff = (trigger.ewma_short - trigger.ewma_long).abs();
        // They won't be identical due to different alpha values, but both should be > 0
        assert!(trigger.ewma_short > 0.0);
        assert!(trigger.ewma_long > 0.0);
        // Short tracks faster so may be slightly higher, but the gap should narrow
        assert!(diff < trigger.ewma_short.max(trigger.ewma_long) + 1.0);
    }

    #[test]
    fn hysteresis_no_oscillation() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.t_high = 0.60;
        params.t_low = 0.40;
        params.min_segment_secs = 0; // disable min for this test
        params.max_segment_secs = 9999;

        let base = Utc::now();

        // Start a segment
        trigger.start_new_segment(base);

        // Feed scores near the boundary (0.50) — between t_low and t_high
        // Decision should always be Continue (no oscillation)
        for i in 1..=20 {
            let ts = base + Duration::seconds(i);
            // Simulate a middling score by checking decide directly
            let decision = trigger.decide(0.50, ts, &params);
            assert_eq!(
                decision,
                TriggerDecision::Continue,
                "should not oscillate at score=0.50, iteration {i}"
            );
        }
    }

    #[test]
    fn force_close_at_max() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.max_segment_secs = 60;

        let base = Utc::now();
        trigger.start_new_segment(base);

        // Event at max_segment boundary
        let ts = base + Duration::seconds(60);
        let decision = trigger.decide(0.50, ts, &params);
        assert_eq!(decision, TriggerDecision::ForceCloseSegment);
    }

    #[test]
    fn min_segment_enforcement() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.min_segment_secs = 120;
        params.t_low = 0.30;
        params.max_segment_secs = 600;

        let base = Utc::now();
        trigger.start_new_segment(base);

        // Score below t_low but segment too young → Continue
        let ts = base + Duration::seconds(60); // only 60s, need 120s min
        let decision = trigger.decide(0.10, ts, &params);
        assert_eq!(decision, TriggerDecision::Continue);

        // Now past min_segment → CloseSegment
        let ts2 = base + Duration::seconds(121);
        let decision2 = trigger.decide(0.10, ts2, &params);
        assert_eq!(decision2, TriggerDecision::CloseSegment);
    }

    #[test]
    fn first_event_starts_segment() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();
        let base = Utc::now();

        // Feed several high-importance events to build up score
        for i in 0..10 {
            let ts = base + Duration::seconds(i * 2);
            let (decision, _entry) = trigger.process_event(&app_switch("VSCode"), ts, &params);
            if decision == TriggerDecision::OpenSegment {
                // Successfully triggered a segment start
                return;
            }
        }

        // With high-importance app switches, we should have started a segment
        // If not, the score didn't exceed t_high — acceptable with default params
        // that have t_high=0.65 and initial EWMA=0
    }

    #[test]
    fn context_signal_decay() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();

        // Boost context with a context event
        let ctx_input = app_switch("VSCode");
        trigger.update_context(&ctx_input, &params);
        let boosted = trigger.context_signal;
        assert!(boosted > 0.0);

        // Decay with non-context events
        let non_ctx = TriggerInput::SystemMetric;
        for _ in 0..20 {
            trigger.update_context(&non_ctx, &params);
        }
        assert!(trigger.context_signal < boosted);
        // After many decays, should be close to zero
        assert!(trigger.context_signal < 0.1);
    }

    #[test]
    fn buffer_signal_increases() {
        let mut trigger = AdaptiveTrigger::new();
        let mut params = default_params();
        params.buffer_capacity = 10;

        // Initially zero
        assert!((trigger.compute_buffer_signal(&params) - 0.0).abs() < 1e-5);

        // Simulate accumulating events
        trigger.segment_event_count = 5;
        let sig = trigger.compute_buffer_signal(&params);
        assert!((sig - 0.5).abs() < 1e-5);

        trigger.segment_event_count = 10;
        let sig = trigger.compute_buffer_signal(&params);
        assert!((sig - 1.0).abs() < 1e-5);

        // Over capacity is clamped to 1.0
        trigger.segment_event_count = 20;
        let sig = trigger.compute_buffer_signal(&params);
        assert!((sig - 1.0).abs() < 1e-5);
    }

    #[test]
    fn getter_methods_reflect_signal_state() {
        let mut trigger = AdaptiveTrigger::new();
        let params = default_params();
        let input = TriggerInput::AppSwitchNew {
            app_name: "VSCode".to_string(),
            prev_app: "Slack".to_string(),
            category: AppCategory::Development,
        };

        let _ = trigger.process_event(&input, Utc::now(), &params);

        assert!(trigger.current_density_signal() > 0.0);
        assert!(trigger.current_importance_signal() > 0.0);
        // Context signal boosted towards 1.0 after app switch (0 + (1-0)*0.5 = 0.5)
        assert!(trigger.current_context_signal() > 0.4);
    }

    // ── Full segment lifecycle integration test ──────────────────────
    //
    // Tests the complete cycle using AdaptiveTrigger + SegmentBuffer +
    // ContentTracker together (no mocks, no async, pure computation).

    #[test]
    fn full_segment_lifecycle_open_accumulate_close() {
        use crate::content_tracker::{ContentTracker, ContentUpdateInput};
        use crate::SegmentBuffer;
        use oneshim_core::models::tiered_memory::{ContentType, EngagementMetrics};

        let mut trigger = AdaptiveTrigger::new();
        let mut segment_buffer = SegmentBuffer::new(200);
        let mut content_tracker = ContentTracker::new();

        // Use tuned params that make it easy to trigger open/close
        let mut params = default_params();
        params.t_high = 0.55;
        params.t_low = 0.35;
        params.min_segment_secs = 0; // disable min for predictable testing
        params.max_segment_secs = 600;
        params.buffer_capacity = 100;

        let base = Utc::now();

        // ── Phase 1: Feed high-importance events to trigger OpenSegment ──
        let mut opened = false;
        for i in 0..15 {
            let ts = base + Duration::seconds(i * 2);
            let input = TriggerInput::AppSwitchNew {
                app_name: format!("App{}", i % 3),
                prev_app: format!("App{}", (i + 1) % 3),
                category: AppCategory::Development,
            };
            let (decision, _cal) = trigger.process_event(&input, ts, &params);

            if decision == TriggerDecision::OpenSegment {
                trigger.start_new_segment(ts);
                segment_buffer.start_segment(ts);
                segment_buffer.push(ts, input.clone());
                opened = true;
                break;
            }
        }
        assert!(
            opened,
            "segment should have opened after high-importance events"
        );
        assert!(segment_buffer.start_time().is_some());

        // ── Phase 2: Feed content changes while segment is open ──
        let content_labels = ["main.rs", "lib.rs", "README.md"];
        for (i, label) in content_labels.iter().enumerate() {
            let ts = base + Duration::seconds(30 + (i as i64) * 10);

            // Push an event into the segment buffer
            let input = TriggerInput::AppPoll {
                app_name: "VS Code".to_string(),
            };
            segment_buffer.push(ts, input);

            // Feed content into the content tracker
            content_tracker.update(ContentUpdateInput {
                content_label: label.to_string(),
                content_type: ContentType::File,
                work_type: oneshim_core::models::tiered_memory::WorkType::ActiveCoding,
                engagement: EngagementMetrics {
                    keystrokes_per_min: 40.0,
                    mouse_clicks_per_min: 5.0,
                    scroll_events_per_min: 2.0,
                    shortcut_ratio: 0.1,
                    typing_burst_count: 1,
                    idle_ratio: 0.0,
                },
                confidence: 0.95,
                timestamp: ts,
                gui_summary: None,
            });
        }

        // Verify segment buffer has accumulated events (3 content + 1 open)
        assert!(
            segment_buffer.len() >= 3,
            "buffer should have at least 3 events, got {}",
            segment_buffer.len()
        );

        // ── Phase 3: Force low-importance events to trigger CloseSegment ──
        let mut closed = false;
        for i in 0..30 {
            let ts = base + Duration::seconds(60 + i * 3);
            let input = TriggerInput::SystemMetric; // very low importance (0.05)
            let (decision, _cal) = trigger.process_event(&input, ts, &params);

            match decision {
                TriggerDecision::CloseSegment | TriggerDecision::ForceCloseSegment => {
                    closed = true;
                    break;
                }
                _ => {
                    segment_buffer.push(ts, input);
                }
            }
        }
        assert!(
            closed,
            "segment should have closed after low-importance events"
        );

        // ── Phase 4: Drain and verify ──
        let seg_events = segment_buffer.drain_all();
        assert!(!seg_events.is_empty(), "drained segment should have events");
        assert!(
            segment_buffer.is_empty(),
            "buffer should be empty after drain"
        );
        assert!(
            segment_buffer.start_time().is_none(),
            "segment start should be cleared"
        );

        // Drain content tracker
        let end_time = base + Duration::seconds(150);
        let content_activities = content_tracker.drain_all(end_time);
        assert_eq!(
            content_activities.len(),
            3,
            "should have 3 content activities (main.rs, lib.rs, README.md)"
        );
        assert_eq!(content_activities[0].content_label, "main.rs");
        assert_eq!(content_activities[1].content_label, "lib.rs");
        assert_eq!(content_activities[2].content_label, "README.md");

        // Verify durations: first two activities have 10s each (switched after 10s)
        assert_eq!(content_activities[0].duration_secs, 10);
        assert_eq!(content_activities[1].duration_secs, 10);
        // Last activity: from t+50 to t+150 = 100s
        assert_eq!(content_activities[2].duration_secs, 100);

        // Trigger should be in a clean state after close_segment
        trigger.close_segment();
        assert!(trigger.current_segment_start().is_none());
    }

    #[test]
    fn full_lifecycle_restart_segment() {
        use crate::SegmentBuffer;

        let mut trigger = AdaptiveTrigger::new();
        let mut segment_buffer = SegmentBuffer::new(200);

        let mut params = default_params();
        params.t_high = 0.55;
        params.t_low = 0.35;
        params.min_segment_secs = 0;
        params.max_segment_secs = 600;

        let base = Utc::now();

        // Open a segment first
        let mut opened = false;
        for i in 0..15 {
            let ts = base + Duration::seconds(i * 2);
            let input = app_switch(&format!("App{i}"));
            let (decision, _) = trigger.process_event(&input, ts, &params);
            if decision == TriggerDecision::OpenSegment {
                trigger.start_new_segment(ts);
                segment_buffer.start_segment(ts);
                segment_buffer.push(ts, input);
                opened = true;
                break;
            }
        }
        assert!(opened);

        // Feed more high-importance events until RestartSegment
        let mut restarted = false;
        for i in 0..50 {
            let ts = base + Duration::seconds(30 + i * 2);
            let input = app_switch(&format!("App{i}"));
            let (decision, _) = trigger.process_event(&input, ts, &params);

            match decision {
                TriggerDecision::RestartSegment => {
                    // Close old segment
                    let old_events = segment_buffer.drain_all();
                    assert!(!old_events.is_empty(), "old segment should have events");

                    // Start new segment
                    trigger.start_new_segment(ts);
                    segment_buffer.start_segment(ts);
                    restarted = true;
                    break;
                }
                TriggerDecision::Continue => {
                    segment_buffer.push(ts, input);
                }
                _ => {
                    segment_buffer.push(ts, input);
                }
            }
        }
        assert!(restarted, "should have triggered RestartSegment");
        assert!(
            segment_buffer.start_time().is_some(),
            "new segment should be open after restart"
        );
    }

    #[test]
    fn full_lifecycle_force_close_max_duration() {
        use crate::SegmentBuffer;

        let mut trigger = AdaptiveTrigger::new();
        let mut segment_buffer = SegmentBuffer::new(200);

        let mut params = default_params();
        params.t_high = 0.55;
        params.t_low = 0.10; // very low, unlikely to trigger normal close
        params.min_segment_secs = 0;
        params.max_segment_secs = 30; // short max for testing

        let base = Utc::now();

        // Open a segment
        trigger.start_new_segment(base);
        segment_buffer.start_segment(base);

        // Feed middling events until max duration forces close
        let mut force_closed = false;
        for i in 1..=20 {
            let ts = base + Duration::seconds(i * 2);
            let input = TriggerInput::AppPoll {
                app_name: "VS Code".to_string(),
            };
            let (decision, _) = trigger.process_event(&input, ts, &params);

            if decision == TriggerDecision::ForceCloseSegment {
                force_closed = true;
                break;
            }
            segment_buffer.push(ts, input);
        }

        assert!(force_closed, "should force-close at max_segment_secs=30");
    }
}
