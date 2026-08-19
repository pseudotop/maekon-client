[English](./ADR-027-suggestion-action-binding.md) | [한국어](./ADR-027-suggestion-action-binding.ko.md)

# ADR-027: Suggestion Action Binding (Derived, Gate-Preserving)

**Status**: Accepted
**Date**: 2026-07-08
**Scope**: `crates/maekon-core/src/models/intent/workflow.rs` (derivation helper + preset-id const), `src-tauri/src/commands/suggestions/` (DTO enrichment + `run_suggestion_action`), `crates/maekon-web/frontend/src/overlay/` (Run affordance)
**Related**: ADR-001 (§6 immutable crate dependency direction, §7 port placement), ADR-002 (single-gate execution boundary), ADR-017 (FeedbackSignalSink)
**Issue**: #7917. Design 3-loop reviewed to zero-important by independent devils-advocate + tech-lead passes (2 rounds each).

---

## Context

The suggestion pipeline (producers → dedup queue → overlay UI → feedback/learning) and the gated automation engine (`ensure_enabled` → `confirmation_policy` → sandboxed steps → hash-chained audit) were fully built but disconnected: a suggestion could not offer "run this now?" and the overlay explicitly advertised "No auto action". Bridging them is the first step from productivity coach to gated agent.

Two candidate architectures were adversarially reviewed:

1. **Persisted binding** — an `action_preset_id: Option<String>` field on the domain `Suggestion`, carried through storage/sync/DTOs.
2. **Derived binding (CHOSEN)** — no schema change; a pure core helper derives the offer at presentation/execution time from data the suggestion already carries.

## Decision

### 1. The binding is DERIVED, never carried on the wire

`maekon_core::models::intent::workflow::suggested_action_preset(suggestion_type, source) -> Option<&'static str>` is the single policy table. MVP maps exactly one pair:

| (type, source) | preset |
|---|---|
| (`NeedFocusTime`, `RuleBased`) | `PRESET_DEEP_WORK_START` (`"deep-work-start"`, a builtin) |
| anything else — including any network (`LlmServer`) or LLM (`LlmLocal`) source | `None` |

Rationale for derivation over persistence (the decisive review finding): the REST SSE path deserializes the domain `Suggestion` directly, so a persisted field would be **wire-injectable** — a server payload could conjure an execution affordance — and would then need a sanitization invariant to patch. Derivation removes that class **by construction**: there is no field to inject, and the wire cannot carry, select, or influence which action binds.

**The source condition is load-bearing.** `NeedFocusTime` is server-mintable (gRPC + SSE share the frozen 10-variant enum), so type-only derivation would still hand network-pushed suggestions a one-click affordance (promptless under `automation.enabled` + field-default `Auto`). LLM sources are likewise excluded: LLM-authored content plus an execution affordance is prompt-injection-adjacent even with a fixed preset.

### 2. Execution flows ONLY through the existing gate chain

The overlay Run button invokes one composition-root Tauri command, `run_suggestion_action(suggestion_id)`, which:

1. reserves the suggestion id (reserve-then-execute; a concurrent second call is refused — under the `Auto` default a double-fire executes twice, and `deep-work-start` closes a window per run);
2. resolves the suggestion (manager queue first, storage fallback — same duality as the pending list);
3. **re-derives** the binding from the suggestion's own `(type, source)` — the client never supplies a preset id; non-`RuleBased` suggestions are refused;
4. resolves the preset against the live list (`builtin_presets()` + `AutomationConfig.custom_presets`) — run-time revalidation, the real dangling-id invariant;
5. calls `AutomationPort::run_workflow(&preset)` — the full existing chain: `ensure_enabled` → `confirmation_policy` (Block / Confirm-HITL via `automation:confirm-request` + 30s timeout→denied / Auto) → sandbox-scoped steps → per-step audit + storage hash-chain;
6. only on a successful `PresetRunResult`: emits the standard accept feedback via `submit_suggestion_feedback_to_runtime(…, "accept", None)` (queue→history, scorer, tally write-through, server notify) and marks `acted_at`. Denied/blocked/failed/timeout runs emit **nothing** — the learned relevance signal must not be polluted by non-executions.

The UI affordance is presentation-only: `SuggestionViewDto.action: Option<{ label }>` (label-only — the client never even sees a preset id), computed by ONE shared helper iff the derivation maps ∧ `automation.enabled` ∧ the preset resolves. History views stay unbound (stale suggestions must not offer execution). Copy is policy-neutral ("runs through the automation gate; your confirmation settings apply") — it never promises a prompt, because the field-default is `Auto`.

## Frozen invariants (violating any of these requires a new ADR)

| Invariant | Where |
|---|---|
| Network-sourced suggestions gain no execution affordance; any FUTURE persisted binding must strip network sources (same reservation as `execute_command`'s unused signed-token path) | derivation predicate + this ADR |
| `automation.enabled` defaults `false`; `ensure_enabled` guards every run | `AutomationConfig` |
| Double default: `confirmation_policy` FIELD default `Auto` (D2-② sign-off) vs `ConfirmationRequirement` ENUM / `ExecutionPolicy.confirmation` default `Confirm` (fail-safe) — both preserved, never merged | `config/sections/privacy.rs`, `config/enums.rs` |
| `can_auto_execute() == false` for record-replay templates; `require_signed_token` default `true`; `min_llm_confidence` 0.65 floor | `record_template.rs`, `policy/models.rs` |
| Audit: automation logger buffer + SHA-256 hash chain computed/persisted in `maekon-storage` via the wired persistence callback | `audit_chain.rs`, `web_server_runtime.rs` |
| ADR-002 single gate: no handler-to-driver bypass; overlay/webview never executes directly | ADR-002 |
| ADR-001 §6: `maekon-suggestion` and `maekon-automation` stay siblings (core-only deps); shared semantics live in `maekon-core` | CI `check-architecture-deps.sh` |
| `SuggestionType` stays proto-frozen at 10 variants; the bridge never adds a variant | guard test + protos |

## Evolution path (re-review triggers)

- **First non-type-derivable producer** (per-instance, contextual, or LLM-authored binding; custom-preset targets) reopens the persisted-field decision — with the network-stripping invariant above as a hard requirement.
- Server-authored bindings require a signed mechanism (proto field + verification), not the JSON wire.
- A distinct `Executed` feedback outcome (today success reuses `Accepted`, the strongest acceptance signal) and timeout-vs-user-denied telemetry are follow-ups.
- The `deep-work-start` preset itself has a recorded persona/behavior gap (developer-centric steps vs. communication-heavy `NeedFocusTime` audience) — tracked as a follow-up issue; the binding table should evolve with preset redesign.
