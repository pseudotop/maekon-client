use chrono::{DateTime, Utc};
use maekon_core::models::tiered_memory::{ResolvedParams, TriggerInput};

/// Sigmoid activation: maps (-inf, +inf) → (0, 1).
pub(super) fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Update the Dual-EWMA density signal using a pure time-weighted approach.
///
/// Each event computes an instantaneous rate (1 / elapsed seconds since last
/// event) and feeds it into both short and long EWMAs. The sigmoid of their
/// normalized difference produces the density signal.
pub(super) fn update_density(
    ewma_short: &mut f32,
    ewma_long: &mut f32,
    last_density_update: &mut Option<DateTime<Utc>>,
    timestamp: DateTime<Utc>,
    params: &ResolvedParams,
) -> f32 {
    let elapsed = last_density_update
        .map(|last| (timestamp - last).num_milliseconds().max(1) as f32 / 1000.0)
        .unwrap_or(1.0);

    *last_density_update = Some(timestamp);

    // Instantaneous rate: 1 event / elapsed seconds, capped at 10 events/sec
    let instant_rate = (1.0 / elapsed).min(10.0);

    // Update both EWMAs with the same rate input
    *ewma_short = params.alpha_short * instant_rate + (1.0 - params.alpha_short) * *ewma_short;
    *ewma_long = params.alpha_long * instant_rate + (1.0 - params.alpha_long) * *ewma_long;

    // Deviation: how much is short-term different from long-term baseline
    let deviation = (*ewma_short - *ewma_long) / ewma_long.max(0.001);
    sigmoid(deviation)
}

/// Update the importance EWMA and return its current value.
pub(super) fn update_importance(
    importance_ewma: &mut f32,
    importance: f32,
    params: &ResolvedParams,
) -> f32 {
    *importance_ewma =
        params.alpha_importance * importance + (1.0 - params.alpha_importance) * *importance_ewma;
    *importance_ewma
}

/// Update the context signal based on context-relevant events.
///
/// Context events (app switch, title change, work type change) boost the
/// signal; other events cause it to decay.
pub(super) fn update_context(
    context_signal: &mut f32,
    input: &TriggerInput,
    params: &ResolvedParams,
) -> f32 {
    let is_context_event = matches!(
        input,
        TriggerInput::AppSwitchNew { .. }
            | TriggerInput::WindowTitleChange { .. }
            | TriggerInput::WorkTypeChange { .. }
            | TriggerInput::IdleTransition { .. }
    );

    if is_context_event {
        // Boost towards 1.0
        *context_signal = *context_signal + (1.0 - *context_signal) * 0.5;
    } else {
        // Decay
        *context_signal *= params.context_decay_rate;
    }

    context_signal.clamp(0.0, 1.0)
}

/// Compute buffer fill signal: ratio of accumulated events to capacity.
pub(super) fn compute_buffer_signal(segment_event_count: usize, params: &ResolvedParams) -> f32 {
    if params.buffer_capacity == 0 {
        return 0.0;
    }
    (segment_event_count as f32 / params.buffer_capacity as f32).min(1.0)
}
