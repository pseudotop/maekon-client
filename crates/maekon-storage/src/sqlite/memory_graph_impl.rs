//! `MemoryGraphPort` implementation over SQLite (ADR-023 substrate).
//!
//! Async port; all blocking SQLite work is offloaded via `with_conn`
//! (spawn_blocking). Pure local persistence, no network dependency.

use async_trait::async_trait;
use maekon_core::error::CoreError;
use maekon_core::models::memory_graph::{
    ClaimKind, ClaimStatus, EdgeType, MemoryClaim, MemoryEdge,
};
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use rusqlite::params;
use tracing::debug;

use super::SqliteStorage;
use crate::error::StorageError;

fn map_claim_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryClaim> {
    let kind: String = row.get(1)?;
    let status: String = row.get(5)?;
    let confidence: f64 = row.get(4)?;
    Ok(MemoryClaim {
        claim_id: row.get(0)?,
        kind: ClaimKind::from_str_lossy(&kind),
        text: row.get(2)?,
        source: row.get(3)?,
        confidence: confidence as f32,
        status: ClaimStatus::from_str_lossy(&status),
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn map_edge_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryEdge> {
    let edge_type: String = row.get(3)?;
    let confidence: f64 = row.get(4)?;
    Ok(MemoryEdge {
        edge_id: row.get(0)?,
        src_id: row.get(1)?,
        dst_id: row.get(2)?,
        edge_type: EdgeType::from_str_lossy(&edge_type),
        confidence: confidence as f32,
        evidence_ref: row.get(5)?,
        source: row.get(6)?,
        created_at: row.get(7)?,
    })
}

const CLAIM_COLS: &str = "claim_id, kind, text, source, confidence, status, created_at, updated_at";
const EDGE_COLS: &str =
    "edge_id, src_id, dst_id, edge_type, confidence, evidence_ref, source, created_at";

#[async_trait]
impl MemoryGraphPort for SqliteStorage {
    async fn save_claim(&self, claim: &MemoryClaim) -> Result<(), CoreError> {
        let claim_id = claim.claim_id.clone();
        let kind = claim.kind.as_str().to_string();
        let text = claim.text.clone();
        let source = claim.source.clone();
        let confidence = claim.confidence as f64;
        let status = claim.status.as_str().to_string();
        let created_at = claim.created_at;
        let updated_at = claim.updated_at;

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO memory_claims
                 (claim_id, kind, text, source, confidence, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![claim_id, kind, text, source, confidence, status, created_at, updated_at],
            )
            .map_err(|e| StorageError::Internal(format!("save_claim: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn get_claim(&self, claim_id: &str) -> Result<Option<MemoryClaim>, CoreError> {
        let claim_id = claim_id.to_string();
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {CLAIM_COLS} FROM memory_claims WHERE claim_id = ?1"
                ))
                .map_err(|e| StorageError::Internal(format!("prepare get_claim: {e}")))?;
            let mut rows = stmt
                .query_map(params![claim_id], map_claim_row)
                .map_err(|e| StorageError::Internal(format!("query get_claim: {e}")))?;
            match rows.next() {
                Some(r) => Ok(Some(r.map_err(|e| {
                    StorageError::Internal(format!("read claim row: {e}"))
                })?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(Into::into)
    }

    async fn list_claims_by_status(
        &self,
        status: ClaimStatus,
    ) -> Result<Vec<MemoryClaim>, CoreError> {
        let status = status.as_str().to_string();
        self.with_conn_read(move |conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {CLAIM_COLS} FROM memory_claims WHERE status = ?1 \
                     ORDER BY updated_at DESC"
                ))
                .map_err(|e| StorageError::Internal(format!("prepare list_claims: {e}")))?;
            let rows = stmt
                .query_map(params![status], map_claim_row)
                .map_err(|e| StorageError::Internal(format!("query list_claims: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| StorageError::Internal(format!("read claim row: {e}")))?);
            }
            Ok(out)
        })
        .await
        .map_err(Into::into)
    }

    async fn set_claim_status(
        &self,
        claim_id: &str,
        status: ClaimStatus,
        updated_at: i64,
    ) -> Result<(), CoreError> {
        let claim_id = claim_id.to_string();
        let status = status.as_str().to_string();
        self.with_conn(move |conn| {
            conn.execute(
                "UPDATE memory_claims SET status = ?1, updated_at = ?2 WHERE claim_id = ?3",
                params![status, updated_at, claim_id],
            )
            .map_err(|e| StorageError::Internal(format!("set_claim_status: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn add_edge(&self, edge: &MemoryEdge) -> Result<(), CoreError> {
        let edge_id = edge.edge_id.clone();
        let src_id = edge.src_id.clone();
        let dst_id = edge.dst_id.clone();
        let edge_type = edge.edge_type.as_str().to_string();
        let confidence = edge.confidence as f64;
        let evidence_ref = edge.evidence_ref.clone();
        let source = edge.source.clone();
        let created_at = edge.created_at;

        self.with_conn(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO memory_edges
                 (edge_id, src_id, dst_id, edge_type, confidence, evidence_ref, source, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    edge_id,
                    src_id,
                    dst_id,
                    edge_type,
                    confidence,
                    evidence_ref,
                    source,
                    created_at
                ],
            )
            .map_err(|e| StorageError::Internal(format!("add_edge: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }

    async fn edges_from(
        &self,
        src_id: &str,
        edge_type: Option<EdgeType>,
    ) -> Result<Vec<MemoryEdge>, CoreError> {
        let src_id = src_id.to_string();
        let type_filter = edge_type.map(|e| e.as_str().to_string());
        self.with_conn_read(move |conn| {
            let mut out = Vec::new();
            match type_filter {
                Some(t) => {
                    let mut stmt = conn
                        .prepare(&format!(
                            "SELECT {EDGE_COLS} FROM memory_edges \
                             WHERE src_id = ?1 AND edge_type = ?2 ORDER BY created_at ASC"
                        ))
                        .map_err(|e| StorageError::Internal(format!("prepare edges_from: {e}")))?;
                    let rows = stmt
                        .query_map(params![src_id, t], map_edge_row)
                        .map_err(|e| StorageError::Internal(format!("query edges_from: {e}")))?;
                    for r in rows {
                        out.push(
                            r.map_err(|e| StorageError::Internal(format!("read edge row: {e}")))?,
                        );
                    }
                }
                None => {
                    let mut stmt = conn
                        .prepare(&format!(
                            "SELECT {EDGE_COLS} FROM memory_edges \
                             WHERE src_id = ?1 ORDER BY created_at ASC"
                        ))
                        .map_err(|e| StorageError::Internal(format!("prepare edges_from: {e}")))?;
                    let rows = stmt
                        .query_map(params![src_id], map_edge_row)
                        .map_err(|e| StorageError::Internal(format!("query edges_from: {e}")))?;
                    for r in rows {
                        out.push(
                            r.map_err(|e| StorageError::Internal(format!("read edge row: {e}")))?,
                        );
                    }
                }
            }
            debug!("edges_from {src_id}: {} edges", out.len());
            Ok(out)
        })
        .await
        .map_err(Into::into)
    }

    async fn prune_claims_older_than(&self, cutoff_epoch_secs: i64) -> Result<u64, CoreError> {
        // Single transaction so the edge cascade and the claim delete commit
        // together: a mid-way failure (e.g. the second DELETE) must not leave
        // edges already gone while their claims survive, nor vice-versa. Mirrors
        // `supersede_claim`'s tx pattern; routed through `with_conn_mut` since
        // `Connection::transaction()` needs `&mut Connection`.
        self.with_conn_mut(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| StorageError::Internal(format!("prune tx begin: {e}")))?;
            // Child before parent: drop edges that touch a to-be-pruned claim on
            // EITHER end first. Matching only `src_id` would orphan epistemic
            // edges (supports/refines/contradicts/supersedes) that point TO a
            // pruned claim via `dst_id` — `prune_orphan_evidence_edges` only GCs
            // `evidence`-typed orphans, so those would dangle forever. Evidence
            // edges are unaffected: their `dst_id` is an `activity_segments.id`,
            // never a `claim_id`, so it can't match a pruned claim.
            // `created_at` is epoch seconds per the v34 DDL (not RFC3339 text).
            tx.execute(
                "DELETE FROM memory_edges WHERE src_id IN \
                 (SELECT claim_id FROM memory_claims WHERE created_at < ?1) \
                 OR dst_id IN \
                 (SELECT claim_id FROM memory_claims WHERE created_at < ?1)",
                params![cutoff_epoch_secs],
            )
            .map_err(|e| StorageError::Internal(format!("prune memory_edges: {e}")))?;
            let deleted = tx
                .execute(
                    "DELETE FROM memory_claims WHERE created_at < ?1",
                    params![cutoff_epoch_secs],
                )
                .map_err(|e| StorageError::Internal(format!("prune memory_claims: {e}")))?;
            tx.commit()
                .map_err(|e| StorageError::Internal(format!("prune commit: {e}")))?;
            debug!("prune_claims_older_than {cutoff_epoch_secs}: {deleted} claims");
            Ok(deleted as u64)
        })
        .await
        .map_err(Into::into)
    }

    async fn prune_orphan_evidence_edges(&self) -> Result<u64, CoreError> {
        self.with_conn(move |conn| {
            // Evidence edges target an `activity_segments.id`; once that segment is
            // retention-deleted the edge is dangling provenance — GC it. Only
            // `evidence`-typed edges reference segments (epistemic edges link claims).
            let deleted = conn
                .execute(
                    "DELETE FROM memory_edges \
                     WHERE edge_type = 'evidence' \
                     AND dst_id NOT IN (SELECT id FROM activity_segments)",
                    [],
                )
                .map_err(|e| StorageError::Internal(format!("prune orphan edges: {e}")))?;
            debug!("prune_orphan_evidence_edges: {deleted} edges");
            Ok(deleted as u64)
        })
        .await
        .map_err(Into::into)
    }

    async fn supersede_claim(
        &self,
        loser_claim_id: &str,
        supersedes_edge: &MemoryEdge,
        updated_at: i64,
    ) -> Result<(), CoreError> {
        let loser = loser_claim_id.to_string();
        let edge_id = supersedes_edge.edge_id.clone();
        let src_id = supersedes_edge.src_id.clone();
        let dst_id = supersedes_edge.dst_id.clone();
        let edge_type = supersedes_edge.edge_type.as_str().to_string();
        let confidence = supersedes_edge.confidence as f64;
        let evidence_ref = supersedes_edge.evidence_ref.clone();
        let source = supersedes_edge.source.clone();
        let created_at = supersedes_edge.created_at;

        // Single transaction so the status flip cannot land without provenance (F3).
        self.with_conn_mut(move |conn| {
            let tx = conn
                .transaction()
                .map_err(|e| StorageError::Internal(format!("supersede tx begin: {e}")))?;
            tx.execute(
                "UPDATE memory_claims SET status = 'superseded', updated_at = ?1 \
                 WHERE claim_id = ?2",
                params![updated_at, loser],
            )
            .map_err(|e| StorageError::Internal(format!("supersede status: {e}")))?;
            tx.execute(
                "INSERT OR IGNORE INTO memory_edges
                 (edge_id, src_id, dst_id, edge_type, confidence, evidence_ref, source, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    edge_id,
                    src_id,
                    dst_id,
                    edge_type,
                    confidence,
                    evidence_ref,
                    source,
                    created_at
                ],
            )
            .map_err(|e| StorageError::Internal(format!("supersede edge: {e}")))?;
            tx.commit()
                .map_err(|e| StorageError::Internal(format!("supersede commit: {e}")))?;
            Ok(())
        })
        .await
        .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(id: &str, status: ClaimStatus) -> MemoryClaim {
        MemoryClaim {
            claim_id: id.to_string(),
            kind: ClaimKind::Reflective,
            text: "deep-work blocks cluster in the morning".to_string(),
            source: "digest_highlight".to_string(),
            confidence: 0.8,
            status,
            created_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    #[tokio::test]
    async fn save_and_get_claim_roundtrip() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let c = claim("clm_001", ClaimStatus::Active);
        storage.save_claim(&c).await.unwrap();

        let got = storage.get_claim("clm_001").await.unwrap().unwrap();
        assert_eq!(got, c);
        assert_eq!(got.kind, ClaimKind::Reflective);
    }

    #[tokio::test]
    async fn get_missing_claim_returns_none() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        assert!(storage.get_claim("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn save_claim_is_idempotent_upsert() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        let mut c = claim("clm_002", ClaimStatus::Active);
        storage.save_claim(&c).await.unwrap();
        c.text = "updated".to_string();
        storage.save_claim(&c).await.unwrap();
        let got = storage.get_claim("clm_002").await.unwrap().unwrap();
        assert_eq!(got.text, "updated");
        // still exactly one row for the status
        let actives = storage
            .list_claims_by_status(ClaimStatus::Active)
            .await
            .unwrap();
        assert_eq!(actives.len(), 1);
    }

    #[tokio::test]
    async fn add_edge_and_edges_from_with_filter() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_claim(&claim("clm_003", ClaimStatus::Active))
            .await
            .unwrap();

        let evidence = MemoryEdge {
            edge_id: "edg_001".to_string(),
            src_id: "clm_003".to_string(),
            dst_id: "ses_042".to_string(),
            edge_type: EdgeType::Evidence,
            confidence: 1.0,
            evidence_ref: Some("ses_042".to_string()),
            source: "rule".to_string(),
            created_at: 1_700_000_000,
        };
        let supports = MemoryEdge {
            edge_id: "edg_002".to_string(),
            src_id: "clm_003".to_string(),
            dst_id: "clm_004".to_string(),
            edge_type: EdgeType::Supports,
            confidence: 0.7,
            evidence_ref: None,
            source: "llm".to_string(),
            created_at: 1_700_000_001,
        };
        storage.add_edge(&evidence).await.unwrap();
        storage.add_edge(&supports).await.unwrap();

        let all = storage.edges_from("clm_003", None).await.unwrap();
        assert_eq!(all.len(), 2);

        let only_evidence = storage
            .edges_from("clm_003", Some(EdgeType::Evidence))
            .await
            .unwrap();
        assert_eq!(only_evidence.len(), 1);
        assert_eq!(only_evidence[0], evidence);

        // idempotent add (same edge_id) does not duplicate
        storage.add_edge(&evidence).await.unwrap();
        assert_eq!(storage.edges_from("clm_003", None).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn add_edge_dedupes_same_triple_with_different_edge_id() {
        // v35 UNIQUE(src_id, dst_id, edge_type): a re-proposed relation carrying a
        // FRESH edge_id (exactly what belief revision generates each daily pass)
        // must NOT create a duplicate row — the INSERT OR IGNORE now collides on
        // the triple, not just the always-unique edge_id PK.
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_claim(&claim("clm_x", ClaimStatus::Active))
            .await
            .unwrap();

        let relation = |edge_id: &str, confidence: f32| MemoryEdge {
            edge_id: edge_id.to_string(),
            src_id: "clm_x".to_string(),
            dst_id: "clm_y".to_string(),
            edge_type: EdgeType::Supports,
            confidence,
            evidence_ref: None,
            source: "llm".to_string(),
            created_at: 1_700_000_000,
        };
        storage.add_edge(&relation("edg_first", 0.7)).await.unwrap();
        // Same (src, dst, type) triple, different edge_id + confidence.
        storage
            .add_edge(&relation("edg_second", 0.95))
            .await
            .unwrap();

        let supports = storage
            .edges_from("clm_x", Some(EdgeType::Supports))
            .await
            .unwrap();
        assert_eq!(supports.len(), 1, "the duplicate relation edge is ignored");
        // First write wins (INSERT OR IGNORE keeps the original row).
        assert_eq!(supports[0].edge_id, "edg_first");

        // A different edge_type between the SAME pair is a distinct triple → kept.
        let contradicts = MemoryEdge {
            edge_id: "edg_contra".to_string(),
            src_id: "clm_x".to_string(),
            dst_id: "clm_y".to_string(),
            edge_type: EdgeType::Contradicts,
            confidence: 0.6,
            evidence_ref: None,
            source: "llm".to_string(),
            created_at: 1_700_000_001,
        };
        storage.add_edge(&contradicts).await.unwrap();
        assert_eq!(storage.edges_from("clm_x", None).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn status_transition_and_list_by_status() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_claim(&claim("clm_005", ClaimStatus::Active))
            .await
            .unwrap();

        assert_eq!(
            storage
                .list_claims_by_status(ClaimStatus::Active)
                .await
                .unwrap()
                .len(),
            1
        );
        storage
            .set_claim_status("clm_005", ClaimStatus::Superseded, 1_700_000_500)
            .await
            .unwrap();

        assert!(storage
            .list_claims_by_status(ClaimStatus::Active)
            .await
            .unwrap()
            .is_empty());
        let superseded = storage
            .list_claims_by_status(ClaimStatus::Superseded)
            .await
            .unwrap();
        assert_eq!(superseded.len(), 1);
        assert_eq!(superseded[0].updated_at, 1_700_000_500);
    }

    fn claim_at(id: &str, created_at: i64) -> MemoryClaim {
        MemoryClaim {
            claim_id: id.to_string(),
            kind: ClaimKind::Episodic,
            text: "t".to_string(),
            source: "digest_timeline".to_string(),
            confidence: 1.0,
            status: ClaimStatus::Active,
            created_at,
            updated_at: created_at,
        }
    }

    fn evidence_edge(edge_id: &str, src: &str, dst: &str) -> MemoryEdge {
        MemoryEdge {
            edge_id: edge_id.to_string(),
            src_id: src.to_string(),
            dst_id: dst.to_string(),
            edge_type: EdgeType::Evidence,
            confidence: 1.0,
            evidence_ref: Some(dst.to_string()),
            source: "rule".to_string(),
            created_at: 1_700_000_000,
        }
    }

    fn insert_segment(storage: &SqliteStorage, id: &str) {
        let conn = storage.connection_arc();
        let guard = conn.test_lock();
        guard
            .execute(
                "INSERT INTO activity_segments \
                 (id, start_time, end_time, duration_secs, trigger_reason, dominant_category) \
                 VALUES (?1, '2026-01-01T00:00:00Z', '2026-01-01T01:00:00Z', 3600, \
                 'regime_change', 'Development')",
                params![id],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn prune_claims_older_than_drops_old_keeps_new_and_cascades_edges() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_claim(&claim_at("clm_old", 1_000))
            .await
            .unwrap();
        storage
            .save_claim(&claim_at("clm_new", 9_000))
            .await
            .unwrap();
        storage
            .add_edge(&evidence_edge("edg_old", "clm_old", "seg_x"))
            .await
            .unwrap();
        storage
            .add_edge(&evidence_edge("edg_new", "clm_new", "seg_y"))
            .await
            .unwrap();

        // cutoff between the two: old (1_000) pruned, new (9_000) survives.
        let pruned = storage.prune_claims_older_than(5_000).await.unwrap();
        assert_eq!(pruned, 1, "exactly the old claim is pruned");
        assert!(storage.get_claim("clm_old").await.unwrap().is_none());
        assert!(storage.get_claim("clm_new").await.unwrap().is_some());
        // the old claim's edge is cascaded; the new claim's edge survives.
        assert!(storage
            .edges_from("clm_old", None)
            .await
            .unwrap()
            .is_empty());
        assert_eq!(storage.edges_from("clm_new", None).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn prune_claims_cascades_edges_pointing_to_pruned_claim_by_dst() {
        // A LIVE claim's epistemic edge that points TO a pruned claim (via
        // `dst_id`) must be cascaded, not left dangling. Pruning matched only
        // `src_id` before the fix, so such edges leaked forever
        // (`prune_orphan_evidence_edges` skips non-`evidence` edges).
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_claim(&claim_at("clm_old", 1_000))
            .await
            .unwrap();
        storage
            .save_claim(&claim_at("clm_live", 9_000))
            .await
            .unwrap();
        // clm_live --supports--> clm_old : src kept, dst pruned.
        storage
            .add_edge(&MemoryEdge {
                edge_id: "edg_cross".to_string(),
                src_id: "clm_live".to_string(),
                dst_id: "clm_old".to_string(),
                edge_type: EdgeType::Supports,
                confidence: 0.8,
                evidence_ref: None,
                source: "llm".to_string(),
                created_at: 1_700_000_000,
            })
            .await
            .unwrap();
        // Sanity: the edge is visible from the live claim before the prune.
        assert_eq!(storage.edges_from("clm_live", None).await.unwrap().len(), 1);

        storage.prune_claims_older_than(5_000).await.unwrap();

        assert!(storage.get_claim("clm_old").await.unwrap().is_none());
        // The edge pointing to the pruned claim is cascaded, not orphaned.
        assert!(
            storage
                .edges_from("clm_live", None)
                .await
                .unwrap()
                .is_empty(),
            "edge pointing to a pruned claim via dst_id must be cascaded"
        );
    }

    #[tokio::test]
    async fn prune_orphan_evidence_edges_gcs_dangling_only() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        insert_segment(&storage, "seg_live");
        storage.save_claim(&claim_at("clm_a", 1_000)).await.unwrap();
        // one edge to a live segment, one to a deleted/never-existed segment.
        storage
            .add_edge(&evidence_edge("edg_live", "clm_a", "seg_live"))
            .await
            .unwrap();
        storage
            .add_edge(&evidence_edge("edg_orphan", "clm_a", "seg_gone"))
            .await
            .unwrap();

        let gcd = storage.prune_orphan_evidence_edges().await.unwrap();
        assert_eq!(gcd, 1, "only the dangling evidence edge is GC'd");
        let remaining = storage.edges_from("clm_a", None).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].dst_id, "seg_live");
    }

    #[tokio::test]
    async fn prune_claims_older_than_commits_both_deletes_atomically() {
        // The edge cascade (DELETE memory_edges) and the claim delete
        // (DELETE memory_claims) now run inside one transaction. On the success
        // path both must land together — never an edge gone while its claim
        // lingers (or the reverse). A no-op cutoff exercises the same tx
        // begin/commit funnel and must leave every row intact while reporting 0.
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_claim(&claim_at("clm_old", 1_000))
            .await
            .unwrap();
        storage
            .save_claim(&claim_at("clm_new", 9_000))
            .await
            .unwrap();
        storage
            .add_edge(&evidence_edge("edg_old", "clm_old", "seg_old"))
            .await
            .unwrap();
        // Live → old epistemic edge (kept-src, pruned-dst) to prove the cascade
        // and the claim delete commit as a unit, not just the src-side rows.
        storage
            .add_edge(&MemoryEdge {
                edge_id: "edg_live_to_old".to_string(),
                src_id: "clm_new".to_string(),
                dst_id: "clm_old".to_string(),
                edge_type: EdgeType::Supports,
                confidence: 0.8,
                evidence_ref: None,
                source: "llm".to_string(),
                created_at: 1_700_000_000,
            })
            .await
            .unwrap();

        // No-op cutoff: nothing is older, so the tx commits with 0 deletions and
        // leaves all state untouched.
        let noop = storage.prune_claims_older_than(500).await.unwrap();
        assert_eq!(noop, 0, "nothing older than the cutoff is pruned");
        assert!(storage.get_claim("clm_old").await.unwrap().is_some());
        assert_eq!(storage.edges_from("clm_new", None).await.unwrap().len(), 1);

        // Effective cutoff: the old claim AND both edges referencing it commit
        // their deletion together; the new claim and its non-referencing rows
        // survive intact.
        let pruned = storage.prune_claims_older_than(5_000).await.unwrap();
        assert_eq!(pruned, 1, "exactly the old claim is pruned");
        assert!(storage.get_claim("clm_old").await.unwrap().is_none());
        assert!(storage.get_claim("clm_new").await.unwrap().is_some());
        // Both the src-side and the dst-side edge are cascaded atomically.
        assert!(storage
            .edges_from("clm_old", None)
            .await
            .unwrap()
            .is_empty());
        assert!(
            storage
                .edges_from("clm_new", None)
                .await
                .unwrap()
                .is_empty(),
            "the live→old edge is cascaded with the claim delete, not orphaned"
        );
    }

    #[tokio::test]
    async fn supersede_claim_flips_status_and_writes_provenance_atomically() {
        let storage = SqliteStorage::open_in_memory(30).unwrap();
        storage
            .save_claim(&claim_at("clm_loser", 1_000))
            .await
            .unwrap();
        storage
            .save_claim(&claim_at("clm_winner", 2_000))
            .await
            .unwrap();

        let supersedes = MemoryEdge {
            edge_id: "edg_sup".to_string(),
            src_id: "clm_winner".to_string(),
            dst_id: "clm_loser".to_string(),
            edge_type: EdgeType::Supersedes,
            confidence: 0.95,
            evidence_ref: None,
            source: "llm".to_string(),
            created_at: 3_000,
        };
        storage
            .supersede_claim("clm_loser", &supersedes, 3_000)
            .await
            .unwrap();

        // Loser flipped Active → Superseded.
        let loser = storage.get_claim("clm_loser").await.unwrap().unwrap();
        assert_eq!(loser.status, ClaimStatus::Superseded);
        assert_eq!(loser.updated_at, 3_000);
        // Winner untouched.
        assert_eq!(
            storage
                .get_claim("clm_winner")
                .await
                .unwrap()
                .unwrap()
                .status,
            ClaimStatus::Active
        );
        // Provenance edge written in the same transaction (winner → loser, llm).
        let edges = storage
            .edges_from("clm_winner", Some(EdgeType::Supersedes))
            .await
            .unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].dst_id, "clm_loser");
        assert_eq!(edges[0].source, "llm");

        // Idempotent re-run (same edge_id) does not error or duplicate.
        storage
            .supersede_claim("clm_loser", &supersedes, 3_500)
            .await
            .unwrap();
        assert_eq!(
            storage
                .edges_from("clm_winner", Some(EdgeType::Supersedes))
                .await
                .unwrap()
                .len(),
            1
        );
    }
}
