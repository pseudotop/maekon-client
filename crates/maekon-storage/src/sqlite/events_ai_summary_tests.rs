use super::*;
use crate::sqlite::web_storage_impl::suggestion_digest_storage::local_date_utc_window;
use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use maekon_core::models::ai_summary::{AiSummaryArtifact, AiSummaryProviderClass};
use maekon_core::models::tiered_memory::{SegmentSummary, TriggerReason};
use std::collections::HashMap;

fn segment_summary_ending_at(end_time: DateTime<Utc>) -> SegmentSummary {
    SegmentSummary {
        segment_id: "seg-test-1".to_string(),
        start_time: end_time - Duration::minutes(30),
        end_time,
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
    }
}

#[tokio::test]
async fn save_activity_segment_round_trips_enriches_and_is_idempotent() {
    let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
    let now = Utc
        .with_ymd_and_hms(2026, 6, 11, 0, 15, 0)
        .single()
        .expect("valid fixed fixture timestamp");
    let summary = segment_summary_ending_at(now);

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
    assert_eq!(enriched.len(), 1, "enrichment must preserve the segment");
    assert_eq!(
        enriched[0].llm_summary.as_deref(),
        Some("focused on auth module"),
        "update_segment_ai_summary must enrich the row that the producer inserted"
    );
    // #13417: date queries select the segment's start day, even across midnight.
    let date = summary
        .start_time
        .with_timezone(&chrono::Local)
        .date_naive()
        .to_string();
    let summary_records = storage
        .get_segments_for_date(&date)
        .expect("summary record re-read failed");
    assert_eq!(
        summary_records.len(),
        1,
        "the start day must contain the row"
    );
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
        summary_records_after.len(),
        1,
        "an idempotent replay must preserve start-day membership"
    );
    assert_eq!(
        summary_records_after[0].ai_summary, artifact,
        "an idempotent segment replay must preserve AI provenance"
    );
}

#[tokio::test]
async fn activity_segment_date_queries_use_start_day_at_midnight_boundaries() {
    for offset_seconds in [0, 9 * 3600, -10 * 3600] {
        let timezone = FixedOffset::east_opt(offset_seconds).expect("valid fixed offset");
        for (hour, minute, second, crosses_midnight) in [
            (0, 0, 0, true),
            (0, 15, 0, true),
            (0, 29, 59, true),
            (0, 30, 0, false),
            (23, 59, 59, false),
        ] {
            let end_time = timezone
                .with_ymd_and_hms(2026, 6, 11, hour, minute, second)
                .single()
                .expect("valid local fixture timestamp");
            let summary = segment_summary_ending_at(end_time.with_timezone(&Utc));
            let storage = SqliteStorage::open_in_memory(30).expect("open_in_memory failed");
            storage
                .save_activity_segment(&summary)
                .await
                .expect("save boundary segment");

            // Exercise the production window and SQL without changing process TZ.
            let query_date = |date: chrono::NaiveDate| {
                let (from, to) = local_date_utc_window(&date.to_string(), &timezone)
                    .expect("valid local date window");
                let conn = storage.conn.test_lock();
                SqliteStorage::get_segments_in_utc_window_inner(&conn, &from, &to)
                    .expect("query boundary segment")
            };
            let start_date = summary.start_time.with_timezone(&timezone).date_naive();
            let start_rows = query_date(start_date);
            assert_eq!(start_rows.len(), 1, "start-day query for end={end_time}");
            assert_eq!(start_rows[0].segment_id, summary.segment_id);
            assert_eq!(
                query_date(end_time.date_naive()).len(),
                usize::from(!crosses_midnight),
                "a cross-midnight segment must not belong to its end day: end={end_time}"
            );

            // Ending at 00:30 puts the start exactly at the included lower bound,
            // which is also the previous day's excluded upper bound.
            assert!(
                query_date(start_date.pred_opt().expect("previous date")).is_empty(),
                "previous-day query must exclude start={}",
                summary.start_time
            );
            assert!(
                query_date(start_date.succ_opt().expect("next date")).is_empty(),
                "next-day query must exclude start={}",
                summary.start_time
            );
        }
    }
}
