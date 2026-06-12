[English](./ADR-023-local-symbolic-memory-graph.md) | [한국어](./ADR-023-local-symbolic-memory-graph.ko.md)

# ADR-023: Local Symbolic Memory-Graph Layer for Standalone Client

**Status**: Accepted — decision ratified and **fully implemented**. Landed: substrate (AC #1, #2, #7, #9), the Phase-1 LLM-free consumer slice (AC #3, #4 — D3 digest→claim promotion + D5 rule taxonomy), the rule-seeded `Associated` app-sequence edges (#4441 box 5), and the Phase-2 LLM-gated belief revision (AC #5, #6, #8 — D1 relation edges + D2 contradiction→supersede, local-LLM-gated, egress-safety-audited). The on-demand markdown render wiring (web export endpoint) remains deferred (tracked in #4441); the broader pre-existing right-to-erasure gap is tracked separately in #4478.
**Date**: 2026-05-30
**Scope**: `crates/maekon-analysis`, `crates/maekon-core/src/ports`, `crates/maekon-core/src/models`, `crates/maekon-storage/src/migration`, `crates/maekon-storage/src/sqlite`
**Related**: ADR-011 (Standalone Analysis Pipeline), ADR-012 (Adaptive Tiered Memory), ADR-013 (LLM Segment Summary + Vector RAG), ADR-018 (Regime Manager Persistence), ADR-001 (Rust Client Architecture Patterns)
**Implementation**: substrate — `crates/maekon-core/src/models/memory_graph.rs`, `crates/maekon-core/src/ports/memory_graph_port.rs`, `crates/maekon-storage/src/migration/v34_memory_graph.rs`, `crates/maekon-storage/src/sqlite/memory_graph_impl.rs`. Phase-1 consumer slice (D3/D5) — `crates/maekon-analysis/src/claim_promoter.rs` (pure promoter), `DigestExporter::to_markdown_with_claims` in `crates/maekon-core/src/models/daily_digest.rs`, and the scheduler aggregation-loop wiring (`src-tauri/src/scheduler/loops/system.rs`). D1/D2 LLM-gated edges tracked separately.

---

## Context

The Maekon client must provide **good UX standalone** — that is, with no Maekon backend server reachable. ADR-011 already establishes that the local memory pipeline is server-optional by design: local embeddings (fastembed/ONNX `all-MiniLM-L6-v2` INT8, `default = ["fastembed-local"]`), IVF/HNSW vector store, hybrid FTS5+vector Reciprocal-Rank-Fusion search, kmeans/gmm/hdbscan regime detection, pattern mining, segment summarization, and digest/markdown export all run client-side with zero network I/O.

The server-side `ontology_reasoning` and `knowledge_management` domains provide a richer *symbolic* memory model — typed knowledge-graph edges, contradiction detection, belief revision, and durable thesis/insight nodes. **These domains give zero standalone benefit**: they live entirely server-side, have no client mirror, and are only exercised when a server is active. The question this ADR answers is whether the *standalone* client should grow its own **local symbolic knowledge-graph / memory layer** (à la a local LLM-Wiki / second-brain: local store + local LLM) on top of its existing local vector/regime memory.

This question is informed by **Andrej Karpathy's LLM Wiki** pattern — the canonical origin of the approach: an LLM-maintained, interlinked local knowledge base of markdown files where each new source is read and *integrated* into entity/topic pages (noting contradictions), so knowledge is compiled once and kept current rather than re-derived per query. Crucially it is a **local, server-less** knowledge graph, which is exactly the standalone model relevant here.

A grounded survey of `crates/` confirms the current local memory layer is **vectors + regimes + flat records only — there is zero graph/edge structure**:

- The core memory unit `SegmentSummary` (`maekon-core/src/models/tiered_memory/segment.rs`) has no field linking it to any other segment. `EmbeddingMetadata`/`SearchResult` are pure retrieval payloads. `Regime` is a centroid point in feature space, not a node with edges.
- `RegimeManager` (`maekon-analysis/src/regime_manager.rs`, ADR-012) holds a flat `Vec<Regime>`; `merge_two` computes a weighted centroid and **destructively removes both source regimes** with no provenance/parent/split. (`rg` for `split`/`provenance`/`merged_from`/`parent_regime` in the regime sources returns empty — note: any "split" mention in prose is aspirational, not implemented.)
- Retrieval (`hybrid_search_service.rs`, `vector_retriever.rs`) is similarity + FTS RRF ranking only — no traversal, no typed-relation following.
- `DigestHighlight` (`models/daily_digest.rs`: typed `{Achievement|Warning|Suggestion}` + optional single `segment_id`) is the **only** claim-like structure and the only soft cross-reference, but it is **ephemeral LLM output regenerated each run**, not a durable node, with no evidence edges and no contradiction/belief-revision over it.
- Storage (`maekon-storage`, `CURRENT_VERSION = 33`) has only structural-containment foreign keys; **no edge/relation/claim/thesis/contradiction/evidence/provenance table exists** (grep-confirmed empty).

The benchmark deltas the standalone client lacks locally are therefore all confirmed **absent**: **D1** typed epistemic edges (supports/refines/contradicts), **D2** contradiction detection + belief revision, **D3** durable thesis/claim nodes with evidence edges (partial: `DigestHighlight` is the nearest analog), **D4** near-miss/contrastive edges, **D5** cognitive memory taxonomy (semantic/episodic/procedural/reflective).

Two facts constrain the decision and must be stated honestly:

1. **The substrate is ready and greenfield-clean.** SQLite is already the store with a clean per-version migration framework (the `v31_regime_manager_state.rs` singleton-blob pattern is a direct precedent). A stable node-identity primitive **already exists**: `segment_id` is the universal key across `SegmentSummary`, `EmbeddingMetadata`, and `DigestHighlight.segment_id`, so new edges can reference real memory units immediately. `maekon-analysis` depends only on `maekon-core`, so a new graph component fits the hexagonal boundaries (ADR-001) without violating them. New nodes get vector + FTS recall for free via the existing tables.

2. **The inference engine for the *symbolic* deltas is an optional local LLM.** There is no symbolic reasoner client-side; typed-edge inference (D1) and contradiction judging (D2) realistically require an LLM. The client *does* have a local-LLM path — the `AnalysisProvider` port, satisfiable 100% offline by pointing `config.ai_provider.llm_api` at a local Ollama endpoint (`AnalysisClient` special-cases `AiProviderType::Ollama`, no API key) — and `DailyInsightGenerator` already performs JSON-structured LLM extraction that generalizes to typed edges/claims. **But this path is optional and degrades to `NoOpAnalysisProvider`** ("No LLM provider configured") when no `llm_api` is set, which is the default install. Any delta whose generation hard-depends on the LLM would therefore be silently empty for offline-purist users unless we design around that.

The decision must follow from this evidence — a durable graph substrate is cheap and useful offline; the LLM-dependent symbolic deltas are valuable but conditional — not from enthusiasm for matching server parity.

## Decision

Adopt **Option C (Hybrid-minimal)**: introduce a durable local memory-graph **substrate** plus the deltas that work **without an LLM**, and gate the LLM-dependent deltas behind the existing optional local-LLM path with explicit graceful degradation. Defer the lowest-value delta.

### 1. Durable graph substrate (no LLM required)

Add a SQLite schema migration (`v34_memory_graph`, advancing `CURRENT_VERSION` to 34) following the `v31` precedent:

```sql
-- claim/thesis nodes (durable; survive digest regeneration)
CREATE TABLE memory_claims (
  claim_id     TEXT PRIMARY KEY,   -- generate_id("clm") per ADR-022
  kind         TEXT NOT NULL,      -- D5 taxonomy: semantic|episodic|procedural|reflective
  text         TEXT NOT NULL,
  source       TEXT NOT NULL,      -- e.g. "digest_highlight", "pattern_miner", "llm"
  confidence   REAL NOT NULL,
  status       TEXT NOT NULL,      -- active|superseded|retracted
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);

-- typed edges between memory units (claims and/or segment_id-keyed units)
CREATE TABLE memory_edges (
  edge_id      TEXT PRIMARY KEY,   -- generate_id("edg")
  src_id       TEXT NOT NULL,      -- claim_id or segment_id
  dst_id       TEXT NOT NULL,
  edge_type    TEXT NOT NULL,      -- evidence|supports|refines|contradicts (D1)
  confidence   REAL NOT NULL,
  evidence_ref TEXT,               -- optional segment_id/frame_id provenance
  source       TEXT NOT NULL,      -- "rule" | "llm"
  created_at   INTEGER NOT NULL
);
CREATE INDEX idx_memory_edges_src ON memory_edges(src_id, edge_type);
CREATE INDEX idx_memory_edges_dst ON memory_edges(dst_id, edge_type);
```

Define a `MemoryGraphPort` trait in `maekon-core/src/ports/` (async, `&self`, per ADR-001), implemented by `SqliteStorage` (which already aggregates 10+ disjoint port traits). Node IDs use `generate_id` (ADR-022) with new prefixes `clm` and `edg` (to be registered in ADR-022's prefix table).

**Rationale**: The substrate is a single migration + one port, reuses `segment_id` as node identity, and adds no LLM dependency. Existing `embedding_vectors`/FTS5 give the new nodes vector and lexical recall without extra work.

### 2. Phase 1 deltas — durable, LLM-free (ship first)

- **D3 durable claim nodes + evidence edges**: Promote daily-digest content into persisted `memory_claims` rows, each with an `evidence`-typed `memory_edges` row to its source `segment_id`. The **offline** source is the digest **timeline** (`TimelineEntry`, always built from closed segments by `DailyDigestGenerator` even with no LLM, and carrying a guaranteed `segment_id`) → an `episodic` claim + evidence edge; this is what makes the graph non-empty on every install. When an LLM insight ran, each `DigestHighlight` is additionally promoted (kind by D5 rule; evidence edge when its optional `segment_id` is present). This turns regenerated-every-run digest content into durable, queryable nodes. `DigestExporter::to_markdown_with_claims` renders the accumulated claims as an LLM-Wiki-style local second-brain — fully offline, no LLM. Implemented in `crates/maekon-analysis/src/claim_promoter.rs` (pure builder) + the scheduler aggregation loop.
- **D5 cognitive taxonomy**: Implement the `kind` column (`semantic|episodic|procedural|reflective`) as a cheap, rule-assigned tag at claim-creation time. Implemented rule: timeline-derived claims → `episodic`; LLM highlights → `Achievement`→`episodic`, `Warning`→`reflective`, `Suggestion`→`procedural` (total over `HighlightType`). No LLM required.
- **Rule-seeded `Associated` edges (#4441 box 5 — landed)**: consecutive timeline claims are linked by a rule-seeded `Associated` edge (`source = "rule"`, `src` = earlier → `dst` = later), realizing the app-**sequence** signal from the production-available digest timeline ORDER. This deliberately uses the timeline rather than `SegmentSummary.patterns_detected` (which the scheduler path leaves empty) and avoids app-node identity — `EdgeType::Associated` connects existing claim nodes, not synthetic app nodes. App-name **co-occurrence** edges (which would need app nodes or a schema change) remain out of scope.

### 3. Phase 2 deltas — LLM-gated (ship only behind a configured local LLM)

When and only when a local LLM is configured (`config.ai_provider.llm_api.is_some()`, including local Ollama):

- **D1 typed epistemic edges** (`supports`/`refines`/`contradicts`): generated by a new relation-extraction component in `maekon-analysis` behind the `AnalysisProvider` port, generalizing the JSON-structured extraction `DailyInsightGenerator` already performs.
- **D2 contradiction detection**: a contradiction pass over claim pairs (vector-recall candidates → LLM judge) emitting `contradicts` edges. **Belief revision is shallow and opt-in**: a contradicted claim transitions `status` `active → superseded` (or confidence decay), recorded as a new node/edge with provenance. It does **not** alter `RegimeManager` merge (ADR-012/ADR-018 remain untouched); belief revision operates only over `memory_claims`, never over regimes.

**Degradation contract**: When no LLM is configured, Phase-2 components degrade through `FallbackAnalysisProvider → NoOpAnalysisProvider` exactly like existing LLM steps (suggestions, segment narratives, daily insight). The graph is **never silently empty**, because the Phase-1 substrate (D3+D5) is populated by rule-based paths regardless. This is the key correctness property that distinguishes C from B.

### 4. Explicitly deferred

- **D4 near-miss / contrastive edges**: deferred. Lowest standalone-UX value per effort; no existing seed signal; revisit only if Phase 1+2 land and user feedback demands contrastive recall.
- **Belief revision over regimes**: explicitly out of scope. ADR-012's destructive centroid merge is unchanged; this ADR adds no provenance/split to regimes.
- **Traversal-first retrieval**: Phase 1/2 keep RRF hybrid search as the primary retriever; edges augment (re-rank/expand), they do not replace `hybrid_search_service`.

## Consequences

### Positive

- A durable, revisable, **offline** knowledge layer (D3+D5) exists on default installs with **no LLM dependency**, rendered to markdown via the existing `DigestExporter` — a real standalone second-brain, not just retrieval.
- The substrate (tables + port) is one v34 migration on a proven precedent (`v31`), reuses `segment_id` node identity, and respects ADR-001 hexagonal boundaries (`maekon-analysis → maekon-core` only).
- D1/D2 layer onto the **same** substrate and the **same** degradable `AnalysisProvider` port when a local LLM (Ollama) is present — full neural-grade graph offline, with an honest NoOp fallback rather than a misleading empty graph.
- No change to ADR-012/ADR-018 regime lifecycle; lower blast radius than Option B.

### Negative

- Two-phase delivery: the richest deltas (D1 typed edges, D2 contradiction) are absent on default no-LLM installs. Mitigated by the non-empty Phase-1 baseline and clear gating.
- New `memory_claims`/`memory_edges` tables add migration and maintenance surface vs Option A (do nothing).
- Phase-1 belief revision is intentionally shallow (supersede / confidence decay), not true multi-hypothesis revision.
- D4 stays unaddressed; one benchmark delta remains open.

### Neutral

- The server `ontology_reasoning`/`knowledge_management` domains remain orthogonal; this layer neither depends on nor benefits from them, and is suppressed by no server-coexistence rule (it is purely local).
- New ID prefixes `clm`/`edg` must be registered in the ADR-022 prefix registry.
- First-run ONNX weight download (already the one network touch for "local" embeddings) is unchanged; the graph layer itself adds no network I/O.

## Alternatives Considered

**A. Keep vector/regime-only memory (do nothing).** Rejected as the long-term answer. Today's offline UX (semantic recall + digests) is genuinely useful but caps at retrieval — no durable, revisable knowledge can accumulate, and the only parity path (server domains) gives zero standalone benefit. Acceptable as a fallback if Phase 1 proves unwanted, but it forecloses the second-brain UX entirely.

**B. Full local symbolic graph at server parity (all of D1–D5, traversal retrieval, belief revision replacing regime merge).** Rejected as gold-plating. Its load-bearing primitives (typed-edge inference, contradiction judging, belief revision) all hard-depend on the **optional** local LLM, so on the default no-LLM install most of the graph would be silently empty for exactly the offline users it targets. Rewriting regime merge into belief revision adds migration/provenance risk against ADR-012/ADR-018 for marginal benefit, and D4 in particular has a poor effort/value ratio. Too much net-new surface (nothing client-side to extend) for the conditional payoff.

**C. Hybrid-minimal (chosen).** Builds the durable substrate once, ships the LLM-free deltas (D3+D5) first so standalone UX improves on every install, and gates the LLM-dependent deltas (D1/D2) behind the existing degradable local-LLM path — with the substrate guaranteeing the graph is never silently empty. Defers the lowest-value delta (D4) and leaves regime lifecycle untouched. Minimal design that materially improves server-less usefulness without gold-plating.

## Acceptance Criteria

1. `v34_memory_graph` migration creates `memory_claims` + `memory_edges` (with `src`/`dst` indexes); `CURRENT_VERSION = 34`; round-trip migration test passes from a v33 DB.
2. `MemoryGraphPort` defined in `maekon-core/src/ports/`, implemented by `SqliteStorage`; `clm`/`edg` prefixes registered in ADR-022.
3. **Phase 1, no LLM (NoOpAnalysisProvider wired):** running a digest produces ≥1 persisted `memory_claims` row with an `evidence` edge to its `segment_id`, each claim carries a valid `kind` (D5) tag, and `DigestExporter::to_markdown` renders the accumulated claims. Verified offline with no `llm_api` configured.
4. **Phase 1 durability:** claims persist across digest regeneration (a re-run does not delete or duplicate prior active claims).
5. **Phase 2, with local Ollama configured:** the relation-extraction pass emits `supports`/`refines`/`contradicts` edges (D1), and a contradiction over a claim pair transitions the loser to `status = superseded` with a provenance edge (D2) — without modifying any `Regime` row.
6. **Degradation:** with no LLM configured, Phase-2 components return cleanly via `FallbackAnalysisProvider → NoOpAnalysisProvider` (no panic), and the Phase-1 graph remains non-empty.
7. `maekon-analysis` retains its no-network-dependency invariant (no `reqwest`/`maekon-network` added); hexagonal boundary (`→ maekon-core` only) preserved. `cargo check/test/clippy/fmt` pass per `docs/STATUS.md`.
8. ADR-012/ADR-018 regime merge behavior is unchanged (regression test on `RegimeManager::merge_two`).
9. ADR appears in `docs/architecture/README.md`; Korean companion `ADR-023-local-symbolic-memory-graph.ko.md` authored.

## PII Mitigations

The memory-graph paths introduce three points where user context text crosses an
LLM boundary. Each point has an assigned mitigation identifier used throughout
the codebase (`MG-PII-NN`).

| ID | Boundary | Mitigation |
|----|----------|------------|
| **MG-PII-01** | `BeliefRevision` claim text sent to enrichment provider | Per-claim masking applied inside `BeliefRevision` before each claim's text is serialised for the provider. Implemented in `crates/maekon-analysis/src/belief_revision.rs`. |
| **MG-PII-02** | `AnalysisClient::new_local_enrichment` — enrichment egress gate | DNS-rebind-hardened loopback pin: resolves the endpoint host once, asserts **every** resolved address is loopback. Non-loopback endpoints are refused at construction time (fail-closed). Implemented in `crates/maekon-network/src/analysis_client/mod.rs`. |
| **MG-PII-03** | `PiiFilter` boundary masker applied by `BeliefRevision` | Same `PiiFilter` + `VisionPiiSanitizer` stack used by the primary analysis path. Applied before send to the enrichment provider. |
| **MG-PII-04** | `build_local_ollama_summary_provider` — segment-summary loopback path | The segment-summary path is **not** routed through `GuardedAnalysisProvider` (the active-window gate + `privacy.external_llm.*` audit events). Rationale: ① loopback = device-local egress boundary (MG-PII-02 precedent, `new_local_enrichment` fail-closed gate enforced); ② `pii_filter_summ` + `VisionPiiSanitizer` are applied identically to the primary path; ③ `llm_summary_enabled` + `embedding.enabled` double opt-in (both default `false`). Residual delta honestly stated: unlike the in-process rule-matcher, **prompt text may persist in the Ollama daemon log** across process restart. This delta is accepted because Ollama runs under the same OS user as Maekon and the loopback boundary prevents off-device egress. Implemented in `src-tauri/src/agent_runtime/analysis_helpers.rs`. |

## Related Docs

- Andrej Karpathy, *LLM Wiki* — the canonical pattern this ADR benchmarks (local, LLM-maintained markdown knowledge base): https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f
- `docs/architecture/ADR-011-standalone-analysis-pipeline.md` — server-optional pipeline baseline
- `docs/architecture/ADR-012-adaptive-tiered-memory.md` — regime/segment lifecycle (unchanged here)
- `docs/architecture/ADR-013-llm-summary-vector-rag.md` — embeddings + RRF retrieval the graph augments
- `docs/architecture/ADR-018-regime-manager-persistence.md` — `v31` migration precedent
- `docs/architecture/ADR-022-client-id-generation-ulid.md` — `generate_id` + prefix registry (`clm`/`edg`)
- `crates/maekon-analysis/src/claim_promoter.rs` — Phase-1 D3/D5 promoter (digest timeline + LLM highlights → claims/edges)
- `crates/maekon-analysis/src/daily_insight_generator.rs` — LLM `DigestHighlight` source (Phase-1 LLM-on path only; offline D3 uses the timeline)
- `crates/maekon-analysis/src/fallback_analysis_provider.rs` — NoOp degradation contract
