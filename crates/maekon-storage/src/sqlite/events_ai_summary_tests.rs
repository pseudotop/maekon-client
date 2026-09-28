use super::*;
use chrono::{Duration, Utc};
use maekon_core::models::ai_summary::{AiSummaryArtifact, AiSummaryProviderClass};
use maekon_core::models::tiered_memory::{SegmentSummary, TriggerReason};
use std::collections::HashMap;

#[tokio::test]
async fn save_activity_segment_round_trips_enriches_and_is_idempotent() {
    let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
    let now = Utc::now();
    let summary = SegmentSummary {
        segment_id: "seg-test-1".to_string(),
        start_time: now - Duration::minutes(30),
        end_time: now,
        duration_secs: 1800,
        // #9735: the FK (regime_id -> regimes.id) IS enforced, so a NULL
        // regime_id is the only value that is unconditionally safe. A
        // dangling one is degraded to NULL by the writer.
        regime_id: None,
        trigger_reason: TriggerReason::ScoreLow,
        event_count: 42,
        app_breakdown: HashMap::from([("VSCode".to_string(), 1200u64)]),
        category_breakdown: HashMap::from([("coding".to_string(), 1200u64)]),
        context_switch_count: 3,
        dominant_category: "coding".to_string(),
        avg_importance: 0.7,
        patterns_detected: vec![],
        content_activities: vec![],
        container: None,
        llm_summary: None,
    };

    storage
        .save_activity_segment(&summary)
        .await
        .expect("save_activity_segment must succeed with a None regime_id");

    let from = now - Duration::hours(1);
    let to = now + Duration::hours(1);
    let rows = storage
        .list_segments_between(from, to)
        .expect("list_segments_between failed");
    assert_eq!(rows.len(), 1, "the locally-saved segment must be readable");
    let row = &rows[0];
    assert_eq!(row.segment_id, "seg-test-1");
    assert_eq!(row.duration_secs, 1800);
    assert_eq!(row.dominant_category, "coding");
    assert_eq!(row.event_count, 42);
    assert_eq!(row.trigger_reason, TriggerReason::ScoreLow);
    assert_eq!(row.app_breakdown.get("VSCode"), Some(&1200u64));

    let artifact = AiSummaryArtifact::generated(
        "focused on auth module".to_string(),
        AiSummaryProviderClass::ExternalApi,
        now,
    );
    storage
        .update_segment_ai_summary("seg-test-1", &artifact)
        .await
        .expect("update_segment_ai_summary failed");
    let enriched = storage
        .list_segments_between(from, to)
        .expect("re-read failed");
    assert_eq!(
        enriched[0].llm_summary.as_deref(),
        Some("focused on auth module"),
        "update_segment_ai_summary must enrich the row that the producer inserted"
    );
    let date = now.with_timezone(&chrono::Local).date_naive().to_string();
    let summary_records = storage
        .get_segments_for_date(&date)
        .expect("summary record re-read failed");
    assert_eq!(summary_records[0].ai_summary, artifact);

    storage
        .save_activity_segment(&summary)
        .await
        .expect("idempotent re-save");
    let after = storage
        .list_segments_between(from, to)
        .expect("re-read failed");
    assert_eq!(
        after.len(),
        1,
        "INSERT OR IGNORE must not duplicate on the id PK"
    );
    assert_eq!(
        after[0].llm_summary.as_deref(),
        Some("focused on auth module"),
        "OR IGNORE must keep the existing enriched row"
    );
    let summary_records_after = storage
        .get_segments_for_date(&date)
        .expect("summary record re-read failed");
    assert_eq!(
        summary_records_after[0].ai_summary, artifact,
        "an idempotent segment replay must preserve AI provenance"
    );
}
