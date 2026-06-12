//! Consent & PII level change audit helpers, and segment record conversion.

use tracing::{info, warn};

use maekon_core::models::storage_records::SegmentSummaryRecord;
use maekon_core::models::tiered_memory::{ContentActivity, SegmentSummary, TriggerReason};

/// Log audit events when full_text_extraction consent or PII extraction
/// level changes between ticks. Returns updated `(prev_consent, prev_pii_level)`.
pub(crate) fn audit_consent_and_pii_changes(
    full_text_consent: bool,
    prev_full_text_consent: bool,
    pii_level: maekon_core::config::PiiFilterLevel,
    prev_pii_level: maekon_core::config::PiiFilterLevel,
) -> (bool, maekon_core::config::PiiFilterLevel) {
    let mut new_consent = prev_full_text_consent;
    let mut new_pii = prev_pii_level;

    if full_text_consent != prev_full_text_consent {
        if full_text_consent {
            info!(
                event = "full_text_extraction_consent_granted",
                "User granted full_text_extraction consent — Off PII level now effective"
            );
        } else {
            warn!(
                event = "full_text_extraction_consent_revoked",
                "User revoked full_text_extraction consent — falling back to Standard PII level"
            );
        }
        new_consent = full_text_consent;
    }

    if pii_level != prev_pii_level {
        info!(
            event = "pii_extraction_level_changed",
            old = ?prev_pii_level,
            new = ?pii_level,
            "PII extraction level changed"
        );
        new_pii = pii_level;
    }

    (new_consent, new_pii)
}

/// Convert a SegmentSummaryRecord (storage row) to SegmentSummary (domain model)
/// for use with DailyDigestGenerator.
pub(crate) fn record_to_segment_summary(r: &SegmentSummaryRecord) -> Option<SegmentSummary> {
    let start_time = r.start_time.parse().ok()?;
    let end_time = r.end_time.parse().ok()?;

    let app_breakdown: std::collections::HashMap<String, u64> =
        serde_json::from_str(&r.app_breakdown).unwrap_or_default();

    let content_activities: Vec<ContentActivity> =
        serde_json::from_str(&r.content_activities_json).unwrap_or_default();

    Some(SegmentSummary {
        segment_id: r.segment_id.clone(),
        start_time,
        end_time,
        duration_secs: r.duration_secs,
        regime_id: r.regime_id.clone(),
        trigger_reason: TriggerReason::RegimeChange,
        event_count: 0,
        app_breakdown,
        category_breakdown: std::collections::HashMap::new(),
        context_switch_count: r.context_switch_count,
        dominant_category: r.dominant_category.clone(),
        avg_importance: 0.5,
        patterns_detected: vec![],
        content_activities,
        container: None,
        llm_summary: r.llm_summary.clone(),
    })
}

/// Build a `SegmentStats` snapshot from the current `AdaptiveTriggerState`.
/// Returns `None` if the content tracker has no active content.
pub(crate) fn build_segment_stats_snapshot(
    ts: &crate::scheduler::AdaptiveTriggerState,
) -> Option<maekon_analysis::SegmentStats> {
    let entries = maekon_analysis::to_content_summary_entries(&ts.content_tracker.peek());
    if entries.is_empty() {
        return None;
    }

    let duration_mins = ts
        .trigger
        .current_segment_start()
        .map(|start| {
            let elapsed = (chrono::Utc::now() - start).num_seconds().max(0) as u32;
            elapsed / 60
        })
        .unwrap_or(0);

    let gui_patterns: Vec<String> = entries
        .iter()
        .flat_map(|e| e.gui_patterns.iter().cloned())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    Some(maekon_analysis::SegmentStats {
        duration_mins,
        regime_label: ts.current_regime_id.clone(),
        event_count: 0, // not tracked per-tick; segment summarizer computes on close
        context_switches: 0,
        dominant_category: entries
            .first()
            .map(|e| e.content_type.clone())
            .unwrap_or_default(),
        content_summary: entries,
        gui_patterns,
    })
}
