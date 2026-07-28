[English](./ADR-032-memory-graph-generation-input-contract.md) | [한국어](./ADR-032-memory-graph-generation-input-contract.ko.md)

# ADR-032: Memory-Graph Generation-Input Contract

**Status**: Proposed
**Date**: 2026-07-25
**Scope**: `maekon-analysis` (retrieval, coaching, belief revision), `maekon-suggestion`, `maekon-core` (`ports/memory_graph_port.rs`, `consent.rs`, `models/prompt_assembly.rs`), `maekon-network` (`analysis_client`), `src-tauri` (agent runtime wiring)
**Related**: ADR-023 (Local Symbolic Memory-Graph), ADR-030 (Work Context Envelope) §11, ADR-024 (Conversation Content Guard Port), ADR-026 (ConsentManagerPort), ADR-013 (LLM Summary + Vector RAG), ADR-012 (Adaptive Tiered Memory), ADR-011 (Standalone Analysis Pipeline)

---

## Context

ADR-023 landed the local symbolic memory-graph substrate (`memory_claims`, `memory_edges`, `MemoryGraphPort`) and the belief-revision loop. Its §5 then recorded an explicit boundary: **claims do not feed generation.** A 2026-07 audit found `coaching_engine` and `maekon-suggestion` hold zero `MemoryGraphPort` references, and even the ADR-aligned retrieval augmentation is unwired. ADR-023 §5 deferred "claims-as-generation-input" as a *new, unratified design* requiring its own ADR. ADR-030 §11 independently states that memory-graph generation policy "remains owned by #8087." This ADR discharges that ownership.

A 2026-07-25 code survey confirms the deferred state still holds, and establishes the baseline this contract must extend rather than contradict:

- **Exactly one LLM path exists, and it is a closed loop.** `BeliefRevision::run_pass` (`crates/maekon-analysis/src/belief_revision.rs:85-214`) reads `list_claims_by_status(Active)`, sends claim text to an enrichment provider, and writes the result back as edges and status flips. Nothing leaves the graph for a user-visible generation.
- **Every other read is display or maintenance.** `handlers/daily_digest.rs:76-85` and `services/memory_claims_service.rs` render claims to the local dashboard; `scheduler/loops/system.rs:805-812` prunes them. `ContextAssembler` (`assembler.rs`), `prompts.rs`, `few_shot_selector.rs`, `query_expander.rs`, `hybrid_search_service.rs`, and `vector_retriever.rs` contain **zero** memory-graph references.
- **The one path is triple-gated off by default**: `belief_revision_enabled = false` (`config/sections/analysis.rs:64`) AND `ConsentPermissions.memory_graph_enrichment = false` (`consent.rs:56-63`, a Tier-7 permission that is deliberately not inherited from `full_text_extraction` or `activity_pattern_learning`) AND `NoOpAnalysisProvider` when `llm_api` is unset.
- **The one path is loopback-pinned, and that is what buys its exemptions.** `AnalysisClient::new_local_enrichment` (`analysis_client/mod.rs:232-247`) refuses non-loopback endpoints at construction with a DNS-rebind-hardened resolve-and-assert, and `extract_relations`/`detect_contradictions` re-check before each send. Because egress is device-local, ADR-023 MG-PII-04 accepts bypassing `GuardedAnalysisProvider`, and **no egress-ledger entry is written** — `crates/maekon-network/src/analysis_client/` contains zero `record_egress` references, while every genuinely off-device path (`scheduler/egress_policy.rs`, `guarded_conversation.rs`, `remote_embedding_client.rs`) does write one.
- **What is projected today is narrow**: `belief_revision.rs:100-104` serialises `[(claim_id, pii_masked_text)]` only. `kind`, `source`, `confidence`, `status`, timestamps, and **all edge data** stay in-process.
- **Selection is unbounded in the dimensions that matter for prompts**: status is filtered to `Active`, but there is no count cap, no token budget, no recency window, and no input-side confidence floor. The only lower bound is `active.len() < 2 → return`.

These facts define the risk asymmetry this ADR must resolve. The existing loop is safe *because* it is closed, local, and off by default — not because claim text is intrinsically low-risk. Claim text is user-derived content distilled from screen activity; routing it into a user-visible generation crosses a boundary the current design never crosses, and routing it to a remote provider would additionally void every exemption the loopback pin currently earns.

ADR-023 §5 further names three designs that are routinely conflated: retrieval re-rank, prompt-context injection, and a coaching gate signal. They have materially different privacy surfaces and must not share a single approval.

## Decision

Adopt a **mode-separated, fail-closed generation-input contract**. No runtime change accompanies this ADR; it pins the contract that any future consumer must satisfy before wiring.

### 1. Three modes, separately gated, ordered by disclosure

| Mode | What reaches the generator | Permitted disclosure |
|------|---------------------------|----------------------|
| **A — Retrieval augmentation** | Edge topology only (`src_id`, `dst_id`, `edge_type`, `confidence`) used to re-rank or expand an existing `hybrid_search` result set | No claim text. Ranking influence only. |
| **B — Gate signal** | A derived scalar or boolean (e.g. "≥N active contradictions in the last 7 days") | No claim text, no IDs. |
| **C — Prompt-context injection** | PII-masked claim text inside a prompt | Full text disclosure to the generator. |

Modes must be adopted in the order **A → B → C**. Each requires its own activation decision, its own consent evaluation (§3), and its own contract tests. **Approval of one mode never implies another.** Mode A is the design ADR-023 already ratified as the intended read path; Mode C is the one ADR-023 §5 called unratified, and it inherits the strictest requirements below.

**Rationale**: the three differ by *what content crosses the boundary*, not by implementation convenience. Collapsing them into a single "use the graph for generation" switch is exactly the ad-hoc introduction ADR-023 §5 refused.

### 2. Bounded projection (input selection)

Any mode that reads the graph for generation MUST select through a single shared projection helper, not by calling `MemoryGraphPort` directly from a consumer. The projection is **fail-closed**: if any bound cannot be evaluated, it yields an empty set rather than an unbounded one.

The projection MUST enforce all of:

1. **Status**: `Active` only. `Superseded` and `Retracted` are excluded at selection time, never merely filtered downstream.
2. **Recency window**: a bounded `updated_at` window. The ADR-023 retention prune (`analysis.embedding.retention_days`, default 90 — `scheduler/loops/system.rs:799-821`) is a storage floor, **not** a generation window; the generation window MUST be independently configured and MUST be ≤ the retention window.
3. **Confidence floor**: an input-side minimum. Distinct from `supersede_confidence_threshold` (0.9), which is an *output*-side gate on belief revision and MUST NOT be reused as the input floor.
4. **Hard count cap** with deterministic ordering — `updated_at DESC`, `claim_id` tie-break (the ordering `memory_claims_service.rs:124-128` already uses). Ordering must be total so a generation is reproducible from the same graph state.
5. **Field allowlist**: `claim_id`, PII-masked `text`, `kind`. Denied: `source`, raw `confidence`, `evidence_ref`, and any `segment_id`/`frame_id` provenance. Provenance identifiers are internal correlation keys and MUST NOT reach a generator.
6. **Edge projection** (Mode A only): `edge_type` and `confidence` may influence ranking. `evidence_ref` may not be projected in any mode.

**Rationale**: the current belief-revision selection has no count or recency bound because its consumer is a local self-maintenance pass whose cost is a daily local LLM call. A generation consumer has a token budget, a latency budget, and a disclosure surface, so the bounds become load-bearing rather than incidental.

### 3. Privacy boundary

1. **Masking is a projection-time invariant.** PII masking (ADR-023 MG-PII-01/MG-PII-03, `sanitize_title_with_level` at the injected `PiiFilter` seam) MUST be applied inside the projection helper, so no consumer can obtain unmasked claim text. Masking at the call site is not acceptable — that is the shape that lets a new consumer silently skip it.
2. **Consent is per-purpose, not per-table.** `memory_graph_enrichment` authorises the *self-maintenance* loop (graph → LLM → graph). Feeding a user-visible generation is a different purpose and MUST NOT borrow that permission. Mode A and Mode B MAY be authorised by an extension of an existing permission if the extension is documented in `consent.rs` and surfaced in the consent UI; **Mode C requires its own permission**, defaulting to `false`, following the Tier-7 precedent that `memory_graph_enrichment` set.
3. **Remote egress voids the MG-PII-04 exemption.** The current bypass of `GuardedAnalysisProvider` and the absence of an egress-ledger entry are justified solely by the loopback pin (`host_is_loopback`, `http_client.rs:79-98`). Therefore:
   - Local (loopback) generation input MAY reuse the existing exemption, unchanged.
   - **Any generation input that can reach a non-loopback provider MUST route through `GuardedAnalysisProvider` (ADR-024) and MUST write an egress-ledger entry** with a registered `event_type` before the send. A remote path without a ledger entry is a contract violation, not a gap.
4. **Trust boundary in prompts.** Claim text is user-derived. In Mode C it MUST be wrapped as `UntrustedContent` via `models/prompt_assembly.rs` and MUST NOT appear in a `TrustedInstruction` segment. This closes an existing shape gap: the belief-revision path builds provider bodies directly (`analysis_client/requests.rs:6-31`) without the segmented-prompt wrapper, which is tolerable for a closed loop but not for a path whose output reaches the user.
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

1. **Projection helper implementation** — a single `maekon-analysis` seam implementing §2, with contract tests asserting each bound is fail-closed. Prerequisite for any mode.
2. **Mode A wiring (retrieval augmentation)** — edges re-rank/expand `hybrid_search_service` results, the read path ADR-023 §4 already scoped and ADR-023 §5 flagged as still unwired.
3. **Egress-ledger event type for remote graph egress** — required by §3.3 before any non-loopback mode; today no `memory_graph`/`belief_revision` event type exists in the ledger.
4. **Segmented-prompt adoption on the belief-revision path** — belief revision builds provider bodies directly today. Not a defect for a closed loop, but adopting `prompt_assembly` there first would let Mode C reuse a proven seam.
5. **Provenance-cited generation** — if citing evidence (`evidence_ref`, `segment_id`) in user-visible output is later wanted, it needs its own ADR; §2.5 forecloses it deliberately.

## Related Docs

- `docs/architecture/ADR-023-local-symbolic-memory-graph.md` §4-§5 — substrate, intended read path, and the deferral this ADR discharges
- `docs/architecture/ADR-030-work-context-envelope-convergence.md` §11 — hands memory-graph generation policy to #8087
- `docs/architecture/ADR-024-conversation-content-guard-port.md` — guard + egress-audit pattern required by §3.3
- `docs/architecture/ADR-026-async-storage-convergence-consent-port.md` — `ConsentManagerPort`, home of the gate in §3.2
- `docs/architecture/ADR-013-llm-summary-vector-rag.md` — the RRF retrieval Mode A augments
- `crates/maekon-analysis/src/belief_revision.rs` — the single existing LLM path and its selection/masking baseline
- `crates/maekon-core/src/models/prompt_assembly.rs` — trust-boundary wrapper required by §3.4
