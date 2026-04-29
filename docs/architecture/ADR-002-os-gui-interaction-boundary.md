[English](./ADR-002-os-gui-interaction-boundary.md) | [한국어](./ADR-002-os-gui-interaction-boundary.ko.md)

# ADR-002: OS GUI Interaction Boundary and Runtime Split

**Status**: Accepted (promoted from Proposed 2026-04-20; M3 native-adapter implementation complete)
**Date**: 2026-02-25
**Scope**: `oneshim-core` (`ElementFinder` / `OverlayDriver` / `FocusProbe` / `InputDriver` ports), `oneshim-automation` (session state machine, capability/ticket handling), `oneshim-vision` (R-tree spatial index, app-specific element type overrides, accessibility adapters), `oneshim-web` (capability-token handler), `src-tauri` (`MagicOverlayDriver` — the WebView bridge that replaced the originally-planned `oneshim-ui` native overlay after ADR-004's Tauri v2 migration)

---

## Context

The current stack already supports:

- Scene analysis (`GET /api/automation/scene`)
- Scene action execution (`POST /api/automation/execute-scene-action`)
- Policy/privacy/audit controls in `oneshim-automation`

However, OS GUI interaction requires stronger guarantees:

1. Identify controls from the currently focused native window.
2. Show explicit visual highlights on the OS screen before action.
3. Execute only after user confirmation.

Pure web rendering cannot reliably draw trusted overlays on arbitrary native windows and cannot guarantee focus consistency at execution time.

---

## Decisions

### 1. Split into Control Plane and Execution Plane

- **Control Plane** (`oneshim-web`): management, monitoring, API orchestration.
- **Execution Plane** (local runtime): focus probing, scene analysis, native overlay highlight, input execution.

`oneshim-web` must never call OS-native interaction directly.

### 2. Keep policy/privacy/audit as the single gate

All GUI execution paths pass through `oneshim-automation` policy/privacy/audit checks. No direct handler-to-driver bypass is allowed.

### 3. Adopt a session-based interaction protocol

Flow:

1. `propose` candidates
2. `highlight` candidates
3. `confirm` a candidate
4. `execute` with a short-lived ticket
5. `verify` and `audit`

One-shot direct execution remains legacy-compatible but is not the primary UX path for high-risk actions.

### 4. Add explicit core contracts for focus and overlay

New core ports in `oneshim-core`:

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

V2 sessions are stored in `oneshim-automation` memory:

- `Arc<RwLock<HashMap<SessionId, GuiInteractionSession>>>`
- TTL-based lifecycle with periodic cleanup (default: every 30 seconds)
- No SQLite persistence in Phase 0-2

If persistence is required later, it must be introduced through `oneshim-core` storage ports.

### 7. Require ticket integrity and session capability authentication

`GuiExecutionTicket` contains:

- `session_id`, `focus_hash`, `scene_id`, `element_id`, `action_hash`
- `issued_at`, `expires_at`, `nonce`
- `signature` (HMAC)

HMAC key source (fixed configuration):

- `ONESHIM_GUI_TICKET_HMAC_SECRET` environment setting is required when GUI V2 endpoints are enabled.
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

Overlay capability ships in `src-tauri` as `MagicOverlayDriver` (WebView bridge; replaces the originally-planned native `oneshim-ui` overlay after ADR-004 Tauri v2 migration). Ports remain unchanged — callers depend on `OverlayDriver` trait from `oneshim-core`.

### 10. Use a dedicated GUI session SSE stream

Primary event delivery for V2 uses dedicated session SSE:

- `GET /api/automation/gui/sessions/:id/events`
- Session-scoped events only (for example, `gui_session.proposed`, `gui_session.highlighted`, `gui_session.executed`, `gui_session.expired`)

Existing `GET /api/stream` may publish coarse operational summaries, but it is not the source of truth for GUI session state.

---

## Target Responsibility Map

| Crate | Responsibility after ADR-002 |
|------|-------------------------------|
| `oneshim-core` | Focus/overlay/session/ticket ports and domain contracts |
| `oneshim-automation` | `GuiInteractionService` orchestration (`propose -> highlight -> confirm -> execute`) + policy/privacy/audit + session state |
| `oneshim-vision` | Accessibility adapters (macOS AX / Windows UIA / Linux AT-SPI), R-tree spatial index, `ElementFinder` implementations |
| `oneshim-web` | Thin transport handlers, validation, session APIs, SSE event publication |
| `src-tauri` (pkg `oneshim-app`) | Composition root wiring + `MagicOverlayDriver` (WebView overlay bridge, replaces the originally-planned `oneshim-ui` native overlay per ADR-004) for `OverlayDriver`, `FocusProbe`, `ElementFinder`, `InputDriver` |

Dependency direction remains unchanged: adapters communicate through `oneshim-core` ports.

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
  -> oneshim-web handler
  -> oneshim-automation GuiInteractionService
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
- `503` GUI V2 misconfigured (`ONESHIM_GUI_TICKET_HMAC_SECRET` missing while GUI V2 enabled)

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
- `docs/crates/oneshim-web.md`
- `docs/crates/oneshim-automation.md`
