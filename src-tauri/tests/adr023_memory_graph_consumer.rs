//! ADR-023 Phase-1 consumer slice (#4441 boxes 3-4) end-to-end.
//!
//! Drives the **real offline aggregation path** — `SegmentSummary` →
//! `DailyDigestGenerator::generate` (NO LLM, so `insight` is `None`) →
//! `claim_promoter` → `SqliteStorage` (the live `MemoryGraphPort` adapter) — and
//! asserts durable claims + evidence edges (AC3) that accumulate idempotently
//! across regeneration (AC4).
//!
//! This intentionally does NOT hand-build a `DailyInsight`: the point is that the
//! LLM-free production path yields a non-empty graph on its own.

use std::collections::HashMap;

use chrono::{Duration, NaiveDate, Utc};

use maekon_analysis::claim_promoter::build_claims_from_digest;
use maekon_analysis::DailyDigestGenerator;
use maekon_core::models::daily_digest::DailyDigest;
use maekon_core::models::memory_graph::{ClaimStatus, EdgeType, MemoryClaim, MemoryEdge};
use maekon_core::models::tiered_memory::{SegmentSummary, TriggerReason};
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_storage::sqlite::SqliteStorage;

const NOW: i64 = 1_700_000_000;

fn date(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A closed segment as produced offline (no LLM summary, no patterns).
fn offline_segment(id: &str) -> SegmentSummary {
    let now = Utc::now();
    SegmentSummary {
        segment_id: id.to_string(),
        start_time: now - Duration::minutes(90),
        end_time: now,
        duration_secs: 90 * 60,
        regime_id: Some("Deep Focus".to_string()),
        trigger_reason: TriggerReason::RegimeChange,
        event_count: 10,
        app_breakdown: HashMap::from([("VS Code".to_string(), 90 * 60)]),
        category_breakdown: HashMap::new(),
        context_switch_count: 2,
        dominant_category: "Development".to_string(),
        avg_importance: 0.6,
        patterns_detected: vec![],
        content_activities: vec![],
        container: None,
        llm_summary: None,
    }
}

fn offline_digest(segment_ids: &[&str], on: NaiveDate) -> DailyDigest {
    let segments: Vec<SegmentSummary> = segment_ids.iter().map(|s| offline_segment(s)).collect();
    // #7678 D2: no regime manager in this offline-path test — the generator
    // falls back to `dominant_category` ("Development") for the timeline label.
    DailyDigestGenerator::generate(&segments, on, None, &[])
}

async fn persist_pairs(storage: &SqliteStorage, pairs: &[(MemoryClaim, Vec<MemoryEdge>)]) {
    for (claim, edges) in pairs {
        storage.save_claim(claim).await.unwrap();
        for edge in edges {
            storage.add_edge(edge).await.unwrap();
        }
    }
}

async fn active_count(storage: &SqliteStorage) -> usize {
    storage
        .list_claims_by_status(ClaimStatus::Active)
        .await
        .unwrap()
        .len()
}

#[tokio::test]
async fn offline_digest_promotes_claims_and_evidence_edges() {
    // REAL offline path: segments → digest (no LLM → insight None, timeline present).
    let digest = offline_digest(&["seg-1", "seg-2"], date(2026, 5, 30));
    assert!(
        digest.insight.is_none(),
        "offline path produces no LLM insight"
    );
    assert!(
        !digest.timeline.is_empty(),
        "offline digest still has a timeline to promote"
    );

    let storage = SqliteStorage::open_in_memory(30).unwrap();
    let pairs = build_claims_from_digest(&digest, NOW);
    assert!(!pairs.is_empty(), "AC3: ≥1 claim promoted offline");
    persist_pairs(&storage, &pairs).await;

    // AC3: ≥1 active claim, with an Evidence edge to a real segment_id.
    let active = storage
        .list_claims_by_status(ClaimStatus::Active)
        .await
        .unwrap();
    assert_eq!(active.len(), pairs.len());

    let timeline_claim = active
        .iter()
        .find(|c| c.source == "digest_timeline")
        .expect("a timeline-sourced claim exists offline");
    assert_eq!(timeline_claim.kind.as_str(), "episodic");

    let edges = storage
        .edges_from(&timeline_claim.claim_id, Some(EdgeType::Evidence))
        .await
        .unwrap();
    assert_eq!(
        edges.len(),
        1,
        "AC3: each timeline claim has an evidence edge"
    );
    assert!(
        edges[0].dst_id.starts_with("seg-"),
        "evidence edge dst is the source segment_id"
    );
    assert_eq!(edges[0].source, "rule");

    // box 5: consecutive timeline claims are linked by a rule-seeded Associated
    // (app-sequence) edge — persisted end-to-end (2 entries → exactly 1 edge).
    let mut associated = 0usize;
    for c in &active {
        associated += storage
            .edges_from(&c.claim_id, Some(EdgeType::Associated))
            .await
            .unwrap()
            .len();
    }
    assert_eq!(
        associated, 1,
        "box 5: one app-sequence edge between the two timeline claims"
    );
}

#[tokio::test]
async fn claims_accumulate_and_resave_is_idempotent() {
    let storage = SqliteStorage::open_in_memory(30).unwrap();

    // Day 1.
    let day1 = build_claims_from_digest(&offline_digest(&["seg-d1"], date(2026, 5, 29)), NOW);
    persist_pairs(&storage, &day1).await;
    let after_day1 = active_count(&storage).await;
    assert!(after_day1 >= 1, "day-1 claims persisted");

    // AC4 (primitive): re-saving the SAME ids is an idempotent upsert — no dup rows.
    // NOTE: the production aggregation loop additionally guards regeneration with a
    // once-per-day `existing.is_none()` check, so the same day never re-promotes with
    // fresh ids; this asserts the substrate-level idempotency the guard relies on.
    persist_pairs(&storage, &day1).await;
    assert_eq!(
        active_count(&storage).await,
        after_day1,
        "AC4: idempotent re-save does not duplicate claims"
    );

    // Durability: a later day accumulates without erasing day 1.
    let day2 = build_claims_from_digest(&offline_digest(&["seg-d2"], date(2026, 5, 30)), NOW);
    persist_pairs(&storage, &day2).await;
    assert!(
        active_count(&storage).await > after_day1,
        "day-2 claims accumulate; day-1 claims retained"
    );
}
