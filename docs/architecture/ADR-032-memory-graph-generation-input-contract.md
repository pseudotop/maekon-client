[English](./ADR-032-memory-graph-generation-input-contract.md) | [한국어](./ADR-032-memory-graph-generation-input-contract.ko.md)

# ADR-032: Memory-Graph Generation-Input Contract

**Status**: Accepted — amended 2026-07-29 after 3-loop review (#9463); originally Proposed 2026-07-25
**Date**: 2026-07-25 (Proposed) · 2026-07-29 (Accepted)
**Scope**: `maekon-analysis` (retrieval, coaching, belief revision), `maekon-suggestion`, `maekon-core` (`ports/memory_graph_port.rs`, `consent.rs`, `models/prompt_assembly.rs`), `maekon-network` (`analysis_client`), `src-tauri` (agent runtime wiring)
**Related**: ADR-023 (Local Symbolic Memory-Graph), ADR-030 (Work Context Envelope) §11, ADR-024 (Conversation Content Guard Port), ADR-026 (ConsentManagerPort), ADR-013 (LLM Summary + Vector RAG), ADR-012 (Adaptive Tiered Memory), ADR-011 (Standalone Analysis Pipeline)

---

## Context

ADR-023 landed the local symbolic memory-graph substrate (`memory_claims`, `memory_edges`, `MemoryGraphPort`) and the belief-revision loop. Its §5 then recorded an explicit boundary: **claims do not feed generation.** A 2026-07 audit found `coaching_engine` and `maekon-suggestion` hold zero `MemoryGraphPort` references, and even the ADR-aligned retrieval augmentation is unwired. ADR-023 §5 deferred "claims-as-generation-input" as a *new, unratified design* requiring its own ADR. ADR-030 §11 independently states that memory-graph generation policy "remains owned by #8087." This ADR discharges that ownership.

A 2026-07-25 code survey confirms the deferred state still holds, and establishes the baseline this contract must extend rather than contradict:

- **Exactly one LLM path exists, and it is a closed loop.** `BeliefRevision::run_pass` (`crates/maekon-analysis/src/belief_revision.rs:85-214`) reads `list_claims_by_status(Active)`, sends claim text to an enrichment provider, and writes the result back as edges and status flips. Nothing leaves the graph for a user-visible generation.
- **Every other read is display or maintenance.** `handlers/daily_digest.rs:76-85` and `services/memory_claims_service.rs` render claims to the local dashboard; `scheduler/loops/system.rs:805-812` prunes them. `ContextAssembler` (`assembler.rs`), `prompts.rs`, `few_shot_selector.rs`, `query_expander.rs`, and `vector_retriever.rs` contain **zero** memory-graph references. (The Proposed draft also cited `hybrid_search_service.rs` here; that file had already been deleted as dead code in PR #5770 — the live retrieval path is `semantic_search_service.rs` in `maekon-web`, which likewise holds zero memory-graph references. See Known Follow-up 2.)
- **The one path is triple-gated off by default**: `belief_revision_enabled = false` (`config/sections/analysis.rs:64`) AND `ConsentPermissions.memory_graph_enrichment = false` (`consent.rs:56-63`, a Tier-7 permission that is deliberately not inherited from `full_text_extraction` or `activity_pattern_learning`) AND `NoOpAnalysisProvider` when `llm_api` is unset.
- **The one path is loopback-pinned, and that is what buys its exemptions.** `AnalysisClient::new_local_enrichment` (`analysis_client/mod.rs:232-247`) refuses non-loopback endpoints at construction with a DNS-rebind-hardened resolve-and-assert, and `extract_relations`/`detect_contradictions` re-check before each send. Because egress is device-local, ADR-023 MG-PII-04 accepts bypassing `GuardedAnalysisProvider`, and **no egress-ledger entry is written** — `crates/maekon-network/src/analysis_client/` contains zero `record_egress` references, while every genuinely off-device path (`scheduler/egress_policy.rs`, `guarded_conversation.rs`, `remote_embedding_client.rs`) does write one.
- **What is projected today is narrow**: `belief_revision.rs:100-104` serialises `[(claim_id, pii_masked_text)]` only. `kind`, `source`, `confidence`, `status`, timestamps, and **all edge data** stay in-process.
- **Selection is unbounded in the dimensions that matter for prompts**: status is filtered to `Active`, but there is no count cap, no token budget, no recency window, and no input-side confidence floor. The only lower bound is `active.len() < 2 → return`.

These facts define the risk asymmetry this ADR must resolve. The existing loop is safe *because* it is closed, local, and off by default — not because claim text is intrinsically low-risk. Claim text is user-derived content distilled from screen activity; routing it into a user-visible generation crosses a boundary the current design never crosses, and routing it to a remote provider would additionally void every exemption the loopback pin currently earns.

ADR-023 §5 further names three designs that are routinely conflated: retrieval re-rank, prompt-context injection, and a coaching gate signal. They have materially different privacy surfaces and must not share a single approval.

## Decision

Adopt a **mode-separated, fail-closed generation-input contract**. No runtime change accompanies this ADR; it pins the contract that any future consumer must satisfy before wiring.

### 1. Three modes, separately gated, ordered by disclosure

| Mode | What the mode reads and uses | Permitted disclosure |
|------|------------------------------|----------------------|
| **A — Retrieval augmentation** | Edge topology (`src_id`, `dst_id`, `edge_type`, `confidence`) read **in-process** to re-rank or expand an existing retrieval result set (live path: `semantic_search_service` in `maekon-web`) | **Nothing reaches any generator.** Ranking influence only; endpoint identifiers are join keys that never leave the ranking computation (§2.6). |
| **B — Gate signal** | A derived scalar or boolean (e.g. "≥N active contradictions in the last 7 days") | No claim text, no IDs. |
| **C — Prompt-context injection** | PII-masked claim text inside a prompt | Full text disclosure to the generator. |

Modes must be adopted in the order **A → B → C**. Each requires its own activation decision, its own consent evaluation (§3), and its own contract tests. **Approval of one mode never implies another.** Mode A is the design ADR-023 already ratified as the intended read path; Mode C is the one ADR-023 §5 called unratified, and it inherits the strictest requirements below.

Mode A is *generator-adjacent*, not generator-facing: it shapes which retrieval results later stages see, but no projected value is itself sent to an LLM. It touches claim rows only to resolve edge endpoints (`claim_id`s); claim `text`, `kind`, and `source` are never read in Mode A.

**Rationale**: the three differ by *what content crosses the boundary*, not by implementation convenience. Collapsing them into a single "use the graph for generation" switch is exactly the ad-hoc introduction ADR-023 §5 refused.

### 2. Bounded projection (input selection)

Any mode that reads the graph for generation MUST select through a single shared projection helper, not by calling `MemoryGraphPort` directly from a consumer.

**Where the helper lives.** Its public interface is a `maekon-core` port trait (working name `MemoryGraphProjectionPort`), its bounded-selection implementation lives in `maekon-analysis`, and `src-tauri` wires it via DI (Port Instance Sharing, exactly as ADR-023's web-render threaded `MemoryGraphPort` through `WebServerRequiredDeps`). Adapter consumers — `maekon-web` (which owns the live retrieval path and has no `maekon-analysis` dependency), `maekon-suggestion`, coaching — depend only on the trait; cross-adapter crate dependencies remain forbidden. Each approved mode adds its **own trait method with its own bounded return type** (type-level mode separation — no mode enum, so approving one method can never widen another mode's disclosure). Sketch for Mode A:

```rust
#[async_trait]
pub trait MemoryGraphProjectionPort: Send + Sync {
    /// Mode A: bounded edge-topology projection for in-process ranking.
    async fn project_edges_for_ranking(&self, now_secs: i64) -> Result<EdgeProjection, CoreError>;
}
```

**Fail-closed, precisely scoped.** If a *bound cannot be evaluated* — missing or invalid config value, unavailable consent authority, unresolvable window — the helper returns `Ok` with an **empty projection**. Genuine storage failures (`MemoryGraphPort` returning `Err(CoreError)`) propagate as `Err` unchanged; they MUST NOT be masked into empty success, otherwise contract tests could not distinguish "denied by policy" from "broken storage". Both behaviours are contract-tested (Known Follow-up 1).

The projection MUST enforce all of:

1. **Status**: `Active` only. `Superseded` and `Retracted` are excluded at selection time, never merely filtered downstream.
2. **Recency window**: a bounded `updated_at` window — `analysis.memory_graph_projection.generation_window_days`, starting default **30**. The ADR-023 retention prune (`analysis.embedding.retention_days`, default 90 — `scheduler/loops/system.rs:799-821`) is a storage floor, **not** a generation window; the generation window is independently configured and MUST be ≤ the retention window.
3. **Confidence floor**: an input-side minimum — `min_input_confidence`, starting default **0.5**. Distinct from `supersede_confidence_threshold` (0.9), which is an *output*-side gate on belief revision and MUST NOT be reused as the input floor.
4. **Hard count cap** with deterministic total ordering — `max_claims` (starting default **64**) for claim selection, ordered `updated_at DESC`, `claim_id` tie-break (the ordering `memory_claims_service.rs:124-128` already uses); `max_edges` (starting default **256**) for edge selection, ordered `created_at DESC`, `edge_id` tie-break. Ordering must be total so a generation is reproducible from the same graph state — reproducibility holds because every bound is config-pinned, never implementer-chosen.
5. **Field allowlist**: `claim_id`, PII-masked `text`, `kind`. Denied: `source`, raw `confidence`, `evidence_ref`, and any `segment_id`/`frame_id` provenance. Provenance identifiers are internal correlation keys and MUST NOT reach a generator. (Mode A resolves edge endpoints against claim rows for `claim_id` only; `text`/`kind`/`source` are not read in Mode A.)
6. **Edge projection** (Mode A only): the projected tuple is (`src_id`, `dst_id`, `edge_type`, `confidence`) — endpoints included, because they are the join keys that make ranking possible (for `Evidence` edges, `dst_id` may reference a `segment_id`, which is exactly how edges join to `SemanticSearchResult.segment_id` rows). This does not conflict with §2.5's provenance ban: §2.5 governs what may reach a generator or persist into generated output, whereas edge endpoints are consumed *inside* the ranking computation and MUST NOT be disclosed beyond it. `evidence_ref` may not be projected in any mode.

All bounds live in one named config section: `analysis.memory_graph_projection` (`MemoryGraphProjectionConfig`, a sibling of `embedding` inside `AnalysisConfig`). The starting defaults above are contractual starting values — tunable via config, but the fields MUST exist, be enforced, and be covered by the fail-closed contract tests; three mode consumers inventing three config paths would void §2's single-helper guarantee.

**Rationale**: the current belief-revision selection has no count or recency bound because its consumer is a local self-maintenance pass whose cost is a daily local LLM call. A generation consumer has a token budget, a latency budget, and a disclosure surface, so the bounds become load-bearing rather than incidental.

### 3. Privacy boundary

1. **Masking is a projection-time invariant.** PII masking (ADR-023 MG-PII-01/MG-PII-03) MUST be applied inside the projection helper for every text-projecting mode (today: Mode C), at the injected `maekon_core::ports::pii_sanitizer::PiiSanitizer` seam — the workspace's cross-crate masking port, already consumed by `semantic_search_service`. (Belief revision's private `PiiFilter` closure alias, which the Proposed draft named, is an analysis-internal convenience, not this contract's seam.) No consumer can obtain unmasked claim text. Mode A and Mode B project no text, so the masking clause is vacuously satisfied for them — stated explicitly so no implementer infers Mode A needs a sanitizer dependency. Masking at the call site is not acceptable — that is the shape that lets a new consumer silently skip it.
2. **Consent is per-purpose, per-mode, and dedicated.** `memory_graph_enrichment` (Tier 7) authorises the *self-maintenance* loop (graph → LLM → graph) and nothing else. Every generation-input mode requires its **own dedicated `ConsentPermissions` boolean**, `#[serde(default)]` (fail-closed `false`), following the one-capability-one-permission convention Tiers 4–9 establish — each field's doc comment states what it is NOT borrowed from. Pinned names: Mode A → `memory_graph_retrieval_ranking` (Tier 10), Mode B → `memory_graph_gate_signal` (Tier 11), Mode C → `memory_graph_prompt_injection` (Tier 12). Each field lands in the PR that ships its mode, with a doc comment citing this ADR; no mode may borrow or "extend" a sibling permission. (The Proposed draft allowed Mode A/B to extend an existing permission; the 2026-07-29 review struck that clause as the same purpose-creep Alternative C rejects.)
3. **Remote egress voids the MG-PII-04 exemption.** The current bypass of `GuardedAnalysisProvider` and the absence of an egress-ledger entry are justified solely by the loopback pin (`host_is_loopback`, `http_client.rs:79-98`). Therefore:
   - Local (loopback) generation input MAY reuse the existing exemption, unchanged.
   - **Any generation input that can reach a non-loopback provider MUST route through `GuardedAnalysisProvider` (ADR-024) and MUST write an egress-ledger entry** with a registered `event_type` before the send. A remote path without a ledger entry is a contract violation, not a gap.
4. **Trust boundary in prompts.** Claim text is user-derived. In Mode C it MUST be wrapped as `UntrustedContent` via `models/prompt_assembly.rs` and MUST NOT appear in a `TrustedInstruction` segment. Each projected claim is wrapped as its **own** `UntrustedContent` (label = its `claim_id`) rather than concatenated into one blob, so provenance stays per-claim traceable — §4.2's re-evaluation duty depends on it. This closes an existing shape gap: the belief-revision path builds provider bodies directly (`analysis_client/requests.rs:6-31`) without the segmented-prompt wrapper, which is tolerable for a closed loop but not for a path whose output reaches the user.
5. **Device-local invariant preserved.** `memory_claims`/`memory_edges` remain excluded from cross-device sync (`sync_extractor.rs:66-68`). No generation-input mode may introduce a sync or upload path for graph rows.

**Rationale**: each clause names the specific exemption it is protecting. The loopback pin is the load-bearing fact behind two separate ADR-023 concessions, so the contract states plainly what happens when that fact stops holding.

### 4. Staleness and invalidation

1. **No projected-claim caching across passes.** A projection is valid for the single generation that consumed it. Retraction and supersession MUST take effect on the next generation without an explicit cache flush.
2. **Retraction is user-visible and immediate in effect.** `POST /api/memory/claims/{id}/retract` (`handlers/memory_claims.rs:58-72`) flips status rather than deleting, preserving provenance. Because §2.1 excludes non-`Active` at selection, retraction removes a claim from all subsequent generations. A generation already emitted is not retroactively invalidated; if a consumer persists generated output that quoted a claim, it MUST record the `claim_id` so the output can be re-evaluated.
3. **Contradiction without supersession does not silently qualify.** A claim carrying an inbound `Contradicts` edge whose belief-revision pass has not yet run (or ran below `supersede_confidence_threshold`) is still `Active`. Mode C MUST exclude claims with unresolved inbound `Contradicts` edges; Modes A and B MAY include them, since neither discloses text.
4. **Retention is a floor, not a policy.** The 90-day prune bounds storage. It does not authorise a 90-day generation window (§2.2).

## Consequences

### Positive

- A future consumer has an executable checklist instead of an open question; "wire the graph into suggestions" stops being a one-line change with an unbounded blast radius.
- The loopback exemptions ADR-023 granted are now explicitly conditional, so extending the graph to a remote provider cannot silently inherit them.
- Mode separation lets the cheapest, already-ratified win (Mode A retrieval augmentation) ship without dragging Mode C's consent and egress requirements along.

### Negative

- Three modes mean three activation decisions and three test surfaces; a consumer that genuinely needs Mode C pays the full cost rather than a partial one.
- The projection helper is indirection that does not exist yet, so the first consumer bears its implementation cost.
- Excluding `evidence_ref` from every mode forecloses provenance-cited generation ("this suggestion is based on your 3pm session") without a follow-up ADR.

### Neutral

- No runtime behaviour changes on adoption; the graph remains display-and-belief-revision only until a consumer is separately approved.
- The contract constrains a consumer that does not exist, so its first real test is the first consumer, not this ADR.

## Alternatives Considered

**A. Amend ADR-023 instead of a new ADR.** Rejected. ADR-023 is `Accepted — fully implemented`; grafting an unimplemented forward contract onto it makes that status ambiguous. The contract is also cross-cutting — it binds ADR-024 (guard/egress), ADR-026 (consent), and ADR-013 (retrieval) — rather than being a memory-graph substrate decision. ADR-030 §11 already treats it as a separately owned policy.

**B. A single "generation input" switch covering all three designs.** Rejected. This is precisely the ad-hoc introduction ADR-023 §5 declined, and it would let Mode A's low-risk approval carry Mode C's text disclosure.

**C. Reuse `memory_graph_enrichment` for generation input.** Rejected. That permission was scoped to the closed self-maintenance loop. Purpose-creep on a consent permission is a privacy regression regardless of the technical similarity of the read.

**D. Extend the MG-PII-04 `GuardedAnalysisProvider` bypass to any generation path.** Rejected. The bypass is justified by the loopback boundary, not by the analysis pipeline's identity. Off-device egress without an audit trail would leave the graph as the only LLM-adjacent surface with no ledger entry.

**E. Wire Mode A now as part of this ADR.** Rejected for scope. ADR-023 already ratifies retrieval augmentation; wiring it is an implementation task that this contract governs but does not need to contain.

## Known Follow-ups

1. **Projection helper implementation** — `MemoryGraphProjectionPort` trait in `maekon-core`, bounded-selection implementation in `maekon-analysis`, DI wiring in `src-tauri`; consumers depend on the trait only (§2). The same PR MUST add `analysis.memory_graph_projection` (`MemoryGraphProjectionConfig`) with the §2 starting defaults, plus contract tests asserting both fail-closed semantics: unevaluable bound → `Ok(empty)`, storage error → `Err`. Prerequisite for any mode.
2. **Mode A wiring (retrieval augmentation)** — edges re-rank/expand the **live** retrieval path: `crates/maekon-web/src/services/semantic_search_service.rs` (`vector_search` / `adaptive_vector_search` / `fuse_keyword_first`), whose results are keyed by `segment_id`. (`HybridSearchService` — the path ADR-023 §4 scoped and the Proposed draft cited — was deleted as dead code in PR #5770, commit `54ce99de46`; ADR-013's RRF fusion now lives in `fuse_keyword_first`.) The join is `EdgeType::Evidence` edges whose `dst_id` references a `segment_id`. The Mode A PR MUST first measure join coverage — how many `Active` claims carry an `Evidence` edge to a still-resolvable `segment_id`. If coverage is negligible, Mode A yields no ranking change; that is an acceptable fail-closed outcome to report honestly, not a defect to paper over. Mode A also lands `memory_graph_retrieval_ranking` (Tier 10, §3.2).
3. **Egress-ledger event type for remote graph egress** — required by §3.3 before any non-loopback mode; today no `memory_graph`/`belief_revision` event type exists in the ledger.
4. **Segmented-prompt adoption on the belief-revision path** — belief revision builds provider bodies directly today. Not a defect for a closed loop, but adopting `prompt_assembly` there first would let Mode C reuse a proven seam.
5. **Provenance-cited generation** — if citing evidence (`evidence_ref`, `segment_id`) in user-visible output is later wanted, it needs its own ADR; §2.5 forecloses it deliberately.
6. **Stale retrieval-path references** — `CLAUDE.md`'s crate summary and `docs/crates/maekon-analysis.md` are corrected alongside this amendment; prose in ADR-013/ADR-023 still names `hybrid_search_service`. The historical ADRs stay as written (they predate PR #5770), but new documents must cite `semantic_search_service`.

## Amendment History

- **2026-07-29 (#9463, 3-loop review: devils-advocate + implementer lens; all findings BLOCKING/IMPORTANT folded in):**
  1. §3.2 rewritten — dedicated per-mode consent permissions (Tiers 10–12, pinned names) replace the struck "extension of an existing permission" clause, restoring consistency with Alternative C and the Tiers 4–9 one-capability-one-permission convention.
  2. §2 bounds pinned — named config section `analysis.memory_graph_projection` with contractual starting defaults (window 30d ≤ retention, floor 0.5, caps 64/256) and total edge ordering.
  3. Projection helper located — `maekon-core` port trait + `maekon-analysis` implementation + `src-tauri` DI; consumers depend on the trait only (the Proposed "single `maekon-analysis` seam" wording was structurally uncallable from `maekon-web`, which has no `maekon-analysis` dependency).
  4. Mode A integration point corrected — `HybridSearchService` was already deleted (PR #5770); the live seam is `semantic_search_service`, join key `Evidence.dst_id → segment_id`, with a mandatory join-coverage measurement in the Mode A PR.
  5. §1 vs §2.5/§2.6 endpoint-disclosure contradiction resolved — endpoints are in-process join keys, never generator-visible; Mode A touches claim rows for `claim_id` only.
  6. Fail-closed semantics split — unevaluable bound → `Ok(empty)`; storage error → `Err`; port signature sketched with type-level mode separation.
  7. §3.1 masking seam corrected to the `PiiSanitizer` core port; Mode A/B explicitly carved out (no text projected).
  8. §3.4 per-claim `UntrustedContent` wrapping pinned for Mode C traceability.

## Related Docs

- `docs/architecture/ADR-023-local-symbolic-memory-graph.md` §4-§5 — substrate, intended read path, and the deferral this ADR discharges
- `docs/architecture/ADR-030-work-context-envelope-convergence.md` §11 — hands memory-graph generation policy to #8087
- `docs/architecture/ADR-024-conversation-content-guard-port.md` — guard + egress-audit pattern required by §3.3
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md` — `ConsentManagerPort`, home of the gate in §3.2
- `docs/architecture/ADR-013-llm-summary-vector-rag.md` — the RRF retrieval Mode A augments (its fusion now lives in `semantic_search_service::fuse_keyword_first`; see Known Follow-up 2)
- `crates/maekon-web/src/services/semantic_search_service.rs` — Mode A's live integration point (`segment_id`-keyed results)
- `crates/maekon-analysis/src/belief_revision.rs` — the single existing LLM path and its selection/masking baseline
- `crates/maekon-core/src/ports/pii_sanitizer.rs` — the cross-crate masking seam required by §3.1
- `crates/maekon-core/src/models/prompt_assembly.rs` — trust-boundary wrapper required by §3.4
