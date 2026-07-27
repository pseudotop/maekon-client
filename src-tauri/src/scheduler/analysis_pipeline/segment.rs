//! Segment lifecycle: open / close / restart decisions and embedding pipeline.

use chrono::{DateTime, Utc};
use maekon_analysis::TriggerDecision;
use maekon_core::models::tiered_memory::{TriggerInput, TriggerReason};
use maekon_core::ports::storage::StorageService;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

/// Caps concurrent Phase-2 LLM summarization + embedding tasks.
///
/// Rapid `CloseSegment`/`ForceCloseSegment` triggers (pathological application
/// switching) would otherwise cause unbounded task accumulation. A cap of 4
/// allows burst tolerance while keeping memory bounded at ~100 MB budget.
/// When all permits are held the current segment is skipped without blocking
/// the scheduler's 1-second hot path.
pub(super) static LLM_SUMMARY_SEMAPHORE: Semaphore = Semaphore::const_new(4);

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

        // Phase 0: Persist the closed segment locally (#5662). This MUST run
        // before the LLM-summary spawn below, whose update_segment_llm_summary
        // is an UPDATE WHERE id — without a prior INSERT it silently affects 0
        // rows. It is the data root for Daily/Weekly Digest, Timeline, Dashboard
        // timetable, regime distribution and Recalibration, all of which were
        // empty on standalone (no local producer existed). with_conn offloads to
        // spawn_blocking so this does not block the scheduler hot path.
        let persisted = match storage.save_activity_segment(&summary).await {
            Ok(()) => true,
            Err(e) => {
                warn!("activity segment persist failure: {e}");
                false
            }
        };

        // Phase 0b (#8051): index the segment's base content into the FTS5
        // `search_fts` table so the dashboard "keyword"/"hybrid" search modes
        // return data. Before this wiring the only content writers were
        // test-only (dead-writer), so keyword search always returned empty.
        //
        // Indexed here — synchronously in the close path, but off the async
        // runtime via `with_conn` → `spawn_blocking` — so the segment becomes
        // searchable immediately even when LLM summarization is disabled or
        // still pending (Phase 2 below re-indexes with the summary once it
        // exists). Only after a successful persist: an FTS row for a segment
        // absent from `activity_segments` would surface a dangling hit.
        //
        // Non-fatal: a failure never blocks segment persistence; it is logged
        // clearly (not silently swallowed) and the next startup backfill
        // re-indexes the segment.
        if persisted {
            if let Some(text_search) = ts.text_search.as_ref() {
                let fts_text = summary.searchable_content();
                if !fts_text.trim().is_empty() {
                    let start_iso = summary.start_time.to_rfc3339();
                    let end_iso = summary.end_time.to_rfc3339();
                    if let Err(e) = text_search
                        .sync_segment_enriched(&summary.segment_id, &fts_text, &start_iso, &end_iso)
                        .await
                    {
                        warn!(
                            segment_id = %summary.segment_id,
                            "FTS content indexing failed (non-fatal; keyword search will \
                             miss this segment until the next startup backfill): {e}"
                        );
                    }
                }
            }
        }

        // Phase 1: Embed content activities immediately
        if let Some(ref pipeline) = ts.embedding_pipeline {
            if let Err(e) = pipeline.process_content_activities(&summary).await {
                warn!("content embedding failure: {e}");
            }
        }

        // Phase 2: Async LLM summary + embed (non-blocking, concurrency-capped)
        //
        // `LLM_SUMMARY_SEMAPHORE` holds at most 4 concurrent tasks so that
        // rapid segment close bursts do not accumulate unbounded work items.
        // The permit is transferred into the spawned task and released on drop
        // (RAII). When the semaphore is exhausted the segment is skipped rather
        // than blocking the scheduler hot path.
        if let Some(ref summarizer) = ts.llm_summarizer {
            match LLM_SUMMARY_SEMAPHORE.try_acquire() {
                Ok(permit) => {
                    let summarizer = summarizer.clone();
                    let storage_clone = storage.clone();
                    let pipeline = ts.embedding_pipeline.clone();
                    let text_search = ts.text_search.clone();
                    let segment_id = summary.segment_id.clone();
                    let end_time = summary.end_time;
                    let mut summary_clone = summary.clone();

                    tokio::spawn(async move {
                        // Permit is held for the duration of the task and
                        // released automatically when this closure returns.
                        let _permit = permit;
                        if let Some(text) = summarizer.summarize(&summary_clone).await {
                            if let Err(e) = storage_clone
                                .update_segment_llm_summary(&segment_id, &text)
                                .await
                            {
                                warn!("LLM summary storage failure: {e}");
                            }
                            // #8051: re-index with the LLM summary now available
                            // so the richest signal (the natural-language
                            // summary) is keyword-searchable. Idempotent upsert
                            // (DELETE+INSERT by segment_id) over the Phase-0b
                            // base index.
                            if let Some(text_search) = text_search {
                                summary_clone.llm_summary = Some(text.clone());
                                let fts_text = summary_clone.searchable_content();
                                let start_iso = summary_clone.start_time.to_rfc3339();
                                let end_iso = summary_clone.end_time.to_rfc3339();
                                if let Err(e) = text_search
                                    .sync_segment_enriched(
                                        &segment_id,
                                        &fts_text,
                                        &start_iso,
                                        &end_iso,
                                    )
                                    .await
                                {
                                    warn!(
                                        segment_id = %segment_id,
                                        "FTS re-index with LLM summary failed (non-fatal): {e}"
                                    );
                                }
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
                Err(_) => {
                    debug!(
                        segment_id = %summary.segment_id,
                        "LLM summary semaphore exhausted — skipping segment summarization"
                    );
                }
            }
        }
    }

    ts.trigger.close_segment();
}
