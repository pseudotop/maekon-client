//! Heatmap aggregation and goal-progress overlay emission helpers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use maekon_core::models::event::InputActivityEvent;

use crate::magic_overlay::{MagicOverlayHandle, OverlayPointerContextPayload};
use crate::scheduler::AdaptiveTriggerState;

const POINTER_CONTEXT_SAMPLE_RATE_HZ: u32 = 30;
const POINTER_CONTEXT_MIN_EMIT_INTERVAL: Duration =
    Duration::from_millis(1000 / POINTER_CONTEXT_SAMPLE_RATE_HZ as u64);
const POINTER_CONTEXT_TTL_MS: u64 = 900;

#[derive(Debug, Default, Clone)]
pub(crate) struct PointerContextEmitterState {
    last_emit_at: Option<Instant>,
    last_click_count: u32,
    visible: bool,
}

impl PointerContextEmitterState {
    pub(super) fn next_payload(
        &mut self,
        input_snap: &InputActivityEvent,
        capture_paused: bool,
        indicator_visible: bool,
        now: Instant,
    ) -> Option<OverlayPointerContextPayload> {
        let click_count = input_snap.mouse.click_count;
        if capture_paused || !indicator_visible {
            return self.hide_once(click_count);
        }

        let Some((raw_x, raw_y)) = input_snap.mouse.last_position else {
            return self.hide_once(click_count);
        };
        if !raw_x.is_finite() || !raw_y.is_finite() {
            return self.hide_once(click_count);
        }

        let click_pulse = click_count > self.last_click_count;
        let elapsed = self
            .last_emit_at
            .and_then(|last| now.checked_duration_since(last))
            .unwrap_or(POINTER_CONTEXT_MIN_EMIT_INTERVAL);
        if !click_pulse && elapsed < POINTER_CONTEXT_MIN_EMIT_INTERVAL {
            self.last_click_count = click_count;
            return None;
        }

        self.visible = true;
        self.last_emit_at = Some(now);
        self.last_click_count = click_count;
        Some(OverlayPointerContextPayload {
            enabled: true,
            x: Some(raw_x.max(0.0)),
            y: Some(raw_y.max(0.0)),
            click_count,
            click_pulse,
            reduced_motion: false,
            ttl_ms: POINTER_CONTEXT_TTL_MS,
            sample_rate_hz: POINTER_CONTEXT_SAMPLE_RATE_HZ,
        })
    }

    fn hide_once(&mut self, click_count: u32) -> Option<OverlayPointerContextPayload> {
        self.last_click_count = click_count;
        if !self.visible {
            return None;
        }
        self.visible = false;
        Some(OverlayPointerContextPayload::hidden(click_count))
    }
}

pub(crate) fn emit_pointer_context_highlight(
    pointer_context_state: &mut PointerContextEmitterState,
    input_snap: &InputActivityEvent,
    overlay_ref: &Option<MagicOverlayHandle>,
    capture_paused: bool,
    indicator_visible: bool,
) {
    let Some(ref overlay) = overlay_ref else {
        return;
    };
    if let Some(payload) = pointer_context_state.next_payload(
        input_snap,
        capture_paused,
        indicator_visible,
        Instant::now(),
    ) {
        overlay.emit_pointer_context(payload);
    }
}

/// Record click positions into the heatmap aggregator and emit a snapshot
/// to the overlay when available.  Also emits goal progress.
pub(crate) async fn emit_heatmap_and_goals(
    adaptive_trigger_state: &mut Option<AdaptiveTriggerState>,
    input_snap: &InputActivityEvent,
    overlay_ref: &Option<MagicOverlayHandle>,
    coaching_engine_ref: &Option<Arc<maekon_analysis::CoachingEngine>>,
) {
    // Heatmap aggregation
    if let Some(ref mut ts) = adaptive_trigger_state {
        if let Some((x, y)) = input_snap.mouse.last_position {
            ts.heatmap_aggregator
                .record(x, y, input_snap.mouse.click_count);
        }
        if let Some(grid) = ts.heatmap_aggregator.take_snapshot() {
            if let Some(ref overlay) = overlay_ref {
                overlay.emit_heatmap(grid);
            }
        }
    }

    // Goal progress emission
    if let Some(ref coaching) = coaching_engine_ref {
        if let Some(ref overlay) = overlay_ref {
            let goals = coaching.all_goal_progress().await;
            if !goals.is_empty() {
                overlay.update_goal_progress(goals);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::event::{KeyboardActivity, MouseActivity};
    use std::time::{Duration, Instant};

    fn input_snap(position: Option<(f32, f32)>, click_count: u32) -> InputActivityEvent {
        InputActivityEvent {
            timestamp: Utc::now(),
            period_secs: 1,
            mouse: MouseActivity {
                click_count,
                last_position: position,
                ..MouseActivity::default()
            },
            keyboard: KeyboardActivity::default(),
            app_name: "Code".to_string(),
            keystroke_profile: None,
        }
    }

    #[test]
    fn pointer_context_emits_only_when_capture_indicator_is_active() {
        let now = Instant::now();
        let mut state = PointerContextEmitterState::default();

        assert!(
            state
                .next_payload(&input_snap(Some((20.0, 30.0)), 0), true, true, now)
                .is_none(),
            "paused capture should not emit a visible pointer payload"
        );
        assert!(
            state
                .next_payload(&input_snap(Some((20.0, 30.0)), 0), false, false, now)
                .is_none(),
            "hidden capture indicator should keep pointer context disabled"
        );

        let payload = state
            .next_payload(&input_snap(Some((20.0, 30.0)), 0), false, true, now)
            .expect("active capture should emit");
        assert!(payload.enabled);
        assert_eq!(payload.x, Some(20.0));
        assert_eq!(payload.y, Some(30.0));
    }

    #[test]
    fn pointer_context_clears_once_when_capture_pauses_after_visible_payload() {
        let now = Instant::now();
        let mut state = PointerContextEmitterState::default();

        assert!(
            state
                .next_payload(&input_snap(Some((20.0, 30.0)), 2), false, true, now)
                .expect("visible payload")
                .enabled
        );

        let hidden = state
            .next_payload(
                &input_snap(Some((20.0, 30.0)), 2),
                true,
                true,
                now + Duration::from_millis(40),
            )
            .expect("first paused tick clears the visible layer");
        assert!(!hidden.enabled);
        assert!(hidden.x.is_none());
        assert!(hidden.y.is_none());

        assert!(
            state
                .next_payload(
                    &input_snap(Some((20.0, 30.0)), 2),
                    true,
                    true,
                    now + Duration::from_millis(80),
                )
                .is_none(),
            "subsequent paused ticks should not emit repeated hidden payloads"
        );
    }

    #[test]
    fn pointer_context_hides_when_position_is_missing_or_invalid() {
        let now = Instant::now();
        let mut state = PointerContextEmitterState::default();

        assert!(
            state
                .next_payload(&input_snap(Some((20.0, 30.0)), 0), false, true, now)
                .expect("visible payload")
                .enabled
        );

        let hidden = state
            .next_payload(
                &input_snap(None, 0),
                false,
                true,
                now + Duration::from_millis(40),
            )
            .expect("missing position clears the visible layer");
        assert!(!hidden.enabled);

        assert!(
            state
                .next_payload(
                    &input_snap(Some((f32::INFINITY, 10.0)), 0),
                    false,
                    true,
                    now + Duration::from_millis(80),
                )
                .is_none(),
            "non-finite coordinates should remain hidden"
        );
    }

    #[test]
    fn pointer_context_clamps_negative_coordinates_without_persisting_trace() {
        let now = Instant::now();
        let mut state = PointerContextEmitterState::default();
        let payload = state
            .next_payload(&input_snap(Some((-12.0, -8.0)), 0), false, true, now)
            .expect("active capture should emit clamped payload");

        assert_eq!(payload.x, Some(0.0));
        assert_eq!(payload.y, Some(0.0));
        assert_eq!(payload.sample_rate_hz, 30);
        assert_eq!(payload.ttl_ms, 900);
    }

    #[test]
    fn pointer_context_click_delta_pulses_even_inside_sample_interval() {
        let now = Instant::now();
        let mut state = PointerContextEmitterState::default();

        let first = state
            .next_payload(&input_snap(Some((10.0, 10.0)), 1), false, true, now)
            .expect("first payload");
        assert!(first.click_pulse);

        assert!(
            state
                .next_payload(
                    &input_snap(Some((11.0, 11.0)), 1),
                    false,
                    true,
                    now + Duration::from_millis(5),
                )
                .is_none(),
            "movement-only updates inside 30 Hz interval should be skipped"
        );

        let click = state
            .next_payload(
                &input_snap(Some((12.0, 12.0)), 2),
                false,
                true,
                now + Duration::from_millis(6),
            )
            .expect("click delta should bypass movement throttle");
        assert!(click.click_pulse);
        assert_eq!(click.click_count, 2);
    }

    #[test]
    fn pointer_context_movement_emits_after_sample_interval() {
        let now = Instant::now();
        let mut state = PointerContextEmitterState::default();

        assert!(state
            .next_payload(&input_snap(Some((10.0, 10.0)), 0), false, true, now)
            .is_some());
        assert!(state
            .next_payload(
                &input_snap(Some((11.0, 11.0)), 0),
                false,
                true,
                now + Duration::from_millis(10),
            )
            .is_none());

        let moved = state
            .next_payload(
                &input_snap(Some((12.0, 12.0)), 0),
                false,
                true,
                now + Duration::from_millis(34),
            )
            .expect("movement emits after 30 Hz interval");
        assert!(!moved.click_pulse);
    }
}
