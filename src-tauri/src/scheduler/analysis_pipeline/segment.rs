//! Segment lifecycle: open / close / restart decisions and embedding pipeline.

use chrono::{DateTime, Utc};
use oneshim_analysis::TriggerDecision;
use oneshim_core::models::tiered_memory::{TriggerInput, TriggerReason};
use oneshim_core::ports::storage::StorageService;
use std::sync::Arc;
use tracing::{info, warn};

use super::super::AdaptiveTriggerState;

/// Handle segment open / close / restart decisions and trigger embedding pipeline.
pub(in crate::scheduler) async fn handle_segment_lifecycle(
    ts: &mut AdaptiveTriggerState,
    decision: TriggerDecision,
    trigger_input: TriggerInput,
    now: DateTime<Utc>,
    storage: &Arc<dyn StorageService>,
) {
    match decision {
        TriggerDecision::OpenSegment => {
            ts.trigger.start_new_segment(now);
            ts.segment_buffer.start_segment(now);
            ts.segment_buffer.push(now, trigger_input);
        }
        TriggerDecision::RestartSegment
        | TriggerDecision::CloseSegment
        | TriggerDecision::ForceCloseSegment => {
            handle_segment_close(ts, decision, now, storage).await;

            // If restart, open new segment
            if matches!(decision, TriggerDecision::RestartSegment) {
                ts.trigger.start_new_segment(now);
                ts.segment_buffer.start_segment(now);
            }
        }
        TriggerDecision::Continue => {
            ts.segment_buffer.push(now, trigger_input);
        }
    }
}

/// Close a segment: summarize, run embedding Phase 1, spawn Phase 2 LLM summary.
async fn handle_segment_close(
    ts: &mut AdaptiveTriggerState,
    decision: TriggerDecision,
    now: DateTime<Utc>,
    storage: &Arc<dyn StorageService>,
) {
    let _seg_events = ts.segment_buffer.drain_all();
    let content_activities = ts.content_tracker.drain_all(now);

    let reason = match decision {
        TriggerDecision::RestartSegment => TriggerReason::ScoreHigh,
        TriggerDecision::CloseSegment => TriggerReason::ScoreLow,
        TriggerDecision::ForceCloseSegment => TriggerReason::ForcedMaxDuration,
        _ => TriggerReason::ScoreHigh,
    };

    if let Some(start) = ts.trigger.current_segment_start() {
        let summary = ts.segment_summarizer.summarize(
            uuid::Uuid::new_v4().to_string(),
            start,
            now,
            &[], // raw events from storage (Phase 1b)
            content_activities,
            None, // container detection (Phase 1b)
            reason,
            ts.current_regime_id.clone(),
        );

        info!(
            segment_id = %summary.segment_id,
            duration = summary.duration_secs,
            events = summary.event_count,
            "segment closed: {}",
            summary.dominant_category
        );

        // Phase 1: Embed content activities immediately
        if let Some(ref pipeline) = ts.embedding_pipeline {
            if let Err(e) = pipeline.process_content_activities(&summary).await {
                warn!("content embedding failure: {e}");
            }
        }

        // Phase 2: Async LLM summary + embed (non-blocking)
        if let Some(ref summarizer) = ts.llm_summarizer {
            let summarizer = summarizer.clone();
            let storage_clone = storage.clone();
            let pipeline = ts.embedding_pipeline.clone();
            let segment_id = summary.segment_id.clone();
            let end_time = summary.end_time;
            let summary_clone = summary.clone();

            tokio::spawn(async move {
                if let Some(text) = summarizer.summarize(&summary_clone).await {
                    if let Err(e) = storage_clone
                        .update_segment_llm_summary(&segment_id, &text)
                        .await
                    {
                        warn!("LLM summary storage failure: {e}");
                    }
                    if let Some(pipeline) = pipeline {
                        if let Err(e) = pipeline
                            .process_llm_summary(&segment_id, &text, end_time)
                            .await
                        {
                            warn!("LLM summary embedding failure: {e}");
                        }
                    }
                }
            });
        }
    }

    ts.trigger.close_segment();
}
