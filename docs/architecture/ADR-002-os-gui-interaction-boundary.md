[English](./ADR-002-os-gui-interaction-boundary.md) | [한국어](./ADR-002-os-gui-interaction-boundary.ko.md)

# ADR-002: OS GUI Interaction Boundary and Runtime Split

**Status**: Accepted (promoted from Proposed 2026-04-20; M3 native-adapter implementation complete)
**Date**: 2026-02-25
**Scope**: `maekon-core` (`ElementFinder` / `OverlayDriver` / `FocusProbe` / `InputDriver` ports), `maekon-automation` (session state machine, capability/ticket handling), `maekon-vision` (R-tree spatial index, app-specific element type overrides, accessibility adapters), `maekon-web` (capability-token handler), `src-tauri` (`MagicOverlayDriver` — the WebView bridge that replaced the originally-planned `maekon-ui` native overlay after ADR-004's Tauri v2 migration)

---

## Context

The current stack already supports:

- Scene analysis (`GET /api/automation/scene`)
- Scene action execution (`POST /api/automation/execute-scene-action`)
- Policy/privacy/audit controls in `maekon-automation`

However, OS GUI interaction requires stronger guarantees:

1. Identify controls from the currently focused native window.
2. Show explicit visual highlights on the OS screen before action.
3. Execute only after user confirmation.

Pure web rendering cannot reliably draw trusted overlays on arbitrary native windows and cannot guarantee focus consistency at execution time.

---

## Decisions

### 1. Split into Control Plane and Execution Plane

- **Control Plane** (`maekon-web`): management, monitoring, API orchestration.
- **Execution Plane** (local runtime): focus probing, scene analysis, native overlay highlight, input execution.

`maekon-web` must never call OS-native interaction directly.

### 2. Keep policy/privacy/audit as the single gate

All GUI execution paths pass through `maekon-automation` policy/privacy/audit checks. No direct handler-to-driver bypass is allowed.

### 3. Adopt a session-based interaction protocol

Flow:

1. `propose` candidates
2. `highlight` candidates
3. `confirm` a candidate
4. `execute` with a short-lived ticket
5. `verify` and `audit`

One-shot direct execution remains legacy-compatible but is not the primary UX path for high-risk actions.

### 4. Add explicit core contracts for focus and overlay

New core ports in `maekon-core`:

```rust
#[async_trait]
pub trait OverlayDriver: Send + Sync {
    async fn show_highlights(&self, req: HighlightRequest) -> Result<HighlightHandle, CoreError>;
    async fn clear_highlights(&self, handle_id: &str) -> Result<(), CoreError>;
}

#[async_trait]
pub trait FocusProbe: Send + Sync {
    async fn current_focus(&self) -> Result<FocusSnapshot, CoreError>;
    async fn validate_execution_binding(
        &self,
        binding: &ExecutionBinding,
    ) -> Result<FocusValidation, CoreError>;
}
```

`validate_execution_binding` is a single call to reduce TOCTOU risk during confirm/execute revalidation.

### 5. Reuse `UiSceneElement` and avoid candidate model duplication

`GuiCandidate` is defined as a wrapper/projection of `UiSceneElement` with additional interaction metadata (ranking reason, eligibility flags), not a duplicated parallel model.

### 6. Use in-memory session storage for V2

V2 sessions are stored in `maekon-automation` memory:

- `Arc<RwLock<HashMap<SessionId, GuiInteractionSession>>>`
- TTL-based lifecycle with periodic cleanup (default: every 30 seconds)
- No SQLite persistence in Phase 0-2

If persistence is required later, it must be introduced through `maekon-core` storage ports.

### 7. Require ticket integrity and session capability authentication

`GuiExecutionTicket` contains:

- `session_id`, `focus_hash`, `scene_id`, `element_id`, `action_hash`
- `issued_at`, `expires_at`, `nonce`
- `signature` (HMAC)

HMAC key source (fixed configuration):

- `MAEKON_GUI_TICKET_HMAC_SECRET` environment setting is required when GUI V2 endpoints are enabled.
- Missing/empty secret is fail-closed: session creation and ticket issuance are rejected.

Session endpoints (`/sessions/:id/*`) require a per-session capability token issued at session creation (for example, `X-Gui-Session-Token`).

### 8. Prefer accessibility-first detection with OCR fallback

Execution plane detection order:

1. Accessibility tree adapter
2. OCR-based finder fallback
3. Optional template matcher

Candidate ranking combines source reliability, confidence, role intent, and focus-window consistency.

### 9. Overlay trust boundary is local and non-interactive

Overlay implementation requirements:

- Always-on-top, non-interactive click-through
- Rendered only by the Maekon local process
- Includes session/candidate marker for operator traceability
- Cleared on timeout, cancel, or completion

Overlay capability ships in `src-tauri` as `MagicOverlayDriver` (WebView bridge; replaces the originally-planned native `maekon-ui` overlay after ADR-004 Tauri v2 migration). Ports remain unchanged — callers depend on `OverlayDriver` trait from `maekon-core`.

### 10. Use a dedicated GUI session SSE stream

Primary event delivery for V2 uses dedicated session SSE:

- `GET /api/automation/gui/sessions/:id/events`
- Session-scoped events only (for example, `gui_session.proposed`, `gui_session.highlighted`, `gui_session.executed`, `gui_session.expired`)

Existing `GET /api/stream` may publish coarse operational summaries, but it is not the source of truth for GUI session state.

---

## Target Responsibility Map

| Crate | Responsibility after ADR-002 |
|------|-------------------------------|
| `maekon-core` | Focus/overlay/session/ticket ports and domain contracts |
| `maekon-automation` | `GuiInteractionService` orchestration (`propose -> highlight -> confirm -> execute`) + policy/privacy/audit + session state |
| `maekon-vision` | Accessibility adapters (macOS AX / Windows UIA / Linux AT-SPI), R-tree spatial index, `ElementFinder` implementations |
| `maekon-web` | Thin transport handlers, validation, session APIs, SSE event publication |
| `src-tauri` (pkg `maekon-app`) | Composition root wiring + `MagicOverlayDriver` (WebView overlay bridge, replaces the originally-planned `maekon-ui` native overlay per ADR-004) for `OverlayDriver`, `FocusProbe`, `ElementFinder`, `InputDriver` |

Dependency direction remains unchanged: adapters communicate through `maekon-core` ports.

---

## API Contract (Proposed V2)

Base path: `/api/automation/gui`

| Method | Path | Purpose |
|-------|------|---------|
| `POST` | `/sessions` | Create proposal session from focused scene |
| `POST` | `/sessions/:id/highlight` | Render candidate highlights on OS overlay |
| `POST` | `/sessions/:id/confirm` | Confirm candidate and issue signed execution ticket |
| `POST` | `/sessions/:id/execute` | Execute action with ticket (atomic revalidation required) |
| `GET` | `/sessions/:id` | Read current session state and candidate summary |
| `DELETE` | `/sessions/:id` | Clear overlay and close session |
| `GET` | `/sessions/:id/events` | Dedicated session SSE stream (primary GUI event channel) |

Auth semantics:

- `POST /sessions` returns a per-session capability token.
- Subsequent `:id` endpoints require that token.
- `GET /sessions/:id/events` also requires the same per-session capability token.
- When `web.allow_external=false`, non-loopback requests are rejected.

Legacy endpoints (`/scene`, `/execute-scene-action`) remain for compatibility and internal tooling.

---

## Runtime Sequence

```text
Web UI
  -> maekon-web handler
  -> maekon-automation GuiInteractionService
     -> FocusProbe.current_focus()
     -> ElementFinder.analyze_scene()
     -> rank candidates
  <- candidates + session token

User requests highlight
  -> OverlayDriver.show_highlights()

User confirms candidate
  -> issue signed GuiExecutionTicket
  -> FocusProbe.validate_execution_binding(ticket.binding)
  -> InputDriver execute action
  -> verification + audit
  -> OverlayDriver.clear_highlights()
```

---

## Security and Privacy Invariants

1. No raw sensitive source data leaves the machine unless explicit policy/consent override allows it.
2. UI payload defaults to masked labels (`text_masked`) for sensitive contexts.
3. All actions, denials, overrides, and ticket failures are audit-logged.
4. Execution requires both a valid session capability token and a valid signed ticket.
5. Focus revalidation is mandatory at execution and performed atomically through a single probe call.
6. Overlay is local-only, non-interactive, and lifecycle-bounded.
7. GUI session SSE must enforce session scoping so one session cannot subscribe to another session's events.

---

## Failure Semantics

Recommended HTTP mapping:

- `400` invalid request schema
- `401` missing/invalid session capability token
- `403` policy/privacy denied
- `409` stale focus or scene drift
- `422` candidate/ticket no longer valid
- `503` execution runtime unavailable (headless/no capability)
- `503` GUI V2 misconfigured (`MAEKON_GUI_TICKET_HMAC_SECRET` missing while GUI V2 enabled)

On `409`/`422`, client should create a new session and repeat `propose -> highlight -> confirm`.

---

## Rollout Plan

### Phase 0 (contracts + base state)

- Add core models/ports/schema versions
- Add in-memory session store + cleanup task
- Add no-op adapters for unsupported environments

### Phase 1a (proposal-only preview)

- `POST /sessions`, `GET /sessions/:id`
- No overlay rendering and no execution

### Phase 1b (highlight preview)

- `POST /sessions/:id/highlight`, `DELETE /sessions/:id`
- Overlay rendering path enabled
- Still no action execution from V2

### Phase 2 (confirmed execution)

- `POST /sessions/:id/confirm`, `POST /sessions/:id/execute`
- Signed ticket validation + atomic focus revalidation
- Policy/privacy/audit fully enforced in V2 path

### Phase 3 (hardening)

- Accessibility adapters per OS (macOS AX, Windows UIA, Linux AT-SPI)
- Improved ranking, retry hints, calibration quality metrics

---

## Test Strategy

- Unit tests for session state machine transitions (`propose/highlight/confirm/execute/cancel/expire`)
- Unit tests for ticket signing/verification/expiry/nonce replay protection
- Unit tests for focus drift handling and atomic validation outcomes
- Integration tests with `MockOverlayDriver`, `MockFocusProbe`, `MockElementFinder`, `MockInputDriver`
- Web handler tests for capability-token enforcement and error mapping (`401/403/409/422/503`)

---

## Consequences

Positive:

- Web remains a control/monitoring surface.
- OS GUI interaction becomes explicit, auditable, and safer.
- Existing Hexagonal boundaries remain intact.

Tradeoffs:

- Added runtime complexity (overlay lifecycle, session TTL, capability and ticket validation)
- Platform-specific adapter work remains high cost in Phase 3

---

## Related Docs

- `docs/architecture/ADR-001-rust-client-architecture-patterns.md`
- `docs/contracts/automation-event-contract.md`
- `docs/crates/maekon-web.md`
- `docs/crates/maekon-automation.md`


## Update 2026-09-20 — Guarded asynchronous candidate decisions

### Context and decision

A structured remote model can rank a closed candidate set, but its output is not a
user confirmation or an execution capability. Extend this ADR's existing proposal
stage; do not introduce a parallel scene model or a new execution path. This dated
amendment takes effect on its normal PR merge. Implementation and provider/data
approval remain separate gates.

1. `maekon-core` owns an async `CandidateDecisionPort` and typed request, binding,
   candidate and outcome models. `maekon-network` owns the HTTP adapter; `src-tauri`
   owns the guard and composition. Automation depends on the core port, never the
   network adapter. Keep the free-generation `LlmProvider` and synchronous local
   `WorkTypeClassifier` contracts unchanged.
2. Project minimal opaque IDs and sanitized semantic descriptions from existing
   `GuiCandidate`/`UiSceneElement` values. Keep original goal hash, local candidate
   binding hash, scene/frame generation, consent/policy revisions and a monotonic
   deadline local. A sanitized payload hash cannot replace the original binding:
   redaction can make distinct goals identical.
3. Outcomes are `selected`, `none`, `delegate` and `unavailable`. None means no
   suitable candidate; unavailable means policy/runtime/error; delegate requests
   another decision and does not automatically call another provider. Selected
   identifies a current candidate and grants no capability.
4. Keep recognition confidence, Choice-relative probabilities, distribution
   confidence and the selected candidate's Noul suitability separate. Do not invent
   a Noul confidence or a generated rationale. A Choice followed by a Noul must
   evaluate the candidate actually selected, count both calls and recheck the guard
   between them. Freeze decision thresholds before exposing the evaluation test set.
5. Before every request and cache hit, verify current consent, sensitive-app and
   exclusion policies, minimum necessary payload, endpoint, credentials, audit and
   budget. Off, local-only, missing approval or missing audit means no external call.
   Existing LLM decorators do not automatically protect this new port. Composition
   exposes only a guarded port, not a raw network client.
6. Hold no locks across HTTP awaits. After every await, re-read goal, candidates,
   generation, policy/consent revisions and monotonic expiry; discard on mismatch.
   Revocation after an allowed in-flight request prevents later calls and result
   reuse. Best-effort cancellation does not erase an already incurred call or cost.
7. The wire DTO excludes coordinates/rectangles, text to type, raw screenshots/AX,
   tickets/nonces/signatures, capabilities, API keys and arbitrary metadata. Goal,
   labels and titles all require sanitization. Credentials go only to the approved
   authentication header. Do not log payloads, raw provider errors or secret-bearing
   Debug output. Shape-valid data is not evidence of policy authorization.
8. Cache by principal/provider/endpoint/model/rubric/schema, authorization revisions,
   input/candidate hashes and local binding. Never reuse across accounts or expired
   snapshots. Hashes provide binding, not anonymization. Revalidate current consent
   even on a cache hit.
9. Decode strictly: reject duplicate/missing/extra IDs, out-of-set choices, nonfinite
   values, invalid distributions, inconsistent choice/argmax, wrong observed model,
   malformed usage and oversized replies. Bound request bytes, response bytes,
   timeout, retry count and total budget; record attempts without hidden retries.
10. Recommendation preserves highlight → confirm → signed ticket → fresh
    focus/policy validation → execute. This decision port cannot weaken a deny,
    confirm for the user, issue tickets or activate an unverified Skill. Runtime
    composition starts disabled for user recommendation and execution.

### Alternatives and consequences

Putting candidate selection into the generative intent port conflates output and
permission semantics. Direct HTTP from automation bypasses the runtime guard.
Adding a parallel scene model duplicates existing identity and freshness ownership.
A separate core port with a guarded adapter adds DTO/error branches and tests, but
keeps provider changes outside automation and retains this ADR's execution boundary.
No new ADR number is needed for this extension of the proposal stage.

### Implementation and validation boundary

Planned files are `crates/maekon-core/src/models/candidate_decision.rs`,
`crates/maekon-core/src/ports/candidate_decision.rs`,
`crates/maekon-network/src/candidate_decision_client.rs` and
`src-tauri/src/provider_adapters/guarded_candidate_decision.rs`, plus their module
and composition declarations. These are planned paths, not implemented behavior.

Test off/local-only/denied calls, sanitized wire capture, response decoding, cache
isolation, revocation or goal/candidate/generation/TTL changes during each await,
and prevention of the second suitability call. Use positive controls and remove a
guard to show that its test becomes red. Mock results are not provider performance
or installed-artifact acceptance. Real evaluation and data/budget approval precede
user exposure; installed freshness and explicit confirmation evidence remain
required for execution.

## Update 2026-09-21 — Optional decision providers

Jev compatibility does not make a Jev account or paid inference a prerequisite for
Maekon. Core eligibility is defined in
[candidate_decision_policy.rs](../../crates/maekon-core/src/models/candidate_decision_policy.rs).
The detailed evaluation contract is `docs/development/ai-evaluation/candidate-decision-providers.md`
in the internal ONESHIM repository.
This amendment extends the proposal stage and takes effect on normal PR merge.

1. Core owns pure, default-off provider/mode/cost eligibility. Local rules, a
   verified loopback model, user-owned official Codex/Claude CLIs, Jev direct and
   Jev Gateway are separate routes. A subprocess may send data to the cloud and
   is never a local-only exemption. No implicit provider or paid fallback occurs.
2. CLI authentication does not prove subscription billing. The initial subscription
   route requires a verified included-subscription observation and a user request;
   API-key billing, unknown billing and background CLI attempts are denied.
   Do not modify or intermediate the official CLI's authentication methods.
3. A promotion is an expiring, account/endpoint/model-bound price observation,
   not permanent free service. Expiry denies a new call even when the policy
   otherwise permits paid APIs. Usage/cost not observed stays unknown.
4. Preserve strict Jev v1 Choice/Noul semantics. The successor outcome contract
   separates Jev distributions, model-reported evidence, local heuristics and
   unobserved evidence; never synthesize Jev probabilities for other providers.
5. Eligibility is not egress or execution authorization. Every adapter retains
   consent/privacy/audit/budget, original binding/deadline, revocation and cache
   isolation checks. CLI adapters additionally require bounded streams/files,
   cancellation and verified decision-only tool/MCP/plugin capabilities.
6. Missing providers preserve the manual selection/confirmation path. A model may
   abstain; a local rule is not claimed to match model quality. Selected still
   grants no ticket, confirmation, Skill activation or execution permission.

#12455 implements this contract and pure policy. #12456 owns generic results and
local runtime; #12457 owns bounded subscription CLI decisions; #12458 owns optional
Gateway transport and pricing. #12423 retains independent frozen evaluation for
each provider; #12424/#12425 retain GUI and installed acceptance gates. Existing
#12176 consent ownership and all execution guards remain in force. Policy unit
tests and the 38-case design matrix are not runtime, model-quality or installed
acceptance evidence.


### Provider-neutral results and local runtime (#12456)

The new CandidateAssessmentPort reuses the existing GUI request and binding.
Selected contains only a candidate ID. Tagged evidence distinguishes a local
exact-label heuristic, categorical model output, and the original Jev distribution.
The v1 compatibility projection explicitly preserves provider/model/rubric, Choice
confidence and probabilities, selected suitability, original attempt IDs and costs,
cache source, decision ID and binding. Unknown usage stays unknown.

Routing occurs before optional HTTP construction or secret lookup. Off returns an
unavailable advisory. Explicit LocalRules works independently of the global AI
provider and uses one unique lowercased, trimmed exact goal/label match. Missing,
duplicate and redaction-collided matches delegate; candidate order and recognition
confidence are not tie breakers. Unsupported routes never trigger paid fallback.
The existing metered Jev v1 route also requires ExplicitPaidApiAllowed.

A local runtime starts without approval. Trusted composition must issue one approval per runtime with fixed expiry and attempt
limits, and bind each fresh snapshot; restart does not restore approval.
Every bind advances the epoch, including A -> B -> A. Account, model or policy
changes require permanent revocation and a new runtime. The first full consent/
privacy revision is pinned; a changed revision cannot renew the original approval.
Local data authorization reuses full-text consent and sensitive/excluded-surface
checks independently of provider-specific remote gates. It does not start OCR.

Before attempts and after asynchronous observations, transport and audit work, the
runtime rechecks authority, the current window and the original monotonic deadline.
An immutable runtime clock anchor also enforces the original wall-clock bound:
suspend cannot renew a bound through rebind/cache, and wall-clock regression
permanently revokes the runtime. Rust Instant alone does not guarantee suspended
time elapses (https://doc.rust-lang.org/std/time/struct.Instant.html).
Quota reservation is atomic and is not refunded after cancellation or audit failure.
Publication checks run under the bind/revoke lock. Rule cache hits require fresh
authority and durable audit, preserve the original deadline and reference the source
decision without duplicating attempts. Model output is not cached. These are advisory
checks: consumers must still validate the current scene and execution/consent guards
at confirmation and dispatch (#12424/#12425, existing #12176 ownership).

The optional Ollama adapter allows only loopback/localhost origins, pins every
resolved address, disables proxies, redirects and retries, and caps response bytes.
It sends only sanitized candidate text with request-local IDs and a closed categorical
schema, with no tools. It never pulls a model, starts a daemon or chooses a remote
fallback. A trusted daemon/configuration approval is bound to the canonical endpoint,
model digest and expiry. Cloud-disabled status and installed, non-remote weights are
checked before and after inference; missing/unknown capability denies the call.
The installed tag's bare SHA-256 hex digest is compared to the canonical
`sha256:` digest in the trusted approval.

Loopback or those probes alone do not attest an arbitrary daemon or eliminate
configuration TOCTOU. The approval reference must come from trusted daemon/configuration
verification, never a user-supplied IPC assertion. No production approval issuer or
GUI consumer is enabled here; its live verification remains part of #12424/#12425.
Fixture tests do not establish real model quality, pricing, account entitlement or
installed-product acceptance (#12423). CLI and Gateway adapters remain #12457/#12458.


### Gateway evidence types and exact price arithmetic (#12458, first increment)

The core defines separate Gateway account, funding, routing, reported-cost and
audit evidence types, plus ports for trusted account observation, candidate
transport and durable audit. The TypeSafe-compatible endpoint and alias remain
separate from the pinned direct Jev model. An absent cost or confidence is unknown.

USD arithmetic uses integer picoUSD with at most twelve fractional digits. Price
estimates include both token directions and the fixed request fee; usage above
65,536 tokens per direction and amounts exceeding u64 are rejected. Reservations
use the maximum permitted usage, including the fee.

This increment supplies types and price arithmetic only. Evidence validation,
account approval, HTTP transport, durable audit implementation and runtime wiring
remain open under #12458. The generic factory still refuses Gateway, and no
account observer, key lookup, model call or GUI consumer is enabled by this change.
A public promotion does not establish account entitlement or zero effective cost.
