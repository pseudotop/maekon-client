//! ADR-023 Phase-1 consumer slice (D3 + D5): promote a `DailyDigest` into durable
//! `MemoryClaim` nodes + `Evidence` edges, fully **offline** (no LLM required).
//!
//! This module is pure value-construction — it holds no storage handle and does
//! no I/O, keeping `maekon-analysis` network-free and storage-free (ADR-023 AC7).
//! The caller (scheduler aggregation loop) persists the returned values through
//! the `MemoryGraphPort`.
//!
//! ## Sources
//! - **Timeline entries** (always present offline — built by
//!   `DailyDigestGenerator::generate` from closed segments): each becomes an
//!   `Episodic` claim + an `Evidence` edge to its `segment_id`. This is what
//!   makes the offline / NoOp-LLM path satisfy AC3 (≥1 claim + ≥1 evidence edge).
//! - **LLM insight highlights** (only present when a summarizer ran): each
//!   becomes a claim whose `ClaimKind` is the D5 rule mapping of its
//!   `HighlightType`, plus an `Evidence` edge when the highlight carries a
//!   `segment_id`.
//! - **App-sequence edges** (#4441 box 5): consecutive timeline claims are linked
//!   by a rule-seeded `Associated` edge (`src` = earlier → `dst` = later). This
//!   uses the production-available timeline ORDER rather than
//!   `SegmentSummary.patterns_detected` (which the scheduler path leaves empty);
//!   app-node-identity co-occurrence is out of scope (no app nodes in the schema).

use maekon_core::models::daily_digest::{DailyDigest, HighlightType};
use maekon_core::models::memory_graph::{
    ClaimKind, ClaimStatus, EdgeType, MemoryClaim, MemoryEdge,
};
use sha2::{Digest, Sha256};
use std::fmt::Write;

/// Confidence assigned to rule-derived (non-LLM) claims and edges. These are
/// deterministic promotions of already-stored digest content, so they carry
/// full confidence (the digest itself is the evidence).
const RULE_CONFIDENCE: f32 = 1.0;

/// D5 deterministic taxonomy rule for an LLM highlight (total over
/// `HighlightType`, reproducible, no LLM).
///
/// - `Achievement` → `Episodic` (a concrete, dated accomplishment)
/// - `Warning` → `Reflective` (a self-observation about behavior)
/// - `Suggestion` → `Procedural` (an actionable next step / how-to)
#[must_use]
pub fn kind_for_highlight(highlight_type: &HighlightType) -> ClaimKind {
    match highlight_type {
        HighlightType::Achievement => ClaimKind::Episodic,
        HighlightType::Warning => ClaimKind::Reflective,
        HighlightType::Suggestion => ClaimKind::Procedural,
    }
}

/// Build `(claim, evidence_edges)` pairs from a digest, fully offline.
///
/// `now_secs` is the epoch-second timestamp stamped onto every produced claim
/// and edge (matches the ADR-023 INTEGER DDL).
#[must_use]
pub fn build_claims_from_digest(
    digest: &DailyDigest,
    now_secs: i64,
) -> Vec<(MemoryClaim, Vec<MemoryEdge>)> {
    let mut out: Vec<(MemoryClaim, Vec<MemoryEdge>)> = Vec::new();
    let date_key = digest.date.to_string();

    // Offline source: timeline entries (no LLM). Each carries a guaranteed
    // `segment_id`, so every timeline claim gets an evidence edge; consecutive
    // timeline claims are linked by a rule-seeded `Associated` (app-sequence)
    // edge (#4441 box 5) — the production-available temporal signal (the
    // scheduler path leaves `SegmentSummary.patterns_detected` empty).
    let mut prev_claim_id: Option<String> = None;
    for entry in &digest.timeline {
        let claim_id = stable_id("clm", &["timeline", &date_key, &entry.segment_id]);
        let claim = MemoryClaim {
            claim_id,
            kind: ClaimKind::Episodic,
            text: format!(
                "{}min in {} ({})",
                entry.duration_mins, entry.regime_label, entry.dominant_app
            ),
            source: "digest_timeline".to_string(),
            confidence: RULE_CONFIDENCE,
            status: ClaimStatus::Active,
            created_at: now_secs,
            updated_at: now_secs,
        };
        let mut edges = vec![evidence_edge(&claim.claim_id, &entry.segment_id, now_secs)];
        // box 5: temporal app-sequence edge from the previous timeline claim
        // (src = earlier → dst = later). Bounded at N-1 edges per digest.
        if let Some(ref prev) = prev_claim_id {
            edges.push(sequence_edge(prev, &claim.claim_id, now_secs));
        }
        prev_claim_id = Some(claim.claim_id.clone());
        out.push((claim, edges));
    }

    // LLM-on source: insight highlights (richer, narrative-derived claims).
    if let Some(ref insight) = digest.insight {
        for (idx, highlight) in insight.highlights.iter().enumerate() {
            let index_key = idx.to_string();
            let segment_key = highlight.segment_id.as_deref().unwrap_or("");
            let claim_id = stable_id(
                "clm",
                &[
                    "highlight",
                    &date_key,
                    &index_key,
                    highlight_type_key(&highlight.highlight_type),
                    &highlight.text,
                    segment_key,
                ],
            );
            let claim = MemoryClaim {
                claim_id,
                kind: kind_for_highlight(&highlight.highlight_type),
                text: highlight.text.clone(), // already PII-filtered by DailyInsightGenerator
                source: "digest_highlight".to_string(),
                confidence: RULE_CONFIDENCE,
                status: ClaimStatus::Active,
                created_at: now_secs,
                updated_at: now_secs,
            };
            // A highlight's `segment_id` is optional; emit the evidence edge only
            // when it is present (suggestion-type highlights typically have none).
            let edges = highlight
                .segment_id
                .as_ref()
                .map(|seg| vec![evidence_edge(&claim.claim_id, seg, now_secs)])
                .unwrap_or_default();
            out.push((claim, edges));
        }
    }

    out
}

fn highlight_type_key(highlight_type: &HighlightType) -> &'static str {
    match highlight_type {
        HighlightType::Achievement => "achievement",
        HighlightType::Warning => "warning",
        HighlightType::Suggestion => "suggestion",
    }
}

fn stable_id(prefix: &str, parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"maekon.claim_promoter.v1");
    for part in parts {
        hasher.update([0x1f]);
        hasher.update(part.as_bytes());
    }

    let digest = hasher.finalize();
    let mut suffix = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        let _ = write!(&mut suffix, "{byte:02x}");
    }
    format!("{prefix}_{suffix}")
}

/// Construct an `Evidence` edge from a claim to its supporting `segment_id`.
fn evidence_edge(claim_id: &str, segment_id: &str, now_secs: i64) -> MemoryEdge {
    MemoryEdge {
        edge_id: stable_id("edg", &["evidence", claim_id, segment_id]),
        src_id: claim_id.to_string(),
        dst_id: segment_id.to_string(),
        edge_type: EdgeType::Evidence,
        confidence: RULE_CONFIDENCE,
        evidence_ref: Some(segment_id.to_string()),
        source: "rule".to_string(),
        created_at: now_secs,
    }
}

/// Construct a rule-seeded `Associated` (app-sequence) edge between two
/// consecutive timeline claims — `src` = earlier, `dst` = later (#4441 box 5).
fn sequence_edge(src_claim_id: &str, dst_claim_id: &str, now_secs: i64) -> MemoryEdge {
    MemoryEdge {
        edge_id: stable_id("edg", &["sequence", src_claim_id, dst_claim_id]),
        src_id: src_claim_id.to_string(),
        dst_id: dst_claim_id.to_string(),
        edge_type: EdgeType::Associated,
        confidence: RULE_CONFIDENCE,
        evidence_ref: None,
        source: "rule".to_string(),
        created_at: now_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use maekon_core::models::daily_digest::{
        DailyInsight, DailyStatistics, DigestHighlight, TimelineEntry,
    };

    const NOW: i64 = 1_700_000_000;

    fn timeline_entry(seg: &str) -> TimelineEntry {
        TimelineEntry {
            segment_id: seg.to_string(),
            start_time: Utc::now(),
            end_time: Utc::now(),
            duration_mins: 90,
            regime_label: "Deep Focus".to_string(),
            regime_color: "#3B82F6".to_string(),
            dominant_app: "VS Code".to_string(),
            content_summary: vec![],
            annotation: None,
        }
    }

    fn digest_with_timeline(segs: &[&str]) -> DailyDigest {
        DailyDigest {
            date: chrono::NaiveDate::from_ymd_opt(2026, 5, 30).unwrap(),
            insight: None,
            timeline: segs.iter().map(|s| timeline_entry(s)).collect(),
            statistics: DailyStatistics::default(),
            generated_at: Utc::now(),
        }
    }

    #[test]
    fn kind_rule_is_total_and_deterministic() {
        assert_eq!(
            kind_for_highlight(&HighlightType::Achievement),
            ClaimKind::Episodic
        );
        assert_eq!(
            kind_for_highlight(&HighlightType::Warning),
            ClaimKind::Reflective
        );
        assert_eq!(
            kind_for_highlight(&HighlightType::Suggestion),
            ClaimKind::Procedural
        );
        // Deterministic: same input → same kind.
        assert_eq!(
            kind_for_highlight(&HighlightType::Warning),
            kind_for_highlight(&HighlightType::Warning)
        );
    }

    #[test]
    fn offline_timeline_yields_episodic_claim_and_evidence_edge() {
        let digest = digest_with_timeline(&["seg-1", "seg-2"]);
        let pairs = build_claims_from_digest(&digest, NOW);

        assert_eq!(pairs.len(), 2, "one claim per timeline entry");
        let (claim, edges) = &pairs[0];
        assert_eq!(claim.kind, ClaimKind::Episodic);
        assert_eq!(claim.source, "digest_timeline");
        assert_eq!(claim.status, ClaimStatus::Active);
        assert_eq!(claim.created_at, NOW);
        assert!(claim.claim_id.starts_with("clm_"));
        assert!(claim.text.contains("Deep Focus"));

        assert_eq!(edges.len(), 1, "every timeline claim has an evidence edge");
        let edge = &edges[0];
        assert_eq!(edge.edge_type, EdgeType::Evidence);
        assert_eq!(edge.src_id, claim.claim_id);
        assert_eq!(edge.dst_id, "seg-1");
        assert_eq!(edge.evidence_ref.as_deref(), Some("seg-1"));
        assert_eq!(edge.source, "rule");
        assert!(edge.edge_id.starts_with("edg_"));
    }

    #[test]
    fn insight_highlights_are_promoted_with_kind_and_conditional_edge() {
        let mut digest = digest_with_timeline(&["seg-1"]);
        digest.insight = Some(DailyInsight {
            narrative: "Solid day".to_string(),
            highlights: vec![
                DigestHighlight {
                    highlight_type: HighlightType::Achievement,
                    text: "2h deep work block".to_string(),
                    segment_id: Some("seg-1".to_string()),
                },
                DigestHighlight {
                    highlight_type: HighlightType::Suggestion,
                    text: "Batch your email".to_string(),
                    segment_id: None,
                },
            ],
        });

        let pairs = build_claims_from_digest(&digest, NOW);
        // 1 timeline claim + 2 highlight claims.
        assert_eq!(pairs.len(), 3);

        let achievement = pairs
            .iter()
            .find(|(c, _)| c.text == "2h deep work block")
            .expect("achievement claim present");
        assert_eq!(achievement.0.kind, ClaimKind::Episodic);
        assert_eq!(achievement.0.source, "digest_highlight");
        assert_eq!(
            achievement.1.len(),
            1,
            "highlight with segment_id has an edge"
        );
        assert_eq!(achievement.1[0].dst_id, "seg-1");

        let suggestion = pairs
            .iter()
            .find(|(c, _)| c.text == "Batch your email")
            .expect("suggestion claim present");
        assert_eq!(suggestion.0.kind, ClaimKind::Procedural);
        assert!(
            suggestion.1.is_empty(),
            "highlight without segment_id emits no evidence edge"
        );
    }

    #[test]
    fn empty_digest_yields_no_claims() {
        let digest = digest_with_timeline(&[]);
        assert!(build_claims_from_digest(&digest, NOW).is_empty());
    }

    #[test]
    fn build_claims_from_digest_is_deterministic_for_same_digest() {
        let digest = digest_with_timeline(&["seg-1", "seg-2"]);

        let first = build_claims_from_digest(&digest, NOW);
        let second = build_claims_from_digest(&digest, NOW);

        let first_claim_ids: Vec<_> = first.iter().map(|(claim, _)| &claim.claim_id).collect();
        let second_claim_ids: Vec<_> = second.iter().map(|(claim, _)| &claim.claim_id).collect();
        assert_eq!(
            first_claim_ids, second_claim_ids,
            "re-promoting the same digest must upsert the same claims"
        );

        let first_edge_ids: Vec<_> = first
            .iter()
            .flat_map(|(_, edges)| edges.iter().map(|edge| &edge.edge_id))
            .collect();
        let second_edge_ids: Vec<_> = second
            .iter()
            .flat_map(|(_, edges)| edges.iter().map(|edge| &edge.edge_id))
            .collect();
        assert_eq!(
            first_edge_ids, second_edge_ids,
            "re-promoting the same digest must upsert the same evidence edges"
        );
    }

    #[test]
    fn consecutive_timeline_claims_get_rule_seeded_sequence_edge() {
        // #4441 box 5: N timeline entries → N-1 Associated (app-sequence) edges.
        let digest = digest_with_timeline(&["seg-1", "seg-2", "seg-3"]);
        let pairs = build_claims_from_digest(&digest, NOW);
        assert_eq!(pairs.len(), 3);

        // First claim: evidence edge only (no predecessor).
        assert!(pairs[0].1.iter().all(|e| e.edge_type == EdgeType::Evidence));

        // Second claim: evidence + a sequence edge from the first claim.
        let seq = pairs[1]
            .1
            .iter()
            .find(|e| e.edge_type == EdgeType::Associated)
            .expect("second claim has an Associated sequence edge");
        assert_eq!(seq.src_id, pairs[0].0.claim_id, "src = earlier claim");
        assert_eq!(seq.dst_id, pairs[1].0.claim_id, "dst = later claim");
        assert_eq!(seq.source, "rule");
        assert!(seq.evidence_ref.is_none());
        assert!(seq.edge_id.starts_with("edg_"));

        // Exactly N-1 = 2 sequence edges across the digest.
        let total_seq: usize = pairs
            .iter()
            .flat_map(|(_, edges)| edges)
            .filter(|e| e.edge_type == EdgeType::Associated)
            .count();
        assert_eq!(total_seq, 2);
    }

    #[test]
    fn single_timeline_claim_has_no_sequence_edge() {
        let pairs = build_claims_from_digest(&digest_with_timeline(&["seg-1"]), NOW);
        assert_eq!(pairs.len(), 1);
        assert!(pairs[0].1.iter().all(|e| e.edge_type == EdgeType::Evidence));
    }
}
