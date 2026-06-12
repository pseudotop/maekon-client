use chrono::{DateTime, Utc};
use maekon_core::models::tiered_memory::ResolvedParams;

use super::TriggerDecision;

/// Hysteresis-based segment decision.
///
/// - No segment open AND score > t_high → `OpenSegment`
/// - Segment open AND score > t_high AND age >= min_segment → `RestartSegment`
/// - score < t_low AND segment open AND age >= min_segment → `CloseSegment`
/// - segment_age >= max_segment → `ForceCloseSegment`
/// - else → `Continue`
pub(super) fn decide(
    segment_start: &Option<DateTime<Utc>>,
    segment_event_count: &mut usize,
    score: f32,
    timestamp: DateTime<Utc>,
    params: &ResolvedParams,
) -> TriggerDecision {
    *segment_event_count += 1;

    if let Some(start) = segment_start {
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
