//! ADR-032 §2 bounded projection helper — the single shared seam between the
//! ADR-023 memory graph and generation-adjacent consumers (Mode A today).
//!
//! Implements `maekon_core::ports::memory_graph_projection::
//! MemoryGraphProjectionPort`. Consumers depend on that trait only; this
//! module is reached exclusively through `src-tauri` DI wiring.
//!
//! Fail-closed semantics are split per the contract:
//! - unevaluable bound (disabled, consent authority unavailable/denied,
//!   invalid window/floor/cap) → `Ok(EdgeProjection::default())`;
//! - genuine `MemoryGraphPort` storage failure → `Err` propagated unchanged.

use std::sync::Arc;

use maekon_core::config_manager::ConfigManager;
use maekon_core::error::CoreError;
use maekon_core::models::memory_graph::{ClaimStatus, EdgeProjection, MemoryEdge, ProjectedEdge};
use maekon_core::ports::consent_manager::ConsentManagerPort;
use maekon_core::ports::memory_graph_port::MemoryGraphPort;
use maekon_core::ports::memory_graph_projection::MemoryGraphProjectionPort;
use tracing::debug;

const SECS_PER_DAY: i64 = 86_400;

/// ADR-032 §2 implementation: bounded, deterministic, fail-closed selection
/// over `MemoryGraphPort`.
///
/// `consent` is an `Option` on purpose: composition roots where no consent
/// manager exists wire `None`, which the contract treats as "consent
/// authority unavailable" — an unevaluable bound, hence a permanent empty
/// projection (never a bypass).
pub struct BoundedMemoryGraphProjection {
    memory_graph: Arc<dyn MemoryGraphPort>,
    consent: Option<Arc<dyn ConsentManagerPort>>,
    config_manager: ConfigManager,
}

impl BoundedMemoryGraphProjection {
    pub fn new(
        memory_graph: Arc<dyn MemoryGraphPort>,
        consent: Option<Arc<dyn ConsentManagerPort>>,
        config_manager: ConfigManager,
    ) -> Self {
        Self {
            memory_graph,
            consent,
            config_manager,
        }
    }
}

#[async_trait::async_trait]
impl MemoryGraphProjectionPort for BoundedMemoryGraphProjection {
    async fn project_edges_for_ranking(&self, now_secs: i64) -> Result<EdgeProjection, CoreError> {
        let cfg = self.config_manager.get();
        let bounds = &cfg.analysis.memory_graph_projection;

        // Policy gates (§2 preamble + §3.2): each miss is an unevaluable
        // bound → empty projection, logged at debug so a quiet Mode A is
        // diagnosable without being noisy.
        if !bounds.enabled {
            return Ok(EdgeProjection::default());
        }
        let Some(consent) = self.consent.as_ref() else {
            debug!("ADR-032 Mode A: consent authority unavailable; empty projection");
            return Ok(EdgeProjection::default());
        };
        if !consent.memory_graph_retrieval_ranking_permitted() {
            debug!("ADR-032 Mode A: memory_graph_retrieval_ranking not granted; empty projection");
            return Ok(EdgeProjection::default());
        }

        // Bound validation (§2.2–§2.4). The generation window is independent
        // of the retention prune and must stay within it; a violation is an
        // invalid bound, never permission to widen selection.
        let retention_days = cfg.analysis.embedding.retention_days;
        if bounds.generation_window_days == 0 || bounds.generation_window_days > retention_days {
            debug!(
                window = bounds.generation_window_days,
                retention = retention_days,
                "ADR-032 Mode A: generation window outside [1, retention]; empty projection"
            );
            return Ok(EdgeProjection::default());
        }
        if !(0.0..=1.0).contains(&bounds.min_input_confidence) {
            debug!(
                floor = bounds.min_input_confidence,
                "ADR-032 Mode A: confidence floor outside [0, 1]; empty projection"
            );
            return Ok(EdgeProjection::default());
        }
        if bounds.max_claims == 0 || bounds.max_edges == 0 {
            debug!("ADR-032 Mode A: zero selection cap; empty projection");
            return Ok(EdgeProjection::default());
        }

        // §2.1 status bound at selection time; storage errors propagate (the
        // `?` here is the Err half of the fail-closed split).
        let mut claims = self
            .memory_graph
            .list_claims_by_status(ClaimStatus::Active)
            .await?;

        // §2.2 recency window + §2.3 input-side confidence floor. Claim rows
        // are touched for `claim_id` selection only — `text`/`kind`/`source`
        // are never read past this filter (§2.5).
        let cutoff = now_secs - i64::from(bounds.generation_window_days) * SECS_PER_DAY;
        claims.retain(|c| {
            c.updated_at >= cutoff && f64::from(c.confidence) >= bounds.min_input_confidence
        });

        // §2.4 deterministic total order + hard cap for claims.
        claims.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.claim_id.cmp(&b.claim_id))
        });
        claims.truncate(bounds.max_claims);
        let claim_ids: Vec<String> = claims.iter().map(|c| c.claim_id.clone()).collect();
        let claims_selected = claim_ids.len();
        drop(claims);
        if claims_selected == 0 {
            return Ok(EdgeProjection {
                edges: Vec::new(),
                claims_selected: 0,
            });
        }

        // One batched read (no N+1), then §2.4 deterministic total order +
        // hard cap for edges.
        let grouped = self.memory_graph.edges_from_many(&claim_ids).await?;
        let mut edges: Vec<MemoryEdge> = grouped.into_values().flatten().collect();
        edges.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.edge_id.cmp(&b.edge_id))
        });
        edges.truncate(bounds.max_edges);

        // §2.6 field allowlist: (src_id, dst_id, edge_type, confidence) only.
        let projected: Vec<ProjectedEdge> = edges
            .into_iter()
            .map(|e| ProjectedEdge {
                src_id: e.src_id,
                dst_id: e.dst_id,
                edge_type: e.edge_type,
                confidence: e.confidence,
            })
            .collect();

        debug!(
            claims_selected,
            edges_projected = projected.len(),
            "ADR-032 Mode A projection"
        );
        Ok(EdgeProjection {
            edges: projected,
            claims_selected,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use maekon_core::config::AppConfig;
    use maekon_core::consent::{ConsentPermissions, ConsentRecord, ConsentStatus};
    use maekon_core::models::memory_graph::{ClaimKind, EdgeType, MemoryClaim};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;

    const NOW: i64 = 1_753_000_000;

    // ---- manual mocks (ADR-001 §5) ----

    struct FakeGraph {
        claims: Vec<MemoryClaim>,
        edges: HashMap<String, Vec<MemoryEdge>>,
        fail_claims: bool,
        fail_edges: bool,
    }

    impl FakeGraph {
        fn new(claims: Vec<MemoryClaim>, edges: HashMap<String, Vec<MemoryEdge>>) -> Self {
            Self {
                claims,
                edges,
                fail_claims: false,
                fail_edges: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl MemoryGraphPort for FakeGraph {
        async fn save_claim(&self, _claim: &MemoryClaim) -> Result<(), CoreError> {
            unreachable!("not used by projection")
        }
        async fn get_claim(&self, _claim_id: &str) -> Result<Option<MemoryClaim>, CoreError> {
            unreachable!("not used by projection")
        }
        async fn list_claims_by_status(
            &self,
            status: ClaimStatus,
        ) -> Result<Vec<MemoryClaim>, CoreError> {
            if self.fail_claims {
                return Err(CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: "claims read failed".to_string(),
                });
            }
            Ok(self
                .claims
                .iter()
                .filter(|c| c.status == status)
                .cloned()
                .collect())
        }
        async fn set_claim_status(
            &self,
            _claim_id: &str,
            _status: ClaimStatus,
            _updated_at: i64,
        ) -> Result<(), CoreError> {
            unreachable!("not used by projection")
        }
        async fn add_edge(&self, _edge: &MemoryEdge) -> Result<(), CoreError> {
            unreachable!("not used by projection")
        }
        async fn edges_from(
            &self,
            _src_id: &str,
            _edge_type: Option<EdgeType>,
        ) -> Result<Vec<MemoryEdge>, CoreError> {
            unreachable!("projection must use the batched read")
        }
        async fn edges_from_many(
            &self,
            src_ids: &[String],
        ) -> Result<HashMap<String, Vec<MemoryEdge>>, CoreError> {
            if self.fail_edges {
                return Err(CoreError::Storage {
                    code: maekon_core::error_codes::StorageCode::Failed,
                    message: "edges read failed".to_string(),
                });
            }
            Ok(src_ids
                .iter()
                .filter_map(|id| self.edges.get(id).map(|v| (id.clone(), v.clone())))
                .collect())
        }
        async fn prune_claims_older_than(&self, _cutoff: i64) -> Result<u64, CoreError> {
            unreachable!("not used by projection")
        }
        async fn prune_orphan_evidence_edges(&self) -> Result<u64, CoreError> {
            unreachable!("not used by projection")
        }
        async fn supersede_claim(
            &self,
            _loser_claim_id: &str,
            _supersedes_edge: &MemoryEdge,
            _updated_at: i64,
        ) -> Result<(), CoreError> {
            unreachable!("not used by projection")
        }
    }

    struct FakeConsent {
        granted: bool,
    }

    impl ConsentManagerPort for FakeConsent {
        fn check_consent(&self) -> ConsentStatus {
            if self.granted {
                ConsentStatus::Valid
            } else {
                ConsentStatus::NotGranted
            }
        }
        fn current_consent(&self) -> Option<ConsentRecord> {
            None
        }
        fn effective_permissions(&self) -> ConsentPermissions {
            ConsentPermissions {
                memory_graph_retrieval_ranking: self.granted,
                ..Default::default()
            }
        }
        fn status_and_permissions(&self) -> (ConsentStatus, ConsentPermissions) {
            (self.check_consent(), self.effective_permissions())
        }
        fn grant_consent(
            &self,
            _permissions: ConsentPermissions,
            _data_retention_days: u32,
        ) -> Result<(), CoreError> {
            unreachable!("not used by projection")
        }
        fn revoke_consent(&self) -> Result<(), CoreError> {
            unreachable!("not used by projection")
        }
        fn has_pending_deletion(&self) -> bool {
            false
        }
        fn pending_erasure_id(&self) -> Option<String> {
            None
        }
        fn clear_pending_deletion(&self) {}
        fn deletion_flag(&self) -> Arc<AtomicBool> {
            Arc::new(AtomicBool::new(false))
        }
        fn erasing(&self) -> Arc<AtomicBool> {
            Arc::new(AtomicBool::new(false))
        }
    }

    fn claim(id: &str, updated_at: i64, confidence: f32, status: ClaimStatus) -> MemoryClaim {
        MemoryClaim {
            claim_id: id.to_string(),
            kind: ClaimKind::Episodic,
            text: format!("SECRET-TEXT-{id}"),
            source: "digest_highlight".to_string(),
            confidence,
            status,
            created_at: updated_at,
            updated_at,
        }
    }

    fn edge(id: &str, src: &str, dst: &str, created_at: i64, confidence: f32) -> MemoryEdge {
        MemoryEdge {
            edge_id: id.to_string(),
            src_id: src.to_string(),
            dst_id: dst.to_string(),
            edge_type: EdgeType::Evidence,
            confidence,
            evidence_ref: Some(format!("seg_{dst}")),
            source: "rule".to_string(),
            created_at,
        }
    }

    fn config_manager(mutate: impl FnOnce(&mut AppConfig)) -> (ConfigManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager =
            ConfigManager::with_path(dir.path().join("config.json")).expect("config manager");
        let mut cfg = manager.get();
        mutate(&mut cfg);
        manager.update(cfg).expect("config update");
        (manager, dir)
    }

    fn enabled_config(mutate: impl FnOnce(&mut AppConfig)) -> (ConfigManager, tempfile::TempDir) {
        config_manager(|cfg| {
            cfg.analysis.memory_graph_projection.enabled = true;
            mutate(cfg);
        })
    }

    fn projection(
        graph: FakeGraph,
        consent: Option<bool>,
        cm: ConfigManager,
    ) -> BoundedMemoryGraphProjection {
        BoundedMemoryGraphProjection::new(
            Arc::new(graph),
            consent.map(|granted| Arc::new(FakeConsent { granted }) as Arc<dyn ConsentManagerPort>),
            cm,
        )
    }

    fn seeded_graph() -> FakeGraph {
        let mut edges = HashMap::new();
        edges.insert(
            "clm_a".to_string(),
            vec![edge("edg_1", "clm_a", "seg_x", NOW - 10, 0.9)],
        );
        FakeGraph::new(
            vec![claim("clm_a", NOW - 100, 0.9, ClaimStatus::Active)],
            edges,
        )
    }

    // ---- unevaluable bounds → Ok(empty) ----

    #[tokio::test]
    async fn disabled_config_yields_empty_even_with_data() {
        let (cm, _dir) = config_manager(|_| {});
        let p = projection(seeded_graph(), Some(true), cm);
        let out = p.project_edges_for_ranking(NOW).await.unwrap();
        assert_eq!(out, EdgeProjection::default());
    }

    #[tokio::test]
    async fn missing_consent_authority_yields_empty() {
        let (cm, _dir) = enabled_config(|_| {});
        let p = projection(seeded_graph(), None, cm);
        let out = p.project_edges_for_ranking(NOW).await.unwrap();
        assert_eq!(out, EdgeProjection::default());
    }

    #[tokio::test]
    async fn denied_consent_yields_empty() {
        let (cm, _dir) = enabled_config(|_| {});
        let p = projection(seeded_graph(), Some(false), cm);
        let out = p.project_edges_for_ranking(NOW).await.unwrap();
        assert_eq!(out, EdgeProjection::default());
    }

    #[tokio::test]
    async fn zero_window_yields_empty() {
        let (cm, _dir) = enabled_config(|cfg| {
            cfg.analysis.memory_graph_projection.generation_window_days = 0;
        });
        let p = projection(seeded_graph(), Some(true), cm);
        assert_eq!(
            p.project_edges_for_ranking(NOW).await.unwrap(),
            EdgeProjection::default()
        );
    }

    #[tokio::test]
    async fn window_beyond_retention_yields_empty() {
        let (cm, _dir) = enabled_config(|cfg| {
            cfg.analysis.embedding.retention_days = 30;
            cfg.analysis.memory_graph_projection.generation_window_days = 31;
        });
        let p = projection(seeded_graph(), Some(true), cm);
        assert_eq!(
            p.project_edges_for_ranking(NOW).await.unwrap(),
            EdgeProjection::default()
        );
    }

    #[tokio::test]
    async fn out_of_range_confidence_floor_yields_empty() {
        let (cm, _dir) = enabled_config(|cfg| {
            cfg.analysis.memory_graph_projection.min_input_confidence = 1.5;
        });
        let p = projection(seeded_graph(), Some(true), cm);
        assert_eq!(
            p.project_edges_for_ranking(NOW).await.unwrap(),
            EdgeProjection::default()
        );
    }

    #[tokio::test]
    async fn zero_caps_yield_empty() {
        for (claims_cap, edges_cap) in [(0usize, 256usize), (64, 0)] {
            let (cm, _dir) = enabled_config(|cfg| {
                cfg.analysis.memory_graph_projection.max_claims = claims_cap;
                cfg.analysis.memory_graph_projection.max_edges = edges_cap;
            });
            let p = projection(seeded_graph(), Some(true), cm);
            assert_eq!(
                p.project_edges_for_ranking(NOW).await.unwrap(),
                EdgeProjection::default()
            );
        }
    }

    // ---- storage failures → Err (never masked) ----

    #[tokio::test]
    async fn claims_storage_error_propagates() {
        let mut graph = seeded_graph();
        graph.fail_claims = true;
        let (cm, _dir) = enabled_config(|_| {});
        let p = projection(graph, Some(true), cm);
        let err = p.project_edges_for_ranking(NOW).await.unwrap_err();
        assert_eq!(err.code(), "storage.failed");
    }

    #[tokio::test]
    async fn edges_storage_error_propagates() {
        let mut graph = seeded_graph();
        graph.fail_edges = true;
        let (cm, _dir) = enabled_config(|_| {});
        let p = projection(graph, Some(true), cm);
        let err = p.project_edges_for_ranking(NOW).await.unwrap_err();
        assert_eq!(err.code(), "storage.failed");
    }

    // ---- bound enforcement on the happy path ----

    #[tokio::test]
    async fn happy_path_projects_allowlisted_edge_fields_only() {
        let (cm, _dir) = enabled_config(|_| {});
        let p = projection(seeded_graph(), Some(true), cm);
        let out = p.project_edges_for_ranking(NOW).await.unwrap();
        assert_eq!(out.claims_selected, 1);
        assert_eq!(out.edges.len(), 1);
        assert_eq!(out.edges[0].src_id, "clm_a");
        assert_eq!(out.edges[0].dst_id, "seg_x");
        assert_eq!(out.edges[0].edge_type, EdgeType::Evidence);
        // §2.5/§2.6 no-text guarantee, asserted at the serialization boundary:
        // nothing a consumer can see contains claim text or provenance refs.
        let json = serde_json::to_string(&out).unwrap();
        assert!(!json.contains("SECRET-TEXT"));
        assert!(!json.contains("evidence_ref"));
        assert!(!json.contains("seg_seg_x"));
    }

    #[tokio::test]
    async fn window_and_floor_filter_claims() {
        let graph = FakeGraph::new(
            vec![
                claim("clm_new", NOW - 100, 0.9, ClaimStatus::Active),
                claim("clm_old", NOW - 40 * SECS_PER_DAY, 0.9, ClaimStatus::Active),
                claim("clm_low", NOW - 100, 0.2, ClaimStatus::Active),
                claim("clm_gone", NOW - 100, 0.9, ClaimStatus::Retracted),
            ],
            HashMap::new(),
        );
        let (cm, _dir) = enabled_config(|_| {});
        let p = projection(graph, Some(true), cm);
        let out = p.project_edges_for_ranking(NOW).await.unwrap();
        assert_eq!(out.claims_selected, 1);
    }

    #[tokio::test]
    async fn claim_cap_keeps_newest_with_claim_id_tiebreak() {
        let graph = FakeGraph::new(
            vec![
                claim("clm_b", NOW - 50, 0.9, ClaimStatus::Active),
                claim("clm_a", NOW - 50, 0.9, ClaimStatus::Active),
                claim("clm_c", NOW - 500, 0.9, ClaimStatus::Active),
            ],
            {
                let mut m = HashMap::new();
                for id in ["clm_a", "clm_b", "clm_c"] {
                    m.insert(
                        id.to_string(),
                        vec![edge(&format!("edg_{id}"), id, "seg", NOW - 1, 0.5)],
                    );
                }
                m
            },
        );
        let (cm, _dir) = enabled_config(|cfg| {
            cfg.analysis.memory_graph_projection.max_claims = 2;
        });
        let p = projection(graph, Some(true), cm);
        let out = p.project_edges_for_ranking(NOW).await.unwrap();
        // Cap 2 at equal updated_at keeps clm_a/clm_b (claim_id ASC tie-break)
        // and drops the older clm_c.
        assert_eq!(out.claims_selected, 2);
        let mut srcs: Vec<&str> = out.edges.iter().map(|e| e.src_id.as_str()).collect();
        srcs.sort_unstable();
        assert_eq!(srcs, vec!["clm_a", "clm_b"]);
    }

    #[tokio::test]
    async fn edge_cap_orders_created_desc_with_edge_id_tiebreak() {
        let mut edges = HashMap::new();
        edges.insert(
            "clm_a".to_string(),
            vec![
                edge("edg_2", "clm_a", "seg_1", NOW - 30, 0.5),
                edge("edg_1", "clm_a", "seg_2", NOW - 10, 0.5),
                edge("edg_3", "clm_a", "seg_3", NOW - 10, 0.5),
            ],
        );
        let graph = FakeGraph::new(
            vec![claim("clm_a", NOW - 100, 0.9, ClaimStatus::Active)],
            edges,
        );
        let (cm, _dir) = enabled_config(|cfg| {
            cfg.analysis.memory_graph_projection.max_edges = 2;
        });
        let p = projection(graph, Some(true), cm);
        let out = p.project_edges_for_ranking(NOW).await.unwrap();
        // created_at DESC keeps the two NOW-10 edges; edge_id ASC tie-break
        // orders edg_1 before edg_3; the older edg_2 is capped away.
        let ids: Vec<&str> = out.edges.iter().map(|e| e.dst_id.as_str()).collect();
        assert_eq!(ids, vec!["seg_2", "seg_3"]);
    }
}
