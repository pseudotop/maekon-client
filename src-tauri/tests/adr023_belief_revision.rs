//! ADR-023 Phase-2 belief revision (D1/D2) end-to-end against a real
//! `SqliteStorage` + a mock LOCAL provider. Verifies relation edges, atomic
//! supersede-with-provenance, threshold gating, NoOp/disabled degradation, and
//! per-claim PII masking at the enrichment boundary (MG-PII-03).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use maekon_analysis::{BeliefPiiFilter, BeliefRevision, NoOpAnalysisProvider};
use maekon_core::error::CoreError;
use maekon_core::models::memory_graph::{
    ClaimKind, ClaimStatus, ClaimStatusChange, EdgeType, MemoryClaim, RelationEdgeProposal,
};
use maekon_core::models::suggestion::Suggestion;
use maekon_core::ports::analysis_provider::AnalysisProvider;
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_storage::sqlite::SqliteStorage;

const NOW: i64 = 1_700_000_000;

/// Mock provider: returns configured proposals and captures the (masked)
/// claims_json it was handed.
#[derive(Default)]
struct MockProvider {
    relations: Vec<RelationEdgeProposal>,
    contradictions: Vec<ClaimStatusChange>,
    seen_claims_json: Mutex<Option<String>>,
}

#[async_trait]
impl AnalysisProvider for MockProvider {
    async fn analyze(&self, _c: &str, _s: &str) -> Result<Vec<Suggestion>, CoreError> {
        Ok(vec![])
    }
    async fn extract_relations(
        &self,
        claims_json: &str,
        _s: &str,
    ) -> Result<Vec<RelationEdgeProposal>, CoreError> {
        *self.seen_claims_json.lock().unwrap() = Some(claims_json.to_string());
        Ok(self.relations.clone())
    }
    async fn detect_contradictions(
        &self,
        _c: &str,
        _s: &str,
    ) -> Result<Vec<ClaimStatusChange>, CoreError> {
        Ok(self.contradictions.clone())
    }
    fn provider_name(&self) -> &str {
        "mock-local"
    }
}

fn claim(id: &str, text: &str) -> MemoryClaim {
    MemoryClaim {
        claim_id: id.to_string(),
        kind: ClaimKind::Reflective,
        text: text.to_string(),
        source: "digest_timeline".to_string(),
        confidence: 1.0,
        status: ClaimStatus::Active,
        created_at: NOW,
        updated_at: NOW,
    }
}

fn no_mask() -> BeliefPiiFilter {
    Arc::new(|s: &str| s.to_string())
}

async fn seed_two(storage: &SqliteStorage) {
    storage
        .save_claim(&claim("clm_a", "prefers tabs"))
        .await
        .unwrap();
    storage
        .save_claim(&claim("clm_b", "prefers spaces"))
        .await
        .unwrap();
}

#[tokio::test]
async fn relation_extraction_writes_typed_llm_edges() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    seed_two(&storage).await;
    let provider = Arc::new(MockProvider {
        relations: vec![RelationEdgeProposal {
            src_claim_id: "clm_a".into(),
            dst_id: "clm_b".into(),
            edge_type: EdgeType::Contradicts,
            confidence: 0.8,
            evidence_ref: None,
        }],
        ..Default::default()
    });
    let br = BeliefRevision::new(
        provider,
        storage.clone() as Arc<dyn MemoryGraphPort>,
        no_mask(),
        0.9,
        true,
    );
    let stats = br.run_pass(NOW).await.unwrap();
    assert_eq!(stats.relations_added, 1);
    let edges = storage
        .edges_from("clm_a", Some(EdgeType::Contradicts))
        .await
        .unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].dst_id, "clm_b");
    assert_eq!(edges[0].source, "llm");
}

#[tokio::test]
async fn self_loop_relation_is_rejected() {
    // An LLM-proposed relation from a claim to itself is meaningless noise and
    // must be dropped (mirrors the D2 winner != loser guard). Two claims are
    // seeded so run_pass proceeds past the <2-active early return.
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    seed_two(&storage).await;
    let provider = Arc::new(MockProvider {
        relations: vec![RelationEdgeProposal {
            src_claim_id: "clm_a".into(),
            dst_id: "clm_a".into(), // self-loop
            edge_type: EdgeType::Supports,
            confidence: 1.0,
            evidence_ref: None,
        }],
        ..Default::default()
    });
    let br = BeliefRevision::new(
        provider,
        storage.clone() as Arc<dyn MemoryGraphPort>,
        no_mask(),
        0.9,
        true,
    );
    let stats = br.run_pass(NOW).await.unwrap();
    assert_eq!(
        stats.relations_added, 0,
        "self-loop relation is not persisted"
    );
    assert!(storage.edges_from("clm_a", None).await.unwrap().is_empty());
}

#[tokio::test]
async fn high_confidence_contradiction_supersedes_with_provenance() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    seed_two(&storage).await;
    let provider = Arc::new(MockProvider {
        contradictions: vec![ClaimStatusChange {
            loser_claim_id: "clm_a".into(),
            winner_claim_id: "clm_b".into(),
            new_status: ClaimStatus::Superseded,
            confidence: 0.95,
            rationale: Some("b is newer".into()),
        }],
        ..Default::default()
    });
    let br = BeliefRevision::new(
        provider,
        storage.clone() as Arc<dyn MemoryGraphPort>,
        no_mask(),
        0.9,
        true,
    );
    let stats = br.run_pass(NOW).await.unwrap();
    assert_eq!(stats.claims_superseded, 1);
    assert_eq!(
        storage.get_claim("clm_a").await.unwrap().unwrap().status,
        ClaimStatus::Superseded
    );
    assert_eq!(
        storage.get_claim("clm_b").await.unwrap().unwrap().status,
        ClaimStatus::Active
    );
    // Provenance edge winner → loser, atomic with the flip.
    let prov = storage
        .edges_from("clm_b", Some(EdgeType::Supersedes))
        .await
        .unwrap();
    assert_eq!(prov.len(), 1);
    assert_eq!(prov[0].dst_id, "clm_a");
    assert_eq!(prov[0].source, "llm");
}

#[tokio::test]
async fn low_confidence_does_not_supersede() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    seed_two(&storage).await;
    let provider = Arc::new(MockProvider {
        contradictions: vec![ClaimStatusChange {
            loser_claim_id: "clm_a".into(),
            winner_claim_id: "clm_b".into(),
            new_status: ClaimStatus::Superseded,
            confidence: 0.5, // below the 0.9 threshold
            rationale: None,
        }],
        ..Default::default()
    });
    let br = BeliefRevision::new(
        provider,
        storage.clone() as Arc<dyn MemoryGraphPort>,
        no_mask(),
        0.9,
        true,
    );
    let stats = br.run_pass(NOW).await.unwrap();
    assert_eq!(stats.claims_superseded, 0);
    assert_eq!(
        storage.get_claim("clm_a").await.unwrap().unwrap().status,
        ClaimStatus::Active
    );
}

#[tokio::test]
async fn noop_provider_writes_nothing() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    seed_two(&storage).await;
    let br = BeliefRevision::new(
        Arc::new(NoOpAnalysisProvider),
        storage.clone() as Arc<dyn MemoryGraphPort>,
        no_mask(),
        0.9,
        true,
    );
    let stats = br.run_pass(NOW).await.unwrap();
    assert_eq!(stats, Default::default());
    assert!(storage.edges_from("clm_a", None).await.unwrap().is_empty());
    assert_eq!(
        storage.get_claim("clm_a").await.unwrap().unwrap().status,
        ClaimStatus::Active
    );
}

#[tokio::test]
async fn disabled_is_a_noop_even_with_proposals() {
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    seed_two(&storage).await;
    let provider = Arc::new(MockProvider {
        relations: vec![RelationEdgeProposal {
            src_claim_id: "clm_a".into(),
            dst_id: "clm_b".into(),
            edge_type: EdgeType::Supports,
            confidence: 1.0,
            evidence_ref: None,
        }],
        ..Default::default()
    });
    let br = BeliefRevision::new(
        provider,
        storage.clone() as Arc<dyn MemoryGraphPort>,
        no_mask(),
        0.9,
        false, // disabled
    );
    let stats = br.run_pass(NOW).await.unwrap();
    assert_eq!(stats, Default::default());
    assert!(storage.edges_from("clm_a", None).await.unwrap().is_empty());
}

#[tokio::test]
async fn claim_text_is_masked_at_enrichment_boundary() {
    // MG-PII-03: each claim's text must be masked BEFORE it reaches the provider.
    let storage = Arc::new(SqliteStorage::open_in_memory(30).unwrap());
    storage
        .save_claim(&claim("clm_a", "email me at secret@example.com"))
        .await
        .unwrap();
    storage.save_claim(&claim("clm_b", "ok")).await.unwrap();

    let provider = Arc::new(MockProvider::default());
    let mask: BeliefPiiFilter = Arc::new(|s: &str| s.replace("secret@example.com", "[EMAIL]"));
    let br = BeliefRevision::new(
        provider.clone() as Arc<dyn AnalysisProvider>,
        storage.clone() as Arc<dyn MemoryGraphPort>,
        mask,
        0.9,
        true,
    );
    br.run_pass(NOW).await.unwrap();

    let seen = provider
        .seen_claims_json
        .lock()
        .unwrap()
        .clone()
        .expect("provider received claims_json");
    assert!(seen.contains("[EMAIL]"), "masked token must be present");
    assert!(
        !seen.contains("secret@example.com"),
        "raw PII must NOT reach the provider"
    );
}
